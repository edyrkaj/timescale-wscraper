# Duapune list-only scrape pattern

**Date:** 2026-10-05  
**Status:** Approved  
**Scope:** DOM list extraction + seeded pattern for `duapune.com`

## Goal

Scrape job cards from duapune.com advanced filter listing into TimescaleDB without opening detail pages.

## List URL

`https://duapune.com/search/advanced/filter?employer=&keyword=&country=&city=&category=&job_type=&is_remote=`

(Any URL containing `duapune.com` can match the seeded pattern.)

## Constraints

- Chromium DOM scrape (Cloudflare blocks plain HTTP)
- List cards only — no detail enrichment
- Description left null
- Respect existing `from_date` / `to_date` filter after parsing card dates

## Card mapping (`.job-listing`)

| Field | Selector / source |
|-------|-------------------|
| url / title | `h1.job-title > a` (job href, not employer) |
| company | `h1.job-title small a` |
| location | `span.location` |
| date | `span.time` → `DD-MM-YYYY` |

## Implementation

1. Extend `DomListConfig` with optional card field selectors.
2. Update `collect_from_dom` to extract card fields when `card_selector` is set; keep link-only path as fallback.
3. Parse `DD-MM-YYYY` dates (already partly supported via slash dates — ensure hyphen form works).
4. Seed pattern in `Db::ensure_schema`.
5. Pagination: optional `next_page_selector` if present; first page works without it.

## Non-goals

- Detail page / full description
- Bypassing Cloudflare without Chromium
- Salary extraction (not on card sample)
