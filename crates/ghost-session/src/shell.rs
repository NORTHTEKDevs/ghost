//! Shell control: one-shot commands and persistent PowerShell sessions.
//!
//! Two modes exist behind the single `ghost_shell` MCP verb:
//!   * one-shot `run` - spawn a shell, run one command, capture merged output.
//!   * persistent sessions (`open`/`send`/`read`/`kill`) - a long-lived PowerShell
//!     process whose variables, cwd and env persist across commands.
//!
//! Persistent framing: the driver reads `<nonce> <base64(utf8 cmd)>` lines from
//! stdin, `Invoke-Expression`s the decoded command with stderr merged, then emits
//! a sentinel line `__GHOST_DONE_<nonce>__ <exitcode>`, where `<nonce>` is an
//! unguessable per-session secret plus a counter, so command output cannot
//! forge a completion. base64 makes any command
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
    /// Monotonic per-session command counter.
    nonce: u64,
    /// Random per-session value mixed into every sentinel, so the completion
    /// marker cannot be predicted -- and therefore cannot be forged -- by
    /// anything the command prints. Without it the marker is
    /// `__GHOST_DONE_1__ 0`, which a command that echoes attacker-controlled
    /// text (a log line, a downloaded file, `cat` of an untrusted path) can
    /// emit itself: Ghost would then report someone else's exit code, treat the
    /// real output as belonging to the next command, and desynchronise the
    /// session for good.
    secret: String,
    /// Set when a `send` timed out and its command is still running. Holds the
    /// nonce whose sentinel `read` must still drain before the session is usable.
    pending: Option<u64>,
    created: Instant,
    pid: Option<u32>,
}

/// Registry of persistent sessions, held in a Mutex on GhostSession.
#[derive(Default)]
pub struct ShellRegistry {
    sessions: HashMap<String, ShellSession>,
    auto_id: u64,
    /// A pre-spawned PowerShell running the persistent driver, waiting for
    /// exactly one `op=run` command. Measured: a fresh `powershell -Command`
    /// costs 232-447 ms of process start and module prep on this machine, and
    /// `op=run` is 60% of all Ghost calls. Handing the command to a process
    /// that already finished starting brings that to single-digit ms. The spare
    /// is replaced right after it is taken, so the next call is warm too.
    warm: Option<ShellSession>,
}

impl ShellRegistry {
    pub fn new() -> Self {
        Self::default()
    }
}

fn shell_disabled() -> bool {
    matches!(std::env::var("GHOST_SHELL"), Ok(v) if v.trim().eq_ignore_ascii_case("off"))
}

/// `GHOST_SHELL_WARM=off` disables the pre-spawned spare (one idle PowerShell
/// per ghost process, roughly 40 MB).
fn warm_disabled() -> bool {
    matches!(std::env::var("GHOST_SHELL_WARM"), Ok(v) if v.trim().eq_ignore_ascii_case("off"))
}

/// Prefix that moves a driver-run command into `cwd`. Single quotes are the
/// PowerShell literal string delimiter and are escaped by doubling.
fn cwd_prefix(cwd: &str) -> String {
    format!("Set-Location -LiteralPath '{}'; ", cwd.replace('\'', "''"))
}

/// Start one persistent driver process (PowerShell here, bash on Linux).
/// Shared by `op=open` sessions and the warm spare.
fn spawn_driver(cwd: Option<&str>) -> Result<ShellSession> {
    #[cfg(target_os = "linux")]
    let mut command = {
        let mut c = Command::new("bash");
        c.args(["--noprofile", "--norc", "-c", BASH_DRIVER_SCRIPT]);
        c
    };
    #[cfg(not(target_os = "linux"))]
    let mut command = {
        let mut c = Command::new("powershell");
        c.args(["-NoProfile", "-NoLogo", "-NonInteractive", "-EncodedCommand"])
            .arg(ps_encoded_command(DRIVER_SCRIPT));
        c
    };
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
    Ok(ShellSession {
        child,
        stdin,
        reader: BufReader::new(stdout),
        nonce: 0,
        secret: session_secret(),
        pending: None,
        created: Instant::now(),
        pid,
    })
}

