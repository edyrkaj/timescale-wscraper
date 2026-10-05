# Custom date picker (month / year jump)

**Date:** 2026-10-05  
**Status:** Approved design  
**Scope:** UI only (`static/`)

## Problem

The scrape form uses native `<input type="date">` for From / To. Jumping across months or years is awkward and inconsistent across browsers. Users need a calendar where they can change **month** and **year** directly, then pick a day.

## Goals

- Month and year selectable via dropdowns in the calendar header
- Still pick a single calendar day
- Values remain date-only `YYYY-MM-DD` for existing `POST /api/jobs`
- No new dependencies; match existing dark UI tokens
- No backend / schema / API changes

## Non-goals

- Time-of-day selection (`datetime`)
- Range highlighting between From and To
- Preset buttons (This month / This year)
- Third-party date libraries
- Mobile-native picker replacement beyond what a small popup provides

## Approach

Lightweight custom picker in vanilla JS/CSS, attached to the existing From / To fields.

## UX

1. Each field shows the current value as `YYYY-MM-DD` (defaults unchanged: To = today, From = 30 days ago).
2. Clicking the field (or a calendar affordance) opens a popup anchored under the field.
3. Popup header:
   - Month `<select>` (January–December)
   - Year `<select>` (current year − 10 through current year + 1)
   - Previous / next month buttons (`‹` / `›`), which update the selects and grid
4. Body: weekday row (**Mon–Sun**) + day grid for the viewed month. Leading/trailing cells from adjacent months are shown muted and non-selectable; only in-month days are selectable.
5. Selected day uses accent highlight. Clicking a day writes `YYYY-MM-DD` into the input, closes the popup, and dispatches `change` on the input.
6. Click outside or Escape closes without changing the value (unless a day was already chosen in this open session).
7. Only one popup open at a time (opening From closes To and vice versa).
8. Typing: input remains editable for ISO dates. On blur, if the value matches `YYYY-MM-DD` and is a real calendar day, normalize it; otherwise restore the last valid value (or empty if never set).
9. Two independent pickers (From and To); no cross-field min/max constraints in the UI (form still requires both; API unchanged).

## Components

| Unit | Responsibility |
|------|----------------|
| Markup in `index.html` | Wrapper around each date input; popup container structure (or created in JS) |
| `initDatePicker(input)` in `app.js` | Open/close, render grid, wire selects and nav, set value |
| Styles in `style.css` | Popup, grid, selects, selected day using `--accent`, `--card`, `--line`, `--ink` |

## Data flow

```
User opens picker → viewMonth/viewYear from input value or today
User changes month/year select or ‹ › → re-render day grid
User clicks day → input.value = YYYY-MM-DD → close popup
Form submit → unchanged: reads from_date / to_date from inputs
```

## Error handling

- Empty input on open → view today
- Out-of-range year when navigating months → clamp year select to allowed range
- Malformed typed value on blur → restore last good value (or empty if never set)

## Testing (manual)

- Open From and To; jump year via dropdown; jump month via dropdown and ‹ ›
- Select a day; confirm value and that scrape enqueue still works
- Click outside / Escape dismisses
- Keyboard type a valid `YYYY-MM-DD` and blur
- Both pickers independent; only one open at a time

## Files touched

- `static/index.html` — structure hooks for From / To
- `static/app.js` — picker logic
- `static/style.css` — popup styles
