use anyhow::Result;
use std::sync::Arc;
use tokio::sync::{broadcast, watch, Mutex};
use tokio_util::sync::CancellationToken;
use tracing::{error, info};
use uuid::Uuid;

use crate::db::Db;
use crate::models::{JobStatus, ProgressEvent, ScrapeJob};
use crate::scraper::{self, BrowserPool};

#[derive(Clone)]
pub struct QueueHandle {
    db: Db,
    browser: Option<Arc<BrowserPool>>,
    events: broadcast::Sender<ProgressEvent>,
    wake: watch::Sender<()>,
    cancel: Arc<Mutex<Option<(Uuid, CancellationToken)>>>,
}

impl QueueHandle {
    pub fn new(
        db: Db,
        browser: Option<Arc<BrowserPool>>,
        events: broadcast::Sender<ProgressEvent>,
    ) -> Self {
        let (wake, _) = watch::channel(());
        Self {
            db,
            browser,
            events,
            wake,
            cancel: Arc::new(Mutex::new(None)),
        }
    }

    pub fn notify(&self) {
        let _ = self.wake.send(());
    }

    pub async fn cancel_job(&self, id: Uuid) -> Result<()> {
        if let Some((_, token)) = self.cancel.lock().await.as_ref().filter(|(jid, _)| *jid == id) {
            token.cancel();
        }
        let _ = self.db.stop_job(id).await?;
        self.emit("Job stop requested").await;
        Ok(())
    }

    pub fn spawn(self) {
        tokio::spawn(async move {
            let mut rx = self.wake.subscribe();
            loop {
                if let Err(err) = self.tick().await {
                    error!(error = %err, "queue tick failed");
                }
                tokio::select! {
                    _ = rx.changed() => {}
                    _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {}
                }
            }
        });
    }

    async fn tick(&self) -> Result<()> {
        if self.cancel.lock().await.is_some() {
            return Ok(());
        }

        let Some(job) = self.db.claim_next_job().await? else {
            return Ok(());
        };

        info!(job_id = %job.id, url = %job.listing_url, "starting scrape job");
        let token = CancellationToken::new();
        *self.cancel.lock().await = Some((job.id, token.clone()));
        self.emit_job(
            &job,
            if job.use_ai {
                "AI scrape…"
            } else {
                "Scraping started"
            },
        )
        .await;

        let result = self.run_job(&job, token.clone()).await;

        *self.cancel.lock().await = None;

        match result {
            Ok((stats, stopped)) => {
                let status = if stopped || token.is_cancelled() {
                    JobStatus::Stopped
                } else {
                    JobStatus::Completed
                };
                let finished = self
                    .db
                    .finish_job(
                        job.id,
                        status,
                        stats.scraped_count,
                        stats.pages_visited,
                        None,
                    )
                    .await?;
                self.emit_job(&finished, "Job finished").await;
            }
            Err(err) => {
                let finished = self
                    .db
                    .finish_job(
                        job.id,
                        JobStatus::Failed,
                        job.scraped_count,
                        job.pages_visited,
                        Some(&err.to_string()),
                    )
                    .await?;
                self.emit_job(&finished, &format!("Job failed: {err}"))
                    .await;
            }
        }

        self.notify();
        Ok(())
    }

    async fn run_job(
        &self,
        job: &ScrapeJob,
        token: CancellationToken,
    ) -> Result<(crate::models::ScrapeStats, bool)> {
        let db = self.db.clone();
        let job_id = job.id;
        let events = self.events.clone();

        let pattern = if job.use_ai {
            None
        } else if let Some(pid) = job.pattern_id {
            self.db.get_pattern(pid).await?
        } else {
            self.db.find_pattern_for_url(&job.listing_url).await?
        };

        let pattern_cfg = pattern.as_ref().and_then(|p| {
            serde_json::from_value::<crate::models::PatternConfig>(p.config.clone()).ok()
        });

        if job.use_ai {
            info!("AI scrape enabled; skipping patterns");
        } else if let Some(p) = &pattern {
            info!(pattern = %p.name, "using scrape pattern");
        } else {
            info!("no DB pattern; will try built-in e-rekrutim inference");
        }

        let browser = self.browser.as_ref().map(|b| b.browser());
        let ai_progress = job.use_ai;
        let ai_provider = crate::models::AiProvider::parse(job.ai_provider.as_deref());

        let stats = scraper::scrape_listing(
            browser,
            &job.listing_url,
            job.from_date,
            job.to_date,
            pattern_cfg.as_ref(),
            job.use_ai,
            ai_provider,
            token.clone(),
            |items, scraped_count, pages_visited| {
                let db = db.clone();
                let events = events.clone();
                async move {
                    db.save_items(&items, job_id).await?;
                    db.update_job_progress(job_id, scraped_count, pages_visited)
                        .await?;
                    let current = db.get_job(job_id).await?;
                    let queue_depth = db.queue_depth().await.unwrap_or(0);
                    let _ = events.send(ProgressEvent {
                        current_job: current,
                        queue_depth,
                        scraped_count,
                        status: "running".into(),
                        message: if ai_progress {
                            format!(
                                "AI scrape ({})… saved batch; total {scraped_count}",
                                ai_provider.as_str()
                            )
                        } else {
                            format!("Saved batch; total {scraped_count}")
                        },
                    });
                    Ok(())
                }
            },
        )
        .await?;

        Ok((stats, token.is_cancelled()))
    }

    async fn emit(&self, message: &str) {
        let current = self.db.current_running().await.ok().flatten();
        let queue_depth = self.db.queue_depth().await.unwrap_or(0);
        let scraped_count = current.as_ref().map(|j| j.scraped_count).unwrap_or(0);
        let status = current
            .as_ref()
            .map(|j| j.status.clone())
            .unwrap_or_else(|| "idle".into());
        let _ = self.events.send(ProgressEvent {
            current_job: current,
            queue_depth,
            scraped_count,
            status,
            message: message.to_string(),
        });
    }

    async fn emit_job(&self, job: &ScrapeJob, message: &str) {
        let queue_depth = self.db.queue_depth().await.unwrap_or(0);
        let _ = self.events.send(ProgressEvent {
            current_job: Some(job.clone()),
            queue_depth,
            scraped_count: job.scraped_count,
            status: job.status.clone(),
            message: message.to_string(),
        });
    }
}
