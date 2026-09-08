#!/usr/bin/env node
// Study page check (WB-10, WB-12). Serves the built bundle with the studies API answered from
// the fixture result in web/fixtures/study-result.json (a 5-minute study of the bundled DEMO.US
// data with an accepted set, curves thinned to 60 points), opens the studies page in terminal
// and modern mode, selects the study, and fails unless every chart renders (IC by horizon,
// deciles, the costless curve, daily IC) with no console error or uncaught exception, the page
// sits at the top, and the results grid comes before the first chart. In terminal mode it then
// switches the form to the daily grid and fails unless the order-book checkboxes are gone, the
// OHLCV set is ticked, the symbols come from a catalog-backed picker (the fixture catalog
// offers DEMO.US), and the submitted study asks for the daily grid, that symbol, and only
// OHLCV features. It also expects a checkbox per registered series from /api/studies/series
// (the fixture declares two series and the four lake feeds), the lake feeds greyed out on the
// daily grid, and a lake study submitted with funding_rate ticked to carry that feature. Every
// other API path proxies to the console at LAYOUT_CONSOLE when one
// answers and returns 503 otherwise (the shell shows "API offline" and carries on). Needs a
// Playwright-compatible Chromium like the layout check; without one, or without a built
// bundle, it reports that it skipped and exits 0.
//
//   node web/scripts/chart-check.mjs
//   node web/scripts/chart-check.mjs --verbose        print every chart's measurement
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { LAUNCH, reachable, readBody, resolveChromium, serveDist } from "./headless.mjs";

const consoleOrigin = process.env.LAYOUT_CONSOLE ?? "http://127.0.0.1:8787/";
const distDir = fileURLToPath(new URL("../dist/", import.meta.url));
const fixturePath = fileURLToPath(new URL("../fixtures/study-result.json", import.meta.url));
const verbose = process.argv.includes("--verbose");
const CHARTS = {
  "IC by horizon": ".ic-decay svg",
  deciles: ".decile-chart svg",
  "costless curve": ".study-curve svg",
  "daily IC": ".daily-ic-strip svg",
};

const runtime = resolveChromium();
if (!runtime) {
  console.log("chart-check: skipped (no playwright or patchright Chromium found)");
  process.exit(0);
}
if (!existsSync(join(distDir, "index.html"))) {
  console.log("chart-check: skipped (no web/dist; run npm run build first)");
  process.exit(0);
}

const result = JSON.parse(readFileSync(fixturePath, "utf8"));
const study = {
  id: "study-fixture",
  name: "Fixture · DEMO.US 5m",
  status: "complete",
  start_date: result.start,
  end_date: result.end,
  config_json: JSON.stringify(result.config),
  created_at: "2026-01-01T00:00:00Z",
  finished_at: "2026-01-01T00:01:00Z",
  error: null,
  artifact_dir: "artifacts/studies/fixture",
};
const catalog = {
  symbol: "DEMO.US",
  code: "DEMO",
  suffix: "US",
  name: "Demo Corporation",
  exchange: "US",
  asset_class: "Common Stock",
  currency: "USD",
  status: "active",
  daily: true,
  five_minute: true,
  one_minute: false,
  tick: false,
  coverage: { daily: { first: "2018-01-02", last: "2025-12-31" }, "5m": { first: "2024-01-02", last: "2024-12-31" } },
  missing_resolutions: [],
};
const lakeInstrument = { exchange: "BINANCE_FUTURES", symbol: "SOLUSDT", first_date: "2026-07-01", last_date: "2026-07-16", days: 16, has_book: true };
const series = [
  { name: "vix", kind: "level", source: "series/vix.csv", availability: "nominal time + 3600 s", lake_only: false },
  { name: "fomc", kind: "event", source: "series/fomc.parquet", availability: "as-of the released_at column", lake_only: false },
  { name: "funding_rate", kind: "lake", source: "lake feed funding.fundingRate", availability: "as-of receipt (recvTimestampMicros)", lake_only: true },
  { name: "funding_annualized", kind: "lake", source: "lake feed funding.annualizedRate", availability: "as-of receipt (recvTimestampMicros)", lake_only: true },
  { name: "open_interest", kind: "lake", source: "lake feed open_interest.openInterest", availability: "as-of receipt (recvTimestampMicros)", lake_only: true },
  { name: "open_interest_usd", kind: "lake", source: "lake feed open_interest.openInterestUsd", availability: "as-of receipt (recvTimestampMicros)", lake_only: true },
];
const BOOK_FEATURES = ["obi_l1", "obi_l5", "obi_l10", "microprice_bps", "trade_imbalance", "spread_bps", "signed_volume"];
const OHLCV_FEATURES = ["return_1", "range_bps", "gap_bps", "high_252_distance"];
/** Studies the page submitted to the fixture server. */
const posted = [];
const json = (body, status = 200) => ({ status, type: "application/json", body: JSON.stringify(body) });
const fixtureApi = async (pathname, req) => {
  if (pathname === "/api/studies" && req.method === "POST") {
    posted.push(JSON.parse(await readBody(req)));
    return json({ ...study, id: "study-posted", name: "posted", status: "running" }, 202);
  }
  if (pathname === "/api/studies") return json([study]);
  if (pathname === `/api/studies/${study.id}`) return json({ study, result });
  if (pathname === "/api/features") return json([]);
  if (pathname === "/api/lake/instruments") return json([lakeInstrument]);
  if (pathname === "/api/studies/series") return json(series);
  if (pathname === "/api/instruments") return json({ instruments: [catalog], total_matches: 1, index_size: 1, indexed_at: "" });
  return null;
};

