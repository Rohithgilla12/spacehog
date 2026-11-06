//! Drives `spacehog_mcp::serve` over real stdio pipes with hand-written
//! JSON-RPC. The test binary re-executes itself as the server (see `main`), so
//! the child runs exactly what `spacehog mcp` runs, against a fixture index.

#[path = "../../core/tests/common/mod.rs"]
mod common;

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use serde_json::{Value, json};

const CHILD: &str = "SPACEHOG_MCP_TEST_CHILD";
const CLIENT: &str = "stdio-test";

fn main() {
    if std::env::var_os(CHILD).is_some() {
        if let Err(e) = spacehog_mcp::serve() {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
        return;
    }
    stdio_session();
    println!("test stdio_session ... ok");
}

struct Session {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    next_id: u64,
}

impl Session {
    fn spawn(home: &Path, data: &Path, logs: &Path, config: &Path, trash: &Path) -> Self {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .env(CHILD, "1")
            .env("HOME", home)
            .env("SPACEHOG_DATA_DIR", data)
            .env("SPACEHOG_STATE_DIR", logs)
            .env("SPACEHOG_CONFIG_DIR", config)
            .env("SPACEHOG_TRASH_DIR", trash)
            .env_remove("SPACEHOG_DB")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            stdin: child.stdin.take(),
            child,
            lines,
            next_id: 0,
        }
    }

    fn send(&mut self, msg: Value) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{msg}").unwrap();
        stdin.flush().unwrap();
    }

    /// Reads stdout until the response to `id`, asserting every line on the
    /// way is a JSON-RPC frame.
    fn response(&mut self, id: u64) -> Value {
        loop {
            let line = self
                .lines
                .recv_timeout(Duration::from_secs(60))
                .expect("server answered within 60 s");
            let frame: Value = serde_json::from_str(&line)
                .unwrap_or_else(|e| panic!("stdout carried a non-JSON line ({e}): {line:?}"));
            assert_eq!(frame["jsonrpc"], "2.0", "not a JSON-RPC frame: {line}");
            if frame["id"] == json!(id) {
                return frame;
            }
        }
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        let frame = self.response(id);
        assert!(frame["error"].is_null(), "{method} failed: {frame}");
        frame["result"].clone()
    }

    fn call_raw(&mut self, tool: &str, args: Value) -> Value {
        self.request("tools/call", json!({ "name": tool, "arguments": args }))
    }

    fn call(&mut self, tool: &str, args: Value) -> Value {
        let result = self.call_raw(tool, args);
        assert_eq!(
            result["isError"], false,
            "{tool} returned an error: {result}"
        );
        let text: Value =
            serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(
            text, result["structuredContent"],
            "text content mirrors the structured result"
        );
        result["structuredContent"].clone()
    }

    fn close(mut self) {
        drop(self.stdin.take());
        for _ in 0..100 {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success(), "server exited with {status}");
                let trailing: Vec<String> = self.lines.iter().collect();
                assert!(
                    trailing.is_empty(),
                    "stdout after the session: {trailing:?}"
                );
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        self.child.kill().unwrap();
        panic!("server did not exit within 10 s of stdin closing");
    }
}

