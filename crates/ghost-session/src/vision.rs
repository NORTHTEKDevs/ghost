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
    if std::env::var("NVIDIA_API_KEY").is_ok() {
        return Ok(Provider::Nvidia);
    }
    if std::env::var("ANTHROPIC_API_KEY").is_ok() {
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

#[derive(Serialize)]
struct OaiImageUrl<'a> {
    url: String,
    #[serde(skip)]
    _marker: std::marker::PhantomData<&'a ()>,
}

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

    parse_coord_response(&text)
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
}
