# Date Picker Month/Year Jump Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace native From/To date chrome with a custom calendar popup that has month and year dropdowns.

**Architecture:** Vanilla JS `initDatePicker(input)` attaches a popup under each date field. Header holds month/year `<select>`s and prev/next; body is a Mon–Sun day grid. Values stay `YYYY-MM-DD` for the existing scrape form API.

**Tech Stack:** Static HTML/CSS/JS already served by Axum (`static/`). No new dependencies.

## Global Constraints

- UI only: `static/index.html`, `static/app.js`, `static/style.css`
- No backend / schema / API changes
- Date-only `YYYY-MM-DD`; no time component
- Year select range: current year − 10 … current year + 1
- Week starts Monday
- Only one popup open at a time
- Match existing CSS variables (`--accent`, `--card`, `--line`, `--ink`, `--muted`)

---

### Task 1: Markup wrappers for From / To

**Files:**
- Modify: `static/index.html` (From/To labels)

**Interfaces:**
- Produces: `#from_date` and `#to_date` remain the value inputs; each wrapped in `.date-field` for positioning

- [x] **Step 1: Wrap date inputs**

Replace the From/To row with:

```html
<div class="row">
  <label>
    From date
    <span class="date-field">
      <input id="from_date" name="from_date" type="text" inputmode="numeric" placeholder="YYYY-MM-DD" required autocomplete="off" />
    </span>
  </label>
  <label>
    To date
    <span class="date-field">
      <input id="to_date" name="to_date" type="text" inputmode="numeric" placeholder="YYYY-MM-DD" required autocomplete="off" />
    </span>
  </label>
</div>
```

Use `type="text"` so the custom popup is the picker (native date UI disabled).

- [x] **Step 2: Manual check**

Open UI: fields still show defaults once JS runs; no native date spinner.

---

### Task 2: Date picker styles

**Files:**
- Modify: `static/style.css`

**Interfaces:**
- Consumes: existing CSS variables
- Produces: `.date-field`, `.date-picker`, `.date-picker-header`, `.date-picker-grid`, `.date-picker-day`, `.is-selected`, `.is-outside`, `.is-open`

- [x] **Step 1: Append picker CSS**

```css
.date-field {
  position: relative;
  display: block;
}

.date-picker {
  position: absolute;
  z-index: 40;
  top: calc(100% + 0.35rem);
  left: 0;
  width: min(288px, 100vw - 2rem);
  padding: 0.75rem;
  border: 1px solid var(--line);
  border-radius: 12px;
  background: #15261f;
  box-shadow: 0 12px 40px rgba(0, 0, 0, 0.45);
  display: none;
}

.date-picker.is-open { display: block; }

.date-picker-header {
  display: grid;
  grid-template-columns: auto 1fr 1fr auto;
  gap: 0.35rem;
  align-items: center;
  margin-bottom: 0.65rem;
}

.date-picker-header select {
  width: 100%;
  border: 1px solid var(--line);
  background: rgba(0, 0, 0, 0.35);
  color: var(--ink);
  border-radius: 8px;
  padding: 0.35rem 0.4rem;
  font: inherit;
  font-size: 0.85rem;
}

.date-picker-header button {
  width: 2rem;
  height: 2rem;
  padding: 0;
  border-radius: 8px;
  border: 1px solid var(--line);
  background: rgba(0, 0, 0, 0.25);
  color: var(--ink);
  cursor: pointer;
}

.date-picker-weekdays,
.date-picker-grid {
  display: grid;
  grid-template-columns: repeat(7, 1fr);
  gap: 0.2rem;
  text-align: center;
}

.date-picker-weekdays {
  margin-bottom: 0.25rem;
  color: var(--muted);
  font-size: 0.7rem;
}

.date-picker-day {
  border: none;
  background: transparent;
  color: var(--ink);
  border-radius: 8px;
  aspect-ratio: 1;
  font: inherit;
  font-size: 0.85rem;
  cursor: pointer;
  padding: 0;
}

.date-picker-day:hover:not(:disabled) {
  background: rgba(61, 214, 140, 0.15);
}

.date-picker-day.is-selected {
  background: var(--accent);
  color: #042016;
  font-weight: 600;
}

.date-picker-day.is-outside,
.date-picker-day:disabled {
  color: var(--muted);
  opacity: 0.45;
  cursor: default;
}
```

- [x] **Step 2: Visual check** after Task 3 wires markup — popup matches dark theme.

---

### Task 3: `initDatePicker` logic

**Files:**
- Modify: `static/app.js`

**Interfaces:**
- Consumes: `#from_date`, `#to_date` inputs inside `.date-field`
- Produces: `initDatePicker(inputEl)` — attaches popup; shared registry so only one popup is open

- [x] **Step 1: Add helpers + initDatePicker before default date assignment**

Insert after `daysAgoIso` / before setting defaults:

