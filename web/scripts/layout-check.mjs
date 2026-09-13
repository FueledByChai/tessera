#!/usr/bin/env node
// Laptop layout check (UI-03). Drives the console headlessly at 1280 and 1440 px wide, in
// terminal and modern mode, through the run overview, the strategies catalog, a strategy page,
// the studies page, and the Data page's three views (DS-01: Inventory, Instrument search with
// a query the console's catalog answers and its first row selected, Updates & schedules), and
// fails when the page body scrolls horizontally or an element's right
// edge passes the viewport without a scrolling ancestor (a wide table must scroll inside its
// own wrapper). The catalog must also show its nine columns (UI-06), so a narrower table that
// happens to fit cannot pass for it. On the strategy page it also fails when the Historical
// runs table is not the first panel after the summary strip (UI-05), then opens the Configure
// run dialog and fails when the dialog passes the viewport, when its rows scroll inside it
// (UI-07: the whole form shows at 1280x800), or when the page behind it scrolls horizontally,
// and counts the fields in the first row of the dialog's Data grid and fails under six (UI-04:
// numeric fields take one auto-fit track, dates and selects two). A fifth page holds the
// dialog to a strategy declaring eight parameters (UI-07): the fixture strategy from
// web/fixtures/strategy-detail.json with four more parameters, long captions and hints among
// them, added to the catalog by this script and measured with the Simple tier showing and
// again with Advanced showing all eight; it needs the bundle served here, so a LAYOUT_URL run
// lists it as skipped.
//
//   node web/scripts/layout-check.mjs                  this checkout's web/dist, API from the
//                                                      console at LAYOUT_CONSOLE (127.0.0.1:8787)
//   LAYOUT_URL=http://127.0.0.1:5173/ node ...         a served app as-is (the vite dev server)
//   node web/scripts/layout-check.mjs --verbose        print every page's measurement
//
// The bundle under test is the one just built (`npm run build`), served by this script with
// /api, /artifacts, and /reports proxied to the running console (the catalog gaining the
// eight-parameter fixture), so the check sees the current stylesheet with real runs,
// strategies, and studies. Needs a Playwright-compatible Chromium:
// `playwright` or `patchright` resolvable from web/, or the CodeGPT VS Code extension's bundled
// copy. Without one, without a built bundle, or without a reachable console, the check reports
// that it skipped and exits 0; the theme check's static rules still apply there. A page the
// console cannot supply (no runs for the run overview, no strategies for the strategy page) is
// listed as skipped and the remaining pages are measured (HK-08); the studies page always is.
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { LAUNCH, reachable, resolveChromium, serveDist } from "./headless.mjs";

const consoleOrigin = process.env.LAYOUT_CONSOLE ?? "http://127.0.0.1:8787/";
const servedUrl = process.env.LAYOUT_URL;
const distDir = fileURLToPath(new URL("../dist/", import.meta.url));
const fixturePath = fileURLToPath(new URL("../fixtures/strategy-detail.json", import.meta.url));
const verbose = process.argv.includes("--verbose");
const WIDTHS = [1280, 1440];
const HEIGHT = 800;

/**
 * The eight-parameter strategy (UI-07): the fixture strategy under its own id and name, its
 * four manifest parameters all on the Simple tier plus four more, two of them Advanced, with
 * a caption that wraps to three lines in a numeric track and hints that would wrap to four or
 * five. Simple shows six numeric fields, Advanced all eight. Its default symbols are explicit,
 * so the Data row carries the symbol picker as well: the tallest form a strategy can put in
 * the dialog short of a ninth parameter.
 */
const EIGHT = (() => {
  const detail = JSON.parse(readFileSync(fixturePath, "utf8"));
  const id = "layout-eight-parameters";
  const name = "Layout fixture · eight parameters";
  const extra = [
    { name: "hold_minutes", label: "Hold minutes", help: "Exit after this long in the trade whatever the price", tier: "simple", unit: "min", kind: "int", default: 30, min: 1, max: 390 },
    { name: "gap_threshold", label: "Gap threshold", help: "Smallest open-to-prior-close gap that qualifies", tier: "simple", unit: "%", kind: "decimal", default: 1.5, min: 0.1, max: 20 },
    { name: "volume_floor", label: "Dollar volume floor", help: "Skip symbols trading under this much a day", tier: "advanced", unit: "$", kind: "decimal", default: 1000000, min: 0, max: null },
    { name: "exit_offset", label: "Exit offset", help: "Ticks above the signal price for the exit order", tier: "advanced", unit: "ticks", kind: "int", default: 2, min: 0, max: 50 },
  ];
  return {
    id,
    name,
    detail: {
      ...detail,
      strategy: { ...detail.strategy, id, name, sdk_strategy_id: id },
      presets: [],
      runs: [],
      sdk: {
        ...detail.sdk,
        id,
        name,
        params: [...detail.sdk.params.map((param) => ({ ...param, tier: "simple" })), ...extra.map((param) => ({ step: null, ...param }))],
      },
    },
  };
})();

