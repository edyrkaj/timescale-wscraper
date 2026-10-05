use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Datelike, NaiveDate, NaiveTime, TimeZone, Utc};
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::page::Page;
use futures_util::StreamExt;
use regex::Regex;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use url::Url;

use crate::models::{ScrapeStats, ScrapedItem};

pub const MAX_PAGES: i32 = 50;
pub const MAX_ITEMS: i32 = 2000;

pub struct BrowserPool {
    browser: Arc<Browser>,
}

impl BrowserPool {
    pub async fn launch() -> Result<Self> {
        let chrome = std::env::var("CHROME_PATH").unwrap_or_else(|_| {
            [
                "/usr/bin/chromium",
                "/usr/bin/chromium-browser",
                "/usr/bin/google-chrome",
                "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            ]
            .into_iter()
            .find(|p| Path::new(p).exists())
            .unwrap_or("/usr/bin/chromium")
            .to_string()
        });

        let config = BrowserConfig::builder()
            .chrome_executable(chrome)
            .arg("--no-sandbox")
            .arg("--disable-dev-shm-usage")
            .arg("--disable-gpu")
            .arg("--headless=new")
            .viewport(None)
            .build()
            .map_err(|e| anyhow!("browser config: {e}"))?;

        let (browser, mut handler) = Browser::launch(config)
            .await
            .context("failed to launch Chromium")?;

        tokio::spawn(async move {
            while let Some(_event) = handler.next().await {}
        });

        Ok(Self {
            browser: Arc::new(browser),
        })
    }

    pub fn browser(&self) -> Arc<Browser> {
        self.browser.clone()
    }
}

pub async fn scrape_listing<F, Fut>(
    browser: Arc<Browser>,
    listing_url: &str,
    from_date: NaiveDate,
    to_date: NaiveDate,
    cancel: CancellationToken,
    mut on_batch: F,
) -> Result<ScrapeStats>
where
    F: FnMut(Vec<ScrapedItem>, i32, i32) -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    let page = browser.new_page("about:blank").await?;
    let mut stats = ScrapeStats::default();
    let mut consecutive_failures = 0i32;
    let mut current_url = listing_url.to_string();
    let source_id = source_id_from_url(listing_url)?;
    let job_link_re = job_link_regex();

    while stats.pages_visited < MAX_PAGES && stats.scraped_count < MAX_ITEMS {
        if cancel.is_cancelled() {
            break;
        }

        stats.pages_visited += 1;
        info!(page = stats.pages_visited, url = %current_url, "visiting listing page");

        match scrape_page(&page, &current_url, &source_id, &job_link_re, from_date, to_date).await {
            Ok((items, next_url, saw_older_than_from)) => {
                consecutive_failures = 0;
                if !items.is_empty() {
                    let batch_len = items.len() as i32;
                    on_batch(items, stats.scraped_count + batch_len, stats.pages_visited).await?;
                    stats.scraped_count += batch_len;
                }

                if saw_older_than_from {
                    info!("dates fell before from_date; stopping pagination");
                    break;
                }

                match next_url {
                    Some(next) if next != current_url => current_url = next,
                    _ => break,
                }
            }
            Err(err) => {
                consecutive_failures += 1;
                warn!(error = %err, failures = consecutive_failures, "page scrape failed");
                if consecutive_failures >= 3 {
                    return Err(anyhow!("too many consecutive page failures: {err}"));
                }
            }
        }
    }

    Ok(stats)
}