/// Standard-alphabet base64 (encode only). Avoids pulling a dependency for a
/// dozen lines; the persistent driver decodes with [Convert]::FromBase64String.
fn b64_encode(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
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
/// Used by the `pwsh` one-shot arm, which is reachable on Linux too.
fn ps_encoded_command(script: &str) -> String {
    let mut utf16le = Vec::with_capacity(script.len() * 2);
    for u in script.encode_utf16() {
        utf16le.extend_from_slice(&u.to_le_bytes());
    }
    b64_encode(&utf16le)
}

/// The persistent-session driver loop. Reads framed commands, runs them,
/// prints a nonce-stamped sentinel after each.
#[cfg(not(target_os = "linux"))]
const DRIVER_SCRIPT: &str = r#"
$ErrorActionPreference='Continue'
$ProgressPreference='SilentlyContinue'
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

/// The same persistent-session protocol as `DRIVER_SCRIPT`, for POSIX shells.
///
/// Reads `<nonce> <base64-command>` lines, evaluates the command with stdout and
/// stderr merged, then writes the sentinel plus the exit code. Base64 framing is
/// what makes this injection-safe: the command text never has to survive shell
/// quoting on the way in.
#[cfg(target_os = "linux")]
const BASH_DRIVER_SCRIPT: &str = r#"
while IFS= read -r line; do
  [ -z "$line" ] && continue
  nonce="${line%% *}"
  b64="${line#* }"
  cmd="$(printf '%s' "$b64" | base64 -d 2>/dev/null)"
  eval "$cmd" 2>&1
  code=$?
  echo "__GHOST_DONE_${nonce}__ $code"
done
"#;

/// The shell used when the caller does not name one.
pub(crate) fn default_shell() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        "bash"
    }
    #[cfg(not(target_os = "linux"))]
    {
        "powershell"
    }
}

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
        let shell = args.get("shell").and_then(|v| v.as_str()).unwrap_or(default_shell());
        let cwd = args.get("cwd").and_then(|v| v.as_str());
        let dur = clamp_timeout(args.get("timeout_ms").and_then(|v| v.as_u64()));

        // Warm path: hand the command to the pre-spawned PowerShell. Only the
        // default shell qualifies (cmd starts in ~15 ms anyway; pwsh is rarely
        // installed). A spare that died is discarded; either way the next
        // spare is started before this command runs, so its warm-up overlaps
        // the agent's thinking instead of the agent's waiting.
        if cfg!(not(target_os = "linux")) && shell == "powershell" && !warm_disabled() {
            let spare = self.shells.lock().unwrap().warm.take();
            self.replenish_warm();
            if let Some(mut sess) = spare {
                if matches!(sess.child.try_wait(), Ok(None)) {
                    return self.run_on_spare(sess, cmd, cwd, dur, shell).await;
                }
            }
        }

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

    /// Start the next spare. Process creation is a few milliseconds; the
    /// PowerShell warm-up itself happens in the child, off the request path.
    fn replenish_warm(&self) {
        match spawn_driver(None) {
            Ok(s) => self.shells.lock().unwrap().warm = Some(s),
            Err(e) => tracing::warn!("ghost_shell: could not start a warm spare: {e}"),
        }
    }

    /// Run one `op=run` command on a warm spare. Same response shape as the
    /// one-shot path plus `warm: true`. The spare is single-use: it is killed
    /// once the command has answered (or timed out), never reused, so state
    /// cannot leak between commands.
    async fn run_on_spare(
        &self,
        mut sess: ShellSession,
        cmd: &str,
        cwd: Option<&str>,
        dur: Duration,
        shell: &str,
    ) -> Result<Value> {
        let started = Instant::now();
        let script = match cwd {
            Some(dir) => format!("{}{cmd}", cwd_prefix(dir)),
            None => cmd.to_string(),
        };
        sess.nonce += 1;
        let token = sentinel_token(&sess.secret, sess.nonce);
        let frame = format!("{} {}\n", token, b64_encode(script.as_bytes()));
        if let Err(e) = sess.stdin.write_all(frame.as_bytes()).await {
            let _ = sess.child.start_kill();
            return Err(GhostError::Config(format!("ghost_shell: warm shell write failed: {e}")));
        }
        let _ = sess.stdin.flush().await;

        let outcome = read_until_sentinel(&mut sess, &token, dur).await;
        let duration_ms = started.elapsed().as_millis() as u64;
        match outcome {
            ReadOutcome::Done { output, exit_code } => {
                let _ = sess.child.start_kill();
                let (output, truncated) = cap_output(output);
                Ok(json!({
                    "ok": true, "output": output, "exit_code": exit_code,
                    "duration_ms": duration_ms, "truncated": truncated,
                    "timed_out": false, "shell": shell, "warm": true,
                }))
            }
            ReadOutcome::TimedOut { output } => {
                let _ = sess.child.start_kill();
                let (output, truncated) = cap_output(output);
                Ok(json!({
                    "ok": false, "output": output, "exit_code": Value::Null,
                    "duration_ms": duration_ms, "truncated": truncated,
                    "timed_out": true, "shell": shell, "warm": true,
                }))
            }
            ReadOutcome::Stopped => {
                let _ = sess.child.start_kill();
                Err(GhostError::Stopped)
            }
            ReadOutcome::Eof { output } => {
                // The command ended the shell itself (`exit N`): the process exit
                // status is the command's exit code, exactly as in the one-shot path.
                let status = sess.child.wait().await.ok();
                let (output, truncated) = cap_output(output);
                Ok(json!({
                    "ok": true, "output": output,
                    "exit_code": status.and_then(|s| s.code()),
                    "duration_ms": duration_ms, "truncated": truncated,
                    "timed_out": false, "shell": shell, "warm": true,
                }))
            }
        }
    }

    async fn shell_open(&self, args: &Value) -> Result<Value> {
        let cwd = args.get("cwd").and_then(|v| v.as_str());
        let id = match args.get("id").and_then(|v| v.as_str()) {
            Some(s) if !s.trim().is_empty() => s.to_string(),
            _ => {
                let mut reg = self.shells.lock().unwrap();
                reg.auto_id += 1;
                format!("s{}", reg.auto_id)
            }
        };
        if self.shells.lock().unwrap().sessions.contains_key(&id) {
            return Err(GhostError::Config(format!(
                "ghost_shell op=open: session '{id}' already exists"
            )));
        }

        let session = spawn_driver(cwd)?;
        let pid = session.pid;
        self.shells.lock().unwrap().sessions.insert(id.clone(), session);
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
            .lock().unwrap()
            .sessions
            .remove(&id)
            .ok_or_else(|| GhostError::Config(format!("ghost_shell: no session '{id}'")))?;

        if sess.pending.is_some() {
            let pend = sess.pending;
            self.shells.lock().unwrap().sessions.insert(id.clone(), sess);
            return Err(GhostError::Config(format!(
                "ghost_shell: session '{id}' is busy running command #{}; call op=read to drain it first",
                pend.unwrap()
            )));
        }

        sess.nonce += 1;
        let nonce = sess.nonce;
        // The driver echoes this field back verbatim, so folding the secret
        // into the nonce needs no change on the driver side.
        let token = sentinel_token(&sess.secret, nonce);
        let frame = format!("{} {}\n", token, b64_encode(cmd.as_bytes()));
        if let Err(e) = sess.stdin.write_all(frame.as_bytes()).await {
            let _ = sess.child.start_kill();
            return Err(GhostError::Config(format!("ghost_shell: session '{id}' write failed: {e}")));
        }
        let _ = sess.stdin.flush().await;

        let outcome = read_until_sentinel(&mut sess, &token, dur).await;
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
            .lock().unwrap()
            .sessions
            .remove(&id)
            .ok_or_else(|| GhostError::Config(format!("ghost_shell: no session '{id}'")))?;

        let nonce = match sess.pending {
            Some(n) => n,
            None => {
                self.shells.lock().unwrap().sessions.insert(id.clone(), sess);
                return Ok(json!({ "ok": true, "id": id, "output": "", "busy": false, "note": "no command pending" }));
            }
        };
        let token = sentinel_token(&sess.secret, nonce);
        let outcome = read_until_sentinel(&mut sess, &token, dur).await;
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
                self.shells.lock().unwrap().sessions.insert(id.clone(), sess);
                Ok(json!({
                    "ok": true, "id": id, "output": output,
                    "exit_code": exit_code, "truncated": truncated,
                    "timed_out": false, "busy": false,
                }))
            }
            ReadOutcome::TimedOut { output } => {
                sess.pending = Some(nonce);
                let (output, truncated) = cap_output(output);
                self.shells.lock().unwrap().sessions.insert(id.clone(), sess);
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
        let reg = self.shells.lock().unwrap();
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
            .lock().unwrap()
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

/// A value the shell session's own output cannot predict.
///
/// `RandomState` is seeded by the OS per process and differs per instance, so
/// two sessions in one process do not share a secret either. This is framing
/// integrity, not cryptography: it has to be unguessable by a command that is
/// merely printing text, and 64 bits of OS entropy is far past that bar.
fn session_secret() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(0x9E37_79B9_7F4A_7C15);
    format!("{:016x}", h.finish())
}

