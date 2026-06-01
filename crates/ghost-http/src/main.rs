//! `ghost-http`: REST server exposing Ghost automation over HTTP.
//!
//! Any language (Python, JS, curl) can drive Windows automation:
//!
//!   curl -X POST http://127.0.0.1:7878/click -d '{"name":"Submit"}' -H "content-type: application/json"
//!   curl http://127.0.0.1:7878/list-windows
//!   curl -X POST http://127.0.0.1:7878/run -d @intent.json -H "content-type: application/json"
//!
//! GhostSession holds !Send COM handles, so it runs on a dedicated OS thread
//! and we dispatch requests to it via a channel actor.

use axum::{extract::State, http::StatusCode, response::Json, routing::{get, post}, Router};
use clap::Parser;
use ghost_session::{By, GhostSession, Region};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

#[derive(Parser)]
#[command(name = "ghost-http", version, about = "HTTP REST server for Ghost desktop automation")]
struct Cli {
    /// Address to bind to.
    #[arg(long, default_value = "127.0.0.1:7878")]
    addr: String,

    /// Per-action timeout in milliseconds.
    #[arg(long, default_value_t = 5000)]
    timeout_ms: u64,
}

/// Commands sent from HTTP handlers to the session thread.
enum Cmd {
    Click { by: By, reply: oneshot::Sender<Result<Value, String>> },
    ClickAt { x: i32, y: i32, reply: oneshot::Sender<Result<Value, String>> },
    Type { by: By, text: String, reply: oneshot::Sender<Result<Value, String>> },
    Press { key: String, reply: oneshot::Sender<Result<Value, String>> },
    Hotkey { mods: Vec<String>, key: String, reply: oneshot::Sender<Result<Value, String>> },
    Screenshot { reply: oneshot::Sender<Result<Vec<u8>, String>> },
    Launch { exe: String, reply: oneshot::Sender<Result<Value, String>> },
    ListWindows { reply: oneshot::Sender<Result<Value, String>> },
    FocusWindow { name: String, reply: oneshot::Sender<Result<Value, String>> },
    WindowState { name: String, state: String, reply: oneshot::Sender<Result<Value, String>> },
    Describe { window: Option<String>, reply: oneshot::Sender<Result<Value, String>> },
    DescribeDelta { window: Option<String>, since_seq: Option<u64>, reply: oneshot::Sender<Result<Value, String>> },
    GetClipboard { reply: oneshot::Sender<Result<Value, String>> },
    SetClipboard { text: String, reply: oneshot::Sender<Result<Value, String>> },
    Run { json: String, reply: oneshot::Sender<Result<Value, String>> },
}

#[derive(Clone)]
struct AppState {
    tx: mpsc::UnboundedSender<Cmd>,
}

