//! Vision fallback for `By::Description` locators — MODEL-AGNOSTIC.
//!
//! Works with any tool-capable vision model behind two wire formats:
//!  - **OpenAI-compatible** (default): NVIDIA Build, OpenAI, Gemini
//!    (openai-compat), Groq, or a local vLLM / Ollama / LM Studio server — they
//!    all speak the same `/v1/chat/completions` format. Point
//!    `GHOST_VISION_BASE_URL` + `GHOST_VISION_MODEL` at your endpoint and set the
//!    key via `GHOST_VISION_API_KEY` (or `OPENAI_API_KEY` / `NVIDIA_API_KEY`).
//!    A local keyless server needs only `GHOST_VISION_BASE_URL`.
//!  - **Anthropic**: claude-haiku-4-5 by default. Set `ANTHROPIC_API_KEY`.
//!
//! Selection: `GHOST_VISION_PROVIDER=openai|nvidia|gemini|groq|ollama|vllm|anthropic`.
//! If unset, auto-detects: any OpenAI-compatible key (or a custom base URL) ->
//! OpenAI-compat path; else `ANTHROPIC_API_KEY` -> Anthropic.
//! Default OpenAI-compat endpoint/model is NVIDIA's free Llama-3.2-90B-Vision.
//!
//! Strategy: capture a tight ROI screenshot of the foreground window,
//! downscale + JPEG-encode for a small payload, then ask the chosen model
//! for the center pixel of the described element. Coords are translated
//! back from the downscaled+cropped image space to absolute screen pixels.

use crate::error::{GhostError, Result};
use crate::env_key_is_set;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

const ANTHROPIC_DEFAULT_MODEL: &str = "claude-haiku-4-5";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const ANTHROPIC_URL: &str = "https://api.anthropic.com/v1/messages";

const NVIDIA_DEFAULT_MODEL: &str = "meta/llama-3.2-90b-vision-instruct";
const NVIDIA_DEFAULT_URL: &str = "https://integrate.api.nvidia.com/v1/chat/completions";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Provider {
    Nvidia,
    Anthropic,
}

/// VLM request timeout. A 30s single-shot call was far beyond acceptable MCP
/// tool-call latency (and the serial stdio loop blocks every queued request
/// behind it). Default 8s; override via GHOST_VLM_TIMEOUT_MS for slow providers.
fn vlm_timeout() -> std::time::Duration {
    let ms = std::env::var("GHOST_VLM_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&v| v >= 1000)
        .unwrap_or(8000);
    std::time::Duration::from_millis(ms)
}

/// Send with one bounded retry on transient failures (connect errors / 5xx).
/// Timeouts are deliberately NOT retried — that would double worst-case latency.
async fn send_with_retry(builder: reqwest::RequestBuilder, ctx: &str) -> Result<reqwest::Response> {
    let retry = builder.try_clone();
    match builder.send().await {
        Ok(resp) if resp.status().is_server_error() => match retry {
            Some(b) => {
                let status = resp.status();
                tracing::warn!(%status, ctx, "vision request got 5xx; retrying once");
                b.send().await
                    .map_err(|e| GhostError::Vision(format!("{ctx} request (retry after {status}): {e}")))
            }
            None => Ok(resp),
        },
        Ok(resp) => Ok(resp),
        Err(e) if e.is_connect() => match retry {
            Some(b) => {
                tracing::warn!(error = %e, ctx, "vision request connect error; retrying once");
                b.send().await
                    .map_err(|e2| GhostError::Vision(format!("{ctx} request (retry after connect error): {e2}")))
            }
            None => Err(GhostError::Vision(format!("{ctx} request: {e}"))),
        },
        Err(e) => Err(GhostError::Vision(format!("{ctx} request: {e}"))),
    }
}

fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(vlm_timeout())
            .user_agent("ghost-vision/0.6.0")
            .build()
            .expect("vision http client")
    })
}

