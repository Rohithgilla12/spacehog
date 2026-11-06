use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::Client;

impl Client {
    const ALL: [Client; 3] = [Client::ClaudeCode, Client::ClaudeDesktop, Client::Cursor];

    fn label(self) -> &'static str {
        match self {
            Client::ClaudeCode => "Claude Code",
            Client::ClaudeDesktop => "Claude Desktop",
            Client::Cursor => "Cursor",
        }
    }

    /// The JSON file holding the client's `mcpServers`. None for Claude Code,
    /// whose config `claude mcp add` owns.
    fn config_file(self, home: &Path) -> Option<PathBuf> {
        match self {
            Client::ClaudeCode => None,
            Client::ClaudeDesktop if cfg!(target_os = "macos") => {
                Some(home.join("Library/Application Support/Claude/claude_desktop_config.json"))
            }
            Client::ClaudeDesktop => Some(home.join(".config/Claude/claude_desktop_config.json")),
            Client::Cursor => Some(home.join(".cursor/mcp.json")),
        }
    }
}

pub(crate) fn install_in(
    home: &Path,
    exe: &Path,
    client: Option<Client>,
    write: bool,
) -> Result<String> {
    let entry = json!({ "command": exe, "args": ["mcp"] });
    let clients = client.map_or(Client::ALL.to_vec(), |c| vec![c]);
    let mut out = Vec::new();
    for c in clients {
        out.push(match c.config_file(home) {
            None => format!(
                "{}{}:\n  claude mcp add spacehog -- {} mcp",
                c.label(),
                if write {
                    " owns its config, so run this yourself"
                } else {
                    ""
                },
                shell_quote(&exe.to_string_lossy())
            ),
            Some(file) if write => merge(c, &file, &entry, client.is_some())?,
            Some(file) => format!(
                "{} ({}):\n{}",
                c.label(),
                file.display(),
                serde_json::to_string_pretty(&json!({ "mcpServers": { "spacehog": entry } }))?
            ),
        });
    }
    Ok(out.join("\n\n"))
}

/// Adds or updates the `spacehog` server in a client config, keeping every
/// other key. Backs the file up first and refuses to touch invalid JSON.
fn merge(client: Client, file: &Path, entry: &Value, explicit: bool) -> Result<String> {
    let label = client.label();
    let dir = file.parent().context("config path has no parent")?;
    if !explicit && !dir.exists() {
        return Ok(format!(
            "{label}: skipped, {} does not exist (pass --client to create it)",
            dir.display()
        ));
    }
    let existing = match std::fs::read_to_string(file) {
        Ok(s) => Some(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| format!("reading {}", file.display())),
    };
    let mut root: Value = match existing.as_deref() {
        Some(s) if !s.trim().is_empty() => serde_json::from_str(s)
            .with_context(|| format!("{} is not valid JSON; left it untouched", file.display()))?,
        _ => json!({}),
    };
    let servers = root
        .as_object_mut()
        .with_context(|| format!("{} is not a JSON object", file.display()))?
        .entry("mcpServers")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .with_context(|| format!("mcpServers in {} is not an object", file.display()))?;
    if servers.get("spacehog") == Some(entry) {
        return Ok(format!("{label}: already configured in {}", file.display()));
    }
    let verb = if servers.contains_key("spacehog") {
        "updated"
    } else {
        "added"
    };
    servers.insert("spacehog".into(), entry.clone());
    let mut note = String::new();
    if existing.is_some() {
        let backup = file.with_extension("json.bak");
        std::fs::copy(file, &backup).with_context(|| format!("backing up {}", file.display()))?;
        note = format!(" (backup at {})", backup.display());
    }
    std::fs::create_dir_all(dir)?;
    std::fs::write(file, serde_json::to_string_pretty(&root)? + "\n")
        .with_context(|| format!("writing {}", file.display()))?;
    Ok(format!(
        "{label}: {verb} spacehog in {}{note}; restart {label} to load it",
        file.display()
    ))
}

fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+:@".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = "/opt/spacehog/bin/spacehog";

    fn read(path: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn write_keeps_other_servers_and_backs_up_the_original() {
        let home = tempfile::tempdir().unwrap();
        let file = home.path().join(".cursor/mcp.json");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let original = r#"{"theme":"dark","mcpServers":{"other":{"command":"x"}}}"#;
        std::fs::write(&file, original).unwrap();

        let msg = install_in(home.path(), Path::new(EXE), Some(Client::Cursor), true).unwrap();

        let cfg = read(&file);
        assert_eq!(cfg["theme"], "dark");
        assert_eq!(cfg["mcpServers"]["other"]["command"], "x");
        assert_eq!(cfg["mcpServers"]["spacehog"]["command"], EXE);
        assert_eq!(cfg["mcpServers"]["spacehog"]["args"], json!(["mcp"]));
        let backup = file.with_extension("json.bak");
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), original);
        assert!(msg.contains("added spacehog"), "{msg}");

        let again = install_in(home.path(), Path::new(EXE), Some(Client::Cursor), true).unwrap();
        assert!(again.contains("already configured"), "{again}");
        assert_eq!(read(&file), cfg);
    }

    #[test]
    fn write_refuses_to_clobber_invalid_json() {
        let home = tempfile::tempdir().unwrap();
        let file = home.path().join(".cursor/mcp.json");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "{ not json").unwrap();

        let err = install_in(home.path(), Path::new(EXE), Some(Client::Cursor), true).unwrap_err();

        assert!(err.to_string().contains("not valid JSON"), "{err}");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{ not json");
    }

    #[test]
    fn write_without_a_client_skips_apps_that_are_not_installed() {
        let home = tempfile::tempdir().unwrap();

        let msg = install_in(home.path(), Path::new(EXE), None, true).unwrap();

        assert!(msg.contains("Cursor: skipped"), "{msg}");
        assert!(msg.contains("Claude Desktop: skipped"), "{msg}");
        assert!(msg.contains(&format!("claude mcp add spacehog -- {EXE} mcp")));
        assert!(!home.path().join(".cursor").exists());
    }

    #[test]
    fn print_shows_all_three_clients_and_quotes_awkward_paths() {
        let home = tempfile::tempdir().unwrap();
        let exe = Path::new("/Users/me/my tools/spacehog");

        let msg = install_in(home.path(), exe, None, false).unwrap();

        assert!(msg.contains("claude mcp add spacehog -- '/Users/me/my tools/spacehog' mcp"));
        assert!(msg.contains("claude_desktop_config.json"));
        assert!(msg.contains(".cursor/mcp.json"));
        assert!(msg.contains(r#""command": "/Users/me/my tools/spacehog""#));
    }
}