const upstream = (await reachable(consoleOrigin)) ? consoleOrigin : null;
const served = await serveDist(distDir, upstream, fixtureApi);
const browser = await runtime.chromium.launch(LAUNCH);
const failures = [];
try {
  for (const mode of ["terminal", "modern"]) {
    const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
    await context.addInitScript((m) => window.localStorage.setItem("bt-display-mode", m), mode);
    const page = await context.newPage();
    const errors = [];
    page.on("console", (message) => {
      // A 503 from the fixture server is expected; anything else at error level is not.
      if (message.type() === "error" && !/Failed to load resource/.test(message.text())) errors.push(message.text());
    });
    page.on("pageerror", (error) => errors.push(String(error)));
    try {
      await page.goto(served.url, { waitUntil: "domcontentloaded" });
      await page.locator(".app-shell").waitFor({ timeout: 15000 });
      await page.getByRole("button", { name: /Studies$/ }).first().click();
      await page.locator(".studies-workspace").waitFor({ timeout: 15000 });
      const opened = await page.evaluate(() => window.scrollY);
      if (opened !== 0) failures.push(`${mode}: the studies page opened scrolled (scrollY ${opened})`);
      // A DOM click, so the driver does not scroll the row into view on the page's behalf.
      await page.locator(".sweep-row").first().waitFor({ timeout: 15000 });
      await page.evaluate(() => document.querySelector(".sweep-row").click());
      await page.locator(".sweep-heatmap").first().waitFor({ timeout: 15000 });
      for (const selector of Object.values(CHARTS)) {
        await page.locator(selector).first().waitFor({ timeout: 15000 }).catch(() => undefined);
      }
      await page.waitForTimeout(300);
      const seen = await page.evaluate((charts) => {
        const top = (el) => el.getBoundingClientRect().top + window.scrollY;
        const grid = document.querySelector(".sweep-heatmap");
        const out = { scrollY: window.scrollY, gridTop: grid ? top(grid) : null, charts: {} };
        for (const [name, selector] of Object.entries(charts)) {
          const el = document.querySelector(selector);
          const rect = el?.getBoundingClientRect();
          out.charts[name] = el
            ? { top: top(el), height: rect.height, width: rect.width, shapes: el.querySelectorAll("polyline, rect, circle, path").length }
            : null;
        }
        return out;
      }, CHARTS);
      if (verbose) console.log(`${mode}: ${JSON.stringify(seen)}`);
      if (seen.scrollY !== 0) failures.push(`${mode}: page is not at the top (scrollY ${seen.scrollY})`);
      if (seen.gridTop == null) failures.push(`${mode}: no results grid`);
      for (const [name, chart] of Object.entries(seen.charts)) {
        if (!chart) {
          failures.push(`${mode}: ${name} chart did not render`);
          continue;
        }
        if (chart.height < 40 || chart.width < 200) failures.push(`${mode}: ${name} chart is ${Math.round(chart.width)}x${Math.round(chart.height)} px`);
        if (chart.shapes === 0) failures.push(`${mode}: ${name} chart has no shapes`);
        if (seen.gridTop != null && chart.top <= seen.gridTop) failures.push(`${mode}: ${name} chart sits above the results grid`);
      }
      if (mode === "terminal") {
        // The form on the daily grid: no order-book checkbox, the OHLCV set ticked, a
        // catalog-backed symbol picker, and a submission that carries exactly that.
        await page.getByLabel("Grid").first().selectOption("daily");
        await page.waitForTimeout(200);
        const form = await page.evaluate(() => {
          const rows = [...document.querySelectorAll(".study-pick-grid .check-row[data-feature]")].map((row) => ({
            id: row.dataset.feature,
            checked: Boolean(row.querySelector("input")?.checked),
            disabled: Boolean(row.querySelector("input")?.disabled),
            series: row.classList.contains("series-row"),
          }));
          return {
            rows,
            picker: Boolean(document.querySelector(".study-pick-grid .instrument-picker .instrument-search input")),
            textarea: Boolean(document.querySelector(".study-pick-grid textarea[placeholder*='SPY']")),
          };
        });
        for (const row of form.rows) {
          if (BOOK_FEATURES.includes(row.id)) failures.push(`${mode}: daily grid still offers the order-book feature ${row.id}`);
        }
        // A checkbox per registered series; lake feeds greyed out on a CSV grid.
        for (const s of series) {
          const row = form.rows.find((r) => r.series && r.id === s.name);
          if (!row) failures.push(`${mode}: no checkbox for the registered series ${s.name}`);
          else if (row.disabled !== s.lake_only) failures.push(`${mode}: series ${s.name} is ${row.disabled ? "" : "not "}greyed out on the daily grid`);
        }
        const ticked = form.rows.filter((row) => row.checked).map((row) => row.id).sort();
        if (ticked.join(",") !== [...OHLCV_FEATURES].sort().join(",")) failures.push(`${mode}: daily grid pre-ticks ${ticked.join(", ") || "nothing"} instead of the OHLCV set`);
        if (!form.picker) failures.push(`${mode}: daily grid has no catalog-backed symbol picker`);
        if (form.textarea) failures.push(`${mode}: daily grid still takes symbols as typed text`);
        if (form.picker) {
          await page.locator(".study-pick-grid .instrument-search input").fill("DEMO");
          await page.locator(".study-pick-grid .instrument-results li").first().waitFor({ timeout: 10000 });
          await page.keyboard.press("Enter");
          await page.locator(".study-pick-grid .instrument-chip strong", { hasText: "DEMO.US" }).waitFor({ timeout: 10000 });
          await page.getByRole("button", { name: /Run study/ }).click();
          await page.waitForFunction(() => document.querySelector(".sweep-submit button")?.textContent?.includes("Run"), null, { timeout: 10000 }).catch(() => undefined);
          await page.waitForTimeout(400);
          const body = posted[posted.length - 1];
          if (!body) failures.push(`${mode}: the daily study was not submitted`);
          else {
            if (body.resolution !== "daily") failures.push(`${mode}: submitted resolution ${JSON.stringify(body.resolution)}, not daily`);
            if (JSON.stringify(body.symbols) !== JSON.stringify(["DEMO.US"])) failures.push(`${mode}: submitted symbols ${JSON.stringify(body.symbols)}`);
            const sent = [...(body.features ?? [])].sort();
            if (sent.join(",") !== [...OHLCV_FEATURES].sort().join(",")) failures.push(`${mode}: submitted features ${sent.join(", ")}`);
            for (const f of sent) if (BOOK_FEATURES.includes(f)) failures.push(`${mode}: submitted the order-book feature ${f} on the daily grid`);
          }
        }
        // Back on the lake: the feeds are live; a study with funding_rate ticked carries it.
        await page.getByLabel("Grid").first().selectOption("1");
        await page.waitForTimeout(200);
        const feed = page.locator('.study-pick-grid .check-row[data-feature="funding_rate"] input');
        if (await feed.isDisabled().catch(() => true)) failures.push(`${mode}: funding_rate is greyed out on the lake grid`);
        else {
          await page.locator(".study-pick-grid .check-row", { hasText: "BINANCE_FUTURES:SOLUSDT" }).locator("input").check();
          await feed.check();
          const before = posted.length;
          await page.getByRole("button", { name: /Run study/ }).click();
          await page.waitForTimeout(500);
          const body = posted.length > before ? posted[posted.length - 1] : null;
          if (!body) failures.push(`${mode}: the lake study was not submitted`);
          else {
            if (body.resolution) failures.push(`${mode}: lake study submitted with resolution ${JSON.stringify(body.resolution)}`);
            if (JSON.stringify(body.symbols) !== JSON.stringify(["BINANCE_FUTURES:SOLUSDT"])) failures.push(`${mode}: lake study symbols ${JSON.stringify(body.symbols)}`);
            if (!(body.features ?? []).includes("funding_rate")) failures.push(`${mode}: lake study features ${JSON.stringify(body.features)} lack funding_rate`);
          }
        }
      }
      for (const error of errors) failures.push(`${mode}: console error: ${error.split("\n")[0]}`);
    } catch (error) {
      failures.push(`${mode}: ${String(error).split("\n")[0]}`);
    }
    await context.close();
  }
} finally {
  await browser.close();
  served.server.close();
}

if (failures.length) {
  console.error(`chart-check: ${failures.length} failure(s)`);
  for (const f of failures) console.error(`  ${f}`);
  process.exit(1);
}
console.log(
  `chart-check: ok (${Object.keys(CHARTS).length} charts from web/fixtures/study-result.json, terminal and modern; ${series.length} series checkboxes; daily grid submitted ${posted[0]?.features?.length ?? 0} OHLCV features, lake grid submitted ${JSON.stringify(posted[1]?.features ?? [])}; shell ${upstream ? "on the console" : "offline"}) via ${runtime.from}`,
);