#[derive(Debug, Clone, Copy)]
pub struct Crop {
    /// Top-left of the crop in absolute screen pixels.
    pub origin: (i32, i32),
    /// Original (uncropped) crop size in screen pixels.
    pub original: (u32, u32),
    /// Final (after downscale) image size in pixels — what the model sees.
    pub final_size: (u32, u32),
}

impl Crop {
    pub fn to_screen(&self, vx: i32, vy: i32) -> (i32, i32) {
        let sx = self.original.0 as f32 / self.final_size.0.max(1) as f32;
        let sy = self.original.1 as f32 / self.final_size.1.max(1) as f32;
        let x = self.origin.0 + (vx as f32 * sx).round() as i32;
        let y = self.origin.1 + (vy as f32 * sy).round() as i32;
        (x, y)
    }
}

#[derive(Deserialize)]
struct Coord {
    #[serde(default)]
    x: Option<i32>,
    #[serde(default)]
    y: Option<i32>,
    #[serde(default)]
    found: Option<bool>,
}

/// Locate a UI element by natural-language description. Returns coordinates
/// in the cropped+downscaled image space; caller must translate via Crop::to_screen.
/// Returns Ok(None) if the model reports the element not found.
pub async fn vision_locate(
    description: &str,
    image_jpeg: &[u8],
    final_size: (u32, u32),
) -> Result<Option<(i32, i32)>> {
    let provider = pick_provider()?;
    match provider {
        Provider::Nvidia => locate_via_openai_compat(description, image_jpeg, final_size).await,
        Provider::Anthropic => locate_via_anthropic(description, image_jpeg, final_size).await,
    }
}

/// Extract named fields from a screenshot in one batched VLM call.
///
/// `fields` is the list of field names to extract.  The model is asked to return a
/// single-line JSON object `{ "<field>": <value_or_null>, ... }`.  Fields the model
/// cannot find are returned as `null`.
///
/// Returns a `serde_json::Map<String, Value>` keyed by field name.
pub async fn vision_extract(
    fields: &[String],
    image_jpeg: &[u8],
) -> Result<serde_json::Map<String, serde_json::Value>> {
    if fields.is_empty() {
        return Ok(serde_json::Map::new());
    }
    let provider = pick_provider()?;
    let prompt = build_extract_prompt(fields);
    let text = match provider {
        Provider::Nvidia => extract_via_openai_compat(&prompt, image_jpeg).await?,
        Provider::Anthropic => extract_via_anthropic(&prompt, image_jpeg).await?,
    };
    parse_extract_response(&text, fields)
}

/// Set-of-Marks grounding: the screenshot has numbered red badges on candidate
/// UI elements. Ask the model to return the NUMBER of the element matching the
/// description (or 0 if none). Returns the 1-based mark index, or None. This is
/// far more reliable than asking a model to regress raw pixel coordinates.
pub async fn vision_pick_mark(
    description: &str,
    marked_jpeg: &[u8],
    labels: &[String],
) -> Result<Option<usize>> {
    let n_marks = labels.len();
    if n_marks == 0 {
        return Ok(None);
    }
    let provider = pick_provider()?;
    let prompt = build_som_prompt(description, labels);
    let text = match provider {
        Provider::Nvidia => extract_via_openai_compat(&prompt, marked_jpeg).await?,
        Provider::Anthropic => extract_via_anthropic(&prompt, marked_jpeg).await?,
    };
    Ok(parse_mark_response(&text, n_marks))
}

/// Prompt combines the visual badges with each element's accessible name (when
/// known) as a hint. The model uses both: the label disambiguates when present,
/// the badge position/appearance carries unnamed elements (icons). Empty labels
/// are shown as "(no label)" so the model relies on the image for those.
fn build_som_prompt(description: &str, labels: &[String]) -> String {
    let n = labels.len();
    let mut list = String::new();
    for (i, name) in labels.iter().enumerate() {
        let shown = if name.trim().is_empty() { "(no label)".to_string() } else { format!("\"{}\"", name.trim()) };
        list.push_str(&format!("{}={}, ", i + 1, shown));
    }
    let list = list.trim_end_matches(", ");
    format!(
        "This screenshot has numbered red badges (1..{n}) on clickable UI elements. \
         The elements and any known labels: [{list}]. \
         Which single badge number best matches: \"{description}\"? \
         Use the labels AND what the element looks like at its badge. \
         Reply with ONLY the number, or 0 if none match. No other text."
    )
}

