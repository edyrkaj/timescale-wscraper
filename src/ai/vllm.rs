use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use tracing::debug;

use super::{env_nonempty, parse_ai_json, AiPageExtract, LlmConfig};
use crate::models::AiProvider;

const DEFAULT_BASE_URL: &str = "http://host.docker.internal:8000/v1";
const DEFAULT_MODEL: &str = "Qwen/Qwen3-8B";
const DEFAULT_MAX_MODEL_LEN: usize = 4096;
const DEFAULT_MAX_TOKENS: usize = 1024;
/// Chat-template tokens vLLM counts on top of the message text.
const TEMPLATE_OVERHEAD_TOKENS: usize = 64;

pub struct VllmProvider;

impl VllmProvider {
    pub fn config_from_env() -> Result<LlmConfig> {
        let api_key = env_nonempty("VLLM_API_KEY").unwrap_or_default();
        let base_url = env_nonempty("VLLM_BASE_URL")
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
            .trim_end_matches('/')
            .to_string();
        let model = env_nonempty("VLLM_MODEL").unwrap_or_else(|| DEFAULT_MODEL.to_string());
        Ok(LlmConfig {
            provider: AiProvider::Vllm,
            base_url,
            api_key,
            model,
        })
    }

    pub async fn extract(cfg: &LlmConfig, system: &str, user: &str) -> Result<AiPageExtract> {
        let endpoint = chat_completions_url(&cfg.base_url);
        let max_model_len = env_usize("VLLM_MAX_MODEL_LEN", DEFAULT_MAX_MODEL_LEN);
        let requested_max_tokens = env_usize("VLLM_MAX_TOKENS", DEFAULT_MAX_TOKENS);
        let (mut user_fitted, max_tokens) =
            fit_request(system, user, max_model_len, requested_max_tokens);

        debug!(
            model = %cfg.model,
            max_tokens,
            system,
            user = %user_fitted,
            "vLLM prompt"
        );

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(180))
            .build()?;

        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let body = serde_json::json!({
                "model": cfg.model,
                "temperature": 0,
                "max_tokens": max_tokens,
                "response_format": { "type": "json_object" },
                "chat_template_kwargs": { "enable_thinking": false },
                "messages": [
                    { "role": "system", "content": system },
                    { "role": "user", "content": user_fitted }
                ]
            });
            let mut req = client
                .post(&endpoint)
                .header("content-type", "application/json");
            if let Some(auth) = authorization_header(&cfg.api_key) {
                req = req.header("Authorization", auth);
            }
            match req.json(&body).send().await {
                Ok(res) => {
                    let status = res.status();
                    let text = res.text().await.unwrap_or_default();
                    if status.as_u16() == 400 && is_context_overflow(&text) && attempt < 3 {
                        user_fitted = shrink_user_prompt(&user_fitted);
                        continue;
                    }
                    if !status.is_success() {
                        return Err(anyhow!(
                            "vLLM HTTP {status}: {}",
                            text.chars().take(400).collect::<String>()
                        ));
                    }
                    let content = message_content(&text)?;
                    debug!(content = %content, "vLLM response");
                    return parse_ai_json(&strip_think_blocks(&content));
                }
                Err(_) if attempt < 2 => continue,
                Err(err) => {
                    return Err(err).with_context(|| format!("vLLM request failed: {endpoint}"));
                }
            }
        }
    }
}

fn env_usize(key: &str, default: usize) -> usize {
    env_nonempty(key)
        .and_then(|s| s.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(default)
}

/// Upper bound on Qwen tokens. Real BPE is often ~4 chars/token; 2 keeps us under the server count.
fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(2)
}

/// Keep system + user + completion inside `max_model_len`, shrinking page text before the links block.
fn fit_request(
    system: &str,
    user: &str,
    max_model_len: usize,
    max_tokens: usize,
) -> (String, usize) {
    let max_tokens = max_tokens
        .min(max_model_len.saturating_sub(TEMPLATE_OVERHEAD_TOKENS + 1))
        .max(1);
    let input_budget = max_model_len.saturating_sub(max_tokens + TEMPLATE_OVERHEAD_TOKENS);
    let user_tokens = input_budget.saturating_sub(estimate_tokens(system));
    let user_chars = user_tokens.saturating_mul(2);
    (fit_user_prompt(user, user_chars), max_tokens)
}

fn fit_user_prompt(user: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    if user.chars().count() <= max_chars {
        return user.to_string();
    }
    const LINKS_MARK: &str = "\n\nLinks (JSON):\n";
    if let Some(idx) = user.rfind(LINKS_MARK) {
        let links = &user[idx..];
        let head = &user[..idx];
        let links_chars = links.chars().count();
        let head_budget = max_chars.saturating_sub(links_chars);
        if head_budget >= 80 {
            let mut fitted = truncate_chars(head, head_budget);
            fitted.push_str(links);
            if fitted.chars().count() <= max_chars {
                return fitted;
            }
        }
    }
    truncate_chars(user, max_chars)
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let keep = max_chars.saturating_sub(1);
    let mut out: String = s.chars().take(keep).collect();
    out.push('…');
    out
}

