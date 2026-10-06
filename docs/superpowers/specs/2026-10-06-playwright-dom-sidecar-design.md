# Playwright DOM sidecar

**Date:** 2026-10-06  
**Status:** Approved  
**Scope:** DOM listing scrapes via Node Playwright; API patterns stay in Rust.

## Goal

Replace `chromiumoxide` for `list.source=dom` with an official Playwright worker so listing pages (e.g. duapune.com) use real waits and headed Chromium under Xvfb.

## Architecture

- Compose service `playwright` (`mcr.microsoft.com/playwright`) runs `POST /scrape-list`.
- Rust `collect_from_dom` HTTP-posts URL + `DomListConfig` and maps returned cards into `ScrapedItem`.
- Date filter, encoding, Timescale writes stay in Rust.
- `list.source=api` unchanged.
- Heuristic / `detail.source=page` may still use `chromiumoxide` in the app image.

## Contract

Request: `{ url, wait_for, wait_ms, card_selector, title_selector, company_selector, location_selector, date_selector, item_link_selector, item_link_regex, next_page_selector, max_pages, max_items, timeout_ms }`

Response: `{ cards: [{ url, title, company, location, dateText }], pages_visited, error? }`

Cloudflare interstitial after timeout → HTTP 503 with diagnostics.

## Non-goals

- Dedicated Turnstile solver
- Replacing API scrapes
