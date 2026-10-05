# Duapune List-Only Scrape Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans or subagent-driven-development.

**Goal:** Scrape duapune.com job listing cards (title, company, location, date, url) via Chromium DOM pattern.

**Architecture:** Extend `DomListConfig` with card selectors; extract per `.job-listing` in `collect_from_dom`; seed pattern in DB.

**Tech Stack:** Existing Rust scraper + Chromiumoxide + seeded `scrape_patterns` row.

## Global Constraints

- List only; `detail.source = none`
- Cloudflare → Chromium required
- Date format on cards: `DD-MM-YYYY`

---

### Task 1: Extend DomListConfig + date parse

**Files:**
- Modify: `src/models.rs` (`DomListConfig`)
- Modify: `src/scraper.rs` (hyphen date parse + collect_from_dom)

- [x] Add card selector fields to `DomListConfig`
- [x] Parse `DD-MM-YYYY` in date helpers
- [x] Extract cards when `card_selector` set
- [x] Seed duapune pattern in `src/db.rs`
- [ ] Manual: enqueue filter URL with pattern, confirm items

---