```javascript
const MONTHS = [
  "January", "February", "March", "April", "May", "June",
  "July", "August", "September", "October", "November", "December",
];
const WEEKDAYS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

let openPickerClose = null;

function parseIsoDate(s) {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(s || "")) return null;
  const [y, m, d] = s.split("-").map(Number);
  const dt = new Date(y, m - 1, d);
  if (dt.getFullYear() !== y || dt.getMonth() !== m - 1 || dt.getDate() !== d) return null;
  return dt;
}

function toIsoDate(dt) {
  const y = dt.getFullYear();
  const m = String(dt.getMonth() + 1).padStart(2, "0");
  const d = String(dt.getDate()).padStart(2, "0");
  return `${y}-${m}-${d}`;
}

function yearRange() {
  const y = new Date().getFullYear();
  return { min: y - 10, max: y + 1 };
}

function initDatePicker(input) {
  const wrap = input.closest(".date-field");
  if (!wrap || wrap.querySelector(".date-picker")) return;

  let lastValid = input.value;
  const popup = document.createElement("div");
  popup.className = "date-picker";
  popup.innerHTML = `
    <div class="date-picker-header">
      <button type="button" data-nav="-1" aria-label="Previous month">‹</button>
      <select data-month></select>
      <select data-year></select>
      <button type="button" data-nav="1" aria-label="Next month">›</button>
    </div>
    <div class="date-picker-weekdays">${WEEKDAYS.map((w) => `<span>${w}</span>`).join("")}</div>
    <div class="date-picker-grid"></div>`;
  wrap.appendChild(popup);

  const monthSel = popup.querySelector("[data-month]");
  const yearSel = popup.querySelector("[data-year]");
  const grid = popup.querySelector(".date-picker-grid");
  const { min, max } = yearRange();
  MONTHS.forEach((name, i) => {
    const opt = document.createElement("option");
    opt.value = String(i);
    opt.textContent = name;
    monthSel.appendChild(opt);
  });
  for (let y = min; y <= max; y++) {
    const opt = document.createElement("option");
    opt.value = String(y);
    opt.textContent = String(y);
    yearSel.appendChild(opt);
  }

  let view = parseIsoDate(input.value) || new Date();

  function close() {
    popup.classList.remove("is-open");
    if (openPickerClose === close) openPickerClose = null;
  }

  function open() {
    if (openPickerClose && openPickerClose !== close) openPickerClose();
    view = parseIsoDate(input.value) || new Date();
    render();
    popup.classList.add("is-open");
    openPickerClose = close;
  }

  function clampView() {
    let y = view.getFullYear();
    let m = view.getMonth();
    if (y < min) { y = min; m = 0; }
    if (y > max) { y = max; m = 11; }
    view = new Date(y, m, 1);
  }

  function render() {
    clampView();
    monthSel.value = String(view.getMonth());
    yearSel.value = String(view.getFullYear());
    const selected = parseIsoDate(input.value);
    const year = view.getFullYear();
    const month = view.getMonth();
    const first = new Date(year, month, 1);
    const startPad = (first.getDay() + 6) % 7; // Mon=0
    const daysInMonth = new Date(year, month + 1, 0).getDate();
    const cells = [];
    for (let i = 0; i < startPad; i++) {
      cells.push(`<button type="button" class="date-picker-day is-outside" disabled tabindex="-1"></button>`);
    }
    for (let d = 1; d <= daysInMonth; d++) {
      const iso = toIsoDate(new Date(year, month, d));
      const sel = selected && toIsoDate(selected) === iso ? " is-selected" : "";
      cells.push(`<button type="button" class="date-picker-day${sel}" data-day="${d}">${d}</button>`);
    }
    grid.innerHTML = cells.join("");
  }

  monthSel.addEventListener("change", () => {
    view = new Date(Number(yearSel.value), Number(monthSel.value), 1);
    render();
  });
  yearSel.addEventListener("change", () => {
    view = new Date(Number(yearSel.value), Number(monthSel.value), 1);
    render();
  });
  popup.querySelectorAll("[data-nav]").forEach((btn) => {
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      const delta = Number(btn.dataset.nav);
      view = new Date(view.getFullYear(), view.getMonth() + delta, 1);
      render();
    });
  });
  grid.addEventListener("click", (e) => {
    const btn = e.target.closest("[data-day]");
    if (!btn) return;
    const d = Number(btn.dataset.day);
    const next = new Date(view.getFullYear(), view.getMonth(), d);
    input.value = toIsoDate(next);
    lastValid = input.value;
    input.dispatchEvent(new Event("change", { bubbles: true }));
    close();
  });

  input.addEventListener("focus", open);
  input.addEventListener("click", open);
  input.addEventListener("blur", () => {
    const parsed = parseIsoDate(input.value.trim());
    if (parsed) {
      input.value = toIsoDate(parsed);
      lastValid = input.value;
    } else {
      input.value = lastValid;
    }
  });

  document.addEventListener("click", (e) => {
    if (!popup.classList.contains("is-open")) return;
    if (wrap.contains(e.target)) return;
    close();
  });
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && popup.classList.contains("is-open")) close();
  });
}
```

- [x] **Step 2: Wire pickers after setting defaults**

```javascript
document.getElementById("to_date").value = todayIso();
document.getElementById("from_date").value = daysAgoIso(30);
initDatePicker(document.getElementById("from_date"));
initDatePicker(document.getElementById("to_date"));
```

- [x] **Step 3: Manual test**

1. Click From → popup with month/year selects
2. Change year dropdown → grid updates
3. Change month / use ‹ › → grid updates
4. Pick a day → input updates, popup closes
5. Open To while From open → From closes
6. Escape / click outside → closes
7. Type invalid date, blur → restores last valid
8. Start scraping → API still receives `from_date` / `to_date`

---

## Spec coverage

| Spec requirement | Task |
|------------------|------|
| Month + year dropdowns | 3 |
| Day pick, `YYYY-MM-DD` | 3 |
| Year −10…+1 | 3 |
| Mon–Sun, muted outside days | 2, 3 |
| One popup at a time | 3 |
| Escape / outside close | 3 |
| Typed ISO blur normalize | 3 |
| No API changes | all |
| Theme tokens | 2 |
