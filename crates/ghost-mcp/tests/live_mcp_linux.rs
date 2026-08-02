//! End-to-end MCP tests on Linux.
//!
//! Every other test exercises a layer. This one exercises the product: it spawns
//! the real `ghost-mcp` binary, speaks JSON-RPC to it over stdio exactly as
//! Claude Code does, and drives a real GTK application through it.
//!
//! That matters because the layers can each be correct while the whole is not --
//! a wrong argument name, a verb that never reaches the engine, a response
//! envelope the client cannot read. Those only show up from the outside.
//!
//! `#[ignore]`d by default; CI runs them with `--ignored` inside a synthesised
//! desktop (Xvfb + D-Bus + at-spi-bus-launcher). See `.github/workflows/linux.yml`.

#![cfg(target_os = "linux")]

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// Decode standard-alphabet base64. Written inline so the test needs no extra
/// dependency just to prove the screenshot payload is a real image.
fn base64_decode(s: &str) -> Option<Vec<u8>> {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    let mut out = Vec::new();
    for c in s.bytes() {
        if c == b'=' || c.is_ascii_whitespace() {
            continue;
        }
        let v = T.iter().position(|&t| t == c)? as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// A GTK app for the duration of one test, killed on drop even on panic.
struct TestApp(Child);

impl Drop for TestApp {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn launch_gtk_app(title: &str) -> TestApp {
    let child = Command::new("zenity")
        .args([
            "--forms",
            &format!("--title={title}"),
            "--text=ghost mcp test",
            "--add-entry=Name",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("zenity must be installed for the live suite");
    TestApp(child)
}

/// The MCP server under test, spoken to exactly as a client would.
struct McpServer {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
}

impl Drop for McpServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl McpServer {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_ghost-mcp"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Leave stderr attached so tracing output lands in the CI log; a
            // silent failure here is the hardest kind to diagnose.
            .stderr(Stdio::inherit())
            .spawn()
            .expect("ghost-mcp binary must start");

        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        Self { child, stdin, stdout, next_id: 0 }
    }

    /// Send a request and return its response, matched by id.
    ///
    /// Notifications and any interleaved output are skipped rather than assumed
    /// absent: a server is allowed to emit them and a client that cannot cope is
    /// broken.
    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        let req = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});

        writeln!(self.stdin, "{req}").expect("write request");
        self.stdin.flush().expect("flush");

        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            assert!(Instant::now() < deadline, "timed out waiting for a reply to {method}");

            let mut line = String::new();
            let n = self.stdout.read_line(&mut line).expect("read response");
            assert!(n > 0, "server closed stdout while waiting for {method}");

            let Ok(v) = serde_json::from_str::<Value>(line.trim()) else { continue };
            if v.get("id").and_then(|i| i.as_i64()) == Some(id) {
                return v;
            }
        }
    }

    fn initialize(&mut self) {
        let resp = self.request(
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "ghost-live-test", "version": "0"}
            }),
        );
        assert!(resp.get("result").is_some(), "initialize must succeed: {resp}");
    }

    /// Call a tool and return the text payload it produced.
    fn call(&mut self, name: &str, args: Value) -> String {
        let resp = self.request("tools/call", json!({"name": name, "arguments": args}));
        assert!(resp.get("error").is_none(), "{name} returned a protocol error: {resp}");

        resp["result"]["content"]
            .as_array()
            .and_then(|c| c.first())
            .and_then(|c| c["text"].as_str())
            .unwrap_or_else(|| panic!("{name} returned no text content: {resp}"))
            .to_string()
    }
}

/// Poll a tool until its payload satisfies `ready`, so tests do not race GTK
/// startup under Xvfb.
fn call_until(
    mcp: &mut McpServer,
    name: &str,
    args: Value,
    timeout: Duration,
    ready: impl Fn(&str) -> bool,
) -> String {
    let deadline = Instant::now() + timeout;
    loop {
        let last = mcp.call(name, args.clone());
        if ready(&last) {
            return last;
        }
        if Instant::now() >= deadline {
            return last;
        }
        std::thread::sleep(Duration::from_millis(400));
    }
}

#[test]
#[ignore = "live: needs an accessibility bus"]
fn server_starts_and_advertises_its_verbs() {
    let mut mcp = McpServer::start();
    mcp.initialize();

    // Parse the tools array. Substring-scanning the whole response would
    // false-pass on a verb that only appears inside another verb's description,
    // and the verbs deliberately cross-reference each other.
    let resp = mcp.request("tools/list", json!({}));
    let names: Vec<&str> = resp["result"]["tools"]
        .as_array()
        .expect("tools/list must return an array")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();

    for verb in ["ghost_see", "ghost_act", "ghost_window", "ghost_shell", "ghost_screenshot"] {
        assert!(names.contains(&verb), "tools/list must advertise {verb}; got {names:?}");
    }
}

