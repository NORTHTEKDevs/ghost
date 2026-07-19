//! Shell control: one-shot commands and persistent PowerShell sessions.
//!
//! Two modes exist behind the single `ghost_shell` MCP verb:
//!   * one-shot `run` — spawn a shell, run one command, capture merged output.
//!   * persistent sessions (`open`/`send`/`read`/`kill`) — a long-lived PowerShell
//!     process whose variables, cwd and env persist across commands.
//!
//! Persistent framing: the driver reads `<nonce> <base64(utf8 cmd)>` lines from
//! stdin, `Invoke-Expression`s the decoded command with stderr merged, then emits
//! a sentinel line `__GHOST_DONE_<nonce>__ <exitcode>`. base64 makes any command
//! text injection-safe; the per-session nonce means a late sentinel from a
//! timed-out command can never be mistaken for a later command's sentinel.
//!
//! Kill-switch: `GHOST_SHELL=off` makes every op return an error.

use std::collections::HashMap;
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use crate::error::{GhostError, Result};
use crate::session::GhostSession;

/// Per-response output cap (chars). Protects the agent context window.
const MAX_OUTPUT_CHARS: usize = 24_000;
/// Default per-command timeout.
const DEFAULT_TIMEOUT_MS: u64 = 30_000;
/// Hard ceiling on any timeout the caller can request.
const MAX_TIMEOUT_MS: u64 = 600_000;
/// How often the read loop wakes to check the stop flag / deadline.
const POLL_MS: u64 = 200;

/// One persistent PowerShell process.
struct ShellSession {
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    /// Monotonic per-session command counter; also the sentinel nonce.
    nonce: u64,
    /// Set when a `send` timed out and its command is still running. Holds the
    /// nonce whose sentinel `read` must still drain before the session is usable.
    pending: Option<u64>,
    created: Instant,
    pid: Option<u32>,
}

/// Registry of persistent sessions, held in a RefCell on GhostSession (all calls
/// run on the single STA block_on thread, so no locking is required).
#[derive(Default)]
pub struct ShellRegistry {
    sessions: HashMap<String, ShellSession>,
    auto_id: u64,
}

impl ShellRegistry {
    pub fn new() -> Self {
        Self::default()
    }
}

fn shell_disabled() -> bool {
    matches!(std::env::var("GHOST_SHELL"), Ok(v) if v.trim().eq_ignore_ascii_case("off"))
}

/// Standard-alphabet base64 (encode only). Avoids pulling a dependency for a
/// dozen lines; the persistent driver decodes with [Convert]::FromBase64String.
fn b64_encode(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 { T[((n >> 6) & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[(n & 63) as usize] as char } else { '=' });
    }
    out
}

/// PowerShell `-EncodedCommand` payload: base64 of the UTF-16LE script bytes.
fn ps_encoded_command(script: &str) -> String {
    let mut utf16le = Vec::with_capacity(script.len() * 2);
    for u in script.encode_utf16() {
        utf16le.extend_from_slice(&u.to_le_bytes());
    }
    b64_encode(&utf16le)
}

/// The persistent-session driver loop. Reads framed commands, runs them,
/// prints a nonce-stamped sentinel after each.
const DRIVER_SCRIPT: &str = r#"
$ErrorActionPreference='Continue'
[Console]::OutputEncoding=[System.Text.Encoding]::UTF8
while($true){
  $line=[Console]::In.ReadLine()
  if($null -eq $line){break}
  if($line.Length -eq 0){continue}
  $sp=$line.IndexOf(' ')
  if($sp -lt 0){continue}
  $nonce=$line.Substring(0,$sp)
  $b64=$line.Substring($sp+1)
  $cmd=[System.Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($b64))
  $global:LASTEXITCODE=0
  try{$o=Invoke-Expression $cmd 2>&1|Out-String;[Console]::Out.Write($o)}catch{[Console]::Out.Write(($_|Out-String))}
  $code=$LASTEXITCODE; if($null -eq $code){$code=0}
  [Console]::Out.WriteLine("__GHOST_DONE_${nonce}__ $code")
  [Console]::Out.Flush()
}
"#;

fn clamp_timeout(ms: Option<u64>) -> Duration {
    Duration::from_millis(ms.unwrap_or(DEFAULT_TIMEOUT_MS).min(MAX_TIMEOUT_MS))
}

