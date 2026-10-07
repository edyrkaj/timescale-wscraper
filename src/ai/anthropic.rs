use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

use super::{env_nonempty, parse_ai_json, AiPageExtract, LlmConfig};
use crate::models::AiProvider;

const DEFAULT_BASE_URL: &str = "https://api.meta.ai";
const DEFAULT_MODEL: &str = "muse-spark-1.3-contributor";

pub struct AnthropicProvider;

impl AnthropicProvider {
    pub fn config_from_env() -> Result<LlmConfig> {
        let api_key = env_nonempty("ANTHROPIC_AUTH_TOKEN")
            .or_else(|| env_nonempty("ANTHROPIC_API_KEY"))
            .or_else(|| env_nonempty("MODEL_API_KEY"))
            .context(
                "ANTHROPIC_AUTH_TOKEN (or ANTHROPIC_API_KEY / MODEL_API_KEY) is required for Anthropic AI scraper",
            )?;
        let base_url = env_nonempty("ANTHROPIC_BASE_URL")
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
            .trim_end_matches('/')
            .to_string();
        let model = env_nonempty("ANTHROPIC_MODEL")
            .or_else(|| env_nonempty("ANTHROPIC_DEFAULT_SONNET_MODEL"))
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());
        Ok(LlmConfig {
            provider: AiProvider::Anthropic,
            base_url,
            api_key,
            model,
        })
    }

    pub async fn extract(cfg: &LlmConfig, system: &str, user: &str) -> Result<AiPageExtract> {
        let body = serde_json::json!({
            "model": cfg.model,
            "max_tokens": 8192,
            "temperature": 0,
            "system": system,
            "messages": [
                { "role": "user", "content": user }
            ]
        });

        let endpoint = messages_endpoint(&cfg.base_url);
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()?;
        let res = client
            .post(&endpoint)
            .header("x-api-key", &cfg.api_key)
            .header("Authorization", format!("Bearer {}", cfg.api_key))
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .with_context(|| format!("Anthropic request failed: {endpoint}"))?;
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow!(
                "Anthropic HTTP {status}: {}",
                text.chars().take(400).collect::<String>()
            ));
        }
        let parsed: AnthropicResponse = serde_json::from_str(&text).with_context(|| {
            format!(
                "unexpected Anthropic payload: {}",
                text.chars().take(240).collect::<String>()
            )
        })?;
        let content = parsed
            .content
            .iter()
            .filter(|b| b.block_type == "text" || b.text.is_some())
            .filter_map(|b| b.text.as_deref())
            .collect::<Vec<_>>()
            .join("\n");
        if content.trim().is_empty() {
            return Err(anyhow!("Anthropic returned no text content"));
        }
        parse_ai_json(&content)
    }
}

fn messages_endpoint(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if base.ends_with("/v1") {
        format!("{base}/messages")
    } else {
        format!("{base}/v1/messages")
    }
}

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    #[serde(default)]
    content: Vec<AnthropicContentBlock>,
}

#[derive(Debug, Deserialize)]
struct AnthropicContentBlock {
    #[serde(default)]
    #[serde(rename = "type")]
    block_type: String,
    #[serde(default)]
    text: Option<String>,
}