#[test]
#[ignore = "live: needs an accessibility bus"]
fn shell_verb_advertises_linux_shells() {
    // An agent picks its shell from this schema. If it still said
    // powershell|cmd on Linux, every ghost_shell call an agent composed would be
    // wrong before it was sent.
    let mut mcp = McpServer::start();
    mcp.initialize();

    // Parse rather than string-search: verb descriptions cross-reference each
    // other, so scanning for the name lands in some other tool's prose.
    let resp = mcp.request("tools/list", json!({}));
    let tools = resp["result"]["tools"].as_array().expect("tools/list must return an array");
    let shell = tools
        .iter()
        .find(|t| t["name"] == "ghost_shell")
        .expect("ghost_shell must be advertised");

    let shells: Vec<&str> = shell["inputSchema"]["properties"]["shell"]["enum"]
        .as_array()
        .expect("the shell property must declare an enum")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();

    assert!(shells.contains(&"bash"), "the shell enum must offer bash on Linux, got {shells:?}");
    assert!(
        !shells.iter().any(|s| s.contains("powershell")),
        "the shell enum must not offer powershell on Linux, got {shells:?}"
    );

    let desc = shell["description"].as_str().unwrap_or_default();
    assert!(
        !desc.contains("PowerShell"),
        "the Linux description must not tell an agent to write PowerShell"
    );
}

#[test]
#[ignore = "live: needs an accessibility bus"]
fn shell_verb_runs_a_real_command() {
    // Proves the bash driver end to end: MCP -> session -> shell -> bash -> back.
    let mut mcp = McpServer::start();
    mcp.initialize();

    let out = mcp.call("ghost_shell", json!({"op": "run", "cmd": "echo ghost-shell-ok"}));
    assert!(out.contains("ghost-shell-ok"), "ghost_shell must return real output: {out}");
}

#[test]
#[ignore = "live: needs an accessibility bus"]
fn persistent_shell_keeps_state_between_sends() {
    // The persistent-session driver is the part with the nonce/base64 framing;
    // a variable surviving between calls is the proof it works.
    let mut mcp = McpServer::start();
    mcp.initialize();

    let opened = mcp.call("ghost_shell", json!({"op": "open"}));
    let id = serde_json::from_str::<Value>(&opened)
        .ok()
        .and_then(|v| v["data"]["id"].as_str().map(String::from))
        .or_else(|| {
            serde_json::from_str::<Value>(&opened).ok().and_then(|v| v["id"].as_str().map(String::from))
        })
        .unwrap_or_else(|| panic!("op=open must return a session id: {opened}"));

    mcp.call("ghost_shell", json!({"op": "send", "id": id, "cmd": "GHOST_TEST=alive"}));
    let out = mcp.call("ghost_shell", json!({"op": "send", "id": id, "cmd": "echo $GHOST_TEST"}));
    assert!(out.contains("alive"), "shell state must persist across sends: {out}");

    mcp.call("ghost_shell", json!({"op": "kill", "id": id}));
}

#[test]
#[ignore = "live: needs an accessibility bus"]
fn window_verb_lists_a_real_application() {
    let _app = launch_gtk_app("GhostMcpWindowProbe");
    let mut mcp = McpServer::start();
    mcp.initialize();

    let out = call_until(
        &mut mcp,
        "ghost_window",
        json!({"op": "list"}),
        Duration::from_secs(25),
        |t| t.contains("GhostMcpWindowProbe"),
    );
    assert!(out.contains("GhostMcpWindowProbe"), "ghost_window must list the app: {out}");
}

#[test]
#[ignore = "live: needs an accessibility bus"]
fn see_verb_returns_real_elements() {
    let _app = launch_gtk_app("GhostMcpSeeProbe");
    let mut mcp = McpServer::start();
    mcp.initialize();

    let out = call_until(
        &mut mcp,
        "ghost_see",
        json!({"mode": "full", "window": "GhostMcpSeeProbe"}),
        Duration::from_secs(25),
        |t| t.contains("\"role\""),
    );

    assert!(out.contains("\"role\""), "ghost_see must return elements with roles: {out}");
    assert!(
        out.contains("button") || out.contains("edit"),
        "ghost_see must surface the dialog's controls: {out}"
    );
}

#[test]
#[ignore = "live: needs a display"]
fn screenshot_verb_returns_an_image() {
    let mut mcp = McpServer::start();
    mcp.initialize();

    let out = mcp.call("ghost_screenshot", json!({}));

    // Length alone is not evidence: a verbose JSON error envelope also exceeds
    // any byte threshold. Decode the payload and check the image magic number,
    // so only real image bytes can pass.
    let v: Value = serde_json::from_str(&out)
        .unwrap_or_else(|e| panic!("ghost_screenshot must return JSON ({e}): {out}"));
    assert_ne!(v["ok"], serde_json::json!(false), "ghost_screenshot failed: {out}");

    let b64 = ["jpeg_base64", "png_base64", "image_base64", "data"]
        .iter()
        .find_map(|k| v["data"][k].as_str().or_else(|| v[k].as_str()))
        .unwrap_or_else(|| panic!("no base64 image field in response: {out}"));

    let raw = base64_decode(b64).expect("image field must be valid base64");
    // Magic numbers as explicit bytes: the PNG signature contains CR/LF, which
    // is exactly the sort of thing that gets silently rewritten in transit.
    let is_png = raw.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    let is_jpeg = raw.starts_with(&[0xFF, 0xD8]);
    assert!(
        is_png || is_jpeg,
        "decoded payload must be a real PNG or JPEG, got {} bytes starting {:02x?}",
        raw.len(),
        &raw[..raw.len().min(8)]
    );
}
