mod anthropic;
mod gemini;

pub use anthropic::AnthropicProvider;
pub use gemini::GeminiProvider;

use anyhow::{anyhow, Result};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::models::AiProvider;

const SYSTEM_PROMPT: &str = "You extract job listings from a rendered listing page. Reply with JSON only, no markdown. Schema: {\"items\":[{\"title\":\"\",\"url\":\"https://...\",\"company\":null,\"location\":null,\"salary\":null,\"description\":null,\"published_at\":\"YYYY-MM-DD or null\"}],\"next_page_url\":null}. Use absolute URLs. next_page_url is the next listing page or null. Prefer jobs in the given date window. Keep description null unless a short snippet is visible. Return at most 40 items. Prefer compact single-line objects so the JSON finishes completely.";

#[derive(Debug, Clone)]
pub struct LlmConfig {
    pub provider: AiProvider,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

impl LlmConfig {
    pub fn from_env(provider: AiProvider) -> Result<Self> {
        match provider {
            AiProvider::Anthropic => AnthropicProvider::config_from_env(),
            AiProvider::Gemini => GeminiProvider::config_from_env(),
        }
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

pub fn system_prompt() -> &'static str {
    SYSTEM_PROMPT
}

pub fn build_user_prompt(
    listing_url: &str,
    from_date: NaiveDate,
    to_date: NaiveDate,
    page: &FetchedPage,
) -> String {
    let links: Vec<serde_json::Value> = page
        .links
        .iter()
        .take(200)
        .map(|l| serde_json::json!({"text": l.text, "href": l.href}))
        .collect();
    format!(
        "Listing URL: {listing_url}\nFinal URL: {}\nPage title: {}\nDate window (inclusive): {from_date} to {to_date}\n\nPage text:\n{}\n\nLinks (JSON):\n{}",
        page.final_url,
        page.title,
        page.text,
        serde_json::to_string(&links).unwrap_or_else(|_| "[]".into())
    )
}

pub fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
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
    &without_close[start..]
}

/// Close truncated LLM JSON so we can keep complete `items` objects.
pub fn repair_truncated_json(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    if s.is_empty() {
        return s;
    }

    // Drop a trailing incomplete string value if quotes are unbalanced.
    let mut in_string = false;
    let mut escape = false;
    let mut last_string_start = None;
    for (i, ch) in s.char_indices() {
        if escape {
            escape = false;
            continue;
        }
        match ch {
            '\\' if in_string => escape = true,
            '"' => {
                if in_string {
                    in_string = false;
                    last_string_start = None;
                } else {
                    in_string = true;
                    last_string_start = Some(i);
                }
            }
            _ => {}
        }
    }
    if in_string {
        if let Some(start) = last_string_start {
            // Truncate back to before the incomplete key/value string,
            // then trim trailing comma/colon junk.
            s.truncate(start);
            while s.ends_with([' ', '\t', '\n', '\r', ',', ':', '{']) {
                if s.ends_with('{') {
                    // unfinished object — drop the '{'
                    s.pop();
                    break;
                }
                s.pop();
            }
        }
    }

    // Prefer salvaging complete objects inside `"items": [ ... ]`.
    if let Some(repaired) = salvage_items_array(&s) {
        return repaired;
    }

    // Generic brace/bracket closer as a last resort.
    let mut stack = Vec::new();
    in_string = false;
    escape = false;
    for ch in s.chars() {
        if escape {
            escape = false;
            continue;
        }
        match ch {
            '\\' if in_string => escape = true,
            '"' => in_string = !in_string,
            '{' if !in_string => stack.push('}'),
            '[' if !in_string => stack.push(']'),
            '}' | ']' if !in_string => {
                let _ = stack.pop();
            }
            _ => {}
        }
    }
    // Trim trailing comma before closing.
    while s.ends_with([' ', '\t', '\n', '\r', ',']) {
        s.pop();
    }
    while let Some(c) = stack.pop() {
        s.push(c);
    }
    s
}

fn salvage_items_array(raw: &str) -> Option<String> {
    let lower = raw.to_ascii_lowercase();
    let items_key = lower.find("\"items\"")?;
    let after_key = &raw[items_key..];
    let bracket = after_key.find('[')?;
    let arr_start = items_key + bracket + 1;
    let arr_body = &raw[arr_start..];

    let mut items = Vec::new();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escape = false;
    let mut obj_start: Option<usize> = None;

    for (i, ch) in arr_body.char_indices() {
        if escape {
            escape = false;
            continue;
        }
        match ch {
            '\\' if in_string => escape = true,
            '"' => in_string = !in_string,
            '{' if !in_string => {
                if depth == 0 {
                    obj_start = Some(i);
                }
                depth += 1;
            }
            '}' if !in_string => {
                if depth == 0 {
                    continue;
                }
                depth -= 1;
                if depth == 0 {
                    if let Some(start) = obj_start.take() {
                        let obj = &arr_body[start..=i];
                        if serde_json::from_str::<AiJobItem>(obj).is_ok()
                            || serde_json::from_str::<serde_json::Value>(obj)
                                .ok()
                                .and_then(|v| serde_json::from_value::<AiJobItem>(v).ok())
                                .is_some()
                        {
                            items.push(obj.to_string());
                        } else if let Ok(v) = serde_json::from_str::<serde_json::Value>(obj) {
                            // Keep objects that at least have title+url
                            if v.get("title").and_then(|t| t.as_str()).is_some()
                                && v.get("url").and_then(|u| u.as_str()).is_some()
                            {
                                items.push(obj.to_string());
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    if items.is_empty() {
        return None;
    }

    // Preserve next_page_url when the model finished that field.
    let next = extract_next_page_url(raw);
    let next_json = match next {
        Some(u) => serde_json::to_string(&u).unwrap_or_else(|_| "null".into()),
        None => "null".into(),
    };
    Some(format!(
        "{{\"items\":[{}],\"next_page_url\":{}}}",
        items.join(","),
        next_json
    ))
}

fn extract_next_page_url(raw: &str) -> Option<String> {
    let re = regex::Regex::new(
        r#""next_page_url"\s*:\s*(null|"([^"\\]|\\.)*")"#,
    )
    .ok()?;
    let caps = re.captures(raw)?;
    let whole = caps.get(1)?.as_str();
    if whole == "null" {
        return None;
    }
    serde_json::from_str::<String>(whole).ok()
}

pub fn parse_ai_json(raw: &str) -> Result<AiPageExtract> {
    let stripped = strip_json_fences(raw);
    if let Ok(parsed) = serde_json::from_str::<AiPageExtract>(stripped) {
        return Ok(parsed);
    }
    let repaired = repair_truncated_json(stripped);
    serde_json::from_str(&repaired).map_err(|err| {
        anyhow!(
            "LLM response was not valid job JSON ({err}): {}",
            repaired.chars().take(240).collect::<String>()
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
    let user = build_user_prompt(listing_url, from_date, to_date, page);
    let system = system_prompt();
    match cfg.provider {
        AiProvider::Anthropic => AnthropicProvider::extract(cfg, system, &user).await,
        AiProvider::Gemini => GeminiProvider::extract(cfg, system, &user).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_markdown_fences() {
        let raw = "```json\n{\"items\":[],\"next_page_url\":null}\n```";
        assert_eq!(
            strip_json_fences(raw),
            "{\"items\":[],\"next_page_url\":null}"
        );
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
    fn salvages_truncated_items_json() {
        let raw = r#"{
  "items": [
    {"title":"A","url":"https://ex.com/a","company":null,"location":null,"salary":null,"description":null,"published_at":null},
    {"title":"B","url":"https://ex.com/b","company":"X","location":"Tirane","salary":null,"description":null,"published_at":null
"#;
        let parsed = parse_ai_json(raw).expect("salvaged");
        assert_eq!(parsed.items.len(), 1);
        assert_eq!(parsed.items[0].title, "A");
        assert!(parsed.next_page_url.is_none());
    }

    #[test]
    fn salvages_many_complete_then_cut_off() {
        let mut raw = String::from(r#"{"items":["#);
        for i in 0..5 {
            if i > 0 {
                raw.push(',');
            }
            raw.push_str(&format!(
                r#"{{"title":"Job {i}","url":"https://ex.com/{i}","published_at":null}}"#
            ));
        }
        raw.push_str(r#",{"title":"Cut","url":"https://ex.com/x","published_at":"#);
        let parsed = parse_ai_json(&raw).expect("salvaged");
        assert_eq!(parsed.items.len(), 5);
        assert_eq!(parsed.items[4].title, "Job 4");
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