/// Parse the model's reply into a 1..=n_marks index, or None (0 / out of range /
/// unparseable). Tolerates prose by taking the first integer in the reply.
fn parse_mark_response(text: &str, n_marks: usize) -> Option<usize> {
    let mut digits = String::new();
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            digits.push(ch);
        } else if !digits.is_empty() {
            break; // first integer run only
        }
    }
    let n: usize = digits.parse().ok()?;
    if n >= 1 && n <= n_marks { Some(n) } else { None }
}

fn build_extract_prompt(fields: &[String]) -> String {
    let field_list = fields.iter().map(|f| format!("\"{f}\"")).collect::<Vec<_>>().join(", ");
    format!(
        "Look at this screenshot and extract the following fields: [{field_list}]. \
         Respond with ONLY a single-line JSON object: {{\"<field>\": <value_or_null>, ...}}. \
         For each field, return the text value visible on screen, or null if not found. \
         No prose, no markdown, no explanation."
    )
}

fn parse_extract_response(
    text: &str,
    fields: &[String],
) -> Result<serde_json::Map<String, serde_json::Value>> {
    let cleaned = text.trim()
        .trim_start_matches("```json").trim_start_matches("```")
        .trim_end_matches("```").trim();
    let json_str = if cleaned.starts_with('{') {
        cleaned.to_string()
    } else if let (Some(start), Some(end)) = (cleaned.find('{'), cleaned.rfind('}')) {
        cleaned[start..=end].to_string()
    } else {
        return Err(GhostError::Vision(format!("no JSON object in extract response: `{cleaned}`")));
    };
    let mut map: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&json_str)
        .map_err(|e| GhostError::Vision(format!("could not parse extract JSON `{json_str}`: {e}")))?;
    // Ensure all requested fields are present (fill missing with null).
    for f in fields {
        map.entry(f.clone()).or_insert(serde_json::Value::Null);
    }
    Ok(map)
}

async fn extract_via_openai_compat(prompt: &str, image_jpeg: &[u8]) -> Result<String> {
    let api_key = openai_compat_api_key()?;
    let model = std::env::var("GHOST_VISION_MODEL")
        .unwrap_or_else(|_| NVIDIA_DEFAULT_MODEL.into());
    let url = std::env::var("GHOST_VISION_BASE_URL")
        .unwrap_or_else(|_| NVIDIA_DEFAULT_URL.into());
    let b64 = base64_encode(image_jpeg);
    let data_url = format!("data:image/jpeg;base64,{b64}");
    let content = serde_json::json!([
        {"type": "image_url", "image_url": {"url": data_url}},
        {"type": "text", "text": prompt}
    ]);
    let req = OaiReq {
        model: &model,
        messages: vec![
            OaiMsg { role: "system", content: serde_json::Value::String(SYSTEM_PROMPT.into()) },
            OaiMsg { role: "user", content },
        ],
        max_tokens: 512,
        temperature: 0.0,
    };
    let mut rb = http_client()
        .post(&url)
        .header("content-type", "application/json")
        .header("accept", "application/json");
    if !api_key.is_empty() {
        rb = rb.header("authorization", format!("Bearer {api_key}"));
    }
    let resp = send_with_retry(rb.json(&req), "openai-compat extract").await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        // LOW: truncate error body to prevent large stack traces / auth metadata from leaking.
        let preview: String = body.chars().take(500).collect();
        return Err(GhostError::Vision(format!("openai-compat extract {status}: {preview}")));
    }
    let parsed: OaiResp = resp.json().await
        .map_err(|e| GhostError::Vision(format!("openai-compat extract parse: {e}")))?;
    Ok(parsed.choices.first()
        .map(|c| c.message.content.clone())
        .unwrap_or_default())
}