/// The nonce field for one command: secret plus counter, no whitespace, so both
/// the bash and PowerShell drivers can echo it back as a single token.
fn sentinel_token(secret: &str, nonce: u64) -> String {
    format!("{secret}x{nonce}")
}

/// Read lines from the session until the nonce-stamped sentinel appears, the
/// deadline passes, the stop flag fires, or stdout hits EOF. Everything before
/// the sentinel is the command's merged output.
async fn read_until_sentinel(sess: &mut ShellSession, token: &str, dur: Duration) -> ReadOutcome {
    let sentinel = format!("__GHOST_DONE_{token}__ ");
    let deadline = Instant::now() + dur;
    let mut output = String::new();

    loop {
        if crate::engine::input::hotkey::is_stopped() {
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
        if crate::engine::input::hotkey::is_stopped() {
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
    #[allow(unused_mut)]
    let mut c = match shell {
        // POSIX shells. `-c` takes the script as one argument, so no quoting of
        // the user's command is needed or performed.
        "bash" | "sh" | "zsh" => {
            let mut c = Command::new(shell);
            c.args(["-c", cmd]);
            c
        }
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
            let supported = if cfg!(target_os = "linux") {
                "bash|sh|zsh|pwsh"
            } else {
                "powershell|pwsh|cmd"
            };
            return Err(GhostError::Config(format!(
                "ghost_shell: unknown shell '{other}'; use {supported}"
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

#[cfg(test)]
mod warm_tests {
    use super::*;

    #[test]
    fn cwd_prefix_escapes_single_quotes_for_a_literal_path() {
        assert_eq!(
            cwd_prefix(r"C:\Users\someone\o'neil"),
            r"Set-Location -LiteralPath 'C:\Users\someone\o''neil'; "
        );
    }

    #[test]
    fn warm_flag_reads_env() {
        std::env::set_var("GHOST_SHELL_WARM", "off");
        assert!(warm_disabled());
        std::env::remove_var("GHOST_SHELL_WARM");
        assert!(!warm_disabled());
    }

    /// The mechanism the warm path relies on: a driver process accepts one
    /// framed command, answers with the sentinel and the native exit code, and
    /// the whole exchange is fast once the process is up. Windows only (real
    /// PowerShell); the exit code comes from `cmd /c exit 3`, a native command.
    #[cfg(windows)]
    #[test]
    fn a_spare_driver_serves_one_framed_command_with_its_exit_code() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let mut sess = spawn_driver(None).expect("spawn driver");
            // Let the child finish starting so the measurement below is the
            // warm cost, not PowerShell's own start-up.
            tokio::time::sleep(Duration::from_millis(1500)).await;
            let started = Instant::now();
            sess.nonce += 1;
            let token = sentinel_token(&sess.secret, sess.nonce);
            let frame = format!("{} {}\n", token, b64_encode(b"Write-Output warm-ok; cmd /c exit 3"));
            sess.stdin.write_all(frame.as_bytes()).await.unwrap();
            sess.stdin.flush().await.unwrap();
            let outcome = read_until_sentinel(&mut sess, &token, Duration::from_secs(20)).await;
            let elapsed = started.elapsed();
            let _ = sess.child.start_kill();
            match outcome {
                ReadOutcome::Done { output, exit_code } => {
                    assert!(output.contains("warm-ok"), "output was {output:?}");
                    assert_eq!(exit_code, Some(3));
                }
                other => panic!("expected Done, got {:?}", matches!(other, ReadOutcome::Done { .. })),
            }
            assert!(
                elapsed < Duration::from_millis(1500),
                "a warm command should not cost a process start: {elapsed:?}"
            );
        });
    }
}