/** Answers the eight-parameter strategy's API from the fixture and adds it to the catalog. */
async function fixtureApi(pathname) {
  const json = (body) => ({ status: 200, type: "application/json", body: JSON.stringify(body) });
  if (pathname === `/api/strategies/${EIGHT.id}`) return json(EIGHT.detail);
  if (pathname === "/api/dashboard") {
    try {
      const response = await fetch(new URL(pathname, consoleOrigin), { signal: AbortSignal.timeout(5000) });
      if (!response.ok) return null;
      const dashboard = await response.json();
      return json({ ...dashboard, strategies: [...(dashboard.strategies ?? []), EIGHT.detail.strategy] });
    } catch {
      return null;
    }
  }
  return null;
}

/** Runs in the page: body overflow and elements past the viewport with no scrolling ancestor. */
function measure() {
  const vw = window.innerWidth;
  const scrollsOrClips = (el) => {
    for (let a = el.parentElement; a && a !== document.body; a = a.parentElement) {
      const o = getComputedStyle(a).overflowX;
      if (o === "auto" || o === "scroll" || o === "hidden" || o === "clip") return true;
    }
    return false;
  };
  const offenders = [];
  for (const el of document.querySelectorAll("body *")) {
    const r = el.getBoundingClientRect();
    if (r.width === 0 || r.height === 0) continue;
    if (r.right > vw + 1 && !scrollsOrClips(el)) {
      const cls = typeof el.className === "string" && el.className ? "." + el.className.trim().split(/\s+/)[0] : "";
      offenders.push(`${el.tagName.toLowerCase()}${cls} right=${Math.round(r.right)}`);
    }
  }
  // A wrapper whose content is wider than itself scrolls, but macOS hides the scrollbar, so the
  // table simply looks cut off at the panel edge. Tables must fit their wrapper at these widths.
  const clipped = [];
  for (const el of document.querySelectorAll("body *")) {
    const o = getComputedStyle(el).overflowX;
    if ((o !== "auto" && o !== "scroll") || el.clientWidth === 0) continue;
    if (el.scrollWidth > el.clientWidth + 1) {
      const cls = typeof el.className === "string" && el.className ? "." + el.className.trim().split(/\s+/).join(".") : "";
      const table = el.querySelector("table");
      const columns = table ? table.querySelectorAll("thead th").length : 0;
      clipped.push(`${el.tagName.toLowerCase()}${cls} content ${el.scrollWidth} > ${el.clientWidth}${columns ? ` (${columns} columns)` : ""}`);
    }
  }
  return {
    scrollWidth: document.documentElement.scrollWidth,
    innerWidth: vw,
    offenders: [...new Set(offenders)].slice(0, 10),
    clipped: [...new Set(clipped)].slice(0, 10),
  };
}

/** The strategy page's form grids are tracks sized so a row holds at least this many fields (UI-04). */
const FIELDS_PER_ROW = 6;

/**
 * Runs in the page: the fields in the first row of the Data grid (the Configure run dialog's
 * second row), the grid's direct children grouped by their top edge. Null when the page has no
 * such grid (a strategy without the SDK form), so the caller can say so instead of passing
 * vacuously. Measured with the dialog open, since a closed dialog's children have no size.
 */
function countFirstRow() {
  const section = [...document.querySelectorAll("dialog.run-dialog[open] .form-section")].find((s) =>
    /^Data\b/.test(s.querySelector("h3")?.textContent?.trim() ?? ""),
  );
  const grid = section?.querySelector(".field-grid");
  if (!grid) return null;
  const tops = [...grid.children]
    .map((el) => el.getBoundingClientRect())
    .filter((r) => r.width > 0 && r.height > 0)
    .map((r) => Math.round(r.top));
  if (!tops.length) return { first: 0, total: 0, width: Math.round(grid.getBoundingClientRect().width) };
  const top = Math.min(...tops);
  return {
    first: tops.filter((t) => Math.abs(t - top) <= 1).length,
    total: tops.length,
    width: Math.round(grid.getBoundingClientRect().width),
  };
}

/**
 * Runs in the page: the panel that follows the summary strip in document order (UI-05: the
 * Historical runs table, with nothing between). Null when the page has no strip.
 */