fn spawn_session_thread(timeout_ms: u64) -> mpsc::UnboundedSender<Cmd> {
    let (tx, mut rx) = mpsc::unbounded_channel::<Cmd>();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        rt.block_on(async move {
            let session = match GhostSession::new() {
                Ok(s) => s.with_timeout(timeout_ms),
                Err(e) => {
                    eprintln!("ghost-http: session init failed: {e}");
                    return;
                }
            };
            while let Some(cmd) = rx.recv().await {
                match cmd {
                    Cmd::Click { by, reply } => {
                        let r = async {
                            let el = session.find(by).await.map_err(|e| e.to_string())?;
                            el.click().map_err(|e| e.to_string())?;
                            Ok(json!({"ok": true}))
                        }.await;
                        let _ = reply.send(r);
                    }
                    Cmd::ClickAt { x, y, reply } => {
                        let r = session.click_at(x, y).await.map(|_| json!({"ok": true})).map_err(|e| e.to_string());
                        let _ = reply.send(r);
                    }
                    Cmd::Type { by, text, reply } => {
                        let r = async {
                            let el = session.find(by).await.map_err(|e| e.to_string())?;
                            el.type_text(&text).map_err(|e| e.to_string())?;
                            Ok(json!({"ok": true}))
                        }.await;
                        let _ = reply.send(r);
                    }
                    Cmd::Press { key, reply } => {
                        let r = session.press(&key).await.map(|_| json!({"ok": true})).map_err(|e| e.to_string());
                        let _ = reply.send(r);
                    }
                    Cmd::Hotkey { mods, key, reply } => {
                        let mods_ref: Vec<&str> = mods.iter().map(|s| s.as_str()).collect();
                        let r = session.hotkey(&mods_ref, &key).await.map(|_| json!({"ok": true})).map_err(|e| e.to_string());
                        let _ = reply.send(r);
                    }
                    Cmd::Screenshot { reply } => {
                        let r = session.screenshot(Region::full()).await.map_err(|e| e.to_string());
                        let _ = reply.send(r);
                    }
                    Cmd::Launch { exe, reply } => {
                        let r = session.launch(&exe).await.map(|pid| json!({"pid": pid})).map_err(|e| e.to_string());
                        let _ = reply.send(r);
                    }
                    Cmd::ListWindows { reply } => {
                        let r = session.list_windows().await.map(|ws| {
                            let list: Vec<Value> = ws.iter().map(|w| json!({
                                "name": w.name, "pid": w.pid, "focused": w.focused
                            })).collect();
                            json!({"windows": list})
                        }).map_err(|e| e.to_string());
                        let _ = reply.send(r);
                    }
                    Cmd::FocusWindow { name, reply } => {
                        let r = session.focus_window(&name).await.map(|_| json!({"ok": true})).map_err(|e| e.to_string());
                        let _ = reply.send(r);
                    }
                    Cmd::WindowState { name, state, reply } => {
                        let r = session.window_state(&name, &state).await.map(|_| json!({"ok": true})).map_err(|e| e.to_string());
                        let _ = reply.send(r);
                    }
                    Cmd::Describe { window, reply } => {
                        let r = session.describe_screen(window.as_deref()).await.map(|els| {
                            let list: Vec<Value> = els.iter().map(|e| json!({
                                "name": e.name, "role": e.role,
                                "left": e.left, "top": e.top, "right": e.right, "bottom": e.bottom
                            })).collect();
                            json!({"elements": list})
                        }).map_err(|e| e.to_string());
                        let _ = reply.send(r);
                    }
                    Cmd::DescribeDelta { window, since_seq, reply } => {
                        let r = session.describe_screen_delta(window.as_deref(), since_seq).await
                            .map(|d| serde_json::to_value(d).unwrap_or_default())
                            .map_err(|e| e.to_string());
                        let _ = reply.send(r);
                    }
                    Cmd::GetClipboard { reply } => {
                        let r = session.get_clipboard().await.map(|t| json!({"text": t})).map_err(|e| e.to_string());
                        let _ = reply.send(r);
                    }
                    Cmd::SetClipboard { text, reply } => {
                        let r = session.set_clipboard(&text).await.map(|_| json!({"ok": true})).map_err(|e| e.to_string());
                        let _ = reply.send(r);
                    }
                    Cmd::Run { json: intent, reply } => {
                        let r = session.execute_intent(&intent).await
                            .map(|res| serde_json::to_value(res).unwrap_or_default())
                            .map_err(|e| e.to_string());
                        let _ = reply.send(r);
                    }
                }
            }
        });
    });
    tx
}

async fn send<T>(tx: &mpsc::UnboundedSender<Cmd>, build: impl FnOnce(oneshot::Sender<Result<T, String>>) -> Cmd) -> Result<T, (StatusCode, String)> {
    let (rtx, rrx) = oneshot::channel();
    tx.send(build(rtx)).map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "session thread gone".into()))?;
    rrx.await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "session dropped reply".into()))?
        .map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, e))
}