/// Tail-truncate to the output cap, appending a marker with the dropped byte count.
fn cap_output(mut s: String) -> (String, bool) {
    if s.chars().count() <= MAX_OUTPUT_CHARS {
        return (s, false);
    }
    let keep_from = s.char_indices().rev().nth(MAX_OUTPUT_CHARS - 1).map(|(i, _)| i).unwrap_or(0);
    let dropped = keep_from;
    s = format!("...[{dropped} bytes truncated]...\n{}", &s[keep_from..]);
    (s, true)
}

impl GhostSession {
    /// Dispatch entry for the `ghost_shell` MCP verb.
    pub async fn shell(&self, args: &Value) -> Result<Value> {
        if shell_disabled() {
            return Err(GhostError::Config(
                "ghost_shell is disabled (GHOST_SHELL=off). Unset the env var to enable shell control.".into(),
            ));
        }
        let op = args.get("op").and_then(|v| v.as_str()).unwrap_or("run");
        match op {
            "run" => self.shell_run(args).await,
            "open" => self.shell_open(args).await,
            "send" => self.shell_send(args).await,
            "read" => self.shell_read(args).await,
            "list" => Ok(self.shell_list()),
            "kill" => self.shell_kill(args).await,
            other => Err(GhostError::Config(format!(
                "ghost_shell: unknown op '{other}'; use run|open|send|read|list|kill"
            ))),
        }
    }

    async fn shell_run(&self, args: &Value) -> Result<Value> {
        let cmd = args
            .get("cmd")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GhostError::Config("ghost_shell op=run: missing 'cmd'".into()))?;
        let shell = args.get("shell").and_then(|v| v.as_str()).unwrap_or("powershell");
        let cwd = args.get("cwd").and_then(|v| v.as_str());
        let dur = clamp_timeout(args.get("timeout_ms").and_then(|v| v.as_u64()));

        let mut command = build_oneshot(shell, cmd)?;
        if let Some(dir) = cwd {
            command.current_dir(dir);
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let started = Instant::now();
        let mut child = command
            .spawn()
            .map_err(|e| GhostError::Config(format!("failed to spawn {shell}: {e}")))?;
        let mut stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();

        let mut obuf = Vec::new();
        let mut ebuf = Vec::new();
        let collect = async {
            use tokio::io::AsyncReadExt;
            let _ = tokio::join!(stdout.read_to_end(&mut obuf), stderr.read_to_end(&mut ebuf));
        };

        let mut timed_out = false;
        let mut stopped = false;
        tokio::select! {
            _ = collect => {}
            _ = tokio::time::sleep(dur) => { timed_out = true; }
            _ = wait_for_stop() => { stopped = true; }
        }
        if timed_out || stopped {
            let _ = child.start_kill();
        }
        let status = child.wait().await.ok();

        if stopped {
            return Err(GhostError::Stopped);
        }

        let mut merged = String::from_utf8_lossy(&obuf).into_owned();
        if !ebuf.is_empty() {
            let err = String::from_utf8_lossy(&ebuf);
            // PowerShell serializes its own error/progress records to stderr as
            // CLIXML when stderr is redirected. Native-child stderr (git, node) is
            // plain text and passes through; CLIXML noise is stripped to text.
            merged.push_str(&sanitize_ps_stderr(&err));
        }
        let (output, truncated) = cap_output(merged);
        Ok(json!({
            "ok": !timed_out,
            "output": output,
            "exit_code": status.and_then(|s| s.code()),
            "duration_ms": started.elapsed().as_millis() as u64,
            "truncated": truncated,
            "timed_out": timed_out,
            "shell": shell,
        }))
    }