async fn extract_via_anthropic(prompt: &str, image_jpeg: &[u8]) -> Result<String> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| GhostError::Config("ANTHROPIC_API_KEY not set".into()))?;
    if api_key.trim().is_empty() {
        return Err(GhostError::Vision("ANTHROPIC_API_KEY is empty/unset — set a valid key before using vision".into()));
    }
    let model = std::env::var("GHOST_VISION_MODEL")
        .unwrap_or_else(|_| ANTHROPIC_DEFAULT_MODEL.into());
    let b64 = base64_encode(image_jpeg);
    let req = AntReq {
        model: &model,
        max_tokens: 512,
        system: SYSTEM_PROMPT,
        messages: vec![AntMsg {
            role: "user",
            content: vec![
                ContentBlock { kind: "image", text: None,
                    source: Some(ImageSource { kind: "base64", media_type: "image/jpeg", data: b64 }) },
                ContentBlock { kind: "text", text: Some(prompt.to_string()), source: None },
            ],
        }],
    };
    let resp = send_with_retry(
        http_client()
            .post(ANTHROPIC_URL)
            .header("x-api-key", api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&req),
        "anthropic extract",
    ).await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        let preview: String = body.chars().take(500).collect();
        return Err(GhostError::Vision(format!("anthropic extract {status}: {preview}")));
    }
    let parsed: AntResp = resp.json().await
        .map_err(|e| GhostError::Vision(format!("anthropic extract parse: {e}")))?;
    Ok(parsed.content.iter()
        .find(|c| c.kind == "text")
        .map(|c| c.text.trim().to_string())
        .unwrap_or_default())
}

fn pick_provider() -> Result<Provider> {
    if let Ok(p) = std::env::var("GHOST_VISION_PROVIDER") {
        return match p.to_lowercase().as_str() {
            // Any OpenAI-compatible endpoint (NVIDIA, OpenAI, Gemini-openai-compat,
            // Groq, local vLLM / Ollama / LM Studio) uses the same wire format.
            "nvidia" | "openai" | "gemini" | "groq" | "ollama" | "vllm" | "lmstudio" | "openai_compat"
                => Ok(Provider::Nvidia),
            "anthropic" | "claude" => Ok(Provider::Anthropic),
            other => Err(GhostError::Config(format!(
                "GHOST_VISION_PROVIDER={other} not recognized; use openai|nvidia|gemini|groq|ollama|vllm|anthropic"
            ))),
        };
    }
    // Auto-detect: any OpenAI-compatible key, or a custom base URL (keyless local
    // server), or Anthropic.
    if env_key_is_set("GHOST_VISION_API_KEY") || env_key_is_set("OPENAI_API_KEY")
        || env_key_is_set("NVIDIA_API_KEY") || std::env::var("GHOST_VISION_BASE_URL").is_ok() {
        return Ok(Provider::Nvidia);
    }
    if env_key_is_set("ANTHROPIC_API_KEY") {
        return Ok(Provider::Anthropic);
    }
    Err(GhostError::Config(
        "no vision provider configured. Set GHOST_VISION_API_KEY (any OpenAI-compatible endpoint) \
         + GHOST_VISION_BASE_URL + GHOST_VISION_MODEL, or NVIDIA_API_KEY (free at build.nvidia.com), \
         or ANTHROPIC_API_KEY".into()
    ))
}