fn json_err(e: (StatusCode, String)) -> (StatusCode, Json<Value>) {
    (e.0, Json(json!({"error": e.1})))
}

#[derive(Deserialize)]
struct ByReq { name: Option<String>, role: Option<String> }
impl ByReq {
    fn into_by(self) -> Result<By, (StatusCode, String)> {
        match (self.name, self.role) {
            (Some(n), _) => Ok(By::name(&n)),
            (_, Some(r)) => Ok(By::role(&r)),
            _ => Err((StatusCode::BAD_REQUEST, "must include name or role".into())),
        }
    }
}

#[derive(Deserialize)] struct ClickAt { x: i32, y: i32 }
#[derive(Deserialize)] struct TypeReq { name: Option<String>, role: Option<String>, text: String }
#[derive(Deserialize)] struct PressReq { key: String }
#[derive(Deserialize)] struct HotkeyReq { mods: Vec<String>, key: String }
#[derive(Deserialize)] struct LaunchReq { exe: String }
#[derive(Deserialize)] struct FocusReq { name: String }
#[derive(Deserialize)] struct WindowStateReq { name: String, state: String }
#[derive(Deserialize)] struct DescribeReq { window: Option<String>, since_seq: Option<u64> }
#[derive(Deserialize)] struct ClipReq { text: String }