    async fn shell_open(&self, args: &Value) -> Result<Value> {
        let cwd = args.get("cwd").and_then(|v| v.as_str());
        let id = match args.get("id").and_then(|v| v.as_str()) {
            Some(s) if !s.trim().is_empty() => s.to_string(),
            _ => {
                let mut reg = self.shells.borrow_mut();
                reg.auto_id += 1;
                format!("s{}", reg.auto_id)
            }
        };
        if self.shells.borrow().sessions.contains_key(&id) {
            return Err(GhostError::Config(format!(
                "ghost_shell op=open: session '{id}' already exists"
            )));
        }

        let mut command = Command::new("powershell");
        command
            .args(["-NoProfile", "-NoLogo", "-NonInteractive", "-EncodedCommand"])
            .arg(ps_encoded_command(DRIVER_SCRIPT));
        if let Some(dir) = cwd {
            command.current_dir(dir);
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);

        let mut child = command
            .spawn()
            .map_err(|e| GhostError::Config(format!("failed to start persistent shell: {e}")))?;
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let pid = child.id();
        let session = ShellSession {
            child,
            stdin,
            reader: BufReader::new(stdout),
            nonce: 0,
            pending: None,
            created: Instant::now(),
            pid,
        };
        self.shells.borrow_mut().sessions.insert(id.clone(), session);
        Ok(json!({ "ok": true, "id": id, "pid": pid }))
    }

    async fn shell_send(&self, args: &Value) -> Result<Value> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GhostError::Config("ghost_shell op=send: missing 'id'".into()))?
            .to_string();
        let cmd = args
            .get("cmd")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GhostError::Config("ghost_shell op=send: missing 'cmd'".into()))?
            .to_string();
        let dur = clamp_timeout(args.get("timeout_ms").and_then(|v| v.as_u64()));

        // Take the session out of the registry so no RefCell borrow is held
        // across an await point; reinsert (or drop, if killed) when done.
        let mut sess = self
            .shells
            .borrow_mut()
            .sessions
            .remove(&id)
            .ok_or_else(|| GhostError::Config(format!("ghost_shell: no session '{id}'")))?;

        if sess.pending.is_some() {
            let pend = sess.pending;
            self.shells.borrow_mut().sessions.insert(id.clone(), sess);
            return Err(GhostError::Config(format!(
                "ghost_shell: session '{id}' is busy running command #{}; call op=read to drain it first",
                pend.unwrap()
            )));
        }

        sess.nonce += 1;
        let nonce = sess.nonce;
        let frame = format!("{} {}\n", nonce, b64_encode(cmd.as_bytes()));
        if let Err(e) = sess.stdin.write_all(frame.as_bytes()).await {
            let _ = sess.child.start_kill();
            return Err(GhostError::Config(format!("ghost_shell: session '{id}' write failed: {e}")));
        }
        let _ = sess.stdin.flush().await;

        let outcome = read_until_sentinel(&mut sess, nonce, dur).await;
        self.finish_send(id, sess, nonce, outcome).await
    }

    async fn shell_read(&self, args: &Value) -> Result<Value> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GhostError::Config("ghost_shell op=read: missing 'id'".into()))?
            .to_string();
        let dur = clamp_timeout(args.get("timeout_ms").and_then(|v| v.as_u64()).or(Some(0)));

        let mut sess = self
            .shells
            .borrow_mut()
            .sessions
            .remove(&id)
            .ok_or_else(|| GhostError::Config(format!("ghost_shell: no session '{id}'")))?;

        let nonce = match sess.pending {
            Some(n) => n,
            None => {
                self.shells.borrow_mut().sessions.insert(id.clone(), sess);
                return Ok(json!({ "ok": true, "id": id, "output": "", "busy": false, "note": "no command pending" }));
            }
        };
        let outcome = read_until_sentinel(&mut sess, nonce, dur).await;
        self.finish_send(id, sess, nonce, outcome).await
    }

    /// Common tail for send/read: apply the read outcome to session state, then
    /// reinsert the session (or drop it on a stop-kill) and build the response.
    async fn finish_send(
        &self,
        id: String,
        mut sess: ShellSession,
        nonce: u64,
        outcome: ReadOutcome,
    ) -> Result<Value> {
        match outcome {
            ReadOutcome::Done { output, exit_code } => {
                sess.pending = None;
                let (output, truncated) = cap_output(output);
                self.shells.borrow_mut().sessions.insert(id.clone(), sess);
                Ok(json!({
                    "ok": true, "id": id, "output": output,
                    "exit_code": exit_code, "truncated": truncated,
                    "timed_out": false, "busy": false,
                }))
            }
            ReadOutcome::TimedOut { output } => {
                sess.pending = Some(nonce);
                let (output, truncated) = cap_output(output);
                self.shells.borrow_mut().sessions.insert(id.clone(), sess);
                Ok(json!({
                    "ok": false, "id": id, "output": output,
                    "truncated": truncated, "timed_out": true, "busy": true,
                    "note": "command #".to_string() + &nonce.to_string() + " still running; call op=read to collect the rest",
                }))
            }
            ReadOutcome::Stopped => {
                let _ = sess.child.start_kill();
                Err(GhostError::Stopped)
            }
            ReadOutcome::Eof { output } => {
                // Driver process exited unexpectedly. Drop the dead session.
                let _ = sess.child.start_kill();
                let (output, _) = cap_output(output);
                Err(GhostError::Config(format!(
                    "ghost_shell: session '{id}' ended unexpectedly. Partial output: {output}"
                )))
            }
        }
    }

    fn shell_list(&self) -> Value {
        let reg = self.shells.borrow();
        let sessions: Vec<Value> = reg
            .sessions
            .iter()
            .map(|(id, s)| {
                json!({
                    "id": id,
                    "pid": s.pid,
                    "busy": s.pending.is_some(),
                    "age_ms": s.created.elapsed().as_millis() as u64,
                    "commands_run": s.nonce,
                })
            })
            .collect();
        json!({ "ok": true, "sessions": sessions })
    }

    async fn shell_kill(&self, args: &Value) -> Result<Value> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GhostError::Config("ghost_shell op=kill: missing 'id'".into()))?
            .to_string();
        let mut sess = self
            .shells
            .borrow_mut()
            .sessions
            .remove(&id)
            .ok_or_else(|| GhostError::Config(format!("ghost_shell: no session '{id}'")))?;
        let _ = sess.child.start_kill();
        let _ = sess.child.wait().await;
        Ok(json!({ "ok": true, "id": id, "killed": true }))
    }
}

