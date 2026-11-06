use std::path::Path;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_spacehog");

/// Wraps a spacehog invocation so the OS refuses every network operation.
/// Returns None when this platform offers no such sandbox to an unprivileged user.
fn denied_network(args: &[&str]) -> Option<Command> {
    if cfg!(target_os = "macos") {
        let mut c = Command::new("/usr/bin/sandbox-exec");
        c.args(["-p", "(version 1)(allow default)(deny network*)", BIN])
            .args(args);
        return Some(c);
    }
    let probe = Command::new("unshare")
        .args(["-rn", "true"])
        .status()
        .ok()?;
    if !probe.success() {
        return None;
    }
    let mut c = Command::new("unshare");
    c.args(["-rn", BIN]).args(args);
    Some(c)
}

fn run(args: &[&str], data: &Path) -> Option<std::process::Output> {
    let mut cmd = denied_network(args)?;
    cmd.env("SPACEHOG_DATA_DIR", data)
        .env("SPACEHOG_STATE_DIR", data.join("state"));
    Some(cmd.output().expect("spawn sandboxed spacehog"))
}

#[test]
fn every_command_works_with_the_network_denied() {
    let tree = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tree.path().join("app/node_modules/x")).unwrap();
    std::fs::write(tree.path().join("app/package.json"), "{}").unwrap();
    std::fs::write(
        tree.path().join("app/node_modules/x/index.js"),
        vec![b'x'; 4096],
    )
    .unwrap();
    let data = tempfile::tempdir().unwrap();
    let root = tree.path().to_str().unwrap();

    let commands: &[&[&str]] = &[
        &["scan", root, "--json"],
        &["top", "--json"],
        &["summary", "--json"],
        &["status", "--json"],
        &["rules", "--json"],
        &["proposals", "--json"],
    ];
    for args in commands {
        let Some(out) = run(args, data.path()) else {
            eprintln!("no unprivileged network sandbox on this platform; skipping");
            return;
        };
        assert!(
            out.status.success(),
            "spacehog {args:?} failed with the network denied: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn the_sandbox_really_denies_network() {
    if !cfg!(target_os = "macos") {
        return;
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port().to_string();
    let nc = ["/usr/bin/nc", "-z", "-w", "2", "127.0.0.1", port.as_str()];
    let open = Command::new(nc[0]).args(&nc[1..]).output().unwrap();
    assert!(
        open.status.success(),
        "control: nc reaches the listener without the sandbox"
    );
    let sandboxed = Command::new("/usr/bin/sandbox-exec")
        .args(["-p", "(version 1)(allow default)(deny network*)"])
        .args(nc)
        .output()
        .unwrap();
    assert!(
        !sandboxed.status.success(),
        "the same connect must fail inside the sandbox"
    );
}

#[test]
fn no_network_client_crates_are_linked() {
    let out = Command::new(env!("CARGO"))
        .args([
            "tree", "-p", "spacehog", "-e", "normal", "--prefix", "none", "--format", "{p}",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    let tree = String::from_utf8_lossy(&out.stdout);
    for banned in [
        "reqwest ",
        "hyper ",
        "ureq ",
        "curl ",
        "isahc ",
        "surf ",
        "attohttpc ",
    ] {
        assert!(
            !tree.lines().any(|l| l.starts_with(banned)),
            "{banned}is in the dependency tree"
        );
    }
}
