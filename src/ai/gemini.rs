use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use tracing::warn;

use super::{env_nonempty, parse_ai_json, AiPageExtract, LlmConfig};
use crate::models::AiProvider;

const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";
const DEFAULT_MODEL: &str = "gemini-3.5-flash";
/// Ordered fallbacks tried on 503 / high demand (primary model is prepended separately).
const DEFAULT_FALLBACK_MODELS: &str = concat!(
    "gemini-3.5-flash,",
    "gemini-3.8-flash,",
    "gemini-3.1-pro-preview,",
    "gemini-3.1-flash-live-preview,",
    "gemini-3.1-flash-lite-preview,",
    "gemini-3.8-flash-live,",
    "gemini-3.8-flash-tts,",
    "gemini-2.5-pro,",
    "gemini-2.5-flash,",
    "gemini-2.5-flash-lite"
);
const MAX_ATTEMPTS: u32 = 4;

pub struct GeminiProvider;

impl GeminiProvider {
    pub fn config_from_env() -> Result<LlmConfig> {
        let api_key = env_nonempty("GEMINI_API_KEY").context(
            "GEMINI_API_KEY is required for Gemini AI scraper — add it to your .env",
        )?;
        let base_url = env_nonempty("GEMINI_BASE_URL")
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
            .trim_end_matches('/')
            .to_string();
        let model =
            env_nonempty("GEMINI_MODEL").unwrap_or_else(|| DEFAULT_MODEL.to_string());
        Ok(LlmConfig {
            provider: AiProvider::Gemini,
            base_url,
            api_key,
            model,
        })
    }

    pub async fn extract(cfg: &LlmConfig, system: &str, user: &str) -> Result<AiPageExtract> {
        let models = candidate_models(&cfg.model);
        let mut last_err = None;

        for model in &models {
            match extract_with_model(cfg, model, system, user).await {
                Ok(parsed) => return Ok(parsed),
                Err(err) => {
                    let msg = err.to_string();
                    if is_transient(&msg) && model.as_str() != models.last().map(String::as_str).unwrap_or("")
                    {
                        warn!(%model, error = %msg, "Gemini transient failure; trying next model");
                        last_err = Some(err);
                        continue;
                    }
                    return Err(err);
                }
            }
        }

        Err(last_err.unwrap_or_else(|| anyhow!("Gemini request failed")))
    }
}

fn candidate_models(primary: &str) -> Vec<String> {
    let mut models = vec![primary.to_string()];
    let fallbacks = env_nonempty("GEMINI_FALLBACK_MODELS")
        .unwrap_or_else(|| DEFAULT_FALLBACK_MODELS.to_string());
    for name in fallbacks.split(',') {
        let name = name.trim();
        if name.is_empty() || models.iter().any(|m| m == name) {
            continue;
        }
        models.push(name.to_string());
    }
    models
}

fn is_transient(msg: &str) -> bool {
    let lower = msg.to_ascii_lowercase();
    lower.contains("503")
        || lower.contains("429")
        || lower.contains("unavailable")
        || lower.contains("high demand")
        || lower.contains("resource_exhausted")
        || lower.contains("try again")
}

async fn extract_with_model(
    cfg: &LlmConfig,
    model: &str,
    system: &str,
    user: &str,
) -> Result<AiPageExtract> {
    let endpoint = format!(
        "{}/models/{}:generateContent?key={}",
        cfg.base_url.trim_end_matches('/'),
        model,
        urlencoding_encode(&cfg.api_key)
    );
    let body = serde_json::json!({
        "systemInstruction": {
            "parts": [{ "text": system }]
        },
        "contents": [{
            "role": "user",
            "parts": [{ "text": user }]
        }],
            "generationConfig": {
                "temperature": 0,
                "maxOutputTokens": 16384,
                "responseMimeType": "application/json"
            }
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()?;

    let mut last_err = None;
    for attempt in 1..=MAX_ATTEMPTS {
        let res = client
            .post(&endpoint)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .with_context(|| {
                format!(
                    "Gemini request failed: {}/models/{model}:generateContent",
                    cfg.base_url
                )
            })?;
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        if status.is_success() {
            let parsed: GeminiResponse = serde_json::from_str(&text).with_context(|| {
                format!(
                    "unexpected Gemini payload: {}",
                    text.chars().take(240).collect::<String>()
                )
            })?;
            let content = parsed
                .candidates
                .first()
                .and_then(|c| c.content.as_ref())
                .map(|c| {
                    c.parts
                        .iter()
                        .filter_map(|p| p.text.as_deref())
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            if content.trim().is_empty() {
                return Err(anyhow!("Gemini returned no text content"));
            }
            return parse_ai_json(&content);
        }

        let err = anyhow!(
            "Gemini HTTP {status}: {}",
            text.chars().take(400).collect::<String>()
        );
        let msg = err.to_string();
        if is_transient(&msg) && attempt < MAX_ATTEMPTS {
            let wait_ms = 500u64 * 2u64.pow(attempt - 1);
            warn!(%model, attempt, wait_ms, error = %msg, "Gemini busy; retrying");
            tokio::time::sleep(std::time::Duration::from_millis(wait_ms)).await;
            last_err = Some(err);
            continue;
        }
        return Err(err);
    }

    Err(last_err.unwrap_or_else(|| anyhow!("Gemini request failed after retries")))
}

fn urlencoding_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[derive(Debug, Deserialize)]
struct GeminiResponse {
    #[serde(default)]
    candidates: Vec<GeminiCandidate>,
}

#[derive(Debug, Deserialize)]
struct GeminiCandidate {
    #[serde(default)]
    content: Option<GeminiContent>,
}

#[derive(Debug, Deserialize)]
struct GeminiContent {
    #[serde(default)]
    parts: Vec<GeminiPart>,
}

#[derive(Debug, Deserialize)]
struct GeminiPart {
    #[serde(default)]
    text: Option<String>,
}