async fn h_click(State(s): State<AppState>, Json(req): Json<ByReq>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let by = req.into_by().map_err(json_err)?;
    send(&s.tx, |reply| Cmd::Click { by, reply }).await.map(Json).map_err(json_err)
}
async fn h_click_at(State(s): State<AppState>, Json(r): Json<ClickAt>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    send(&s.tx, |reply| Cmd::ClickAt { x: r.x, y: r.y, reply }).await.map(Json).map_err(json_err)
}
async fn h_type(State(s): State<AppState>, Json(r): Json<TypeReq>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let by = ByReq { name: r.name, role: r.role }.into_by().map_err(json_err)?;
    send(&s.tx, |reply| Cmd::Type { by, text: r.text, reply }).await.map(Json).map_err(json_err)
}
async fn h_press(State(s): State<AppState>, Json(r): Json<PressReq>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    send(&s.tx, |reply| Cmd::Press { key: r.key, reply }).await.map(Json).map_err(json_err)
}
async fn h_hotkey(State(s): State<AppState>, Json(r): Json<HotkeyReq>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    send(&s.tx, |reply| Cmd::Hotkey { mods: r.mods, key: r.key, reply }).await.map(Json).map_err(json_err)
}
async fn h_screenshot(State(s): State<AppState>) -> Result<([(axum::http::HeaderName, &'static str); 1], Vec<u8>), (StatusCode, Json<Value>)> {
    let png = send(&s.tx, |reply| Cmd::Screenshot { reply }).await.map_err(json_err)?;
    Ok(([(axum::http::header::CONTENT_TYPE, "image/png")], png))
}
async fn h_launch(State(s): State<AppState>, Json(r): Json<LaunchReq>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    send(&s.tx, |reply| Cmd::Launch { exe: r.exe, reply }).await.map(Json).map_err(json_err)
}
async fn h_list_windows(State(s): State<AppState>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    send(&s.tx, |reply| Cmd::ListWindows { reply }).await.map(Json).map_err(json_err)
}
async fn h_focus_window(State(s): State<AppState>, Json(r): Json<FocusReq>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    send(&s.tx, |reply| Cmd::FocusWindow { name: r.name, reply }).await.map(Json).map_err(json_err)
}
async fn h_window_state(State(s): State<AppState>, Json(r): Json<WindowStateReq>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    send(&s.tx, |reply| Cmd::WindowState { name: r.name, state: r.state, reply }).await.map(Json).map_err(json_err)
}
async fn h_describe(State(s): State<AppState>, Json(r): Json<DescribeReq>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if r.since_seq.is_some() {
        send(&s.tx, |reply| Cmd::DescribeDelta { window: r.window, since_seq: r.since_seq, reply }).await.map(Json).map_err(json_err)
    } else {
        send(&s.tx, |reply| Cmd::Describe { window: r.window, reply }).await.map(Json).map_err(json_err)
    }
}
async fn h_get_clip(State(s): State<AppState>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    send(&s.tx, |reply| Cmd::GetClipboard { reply }).await.map(Json).map_err(json_err)
}
async fn h_set_clip(State(s): State<AppState>, Json(r): Json<ClipReq>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    send(&s.tx, |reply| Cmd::SetClipboard { text: r.text, reply }).await.map(Json).map_err(json_err)
}
async fn h_run(State(s): State<AppState>, body: String) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    send(&s.tx, |reply| Cmd::Run { json: body, reply }).await.map(Json).map_err(json_err)
}

async fn h_health() -> Json<Value> {
    Json(json!({"status": "ok", "version": env!("CARGO_PKG_VERSION"), "service": "ghost-http"}))
}

async fn h_tools() -> Json<Value> {
    Json(json!({
        "endpoints": [
            {"path": "/health", "method": "GET"},
            {"path": "/click", "method": "POST", "body": {"name": "string?", "role": "string?"}},
            {"path": "/click-at", "method": "POST", "body": {"x": "int", "y": "int"}},
            {"path": "/type", "method": "POST", "body": {"name": "string?", "role": "string?", "text": "string"}},
            {"path": "/press", "method": "POST", "body": {"key": "string"}},
            {"path": "/hotkey", "method": "POST", "body": {"mods": ["string"], "key": "string"}},
            {"path": "/screenshot", "method": "GET", "response": "image/png"},
            {"path": "/launch", "method": "POST", "body": {"exe": "string"}},
            {"path": "/list-windows", "method": "GET"},
            {"path": "/focus-window", "method": "POST", "body": {"name": "string"}},
            {"path": "/window-state", "method": "POST", "body": {"name": "string", "state": "maximize|minimize|restore|close"}},
            {"path": "/describe", "method": "POST", "body": {"window": "string?", "since_seq": "int?"}},
            {"path": "/clipboard", "method": "GET"},
            {"path": "/clipboard", "method": "POST", "body": {"text": "string"}},
            {"path": "/run", "method": "POST", "body": "intent JSON (steps, abort_if, retry_if, max_duration_ms)"}
        ]
    }))
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_writer(std::io::stderr).init();
    let cli = Cli::parse();

    let tx = spawn_session_thread(cli.timeout_ms);
    let state = AppState { tx };

    let app = Router::new()
        .route("/health", get(h_health))
        .route("/", get(h_tools))
        .route("/tools", get(h_tools))
        .route("/click", post(h_click))
        .route("/click-at", post(h_click_at))
        .route("/type", post(h_type))
        .route("/press", post(h_press))
        .route("/hotkey", post(h_hotkey))
        .route("/screenshot", get(h_screenshot))
        .route("/launch", post(h_launch))
        .route("/list-windows", get(h_list_windows))
        .route("/focus-window", post(h_focus_window))
        .route("/window-state", post(h_window_state))
        .route("/describe", post(h_describe))
        .route("/clipboard", get(h_get_clip).post(h_set_clip))
        .route("/run", post(h_run))
        // MEDIUM-5: removed CorsLayer::permissive() — it allowed any web page on the same
        // machine to POST to ghost-http via fetch() (localhost CSRF). Callers are curl/local
        // scripts, not browsers. No CORS headers = browsers block cross-origin requests.
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&cli.addr).await
        .expect("bind failed");
    eprintln!("ghost-http listening on http://{}", cli.addr);
    eprintln!("  curl http://{}/tools  for endpoint docs", cli.addr);
    axum::serve(listener, app).await.expect("serve failed");
}