/// Resolve the bearer key for the OpenAI-compatible path, provider-agnostic:
/// GHOST_VISION_API_KEY > OPENAI_API_KEY > NVIDIA_API_KEY. Returns an empty string
/// (keyless) when a custom GHOST_VISION_BASE_URL is set with no key — local
/// servers (vLLM/Ollama/LM Studio) typically need none.
fn openai_compat_api_key() -> Result<String> {
    for var in ["GHOST_VISION_API_KEY", "OPENAI_API_KEY", "NVIDIA_API_KEY"] {
        if let Ok(k) = std::env::var(var) {
            let k = k.trim();
            if !k.is_empty() {
                // Return trimmed — a trailing newline from a .env loader would
                // corrupt the `Authorization: Bearer <key>` header.
                return Ok(k.to_string());
            }
        }
    }
    if std::env::var("GHOST_VISION_BASE_URL").is_ok() {
        return Ok(String::new());
    }
    Err(GhostError::Config(
        "no vision API key set. Use GHOST_VISION_API_KEY (any OpenAI-compatible endpoint), \
         OPENAI_API_KEY, or NVIDIA_API_KEY; or set GHOST_VISION_BASE_URL for a keyless local server".into()
    ))
}

fn build_prompt(description: &str, final_size: (u32, u32)) -> String {
    format!(
        "The image is {}x{} pixels. Find the UI element described by: \"{}\". \
         Respond with ONLY a JSON object on a single line, no other text. \
         If found: {{\"x\": <center_x>, \"y\": <center_y>}} where x,y are pixel coordinates in the image. \
         If not found: {{\"found\": false}}.",
        final_size.0, final_size.1, description.replace('"', "\\\"")
    )
}

const SYSTEM_PROMPT: &str = "You are a precise UI element locator. Always respond with a single-line JSON object. No prose, no explanation, no markdown.";

/// MEDIUM-7: route VLM response text through the typed ghost-ground parser which
/// handles UI-TARS/Qwen norm-coord, action-call, bare-tuple, and prose-JSON formats.
/// Pixel-space coords (> 1000 in either axis) are passed through directly.
/// Norm-space coords (0-1000) are converted to pixels using `final_size`.
fn parse_vlm_response_to_coords(text: &str, final_size: (u32, u32)) -> Result<Option<(i32, i32)>> {
    use ghost_ground::parser::{parse_vlm_response as gnd_parse, CoordSpace};
    use ghost_ground::types::norm_to_px;

    match gnd_parse(text) {
        None => Ok(None),
        Some(r) if r.not_found => Ok(None),
        Some(r) => match r.coord {
            None => Ok(None),
            Some(c) => {
                let (px, py) = match c.space {
                    CoordSpace::Norm => (
                        norm_to_px(c.x.clamp(0, 1000) as u16, final_size.0),
                        norm_to_px(c.y.clamp(0, 1000) as u16, final_size.1),
                    ),
                    CoordSpace::Pixels => (c.x, c.y),
                };
                Ok(Some((px, py)))
            }
        },
    }
}

/// Legacy pixel-space-only parser (kept for unit tests; live paths use parse_vlm_response_to_coords).
#[allow(dead_code)]
fn parse_coord_response(text: &str) -> Result<Option<(i32, i32)>> {
    let cleaned = text.trim()
        .trim_start_matches("```json").trim_start_matches("```")
        .trim_end_matches("```").trim();

    // Some models emit prose around the JSON. Find the first {..} object.
    let json_str = if cleaned.starts_with('{') {
        cleaned.to_string()
    } else if let (Some(start), Some(end)) = (cleaned.find('{'), cleaned.rfind('}')) {
        cleaned[start..=end].to_string()
    } else {
        return Err(GhostError::Vision(format!("no JSON object in vision response: `{cleaned}`")));
    };

    let coord: Coord = serde_json::from_str(&json_str)
        .map_err(|e| GhostError::Vision(format!("could not parse coords from `{json_str}`: {e}")))?;
    if coord.found == Some(false) {
        return Ok(None);
    }
    Ok(match (coord.x, coord.y) {
        (Some(x), Some(y)) => Some((x, y)),
        _ => None,
    })
}

// ============================================================================
// OpenAI-compatible path (NVIDIA Build, Ollama, vLLM, llama.cpp, etc.)
// ============================================================================

// OaiImageUrl and OaiContent are Serialize types used as schema documentation for the
// OpenAI-compat wire format. The actual HTTP body is built via serde_json::json! for
// flexibility (mixed array of image_url + text parts), so the structs are not directly
// constructed — they serve as the typed reference for the schema.
#[allow(dead_code)]
#[derive(Serialize)]
struct OaiImageUrl<'a> {
    url: String,
    #[serde(skip)]
    _marker: std::marker::PhantomData<&'a ()>,
}

