//! Vision fallback for `By::Description` locators.
//!
//! Two providers are supported:
//!  - **NVIDIA Build** (default): OpenAI-compatible endpoint at
//!    integrate.api.nvidia.com. Free with NVIDIA Developer signup.
//!    Default model: meta/llama-3.2-90b-vision-instruct.
//!    Set NVIDIA_API_KEY.
//!  - **Anthropic** (fallback): claude-haiku-4-5 by default.
//!    Set ANTHROPIC_API_KEY.
//!
//! Selection: GHOST_VISION_PROVIDER=nvidia|anthropic. If unset, picks the
//! first provider whose API key is present (NVIDIA preferred).
//! Override model via GHOST_VISION_MODEL.
//! Override NVIDIA endpoint via GHOST_VISION_BASE_URL (useful for self-hosted
//! Ollama/vLLM/llama.cpp servers that speak OpenAI-compat — point it at e.g.
//! http://localhost:11434/v1).
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

fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("ghost-vision/0.5.0")
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
    let api_key = std::env::var("NVIDIA_API_KEY")
        .map_err(|_| GhostError::Config("NVIDIA_API_KEY not set".into()))?;
    if api_key.trim().is_empty() {
        return Err(GhostError::Vision("NVIDIA_API_KEY is empty/unset — set a valid key before using vision".into()));
    }
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
    let resp = http_client()
        .post(&url)
        .header("authorization", format!("Bearer {api_key}"))
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .json(&req)
        .send()
        .await
        .map_err(|e| GhostError::Vision(format!("openai-compat extract request: {e}")))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(GhostError::Vision(format!("openai-compat extract {status}: {body}")));
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
    let resp = http_client()
        .post(ANTHROPIC_URL)
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("content-type", "application/json")
        .json(&req)
        .send()
        .await
        .map_err(|e| GhostError::Vision(format!("anthropic extract request: {e}")))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(GhostError::Vision(format!("anthropic extract {status}: {body}")));
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
            "nvidia" | "openai" | "ollama" | "vllm" | "openai_compat" => Ok(Provider::Nvidia),
            "anthropic" | "claude" => Ok(Provider::Anthropic),
            other => Err(GhostError::Config(format!(
                "GHOST_VISION_PROVIDER={other} not recognized; use nvidia or anthropic"
            ))),
        };
    }
    if env_key_is_set("NVIDIA_API_KEY") {
        return Ok(Provider::Nvidia);
    }
    if env_key_is_set("ANTHROPIC_API_KEY") {
        return Ok(Provider::Anthropic);
    }
    Err(GhostError::Config(
        "no vision provider configured. Set NVIDIA_API_KEY (free at build.nvidia.com) or ANTHROPIC_API_KEY".into()
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
    let api_key = std::env::var("NVIDIA_API_KEY")
        .map_err(|_| GhostError::Config("NVIDIA_API_KEY not set; sign up free at build.nvidia.com".into()))?;
    if api_key.trim().is_empty() {
        return Err(GhostError::Vision("NVIDIA_API_KEY is empty/unset — set a valid key before using vision".into()));
    }
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

    let resp = http_client()
        .post(&url)
        .header("authorization", format!("Bearer {api_key}"))
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .json(&req)
        .send()
        .await
        .map_err(|e| GhostError::Vision(format!("openai-compat request failed: {e}")))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(GhostError::Vision(format!("openai-compat {status}: {body}")));
    }

    let parsed: OaiResp = resp.json().await
        .map_err(|e| GhostError::Vision(format!("openai-compat response parse: {e}")))?;
    let text = parsed.choices.first()
        .map(|c| c.message.content.clone())
        .ok_or_else(|| GhostError::Vision("no choices in response".into()))?;

    tracing::debug!(model = %model, url = %url, raw_response = %text, "openai-compat VLM raw response");
    let result = parse_coord_response(&text);
    tracing::debug!(parsed = ?result, "parse_coord_response result");
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

    let resp = http_client()
        .post(ANTHROPIC_URL)
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("content-type", "application/json")
        .json(&req)
        .send()
        .await
        .map_err(|e| GhostError::Vision(format!("anthropic request failed: {e}")))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(GhostError::Vision(format!("anthropic {status}: {body}")));
    }

    let parsed: AntResp = resp.json().await
        .map_err(|e| GhostError::Vision(format!("anthropic response parse: {e}")))?;
    let text = parsed.content.iter()
        .find(|c| c.kind == "text")
        .map(|c| c.text.trim().to_string())
        .ok_or_else(|| GhostError::Vision("anthropic returned no text content".into()))?;

    parse_coord_response(&text)
}

fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
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
        std::env::set_var("GHOST_VISION_PROVIDER", "anthropic");
        std::env::set_var("ANTHROPIC_API_KEY", "test");
        std::env::set_var("NVIDIA_API_KEY", "test");
        let p = pick_provider().unwrap();
        std::env::remove_var("GHOST_VISION_PROVIDER");
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::remove_var("NVIDIA_API_KEY");
        assert_eq!(p, Provider::Anthropic);
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
