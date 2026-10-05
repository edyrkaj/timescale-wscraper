use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Datelike, NaiveDate, NaiveTime, TimeZone, Utc};
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::page::Page;
use futures_util::StreamExt;
use regex::Regex;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use url::Url;

use crate::models::{
    ApiListConfig, DetailConfig, DetailSource, DomListConfig, ListSource, PatternConfig,
    ScrapeStats, ScrapedItem,
};

pub const MAX_PAGES: i32 = 50;
pub const MAX_ITEMS: i32 = 2000;

pub struct BrowserPool {
    browser: Arc<Browser>,
}

impl BrowserPool {
    pub async fn try_launch() -> Result<Self> {
        Self::launch().await
    }

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
    browser: Option<Arc<Browser>>,
    listing_url: &str,
    from_date: NaiveDate,
    to_date: NaiveDate,
    pattern: Option<&PatternConfig>,
    cancel: CancellationToken,
    on_batch: F,
) -> Result<ScrapeStats>
where
    F: FnMut(Vec<ScrapedItem>, i32, i32) -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    let inferred;
    let cfg = match pattern {
        Some(c) => Some(c),
        None => {
            inferred = infer_erekrutim_pattern(listing_url);
            inferred.as_ref()
        }
    };

    if let Some(cfg) = cfg {
        info!("using pattern-based scrape (API/DOM)");
        return scrape_with_pattern(browser, listing_url, from_date, to_date, cfg, cancel, on_batch)
            .await;
    }

    let browser = browser.context("no scrape pattern matched and Chromium is unavailable")?;
    scrape_heuristic(browser, listing_url, from_date, to_date, cancel, on_batch).await
}

/// Build an API list pattern for Albanian e-rekrutim style `/shpalljet` portals.
pub fn infer_erekrutim_pattern(listing_url: &str) -> Option<PatternConfig> {
    let parsed = Url::parse(listing_url).ok()?;
    let host = parsed.host_str()?.to_lowercase();
    let path = parsed.path().to_lowercase();
    if !path.contains("shpalljet") && !host.contains("rekrutimi") {
        return None;
    }
    // Prefer hosts that look like the known Angular e-rekrutim apps
    if !(host.contains("rekrutimi") || path.contains("shpalljet")) {
        return None;
    }

    let origin = format!("{}://{}", parsed.scheme(), parsed.host_str()?);
    Some(PatternConfig {
        list: crate::models::ListConfig {
            source: ListSource::Api,
            api: Some(ApiListConfig {
                url_template: format!(
                    "{origin}/api/api/Job/public-announcements?PageNumber={{page}}&PageSize={{page_size}}"
                ),
                page_size: 50,
                items_path: "$".into(),
                id_path: "id".into(),
                published_at_path: Some("job.0.publishedDate".into()),
                title_path: Some("job.0.jobPositionsResponse.0.positionName".into()),
                company_path: None,
                company_literal: Some(host.clone()),
                location_path: Some("job.0.jobPositionsResponse.0.organisationalUnit".into()),
                salary_path: Some("job.0.jobPositionsResponse.0.categoryName".into()),
                description_path: None,
                detail_url_template: Some(format!("{origin}/shpalljet/{{id}}")),
            }),
            dom: None,
        },
        detail: Some(DetailConfig {
            source: DetailSource::Api,
            wait_ms: None,
            api_url_template: Some(format!(
                "{origin}/api/api/Job/get-positions-by-job/{{id}}"
            )),
            description_path: Some("0.positionDescription".into()),
            fields: Default::default(),
        }),
    })
}