function panelAfterStrip() {
  const strip = document.querySelector(".run-strip");
  if (!strip) return null;
  const panels = [...document.querySelectorAll(".panel")];
  const next = panels[panels.indexOf(strip) + 1] ?? null;
  return {
    history: Boolean(next?.classList.contains("strategy-run-history")),
    next: next ? `${next.tagName.toLowerCase()}.${next.className.trim().split(/\s+/).join(".")}` : "nothing",
  };
}

/** Runs in the page: where the open Configure run dialog sits against the viewport. */
function measureDialog() {
  const dialog = document.querySelector("dialog.run-dialog[open]");
  if (!dialog) return null;
  const r = dialog.getBoundingClientRect();
  const body = dialog.querySelector(".run-dialog-body") ?? dialog;
  return {
    left: Math.round(r.left),
    top: Math.round(r.top),
    right: Math.round(r.right),
    bottom: Math.round(r.bottom),
    innerWidth: window.innerWidth,
    innerHeight: window.innerHeight,
    scrollWidth: document.documentElement.scrollWidth,
    scrollsInside: body.scrollHeight > body.clientHeight + 1,
    contentHeight: Math.round(r.height - body.clientHeight + body.scrollHeight),
    params: [...dialog.querySelectorAll(".form-section")].filter((s) => /^Parameters\b/.test(s.querySelector("h3")?.textContent?.trim() ?? ""))[0]?.querySelectorAll(".field-grid > *").length ?? 0,
  };
}

/** What the console has to offer, so pages it cannot supply are skipped rather than failed. */
async function supply(origin) {
  const count = async (path, pick) => {
    try {
      const response = await fetch(new URL(path, origin), { signal: AbortSignal.timeout(5000) });
      if (!response.ok) return 0;
      return pick(await response.json()).length;
    } catch {
      return 0;
    }
  };
  let instrument = "";
  try {
    const response = await fetch(new URL("/api/instruments?limit=1", origin), { signal: AbortSignal.timeout(5000) });
    if (response.ok) instrument = (await response.json())?.instruments?.[0]?.code ?? "";
  } catch {
    // no catalog: the instrument search page is measured with an empty result
  }
  return {
    runs: await count("/api/runs", (body) => (Array.isArray(body) ? body : [])),
    strategies: await count("/api/dashboard", (body) => body?.strategies ?? []),
    /** A code the console's catalog answers, so the instrument table is measured with rows. */
    instrument,
  };
}

/** The reason a page cannot be opened on this console, or null when it can. */
const NEEDS = {
  "run overview": (s) => (s.runs ? null : "the console has no runs"),
  "strategies catalog": (s) => (s.strategies ? null : "the console has no strategies"),
  "strategy page": (s) => (s.strategies ? null : "the console has no strategies"),
  "eight-parameter strategy": () => (servedUrl ? "the fixture strategy needs the bundle served by this script, not LAYOUT_URL" : null),
  studies: () => null,
  "data inventory": () => null,
  "data instrument search": () => null,
  "data updates": () => null,
};

/** The Data page on one of its three views (DS-01); the tab is clicked in the DOM so the
 *  driver does not scroll the page on the view's behalf. */
async function openDataView(page, view, ready) {
  await page.getByRole("button", { name: /Data$/ }).first().click();
  await page.locator(".data-workspace").waitFor({ timeout: 15000 });
  await page.evaluate((v) => document.querySelector(`nav.data-tabs button[data-view="${v}"]`).click(), view);
  await page.locator(ready).first().waitFor({ timeout: 15000 });
}

/** The columns the strategies catalog shows (UI-06): #, name, asset, runs, CAGR, Sharpe,
 *  max DD, last run, open. */
const CATALOG_COLUMNS = 9;

/** Opens the catalog and the strategy page behind the given catalog name. */
async function openStrategy(page, name) {
  await page.getByRole("button", { name: /Strategies$/ }).first().click();
  await name.waitFor({ timeout: 15000 });
  await name.click();
  await page.getByRole("button", { name: /Open strategy/ }).first().click();
  await page.locator(".run-strip").first().waitFor({ timeout: 15000 });
}

/**
 * The Configure run dialog on an open strategy page: opened from the strip, inside the
 * viewport, its rows not scrolling inside it, the page behind it not scrolling horizontally,
 * its Data row holding six fields. With `tiers`, the Advanced tier is switched on after the
 * Simple measurement and the dialog measured again (the eight-parameter strategy). Returns
 * the failures; a dialog that cannot open is one failure.
 */