fn shrink_user_prompt(user: &str) -> String {
    let chars = user.chars().count();
    fit_user_prompt(user, chars.saturating_div(2).max(1))
}

fn is_context_overflow(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    lower.contains("maximum context length") || lower.contains("input_tokens")
}

fn chat_completions_url(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if base.ends_with("/v1") {
        format!("{base}/chat/completions")
    } else {
        format!("{base}/v1/chat/completions")
    }
}

/// Bearer token only when a key is configured. Local vLLM is called with no key.
pub fn authorization_header(api_key: &str) -> Option<String> {
    let key = api_key.trim();
    if key.is_empty() {
        None
    } else {
        Some(format!("Bearer {key}"))
    }
}

fn message_content(payload: &str) -> Result<String> {
    let parsed: VllmResponse = serde_json::from_str(payload).with_context(|| {
        format!(
            "unexpected vLLM payload: {}",
            payload.chars().take(240).collect::<String>()
        )
    })?;
    let content = parsed
        .choices
        .first()
        .and_then(|c| c.message.content.clone())
        .unwrap_or_default();
    if content.trim().is_empty() {
        return Err(anyhow!("vLLM returned no text content"));
    }
    Ok(content)
}

fn strip_think_blocks(raw: &str) -> String {
    let Ok(re) = regex::Regex::new(r"(?s)<think>.*?</think>") else {
        return raw.trim().to_string();
    };
    re.replace_all(raw, "").trim().to_string()
}

#[derive(Debug, Deserialize)]
struct VllmResponse {
    #[serde(default)]
    choices: Vec<VllmChoice>,
}

#[derive(Debug, Deserialize)]
struct VllmChoice {
    #[serde(default)]
    message: VllmMessage,
}

#[derive(Debug, Deserialize, Default)]
struct VllmMessage {
    #[serde(default)]
    content: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completions_url_keeps_existing_v1() {
        assert_eq!(
            chat_completions_url("http://host.docker.internal:8000/v1"),
            "http://host.docker.internal:8000/v1/chat/completions"
        );
        assert_eq!(
            chat_completions_url("http://127.0.0.1:8000/v1/"),
            "http://127.0.0.1:8000/v1/chat/completions"
        );
    }

    #[test]
    fn completions_url_adds_v1_when_missing() {
        assert_eq!(
            chat_completions_url("http://127.0.0.1:8000"),
            "http://127.0.0.1:8000/v1/chat/completions"
        );
    }

    #[test]
    fn missing_key_omits_authorization() {
        assert_eq!(authorization_header(""), None);
        assert_eq!(authorization_header("   "), None);
        assert_eq!(
            authorization_header("local-token"),
            Some("Bearer local-token".into())
        );
    }

    #[test]
    fn reads_content_and_ignores_reasoning() {
        let payload = r#"{
            "choices": [{
                "message": {
                    "role": "assistant",
                    "reasoning": "look at the listings",
                    "reasoning_content": "still thinking",
                    "content": "{\"items\":[{\"title\":\"Dev\",\"url\":\"https://ex.com/1\"}],\"next_page_url\":null}"
                }
            }]
        }"#;
        let content = message_content(payload).expect("content");
        assert!(!content.contains("reasoning"));
        let parsed = parse_ai_json(&strip_think_blocks(&content)).expect("json");
        assert_eq!(parsed.items.len(), 1);
        assert_eq!(parsed.items[0].title, "Dev");
    }

    #[test]
    fn fits_prompt_under_context_window() {
        let system = "extract jobs";
        let page = "word ".repeat(3000);
        let user =
            format!("Page text:\n{page}\n\nLinks (JSON):\n[{{\"href\":\"https://ex.com/j\"}}]");
        let (fitted, max_tokens) = fit_request(&system, &user, 4096, 1024);
        let input = estimate_tokens(&system) + estimate_tokens(&fitted) + TEMPLATE_OVERHEAD_TOKENS;
        assert!(input + max_tokens <= 4096);
        assert!(fitted.contains("Links (JSON):"));
        assert!(fitted.contains('…'));
        assert!(fitted.chars().count() < user.chars().count());
    }

    #[test]
    fn leaves_short_prompt_unchanged() {
        let user = "Page text:\nDev\n\nLinks (JSON):\n[]";
        let (fitted, max_tokens) = fit_request("sys", user, 4096, 1024);
        assert_eq!(fitted, user);
        assert_eq!(max_tokens, 1024);
    }

    #[test]
    fn strips_think_block_before_json() {
        let raw =
            "<think>draft the list\n{\"oops\":1}</think>\n{\"items\":[],\"next_page_url\":null}";
        let stripped = strip_think_blocks(raw);
        assert!(!stripped.contains("<think>"));
        let parsed = parse_ai_json(&stripped).expect("json");
        assert!(parsed.items.is_empty());
    }
}