async fn scrape_with_pattern<F, Fut>(
    browser: Option<Arc<Browser>>,
    listing_url: &str,
    from_date: NaiveDate,
    to_date: NaiveDate,
    pattern: &PatternConfig,
    cancel: CancellationToken,
    mut on_batch: F,
) -> Result<ScrapeStats>
where
    F: FnMut(Vec<ScrapedItem>, i32, i32) -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    let source_id = source_id_from_url(listing_url)?;
    let mut stats = ScrapeStats::default();

    let mut candidates = match pattern.list.source {
        ListSource::Api => {
            let api = pattern
                .list
                .api
                .as_ref()
                .context("pattern list.source=api but api config missing")?;
            collect_from_api(api, &source_id, from_date, to_date, &cancel, &mut stats).await?
        }
        ListSource::Dom => {
            let browser = browser.clone().context("DOM scrape requires Chromium")?;
            let dom = pattern
                .list
                .dom
                .as_ref()
                .context("pattern list.source=dom but dom config missing")?;
            collect_from_dom(browser, listing_url, dom, &source_id, &cancel, &mut stats).await?
        }
    };

    info!(count = candidates.len(), "list candidates collected");

    if let Some(detail) = &pattern.detail {
        match detail.source {
            DetailSource::Page => {
                let browser = browser.context("detail.source=page requires Chromium")?;
                let page = browser.new_page("about:blank").await?;
                let mut enriched = Vec::new();
                for mut item in candidates {
                    if cancel.is_cancelled() || stats.scraped_count >= MAX_ITEMS {
                        break;
                    }
                    if let Err(err) = enrich_from_detail_page(&page, &mut item, detail).await {
                        warn!(error = %err, url = %item.url, "detail enrich failed");
                    }
                    let d = item.item_timestamp.date_naive();
                    if d < from_date || d > to_date {
                        continue;
                    }
                    enriched.push(item);
                    if enriched.len() >= 10 {
                        let batch_len = enriched.len() as i32;
                        stats.scraped_count += batch_len;
                        on_batch(
                            std::mem::take(&mut enriched),
                            stats.scraped_count,
                            stats.pages_visited,
                        )
                        .await?;
                    }
                }
                if !enriched.is_empty() {
                    let batch_len = enriched.len() as i32;
                    stats.scraped_count += batch_len;
                    on_batch(enriched, stats.scraped_count, stats.pages_visited).await?;
                }
                return Ok(stats);
            }
            DetailSource::Api => {
                let client = reqwest::Client::builder()
                    .user_agent("timescale-wscraper/0.1")
                    .build()?;
                let template = detail
                    .api_url_template
                    .as_ref()
                    .context("detail.source=api requires api_url_template")?;
                let desc_path = detail
                    .description_path
                    .clone()
                    .unwrap_or_else(|| "0.positionDescription".into());

                let mut enriched = Vec::new();
                for mut item in candidates {
                    if cancel.is_cancelled() || stats.scraped_count >= MAX_ITEMS {
                        break;
                    }
                    // Extract id from detail URL path last segment
                    let id = item
                        .url
                        .rsplit('/')
                        .next()
                        .unwrap_or("")
                        .to_string();
                    let url = template.replace("{id}", &id);
                    match client.get(&url).send().await {
                        Ok(res) if res.status().is_success() => {
                            if let Ok(body) = res.json::<Value>().await {
                                if let Some(desc) = json_path_string(&body, &desc_path) {
                                    let plain = strip_html(&desc);
                                    if !plain.is_empty() {
                                        item.description = Some(plain.chars().take(4000).collect());
                                    }
                                }
                                // Prefer richer title from positionName + branch when present
                                if let Some(name) = json_path_string(&body, "0.positionName") {
                                    let branch = json_path_string(&body, "0.positionBranch")
                                        .unwrap_or_default();
                                    if !name.is_empty() {
                                        item.title = if branch.is_empty() {
                                            name
                                        } else {
                                            format!("{name}, {branch}")
                                        };
                                    }
                                }
                            }
                        }
                        Ok(res) => warn!(status = %res.status(), %url, "detail API non-success"),
                        Err(err) => warn!(error = %err, %url, "detail API failed"),
                    }

                    let d = item.item_timestamp.date_naive();
                    if d < from_date || d > to_date {
                        continue;
                    }
                    enriched.push(item);
                    if enriched.len() >= 10 {
                        let batch_len = enriched.len() as i32;
                        stats.scraped_count += batch_len;
                        on_batch(
                            std::mem::take(&mut enriched),
                            stats.scraped_count,
                            stats.pages_visited,
                        )
                        .await?;
                    }
                }
                if !enriched.is_empty() {
                    let batch_len = enriched.len() as i32;
                    stats.scraped_count += batch_len;
                    on_batch(enriched, stats.scraped_count, stats.pages_visited).await?;
                }
                return Ok(stats);
            }
            DetailSource::None => {}
        }
    }

    // No detail enrichment: save list candidates (already date-filtered in API path)
    let mut batch = Vec::new();
    for item in candidates.drain(..) {
        if cancel.is_cancelled() || stats.scraped_count >= MAX_ITEMS {
            break;
        }
        let d = item.item_timestamp.date_naive();
        if d < from_date || d > to_date {
            continue;
        }
        batch.push(item);
        if batch.len() >= 25 {
            let batch_len = batch.len() as i32;
            stats.scraped_count += batch_len;
            on_batch(std::mem::take(&mut batch), stats.scraped_count, stats.pages_visited).await?;
        }
    }
    if !batch.is_empty() {
        let batch_len = batch.len() as i32;
        stats.scraped_count += batch_len;
        on_batch(batch, stats.scraped_count, stats.pages_visited).await?;
    }

    Ok(stats)
}