enum ReadOutcome {
    Done { output: String, exit_code: Option<i64> },
    TimedOut { output: String },
    Stopped,
    Eof { output: String },
}

/// Read lines from the session until the nonce-stamped sentinel appears, the
/// deadline passes, the stop flag fires, or stdout hits EOF. Everything before
/// the sentinel is the command's merged output.
async fn read_until_sentinel(sess: &mut ShellSession, nonce: u64, dur: Duration) -> ReadOutcome {
    let sentinel = format!("__GHOST_DONE_{nonce}__ ");
    let deadline = Instant::now() + dur;
    let mut output = String::new();

    loop {
        if ghost_core::input::hotkey::is_stopped() {
            return ReadOutcome::Stopped;
        }
        let now = Instant::now();
        if now >= deadline {
            return ReadOutcome::TimedOut { output };
        }
        let slice = (deadline - now).min(Duration::from_millis(POLL_MS));
        let mut line = String::new();
        match tokio::time::timeout(slice, sess.reader.read_line(&mut line)).await {
            Ok(Ok(0)) => return ReadOutcome::Eof { output }, // EOF: driver exited
            Ok(Ok(_)) => {
                if let Some(rest) = line.strip_prefix(&sentinel) {
                    let exit_code = rest.trim().parse::<i64>().ok();
                    return ReadOutcome::Done { output, exit_code };
                }
                output.push_str(&line);
            }
            Ok(Err(_)) => return ReadOutcome::Eof { output }, // pipe error
            Err(_) => { /* slice elapsed: loop to recheck stop/deadline */ }
        }
    }
}

/// Resolve on the next stop-flag rising edge. Polls the atomic on POLL_MS.
async fn wait_for_stop() {
    loop {
        if ghost_core::input::hotkey::is_stopped() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(POLL_MS)).await;
    }
}

/// Prepended to one-shot PowerShell commands: silence the progress stream so
/// "Preparing modules for first use" and friends don't leak as CLIXML on stderr.
/// Runs as the first statement, so the user's own `exit N` still propagates.
const PS_ONESHOT_PREAMBLE: &str = "$ProgressPreference='SilentlyContinue';\n";

/// Extract readable text from PowerShell CLIXML stderr, dropping the envelope.
/// Native (non-PowerShell) stderr has no CLIXML marker and passes through as-is.
fn sanitize_ps_stderr(s: &str) -> String {
    if !s.contains("#< CLIXML") {
        return s.to_string();
    }
    // CLIXML error records store their message in <S ...>text</S> string nodes.
    let mut out = String::new();
    let mut rest = s;
    while let Some(open) = rest.find("<S ") {
        let after = &rest[open..];
        if let (Some(gt), Some(close)) = (after.find('>'), after.find("</S>")) {
            if gt < close {
                let text = &after[gt + 1..close];
                out.push_str(&xml_unescape(text));
                rest = &after[close + 4..];
                continue;
            }
        }
        break;
    }
    out
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("_x000D_", "")
        .replace("_x000A_", "\n")
}