fn stdio_session() {
    let (_home_dir, home) = common::home();
    let data = tempfile::tempdir().unwrap();
    let logs = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    let trash = tempfile::tempdir().unwrap();
    drop(common::scan(&home, data.path()));
    let stale = home.join("code/old/node_modules");
    let essay = home.join("Documents/essay.txt");

    let mut s = Session::spawn(&home, data.path(), logs.path(), config.path(), trash.path());

    let init = s.request(
        "initialize",
        json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": CLIENT, "version": "0" }
        }),
    );
    assert_eq!(init["serverInfo"]["name"], "spacehog");
    assert_eq!(init["protocolVersion"], "2025-06-18");
    let instructions = init["instructions"].as_str().unwrap();
    assert!(instructions.contains("propose_cleanup") && instructions.contains("approve"));
    for cap in ["tools", "resources", "prompts"] {
        assert!(init["capabilities"][cap].is_object(), "{cap} capability");
    }
    s.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));

    let tools = s.request("tools/list", json!({}));
    let mut names: Vec<&str> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "changes",
            "execute_cleanup",
            "explain",
            "list_proposals",
            "propose_cleanup",
            "query",
            "rules",
            "scan",
            "status",
            "summary",
            "top",
            "undo"
        ]
    );

    let status = s.call("status", json!({}));
    assert_eq!(status["last_scan"]["root"], home.to_str().unwrap());
    assert_eq!(status["last_scan"]["status"], "done");
    assert!(status["last_scan"]["bytes_human"].is_string());
    assert_eq!(status["pending_proposals"], 0);

    let top = s.call(
        "top",
        json!({ "category": "dev-caches", "stale_days": 90, "safety": ["regenerable"], "kind": null }),
    );
    let items = top["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "only the untouched project is stale: {top}");
    assert_eq!(items[0]["path"], stale.to_str().unwrap());
    assert_eq!(items[0]["bucket"], "dev-caches");
    assert_eq!(items[0]["rule_id"], "node-modules");
    assert!(items[0]["stale_days"].as_i64().unwrap() >= 199);
    assert_eq!(items[0]["size_human"], "4.1 kB");

    let explained = s.call("explain", json!({ "path": stale }));
    assert_eq!(explained["item"]["rule_id"], "node-modules");
    let staleness = explained["staleness"].as_str().unwrap();
    assert!(staleness.starts_with("stale:"), "{staleness}");

    let unknown = s.call_raw("top", json!({ "category": "no-such-bucket" }));
    assert_eq!(unknown["isError"], true);
    assert_eq!(unknown["structuredContent"]["error"], "invalid_params");

    let proposed = s.call(
        "propose_cleanup",
        json!({ "paths": [stale, essay], "rationale": "stale node_modules in an abandoned project" }),
    );
    let proposal_id = proposed["proposal_id"].as_str().unwrap().to_string();
    assert!(proposal_id.starts_with('p'), "{proposal_id}");
    assert_eq!(proposed["accepted"][0]["path"], stale.to_str().unwrap());
    assert_eq!(proposed["rejected"][0]["path"], essay.to_str().unwrap());
    let reason = proposed["rejected"][0]["reason"].as_str().unwrap();
    assert!(reason.starts_with("protected"), "{reason}");
    assert_eq!(proposed["requires_approval"], true);

    let pending = s.call("list_proposals", json!({ "status": "pending" }));
    assert_eq!(pending["proposals"][0]["id"], proposal_id.as_str());
    assert_eq!(pending["proposals"][0]["agent"], CLIENT);

    let total = proposed["total_human"].as_str().unwrap().to_string();
    let unapproved = s.call_raw(
        "execute_cleanup",
        json!({ "proposal_id": proposal_id, "confirm": total }),
    );
    assert_eq!(unapproved["structuredContent"]["error"], "refused");
    let why = unapproved["structuredContent"]["message"].as_str().unwrap();
    assert!(why.contains("pending approval"), "{why}");
    assert!(stale.exists(), "nothing runs before a human approves");

    spacehog_core::Index::open(&data.path().join("index.db"))
        .unwrap()
        .approve(&proposal_id)
        .unwrap();
    let careless = s.call_raw(
        "execute_cleanup",
        json!({ "proposal_id": proposal_id, "confirm": "yes" }),
    );
    assert_eq!(careless["structuredContent"]["error"], "refused");

    let executed = s.call(
        "execute_cleanup",
        json!({ "proposal_id": proposal_id, "confirm": total }),
    );
    assert_eq!(executed["status"], "executed");
    assert_eq!(executed["results"][0]["outcome"], "trashed");
    assert!(!stale.exists());
    assert!(
        trash.path().join("node_modules").exists(),
        "landed in the trash dir"
    );
    let journal_id = executed["journal_id"].as_str().unwrap().to_string();

    let undone = s.call("undo", json!({ "journal_id": journal_id }));
    assert_eq!(undone["items"][0]["outcome"], "restored");
    assert!(stale.join("x/index.js").exists(), "undo puts it back");

    let resource = s.request("resources/read", json!({ "uri": "spacehog://proposals" }));
    let body: Value =
        serde_json::from_str(resource["contents"][0]["text"].as_str().unwrap()).unwrap();
    assert!(
        body["proposals"].as_array().unwrap().is_empty(),
        "the executed proposal is no longer pending: {body}"
    );

    let prompt = s.request(
        "prompts/get",
        json!({ "name": "free-space", "arguments": { "gb": "20" } }),
    );
    let text = prompt["messages"][0]["content"]["text"].as_str().unwrap();
    assert!(
        text.starts_with("Find the safest way to free 20 GB"),
        "{text}"
    );

    let scanned = s.call(
        "scan",
        json!({ "root": home, "wait": true, "incremental": true }),
    );
    assert_eq!(scanned["started"], true);
    assert_eq!(scanned["status"], "done");
    assert!(
        scanned["note"]
            .as_str()
            .unwrap()
            .contains("not supported yet")
    );
    assert!(!scanned["summary"]["buckets"].as_array().unwrap().is_empty());
    let after = s.call("status", json!({}));
    assert_eq!(after["last_scan"]["id"], scanned["scan_id"]);
    let changed = s.call("changes", json!({ "since": null }));
    assert_eq!(changed["to"]["id"], scanned["scan_id"]);
    assert!(changed["buckets"].is_array());
    let bad = s.call_raw("changes", json!({ "since": "7 fortnights" }));
    assert_eq!(bad["structuredContent"]["error"], "invalid_params");

    s.close();
    let log = std::fs::read_to_string(logs.path().join("mcp.log")).unwrap();
    assert!(log.contains("serving on stdio"), "{log}");
    assert!(log.contains("tool=\"propose_cleanup\""), "{log}");
}