fn strip_html(input: &str) -> String {
    let re = Regex::new(r"<[^>]+>").unwrap_or_else(|_| Regex::new(r"a^").unwrap());
    let text = re.replace_all(input, " ");
    let mut decoded = text.into_owned();
    for _ in 0..5 {
        let next = html_escape::decode_html_entities(&decoded);
        if next.as_ref() == decoded.as_str() {
            break;
        }
        decoded = next.into_owned();
    }
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

async fn collect_from_api(
    api: &ApiListConfig,
    source_id: &str,
    from_date: NaiveDate,
    to_date: NaiveDate,
    cancel: &CancellationToken,
    stats: &mut ScrapeStats,
) -> Result<Vec<ScrapedItem>> {
    let client = reqwest::Client::builder()
        .user_agent("timescale-wscraper/0.1")
        .build()?;

    let mut items = Vec::new();
    let mut page: u32 = 1;
    let mut saw_older = false;

    while page <= MAX_PAGES as u32 && items.len() < MAX_ITEMS as usize {
        if cancel.is_cancelled() || saw_older {
            break;
        }
        stats.pages_visited += 1;
        let url = api
            .url_template
            .replace("{page}", &page.to_string())
            .replace("{page_size}", &api.page_size.to_string());
        info!(%url, page, "fetching pattern list API");

        let body: Value = client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?
            .error_for_status()
            .with_context(|| format!("bad status for {url}"))?
            .json()
            .await
            .context("decode list JSON")?;

        let arr = json_path(&body, &api.items_path)
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        if arr.is_empty() {
            break;
        }

        let arr_len = arr.len();
        for entry in arr {
            let id = json_path(&entry, &api.id_path)
                .and_then(|v| v.as_str().map(|s| s.to_string()).or_else(|| {
                    v.as_i64().map(|n| n.to_string())
                }))
                .unwrap_or_else(|| external_id_for_url(&entry.to_string()));

            let published = api
                .published_at_path
                .as_ref()
                .and_then(|p| json_path(&entry, p))
                .and_then(parse_json_date);

            if let Some(d) = published {
                if d < from_date {
                    saw_older = true;
                    continue;
                }
                if d > to_date {
                    continue;
                }
            }

            let title = api
                .title_path
                .as_ref()
                .and_then(|p| json_path_string(&entry, p))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("Job {id}"));

            let company = api
                .company_path
                .as_ref()
                .and_then(|p| json_path_string(&entry, p))
                .or_else(|| api.company_literal.clone());

            let location = api
                .location_path
                .as_ref()
                .and_then(|p| json_path_string(&entry, p));
            let salary = api
                .salary_path
                .as_ref()
                .and_then(|p| json_path_string(&entry, p));
            let description = api
                .description_path
                .as_ref()
                .and_then(|p| json_path_string(&entry, p));

            let detail_url = api
                .detail_url_template
                .as_ref()
                .map(|t| t.replace("{id}", &id))
                .unwrap_or_else(|| id.clone());

            items.push(ScrapedItem {
                source_id: source_id.to_string(),
                external_id: external_id_for_url(&detail_url),
                item_timestamp: published
                    .map(date_to_utc)
                    .unwrap_or_else(Utc::now),
                title,
                url: detail_url,
                company,
                location,
                salary,
                description,
            });
        }

        if arr_len < api.page_size as usize {
            break;
        }
        page += 1;
    }

    Ok(items)
}