async function checkDialog(page, label, tiers) {
  const failures = [];
  try {
    await page.locator(".run-strip-configure").first().click();
    await page.locator("dialog.run-dialog[open]").waitFor({ timeout: 10000 });
  } catch (error) {
    return [`${label}: could not open the Configure run dialog (${String(error).split("\n")[0]})`];
  }
  for (const tier of tiers ? ["Simple", "Advanced"] : [""]) {
    if (tier === "Advanced") await page.locator("dialog.run-dialog[open] .mode-switch button", { hasText: tier }).first().click();
    await page.waitForTimeout(400);
    const at = tier ? `${label} (${tier})` : label;
    const dialog = await page.evaluate(measureDialog);
    const behind = await page.evaluate(measure);
    if (verbose || dialog.scrollsInside) {
      console.log(`${at}: dialog ${dialog.left},${dialog.top} to ${dialog.right},${dialog.bottom} in ${dialog.innerWidth}x${dialog.innerHeight}${dialog.scrollsInside ? ` (its rows scroll inside: ${dialog.contentHeight}px of content)` : ""}`);
    }
    if (dialog.right > dialog.innerWidth + 1 || dialog.bottom > dialog.innerHeight + 1 || dialog.left < -1 || dialog.top < -1) {
      failures.push(`${at}: the dialog passes the viewport (${dialog.left},${dialog.top} to ${dialog.right},${dialog.bottom} in ${dialog.innerWidth}x${dialog.innerHeight})`);
    }
    if (dialog.scrollsInside) {
      failures.push(`${at}: the dialog's rows scroll inside it (${dialog.contentHeight}px of content in ${dialog.bottom - dialog.top}px, ${dialog.params} parameter fields)`);
    }
    if (behind.scrollWidth > behind.innerWidth) failures.push(`${at}: the page scrolls horizontally with the dialog open (${behind.scrollWidth} > ${behind.innerWidth})`);
    for (const o of behind.offenders) failures.push(`${at}: with the dialog open, past the viewport: ${o}`);
    const row = await page.evaluate(countFirstRow);
    if (!row) failures.push(`${at}: no Data grid in the dialog to count (the strategy has no SDK form)`);
    else {
      if (verbose) console.log(`${at}: Data grid ${row.width}px, ${row.first} of ${row.total} fields in the first row`);
      if (row.first < FIELDS_PER_ROW) {
        failures.push(`${at}: the dialog's Data grid holds ${row.first} field(s) in its first row, under ${FIELDS_PER_ROW} (grid ${row.width}px, ${row.total} fields)`);
      }
    }
  }
  return failures;
}

/** The pages, reached by clicking, since the console has no routes. Each opens the page and
 *  returns any failures of its own beyond the layout measurement. */
const PAGES = {
  "run overview": async (page) => {
    await page.getByRole("button", { name: /Runs$/ }).first().click();
    const row = page.locator("table tbody tr").first();
    await row.waitFor({ timeout: 15000 });
    await row.locator("td").nth(1).click();
    await page.locator(".equity-chart svg, .run-failure").first().waitFor({ timeout: 30000 });
    await page.getByRole("button", { name: /^Overview/ }).first().click().catch(() => {});
  },
  "strategies catalog": async (page) => {
    await page.getByRole("button", { name: /Strategies$/ }).first().click();
    await page.locator(".catalog-name").first().waitFor({ timeout: 15000 });
    const columns = await page.locator(".catalog-table thead th").count();
    if (columns !== CATALOG_COLUMNS) {
      return [`the catalog shows ${columns} columns, not ${CATALOG_COLUMNS}`];
    }
    return [];
  },
  "strategy page": async (page) => {
    // The console's first strategy: the fixture this script adds sits wherever the catalog's
    // name order puts it, so it is passed over here and opened by its own page below.
    await openStrategy(page, page.locator(".catalog-name").filter({ hasNotText: EIGHT.name }).first());
  },
  "eight-parameter strategy": async (page) => {
    await openStrategy(page, page.locator(".catalog-name").filter({ hasText: EIGHT.name }).first());
  },
  studies: async (page) => {
    await page.getByRole("button", { name: /Studies$/ }).first().click();
    await page.locator(".studies-workspace").waitFor({ timeout: 15000 });
    const study = page.locator(".sweep-row").first();
    if (await study.count()) {
      await study.click();
      await page.locator(".sweep-heatmap, .empty-state").first().waitFor({ timeout: 15000 });
    }
  },
  "data inventory": async (page) => {
    await openDataView(page, "inventory", ".data-sources-panel tbody tr, .data-sources-panel .empty-state");
  },
  "data instrument search": async (page) => {
    await openDataView(page, "instruments", ".instrument-search-view");
    if (supplied.instrument) {
      await page.locator(".instrument-search-view .instrument-search input").fill(supplied.instrument);
      await page.locator(".instrument-table tbody tr, .instrument-search-view .empty-state, .instrument-empty").first().waitFor({ timeout: 15000 });
      await page.evaluate(() => document.querySelector(".instrument-table tbody tr")?.click());
    }
  },
  "data updates": async (page) => {
    await openDataView(page, "updates", ".data-workspace .dataset-schedules-panel");
  },
};

