use anyhow::{anyhow, Context, Result};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
const DEFAULT_MODEL: &str = "gpt-4o-mini";

#[derive(Debug, Clone)]
pub struct LlmConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

impl LlmConfig {
    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("OPENAI_API_KEY")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .context("OPENAI_API_KEY is required for AI scraper")?;
        let base_url = std::env::var("OPENAI_BASE_URL")
            .ok()
            .map(|s| s.trim().trim_end_matches('/').to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        let model = std::env::var("OPENAI_MODEL")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());
        Ok(Self {
            base_url,
            api_key,
            model,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PageLink {
    pub text: String,
    pub href: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct FetchedPage {
    pub final_url: String,
    pub title: String,
    pub text: String,
    pub links: Vec<PageLink>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct AiJobItem {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub company: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub salary: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub published_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
pub struct AiPageExtract {
    #[serde(default)]
    pub items: Vec<AiJobItem>,
    #[serde(default)]
    pub next_page_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatMessage,
}

#[derive(Debug, Deserialize)]
struct ChatMessage {
    content: Option<String>,
}

pub fn strip_json_fences(raw: &str) -> &str {
    let trimmed = raw.trim();
    let without_open = if let Some(rest) = trimmed.strip_prefix("```json") {
        rest
    } else if let Some(rest) = trimmed.strip_prefix("```") {
        rest
    } else {
        trimmed
    };
    let without_close = without_open
        .strip_suffix("```")
        .unwrap_or(without_open)
        .trim();
    let start = without_close.find('{').unwrap_or(0);
    let end = without_close.rfind('}').map(|i| i + 1).unwrap_or(without_close.len());
    without_close.get(start..end).unwrap_or(without_close)
}

pub fn parse_ai_json(raw: &str) -> Result<AiPageExtract> {
    let json = strip_json_fences(raw);
    serde_json::from_str(json).with_context(|| {
        format!(
            "LLM response was not valid job JSON: {}",
            json.chars().take(240).collect::<String>()
        )
    })
}

pub fn parse_published_at(raw: &str) -> Option<NaiveDate> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    let date_part = s.get(..10).unwrap_or(s);
    NaiveDate::parse_from_str(date_part, "%Y-%m-%d")
        .ok()
        .or_else(|| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
        .or_else(|| parse_slash_date(s))
}

fn parse_slash_date(s: &str) -> Option<NaiveDate> {
    let re = regex::Regex::new(r"\b(\d{1,2})[/-](\d{1,2})[/-](20\d{2})\b").ok()?;
    let caps = re.captures(s)?;
    let a: u32 = caps.get(1)?.as_str().parse().ok()?;
    let b: u32 = caps.get(2)?.as_str().parse().ok()?;
    let y: i32 = caps.get(3)?.as_str().parse().ok()?;
    NaiveDate::from_ymd_opt(y, b, a).or_else(|| NaiveDate::from_ymd_opt(y, a, b))
}

pub fn item_in_window(published: Option<NaiveDate>, from: NaiveDate, to: NaiveDate) -> bool {
    match published {
        None => true,
        Some(d) => d >= from && d <= to,
    }
}

pub fn filter_extract(
    extract: AiPageExtract,
    from: NaiveDate,
    to: NaiveDate,
) -> (Vec<(AiJobItem, Option<NaiveDate>)>, bool) {
    let mut kept = Vec::new();
    let mut saw_older = false;
    for item in extract.items {
        let published = item.published_at.as_deref().and_then(parse_published_at);
        if let Some(d) = published {
            if d < from {
                saw_older = true;
                continue;
            }
            if d > to {
                continue;
            }
        }
        if item.title.trim().is_empty() || item.url.trim().is_empty() {
            continue;
        }
        kept.push((item, published));
    }
    (kept, saw_older)
}

pub async fn extract_jobs_from_page(
    cfg: &LlmConfig,
    listing_url: &str,
    from_date: NaiveDate,
    to_date: NaiveDate,
    page: &FetchedPage,
) -> Result<AiPageExtract> {
    let links: Vec<serde_json::Value> = page
        .links
        .iter()
        .take(200)
        .map(|l| serde_json::json!({"text": l.text, "href": l.href}))
        .collect();
    let user = format!(
        "Listing URL: {listing_url}\nFinal URL: {}\nPage title: {}\nDate window (inclusive): {from_date} to {to_date}\n\nPage text:\n{}\n\nLinks (JSON):\n{}",
        page.final_url,
        page.title,
        page.text,
        serde_json::to_string(&links).unwrap_or_else(|_| "[]".into())
    );

    let body = serde_json::json!({
        "model": cfg.model,
        "temperature": 0,
        "messages": [
            {
                "role": "system",
                "content": "You extract job listings from a rendered listing page. Reply with JSON only, no markdown. Schema: {\"items\":[{\"title\":\"\",\"url\":\"https://...\",\"company\":null,\"location\":null,\"salary\":null,\"description\":null,\"published_at\":\"YYYY-MM-DD or null\"}],\"next_page_url\":null}. Use absolute URLs. next_page_url is the next listing page or null. Prefer jobs in the given date window."
            },
            { "role": "user", "content": user }
        ]
    });

    let endpoint = format!("{}/chat/completions", cfg.base_url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()?;
    let res = client
        .post(&endpoint)
        .bearer_auth(&cfg.api_key)
        .json(&body)
        .send()
        .await
        .with_context(|| format!("LLM request failed: {endpoint}"))?;
    let status = res.status();
    let text = res.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!(
            "LLM HTTP {status}: {}",
            text.chars().take(400).collect::<String>()
        ));
    }
    let chat: ChatResponse = serde_json::from_str(&text)
        .with_context(|| format!("unexpected LLM payload: {}", text.chars().take(240).collect::<String>()))?;
    let content = chat
        .choices
        .first()
        .and_then(|c| c.message.content.as_deref())
        .ok_or_else(|| anyhow!("LLM returned no message content"))?;
    parse_ai_json(content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_markdown_fences() {
        let raw = "```json\n{\"items\":[],\"next_page_url\":null}\n```";
        assert_eq!(strip_json_fences(raw), "{\"items\":[],\"next_page_url\":null}");
    }

    #[test]
    fn parses_fenced_extract() {
        let raw = "```\n{\"items\":[{\"title\":\"Dev\",\"url\":\"https://ex.com/j/1\",\"published_at\":\"2026-10-01\"}],\"next_page_url\":\"https://ex.com/p=2\"}\n```";
        let parsed = parse_ai_json(raw).expect("json");
        assert_eq!(parsed.items.len(), 1);
        assert_eq!(parsed.items[0].title, "Dev");
        assert_eq!(
            parsed.next_page_url.as_deref(),
            Some("https://ex.com/p=2")
        );
    }

    #[test]
    fn date_filter_keeps_undated_drops_older() {
        let extract = AiPageExtract {
            items: vec![
                AiJobItem {
                    title: "Old".into(),
                    url: "https://ex.com/old".into(),
                    company: None,
                    location: None,
                    salary: None,
                    description: None,
                    published_at: Some("2026-01-01".into()),
                },
                AiJobItem {
                    title: "In".into(),
                    url: "https://ex.com/in".into(),
                    company: None,
                    location: None,
                    salary: None,
                    description: None,
                    published_at: Some("2026-10-02".into()),
                },
                AiJobItem {
                    title: "Undated".into(),
                    url: "https://ex.com/u".into(),
                    company: None,
                    location: None,
                    salary: None,
                    description: None,
                    published_at: None,
                },
                AiJobItem {
                    title: "Future".into(),
                    url: "https://ex.com/f".into(),
                    company: None,
                    location: None,
                    salary: None,
                    description: None,
                    published_at: Some("2026-12-01".into()),
                },
            ],
            next_page_url: None,
        };
        let from = NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
        let to = NaiveDate::from_ymd_opt(2026, 10, 31).unwrap();
        let (kept, saw_older) = filter_extract(extract, from, to);
        assert!(saw_older);
        let titles: Vec<_> = kept.iter().map(|(i, _)| i.title.as_str()).collect();
        assert_eq!(titles, vec!["In", "Undated"]);
        assert!(item_in_window(None, from, to));
        assert!(!item_in_window(
            Some(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
            from,
            to
        ));
    }
}