async fn scrape_page(
    page: &Page,
    listing_url: &str,
    source_id: &str,
    job_link_re: &Regex,
    from_date: NaiveDate,
    to_date: NaiveDate,
) -> Result<(Vec<ScrapedItem>, Option<String>, bool)> {
    page.goto(listing_url).await?;
    page.wait_for_navigation().await.ok();
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;

    let extract = page
        .evaluate(
            r#"
            (() => {
              const out = [];
              const seen = new Set();
              for (const a of document.querySelectorAll('a[href]')) {
                const href = a.href;
                if (!href || seen.has(href)) continue;
                seen.add(href);
                const text = (a.innerText || a.textContent || '').trim();
                const parentText = (a.closest('li, article, div, tr, section')?.innerText || '').slice(0, 1000);
                out.push({ href, text, parentText });
              }
              let nextHref = null;
              for (const a of document.querySelectorAll('a[href]')) {
                const t = (a.innerText || a.textContent || '').toLowerCase().trim();
                const rel = (a.getAttribute('rel') || '').toLowerCase();
                const cls = (a.className || '').toString().toLowerCase();
                if (rel === 'next' || t === 'next' || t.includes('next page') || t === '>' || t === '›'
                    || cls.includes('pagination-next') || cls.includes('next')) {
                  nextHref = a.href;
                  break;
                }
              }
              return { anchors: out.slice(0, 500), nextHref };
            })()
            "#,
        )
        .await?;

    let data: PageExtract = extract.into_value().unwrap_or(PageExtract {
        anchors: vec![],
        next_href: None,
    });

    let mut items = Vec::new();
    let mut saw_older = false;
    let mut seen_urls = std::collections::HashSet::new();

    for anchor in data.anchors {
        if !job_link_re.is_match(&anchor.href) && !looks_like_job_text(&anchor.text) {
            continue;
        }
        if !seen_urls.insert(anchor.href.clone()) {
            continue;
        }

        let title = if anchor.text.len() >= 3 {
            anchor.text.clone()
        } else {
            guess_title(&anchor.parent_text).unwrap_or_else(|| "Untitled job".into())
        };

        let company = guess_field(&anchor.parent_text, &["company", "employer", "at "]);
        let location = guess_field(&anchor.parent_text, &["location", "remote", "hybrid", "onsite"]);
        let salary = guess_salary(&anchor.parent_text);
        let parsed_date = parse_date_from_text(&anchor.parent_text);

        let item_timestamp = match parsed_date {
            Some(d) => {
                if d < from_date {
                    saw_older = true;
                    continue;
                }
                if d > to_date {
                    continue;
                }
                date_to_utc(d)
            }
            None => Utc::now(),
        };

        items.push(ScrapedItem {
            source_id: source_id.to_string(),
            external_id: external_id_for_url(&anchor.href),
            item_timestamp,
            title: title.chars().take(300).collect(),
            url: anchor.href,
            company,
            location,
            salary,
            description: Some(anchor.parent_text.chars().take(500).collect()),
        });

        if items.len() as i32 >= MAX_ITEMS {
            break;
        }
    }

    Ok((items, data.next_href, saw_older))
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageExtract {
    anchors: Vec<AnchorInfo>,
    next_href: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AnchorInfo {
    href: String,
    text: String,
    #[serde(default, alias = "parentText")]
    parent_text: String,
}

fn source_id_from_url(listing_url: &str) -> Result<String> {
    let parsed = Url::parse(listing_url).context("invalid listing URL")?;
    Ok(parsed.host_str().unwrap_or("unknown").to_string())
}

fn external_id_for_url(job_url: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(job_url.as_bytes());
    hex::encode(hasher.finalize())[..32].to_string()
}

fn job_link_regex() -> Regex {
    Regex::new(
        r"(?i)/(jobs?|careers?|positions?|vacancies|openings|employment|hiring)(/|$|\?|#)",
    )
    .expect("valid regex")
}

fn looks_like_job_text(text: &str) -> bool {
    let t = text.to_lowercase();
    (t.contains("engineer")
        || t.contains("developer")
        || t.contains("manager")
        || t.contains("analyst")
        || t.contains("designer")
        || t.contains("intern")
        || t.contains("hiring"))
        && text.len() >= 8
        && text.len() <= 160
}

fn guess_title(parent: &str) -> Option<String> {
    parent
        .lines()
        .map(str::trim)
        .find(|l| l.len() >= 5 && l.len() <= 160)
        .map(|s| s.to_string())
}

fn guess_field(parent: &str, keywords: &[&str]) -> Option<String> {
    for line in parent.lines().map(str::trim) {
        let lower = line.to_lowercase();
        if keywords.iter().any(|k| lower.contains(k)) && line.len() <= 120 {
            return Some(line.to_string());
        }
    }
    None
}

fn guess_salary(parent: &str) -> Option<String> {
    let re = Regex::new(r"(?i)(\$|€|£)\s?\d[\d,]*(?:\s?[-–]\s?(\$|€|£)?\s?\d[\d,]*)?(?:\s?(k|k|/yr|/year|per year|a year))?").ok()?;
    re.find(parent).map(|m| m.as_str().to_string())
}

fn parse_date_from_text(text: &str) -> Option<NaiveDate> {
    let lower = text.to_lowercase();
    let today = Utc::now().date_naive();

    if lower.contains("today") {
        return Some(today);
    }
    if lower.contains("yesterday") {
        return today.checked_sub_signed(chrono::Duration::days(1));
    }

    let days_ago = Regex::new(r"(\d+)\s+days?\s+ago").ok()?;
    if let Some(caps) = days_ago.captures(&lower) {
        let n: i64 = caps.get(1)?.as_str().parse().ok()?;
        return today.checked_sub_signed(chrono::Duration::days(n));
    }

    let iso = Regex::new(r"\b(20\d{2})-(\d{2})-(\d{2})\b").ok()?;
    if let Some(caps) = iso.captures(text) {
        let y: i32 = caps.get(1)?.as_str().parse().ok()?;
        let m: u32 = caps.get(2)?.as_str().parse().ok()?;
        let d: u32 = caps.get(3)?.as_str().parse().ok()?;
        return NaiveDate::from_ymd_opt(y, m, d);
    }

    let slash = Regex::new(r"\b(\d{1,2})/(\d{1,2})/(20\d{2})\b").ok()?;
    if let Some(caps) = slash.captures(text) {
        let a: u32 = caps.get(1)?.as_str().parse().ok()?;
        let b: u32 = caps.get(2)?.as_str().parse().ok()?;
        let y: i32 = caps.get(3)?.as_str().parse().ok()?;
        return NaiveDate::from_ymd_opt(y, a, b).or_else(|| NaiveDate::from_ymd_opt(y, b, a));
    }

    let months = [
        ("jan", 1),
        ("feb", 2),
        ("mar", 3),
        ("apr", 4),
        ("may", 5),
        ("jun", 6),
        ("jul", 7),
        ("aug", 8),
        ("sep", 9),
        ("oct", 10),
        ("nov", 11),
        ("dec", 12),
    ];
    for (name, month) in months {
        let re = Regex::new(&format!(r"(?i)\b{name}[a-z]*\.?\s+(\d{{1,2}})(?:,?\s+(20\d{{2}}))?\b"))
            .ok()?;
        if let Some(caps) = re.captures(text) {
            let day: u32 = caps.get(1)?.as_str().parse().ok()?;
            let year: i32 = caps
                .get(2)
                .and_then(|m| m.as_str().parse().ok())
                .unwrap_or(today.year());
            return NaiveDate::from_ymd_opt(year, month, day);
        }
    }

    None
}

fn date_to_utc(d: NaiveDate) -> DateTime<Utc> {
    Utc.from_utc_datetime(&d.and_time(NaiveTime::from_hms_opt(12, 0, 0).unwrap()))
}