const runtime = await resolveChromium();
if (!runtime) {
  console.log("layout-check: skipped (no playwright or patchright Chromium found)");
  process.exit(0);
}
if (!(await reachable(servedUrl ?? consoleOrigin))) {
  console.log(`layout-check: skipped (no console at ${servedUrl ?? consoleOrigin})`);
  process.exit(0);
}
let served = null;
if (!servedUrl && !existsSync(join(distDir, "index.html"))) {
  console.log("layout-check: skipped (no web/dist; run npm run build first)");
  process.exit(0);
}
if (!servedUrl) served = await serveDist(distDir, consoleOrigin, fixtureApi);
const url = servedUrl ?? served.url;
const supplied = await supply(servedUrl ?? consoleOrigin);
const skipped = new Map();
for (const [name, needs] of Object.entries(NEEDS)) {
  const reason = needs(supplied);
  if (reason) skipped.set(name, reason);
}
for (const [name, reason] of skipped) console.log(`layout-check: ${name} skipped (${reason})`);

const browser = await runtime.chromium.launch(LAUNCH);
const failures = [];
let measured = 0;
try {
  for (const mode of ["terminal", "modern"]) {
    for (const width of WIDTHS) {
      const context = await browser.newContext({ viewport: { width, height: HEIGHT } });
      await context.addInitScript((m) => window.localStorage.setItem("bt-display-mode", m), mode);
      const page = await context.newPage();
      for (const [name, open] of Object.entries(PAGES)) {
        if (skipped.has(name)) continue;
        await page.goto(url, { waitUntil: "domcontentloaded" });
        await page.locator(".app-shell").waitFor({ timeout: 15000 });
        let own = [];
        try {
          own = (await open(page)) ?? [];
        } catch (error) {
          failures.push(`${mode} ${width}px ${name}: could not open (${String(error).split("\n")[0]})`);
          continue;
        }
        await page.waitForTimeout(400);
        const result = await page.evaluate(measure);
        measured += 1;
        const label = `${mode} ${width}px ${name}`;
        for (const failure of own) failures.push(`${label}: ${failure}`);
        const wide = result.scrollWidth > result.innerWidth;
        const notes = [...result.offenders.map((o) => `past the viewport: ${o}`), ...result.clipped.map((c) => `cut off in its wrapper: ${c}`)];
        if (verbose || wide || notes.length) {
          console.log(`${label}: scrollWidth ${result.scrollWidth} / ${result.innerWidth}${notes.length ? "\n  " + notes.join("\n  ") : ""}`);
        }
        if (wide) failures.push(`${label}: page scrolls horizontally (${result.scrollWidth} > ${result.innerWidth})`);
        for (const note of notes) failures.push(`${label}: ${note}`);
        if (name === "strategy page") {
          // The order: the history table is the first panel after the summary strip.
          const order = await page.evaluate(panelAfterStrip);
          if (!order) failures.push(`${label}: no summary strip on the page`);
          else if (!order.history) failures.push(`${label}: the panel after the summary strip is ${order.next}, not the Historical runs table`);
          failures.push(...(await checkDialog(page, label, false)));
        }
        if (name === "eight-parameter strategy") failures.push(...(await checkDialog(page, label, true)));
      }
      await context.close();
    }
  }
} finally {
  await browser.close();
  served?.server.close();
}

if (failures.length) {
  console.error(`layout-check: ${failures.length} failure(s)`);
  for (const f of failures) console.error(`  ${f}`);
  process.exit(1);
}
const pages = Object.keys(PAGES).length - skipped.size;
console.log(
  `layout-check: ok (${WIDTHS.join("/")} px, terminal and modern, ${pages} of ${Object.keys(PAGES).length} pages measured${skipped.size ? `, skipped: ${[...skipped.keys()].join(", ")}` : ""}; ${measured} measurements${skipped.has("strategy page") ? "" : `, history first after the strip, the dialog inside the viewport with ${FIELDS_PER_ROW}+ fields in its Data row`}${skipped.has("eight-parameter strategy") ? "" : ", the eight-parameter dialog not scrolling inside"}, ${served ? "built bundle with the console's data" : servedUrl}) via ${runtime.from}`,
);