#[allow(dead_code)]
#[derive(Serialize)]
struct OaiContent<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    image_url: Option<OaiImageUrl<'a>>,
}

#[derive(Serialize)]
struct OaiMsg<'a> {
    role: &'a str,
    content: serde_json::Value,
}

#[derive(Serialize)]
struct OaiReq<'a> {
    model: &'a str,
    messages: Vec<OaiMsg<'a>>,
    max_tokens: u32,
    temperature: f32,
}

#[derive(Deserialize)]
struct OaiRespMsg {
    #[serde(default)]
    content: String,
}

#[derive(Deserialize)]
struct OaiRespChoice {
    message: OaiRespMsg,
}

#[derive(Deserialize)]
struct OaiResp {
    choices: Vec<OaiRespChoice>,
}

async fn locate_via_openai_compat(
    description: &str,
    image_jpeg: &[u8],
    final_size: (u32, u32),
) -> Result<Option<(i32, i32)>> {
    let api_key = openai_compat_api_key()?;
    let model = std::env::var("GHOST_VISION_MODEL")
        .unwrap_or_else(|_| NVIDIA_DEFAULT_MODEL.into());
    let url = std::env::var("GHOST_VISION_BASE_URL")
        .unwrap_or_else(|_| NVIDIA_DEFAULT_URL.into());

    let b64 = base64_encode(image_jpeg);
    let data_url = format!("data:image/jpeg;base64,{b64}");
    let prompt = build_prompt(description, final_size);

    let content = serde_json::json!([
        {"type": "image_url", "image_url": {"url": data_url}},
        {"type": "text", "text": prompt}
    ]);
    let req = OaiReq {
        model: &model,
        messages: vec![
            OaiMsg { role: "system", content: serde_json::Value::String(SYSTEM_PROMPT.into()) },
            OaiMsg { role: "user", content },
        ],
        max_tokens: 64,
        temperature: 0.0,
    };

    let mut rb = http_client()
        .post(&url)
        .header("content-type", "application/json")
        .header("accept", "application/json");
    if !api_key.is_empty() {
        rb = rb.header("authorization", format!("Bearer {api_key}"));
    }
    let resp = send_with_retry(rb.json(&req), "openai-compat locate").await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        let preview: String = body.chars().take(500).collect();
        return Err(GhostError::Vision(format!("openai-compat {status}: {preview}")));
    }

    let parsed: OaiResp = resp.json().await
        .map_err(|e| GhostError::Vision(format!("openai-compat response parse: {e}")))?;
    let text = parsed.choices.first()
        .map(|c| c.message.content.clone())
        .ok_or_else(|| GhostError::Vision("no choices in response".into()))?;

    tracing::debug!(model = %model, url = %url, raw_response = %text, "openai-compat VLM raw response");
    // MEDIUM-7: use the typed ghost-ground parser so UI-TARS/Qwen norm-coord
    // and prose formats work, not just bare pixel-space JSON.
    let result = parse_vlm_response_to_coords(&text, final_size);
    tracing::debug!(parsed = ?result, "parse_vlm_response result");
    result
}

// ============================================================================
// Anthropic path (existing)
// ============================================================================

#[derive(Serialize)]
struct ImageSource<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    media_type: &'a str,
    data: String,
}

#[derive(Serialize)]
struct ContentBlock<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<ImageSource<'a>>,
}

#[derive(Serialize)]
struct AntMsg<'a> {
    role: &'a str,
    content: Vec<ContentBlock<'a>>,
}

#[derive(Serialize)]
struct AntReq<'a> {
    model: &'a str,
    max_tokens: u32,
    system: &'a str,
    messages: Vec<AntMsg<'a>>,
}

#[derive(Deserialize)]
struct AntRespContent {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: String,
}