/// Build a one-shot command for the requested shell.
fn build_oneshot(shell: &str, cmd: &str) -> Result<Command> {
    let mut c = match shell {
        "powershell" => {
            let mut c = Command::new("powershell");
            c.args(["-NoProfile", "-NoLogo", "-NonInteractive", "-EncodedCommand"])
                .arg(ps_encoded_command(&format!("{PS_ONESHOT_PREAMBLE}{cmd}")));
            c
        }
        "pwsh" => {
            let mut c = Command::new("pwsh");
            c.args(["-NoProfile", "-NoLogo", "-NonInteractive", "-EncodedCommand"])
                .arg(ps_encoded_command(&format!("{PS_ONESHOT_PREAMBLE}{cmd}")));
            c
        }
        "cmd" => {
            let mut c = Command::new("cmd");
            c.args(["/S", "/C", cmd]);
            c
        }
        other => {
            return Err(GhostError::Config(format!(
                "ghost_shell: unknown shell '{other}'; use powershell|pwsh|cmd"
            )))
        }
    };
    // Reap the shell's own process tree isn't attempted here; grandchildren of a
    // Start-Process launch are intentionally left running.
    c.kill_on_drop(true);
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b64_matches_known_vectors() {
        assert_eq!(b64_encode(b""), "");
        assert_eq!(b64_encode(b"f"), "Zg==");
        assert_eq!(b64_encode(b"fo"), "Zm8=");
        assert_eq!(b64_encode(b"foo"), "Zm9v");
        assert_eq!(b64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(b64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(b64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn ps_encoded_is_utf16le_base64() {
        // "A" -> UTF-16LE bytes 0x41 0x00 -> base64 "QQA="
        assert_eq!(ps_encoded_command("A"), "QQA=");
    }

    #[test]
    fn cap_output_passes_short_strings() {
        let (s, t) = cap_output("hello".into());
        assert_eq!(s, "hello");
        assert!(!t);
    }

    #[test]
    fn cap_output_truncates_long_strings() {
        let big = "x".repeat(MAX_OUTPUT_CHARS + 500);
        let (s, t) = cap_output(big);
        assert!(t);
        assert!(s.contains("truncated"));
        assert!(s.chars().count() <= MAX_OUTPUT_CHARS + 40);
    }

    #[test]
    fn clamp_timeout_defaults_and_caps() {
        assert_eq!(clamp_timeout(None), Duration::from_millis(DEFAULT_TIMEOUT_MS));
        assert_eq!(clamp_timeout(Some(999_999_999)), Duration::from_millis(MAX_TIMEOUT_MS));
        assert_eq!(clamp_timeout(Some(1500)), Duration::from_millis(1500));
    }

    #[test]
    fn sanitize_passes_plain_native_stderr() {
        let s = "fatal: not a git repository\n";
        assert_eq!(sanitize_ps_stderr(s), s);
    }

    #[test]
    fn sanitize_strips_clixml_progress_to_empty() {
        let clixml = "#< CLIXML\r\n<Objs Version=\"1.1.0.1\"><Obj S=\"progress\"><TN><T>x</T></TN></Obj></Objs>";
        // A pure progress record has no <S ...> text nodes -> nothing readable.
        assert_eq!(sanitize_ps_stderr(clixml), "");
    }

    #[test]
    fn sanitize_extracts_clixml_error_text() {
        let clixml = "#< CLIXML\r\n<Objs><S S=\"Error\">boom went _x000A_the thing</S></Objs>";
        assert_eq!(sanitize_ps_stderr(clixml), "boom went \nthe thing");
    }

    #[test]
    fn shell_disabled_reads_env() {
        std::env::set_var("GHOST_SHELL", "off");
        assert!(shell_disabled());
        std::env::set_var("GHOST_SHELL", "OFF");
        assert!(shell_disabled());
        std::env::remove_var("GHOST_SHELL");
        assert!(!shell_disabled());
    }
}