async fn collect_from_dom(
    browser: Arc<Browser>,
    listing_url: &str,
    dom: &DomListConfig,
    source_id: &str,
    cancel: &CancellationToken,
    stats: &mut ScrapeStats,
) -> Result<Vec<ScrapedItem>> {
    let page = browser.new_page("about:blank").await?;
    let mut current = listing_url.to_string();
    let mut seen = std::collections::HashSet::new();
    let mut items = Vec::new();
    let link_re = dom
        .item_link_regex
        .as_ref()
        .and_then(|r| Regex::new(r).ok());
    let use_cards = dom.card_selector.as_ref().is_some_and(|s| !s.is_empty());

    while stats.pages_visited < MAX_PAGES && items.len() < MAX_ITEMS as usize {
        if cancel.is_cancelled() {
            break;
        }
        stats.pages_visited += 1;
        page.goto(&current).await?;
        page.wait_for_navigation().await.ok();
        let wait_ms = dom.wait_ms.unwrap_or(2000);
        tokio::time::sleep(std::time::Duration::from_millis(wait_ms)).await;

        if let Some(sel) = &dom.wait_for {
            for _ in 0..20 {
                let present: bool = page
                    .evaluate(format!(
                        "!!document.querySelector({})",
                        serde_json::to_string(sel).unwrap()
                    ))
                    .await
                    .ok()
                    .and_then(|r| r.into_value().ok())
                    .unwrap_or(false);
                if present {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
        }

        if use_cards {
            let cfg = serde_json::json!({
                "card": dom.card_selector,
                "title": dom.title_selector,
                "company": dom.company_selector,
                "location": dom.location_selector,
                "date": dom.date_selector,
            });
            let cards: Vec<DomCardExtract> = page
                .evaluate(format!(
                    r#"
                    (() => {{
                      const cfg = {cfg};
                      const text = (el) => (el && (el.innerText || el.textContent) || '').trim();
                      return [...document.querySelectorAll(cfg.card)].map((card) => {{
                        const titleEl = cfg.title ? card.querySelector(cfg.title) : null;
                        const companyEl = cfg.company ? card.querySelector(cfg.company) : null;
                        const locationEl = cfg.location ? card.querySelector(cfg.location) : null;
                        const dateEl = cfg.date ? card.querySelector(cfg.date) : null;
                        return {{
                          url: titleEl && titleEl.href ? titleEl.href : null,
                          title: text(titleEl) || null,
                          company: text(companyEl) || null,
                          location: text(locationEl) || null,
                          dateText: text(dateEl) || null
                        }};
                      }}).filter((row) => row.url);
                    }})()
                    "#,
                    cfg = cfg
                ))
                .await?
                .into_value()
                .unwrap_or_default();

            for card in cards {
                let href = match card.url {
                    Some(u) if !u.is_empty() => u,
                    _ => continue,
                };
                if let Some(re) = &link_re {
                    if !re.is_match(&href) {
                        continue;
                    }
                }
                if !seen.insert(href.clone()) {
                    continue;
                }
                let ts = card
                    .date_text
                    .as_deref()
                    .and_then(parse_date_from_text)
                    .map(date_to_utc)
                    .unwrap_or_else(Utc::now);
                let title = card
                    .title
                    .filter(|t| t.len() >= 2)
                    .unwrap_or_else(|| "Untitled job".into());
                items.push(ScrapedItem {
                    source_id: source_id.to_string(),
                    external_id: external_id_for_url(&href),
                    item_timestamp: ts,
                    title: title.chars().take(300).collect(),
                    url: href,
                    company: card.company.filter(|s| !s.is_empty()),
                    location: card.location.filter(|s| !s.is_empty()),
                    salary: None,
                    description: None,
                });
            }
        } else {
            let selector = dom
                .item_link_selector
                .clone()
                .unwrap_or_else(|| "a[href]".into());

            let hrefs: Vec<String> = page
                .evaluate(format!(
                    r#"
                    (() => {{
                      const sel = {sel};
                      return [...document.querySelectorAll(sel)]
                        .map(a => a.href)
                        .filter(Boolean);
                    }})()
                    "#,
                    sel = serde_json::to_string(&selector).unwrap()
                ))
                .await?
                .into_value()
                .unwrap_or_default();

            for href in hrefs {
                if let Some(re) = &link_re {
                    if !re.is_match(&href) {
                        continue;
                    }
                }
                if !seen.insert(href.clone()) {
                    continue;
                }
                items.push(ScrapedItem {
                    source_id: source_id.to_string(),
                    external_id: external_id_for_url(&href),
                    item_timestamp: Utc::now(),
                    title: "Untitled job".into(),
                    url: href,
                    company: None,
                    location: None,
                    salary: None,
                    description: None,
                });
            }
        }

        let next_sel = match &dom.next_page_selector {
            Some(s) => s.clone(),
            None => break,
        };
        let next_href: Option<String> = page
            .evaluate(format!(
                r#"
                (() => {{
                  const el = document.querySelector({sel});
                  if (!el) return null;
                  if (el.disabled || el.getAttribute('aria-disabled') === 'true') return null;
                  if (el.href) return el.href;
                  el.click();
                  return 'clicked';
                }})()
                "#,
                sel = serde_json::to_string(&next_sel).unwrap()
            ))
            .await?
            .into_value()
            .ok()
            .flatten();

        match next_href {
            Some(h) if h != "clicked" && h != current => current = h,
            Some(_) => {
                tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
            }
            None => break,
        }
    }

    Ok(items)
}

#[derive(Debug, Default, Clone, serde::Deserialize)]
struct DomCardExtract {
    url: Option<String>,
    title: Option<String>,
    company: Option<String>,
    location: Option<String>,
    #[serde(rename = "dateText")]
    date_text: Option<String>,
}

async fn enrich_from_detail_page(
    page: &Page,
    item: &mut ScrapedItem,
    detail: &DetailConfig,
) -> Result<()> {
    page.goto(&item.url).await?;
    page.wait_for_navigation().await.ok();
    let wait_ms = detail.wait_ms.unwrap_or(2000);
    tokio::time::sleep(std::time::Duration::from_millis(wait_ms)).await;

    let fields = &detail.fields;
    let payload = serde_json::json!({
        "titleSelector": fields.title_selector,
        "titleLabel": fields.title_label,
        "companySelector": fields.company_selector,
        "companyLabel": fields.company_label,
        "locationSelector": fields.location_selector,
        "locationLabel": fields.location_label,
        "salarySelector": fields.salary_selector,
        "salaryLabel": fields.salary_label,
        "descriptionSelector": fields.description_selector,
        "dateSelector": fields.date_selector,
        "dateLabel": fields.date_label,
    });

    let extracted: DetailExtract = page
        .evaluate(format!(
            r#"
            (() => {{
              const cfg = {cfg};
              const byLabel = (label) => {{
                if (!label) return null;
                const nodes = [...document.querySelectorAll('div, span, p, dt, th, strong, label, h1, h2, h3, h4')];
                for (const n of nodes) {{
                  const t = (n.innerText || '').trim();
                  if (t === label || t.startsWith(label)) {{
                    const sibling = n.nextElementSibling;
                    if (sibling && (sibling.innerText || '').trim()) return sibling.innerText.trim().slice(0, 2000);
                    const parent = n.parentElement;
                    if (parent) {{
                      const parts = (parent.innerText || '').split('\\n').map(s => s.trim()).filter(Boolean);
                      const idx = parts.findIndex(p => p === label || p.startsWith(label));
                      if (idx >= 0 && parts[idx+1]) return parts[idx+1].slice(0, 2000);
                    }}
                  }}
                }}
                return null;
              }};
              const bySel = (sel) => sel ? (document.querySelector(sel)?.innerText || '').trim().slice(0, 4000) || null : null;
              return {{
                title: bySel(cfg.titleSelector) || byLabel(cfg.titleLabel),
                company: bySel(cfg.companySelector) || byLabel(cfg.companyLabel),
                location: bySel(cfg.locationSelector) || byLabel(cfg.locationLabel),
                salary: bySel(cfg.salarySelector) || byLabel(cfg.salaryLabel),
                description: bySel(cfg.descriptionSelector),
                dateText: bySel(cfg.dateSelector) || byLabel(cfg.dateLabel) || document.body.innerText.slice(0, 5000)
              }};
            }})()
            "#,
            cfg = payload
        ))
        .await?
        .into_value()
        .unwrap_or_default();

    if let Some(t) = extracted.title.filter(|s| s.len() >= 3) {
        item.title = t.chars().take(300).collect();
    }
    if item.company.is_none() {
        item.company = extracted.company;
    }
    if item.location.is_none() {
        item.location = extracted.location;
    }
    if item.salary.is_none() {
        item.salary = extracted.salary;
    }
    if let Some(d) = extracted.description {
        item.description = Some(d);
    }

    let date_text = extracted.date_text.unwrap_or_default();
    if let Some(re_s) = &fields.date_regex {
        if let Ok(re) = Regex::new(re_s) {
            if let Some(caps) = re.captures(&date_text) {
                if let Some(m) = caps.get(1).or_else(|| caps.get(0)) {
                    if let Some(d) = parse_slash_date(m.as_str()) {
                        item.item_timestamp = date_to_utc(d);
                    }
                }
            }
        }
    } else if let Some(d) = parse_date_from_text(&date_text) {
        item.item_timestamp = date_to_utc(d);
    }

    Ok(())
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct DetailExtract {
    title: Option<String>,
    company: Option<String>,
    location: Option<String>,
    salary: Option<String>,
    description: Option<String>,
    date_text: Option<String>,
}

async fn scrape_heuristic<F, Fut>(
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

        match scrape_page_heuristic(
            &page,
            &current_url,
            &source_id,
            &job_link_re,
            from_date,
            to_date,
        )
        .await
        {
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

async fn scrape_page_heuristic(
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

fn json_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    if path == "$" || path.is_empty() {
        return Some(value);
    }
    let mut cur = value;
    for part in path.split('.') {
        if part.is_empty() {
            continue;
        }
        cur = if let Ok(idx) = part.parse::<usize>() {
            cur.as_array()?.get(idx)?
        } else {
            cur.get(part)?
        };
    }
    Some(cur)
}

fn json_path_string(value: &Value, path: &str) -> Option<String> {
    let v = json_path(value, path)?;
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn parse_json_date(v: &Value) -> Option<NaiveDate> {
    match v {
        Value::String(s) => {
            if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
                return Some(dt.with_timezone(&Utc).date_naive());
            }
            if s.len() >= 10 {
                if let Ok(d) = NaiveDate::parse_from_str(&s[..10], "%Y-%m-%d") {
                    return Some(d);
                }
            }
            parse_slash_date(s).or_else(|| parse_date_from_text(s))
        }
        _ => None,
    }
}

fn parse_slash_date(s: &str) -> Option<NaiveDate> {
    // Albanian boards use DD/MM/YYYY or DD-MM-YYYY
    let re = Regex::new(r"\b(\d{1,2})[/-](\d{1,2})[/-](20\d{2})\b").ok()?;
    let caps = re.captures(s)?;
    let a: u32 = caps.get(1)?.as_str().parse().ok()?;
    let b: u32 = caps.get(2)?.as_str().parse().ok()?;
    let y: i32 = caps.get(3)?.as_str().parse().ok()?;
    // DD/MM/YYYY preferred for Albanian sites
    NaiveDate::from_ymd_opt(y, b, a).or_else(|| NaiveDate::from_ymd_opt(y, a, b))
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
        r"(?i)/(jobs?|careers?|positions?|vacancies|openings|employment|hiring|shpalljet)(/|$|\?|#)",
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
        || t.contains("hiring")
        || t.contains("specialist"))
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

    if let Some(d) = parse_slash_date(text) {
        return Some(d);
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

    let months = [
        ("jan", 1u32),
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
        let re =
            Regex::new(&format!(r"(?i)\b{name}[a-z]*\.?\s+(\d{{1,2}})(?:,?\s+(20\d{{2}}))?\b"))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_duapune_hyphen_dates() {
        let d = parse_slash_date("02-11-2026").expect("date");
        assert_eq!(d, NaiveDate::from_ymd_opt(2026, 11, 2).unwrap());
    }
}