#[derive(Deserialize)]
struct AntResp {
    content: Vec<AntRespContent>,
}

async fn locate_via_anthropic(
    description: &str,
    image_jpeg: &[u8],
    final_size: (u32, u32),
) -> Result<Option<(i32, i32)>> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| GhostError::Config("ANTHROPIC_API_KEY not set".into()))?;
    if api_key.trim().is_empty() {
        return Err(GhostError::Vision("ANTHROPIC_API_KEY is empty/unset — set a valid key before using vision".into()));
    }
    let model = std::env::var("GHOST_VISION_MODEL")
        .unwrap_or_else(|_| ANTHROPIC_DEFAULT_MODEL.into());

    let b64 = base64_encode(image_jpeg);
    let prompt = build_prompt(description, final_size);

    let req = AntReq {
        model: &model,
        max_tokens: 64,
        system: SYSTEM_PROMPT,
        messages: vec![AntMsg {
            role: "user",
            content: vec![
                ContentBlock {
                    kind: "image",
                    text: None,
                    source: Some(ImageSource {
                        kind: "base64",
                        media_type: "image/jpeg",
                        data: b64,
                    }),
                },
                ContentBlock {
                    kind: "text",
                    text: Some(prompt),
                    source: None,
                },
            ],
        }],
    };

    let resp = send_with_retry(
        http_client()
            .post(ANTHROPIC_URL)
            .header("x-api-key", api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&req),
        "anthropic locate",
    ).await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        let preview: String = body.chars().take(500).collect();
        return Err(GhostError::Vision(format!("anthropic {status}: {preview}")));
    }

    let parsed: AntResp = resp.json().await
        .map_err(|e| GhostError::Vision(format!("anthropic response parse: {e}")))?;
    let text = parsed.content.iter()
        .find(|c| c.kind == "text")
        .map(|c| c.text.trim().to_string())
        .ok_or_else(|| GhostError::Vision("anthropic returned no text content".into()))?;

    // MEDIUM-7: use the typed ghost-ground parser so UI-TARS/Qwen norm-coord
    // and prose formats work, not just bare pixel-space JSON.
    parse_vlm_response_to_coords(&text, final_size)
}

fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as usize;
        let b1 = if chunk.len() > 1 { chunk[1] as usize } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as usize } else { 0 };
        out.push(TABLE[b0 >> 2] as char);
        out.push(TABLE[((b0 & 3) << 4) | (b1 >> 4)] as char);
        out.push(if chunk.len() > 1 { TABLE[((b1 & 0xf) << 2) | (b2 >> 6)] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[b2 & 0x3f] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mark_response_variants() {
        assert_eq!(parse_mark_response("7", 26), Some(7));
        assert_eq!(parse_mark_response("  26 ", 26), Some(26));
        assert_eq!(parse_mark_response("The answer is 3.", 26), Some(3));
        assert_eq!(parse_mark_response("0", 26), None);          // 0 = none
        assert_eq!(parse_mark_response("99", 26), None);          // out of range
        assert_eq!(parse_mark_response("none", 26), None);        // no digits
        assert_eq!(parse_mark_response("badge 12 matches", 26), Some(12));
    }

    #[test]
    fn crop_to_screen_no_downscale() {
        let c = Crop { origin: (100, 200), original: (400, 300), final_size: (400, 300) };
        assert_eq!(c.to_screen(50, 60), (150, 260));
    }

    #[test]
    fn crop_to_screen_with_downscale() {
        let c = Crop { origin: (10, 20), original: (1600, 1200), final_size: (800, 600) };
        assert_eq!(c.to_screen(200, 100), (410, 220));
    }

    #[test]
    fn crop_to_screen_zero_origin() {
        let c = Crop { origin: (0, 0), original: (768, 768), final_size: (768, 768) };
        assert_eq!(c.to_screen(384, 384), (384, 384));
    }

    #[test]
    fn base64_three_bytes_known_vector() {
        assert_eq!(base64_encode(b"Man"), "TWFu");
    }

    #[test]
    fn parse_coord_response_pure_json() {
        let r = parse_coord_response("{\"x\": 10, \"y\": 20}").unwrap();
        assert_eq!(r, Some((10, 20)));
    }

    #[test]
    fn parse_coord_response_with_code_fence() {
        let r = parse_coord_response("```json\n{\"x\":1,\"y\":2}\n```").unwrap();
        assert_eq!(r, Some((1, 2)));
    }

    #[test]
    fn parse_coord_response_with_prose() {
        let r = parse_coord_response("The button is at {\"x\": 50, \"y\": 60}.").unwrap();
        assert_eq!(r, Some((50, 60)));
    }

    #[test]
    fn parse_coord_response_not_found() {
        let r = parse_coord_response("{\"found\": false}").unwrap();
        assert_eq!(r, None);
    }

    #[test]
    fn pick_provider_with_explicit_override() {
        // LOW: use catch_unwind so env vars are always cleaned up even on panic,
        // guarding against cross-test env-var pollution in a parallel test harness.
        std::env::set_var("GHOST_VISION_PROVIDER", "anthropic");
        std::env::set_var("ANTHROPIC_API_KEY", "test");
        std::env::set_var("NVIDIA_API_KEY", "test");
        let result = std::panic::catch_unwind(pick_provider);
        std::env::remove_var("GHOST_VISION_PROVIDER");
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::remove_var("NVIDIA_API_KEY");
        assert_eq!(result.unwrap().unwrap(), Provider::Anthropic);
    }

    // --- parse_extract_response (field extraction VLM response parsing) ---

    #[test]
    fn parse_extract_pure_json_all_values() {
        let fields = vec!["title".to_string(), "price".to_string()];
        let text = r#"{"title": "My Product", "price": "$9.99"}"#;
        let map = parse_extract_response(text, &fields).unwrap();
        assert_eq!(map["title"], serde_json::json!("My Product"));
        assert_eq!(map["price"], serde_json::json!("$9.99"));
    }

    #[test]
    fn parse_extract_missing_field_filled_with_null() {
        let fields = vec!["title".to_string(), "status".to_string()];
        let text = r#"{"title": "Hello"}"#;
        let map = parse_extract_response(text, &fields).unwrap();
        assert_eq!(map["title"], serde_json::json!("Hello"));
        // status was missing from response → should be null
        assert!(map["status"].is_null());
    }

    #[test]
    fn parse_extract_explicit_null_field() {
        let fields = vec!["title".to_string(), "price".to_string()];
        let text = r#"{"title": "Widget", "price": null}"#;
        let map = parse_extract_response(text, &fields).unwrap();
        assert_eq!(map["title"], serde_json::json!("Widget"));
        assert!(map["price"].is_null());
    }

    #[test]
    fn parse_extract_with_code_fence() {
        let fields = vec!["name".to_string()];
        let text = "```json\n{\"name\": \"Alice\"}\n```";
        let map = parse_extract_response(text, &fields).unwrap();
        assert_eq!(map["name"], serde_json::json!("Alice"));
    }

    #[test]
    fn parse_extract_with_prose_around_json() {
        let fields = vec!["name".to_string()];
        let text = "Here is the extracted data: {\"name\": \"Bob\"}. That's all.";
        let map = parse_extract_response(text, &fields).unwrap();
        assert_eq!(map["name"], serde_json::json!("Bob"));
    }

    #[test]
    fn parse_extract_no_json_returns_error() {
        let fields = vec!["name".to_string()];
        let text = "I cannot find any data in this image.";
        assert!(parse_extract_response(text, &fields).is_err());
    }

    #[test]
    fn build_extract_prompt_includes_all_fields() {
        let fields = vec!["title".to_string(), "price".to_string(), "status".to_string()];
        let prompt = build_extract_prompt(&fields);
        assert!(prompt.contains("\"title\""));
        assert!(prompt.contains("\"price\""));
        assert!(prompt.contains("\"status\""));
        assert!(prompt.contains("JSON"));
    }
}
