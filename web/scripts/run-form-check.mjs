#!/usr/bin/env node
// Run form check (UI-05, decision 0003). Serves the built bundle with the strategy API
// answered from web/fixtures/strategy-detail.json (the bundled RSI Mean Reversion strategy
// with one seeded preset and two synthetic runs), opens its strategy page in terminal and
// modern mode, and proves the summary strip is a faithful rendering of the request the Run
// button sends:
//   1. the Configure run dialog opens as a native modal dialog with the four labelled rows
//      (Identity, Data, Limits, Parameters) and the presets inside it;
//   2. applying the seeded preset and pressing Escape closes the dialog and the strip shows the
//      preset's name and every one of its values (the edits survive the close);
//   3. the strip's Run posts a job whose body carries every value the strip shows;
//   4. back on the page, the preset applied again and Cancel pressed, the strip still shows it;
//      Run inside the dialog closes the dialog and posts a body equal to the strip's, and equal
//      to the body the strip's Run sent.
// Every other API path proxies to the console at LAYOUT_CONSOLE when one answers and returns
// 503 otherwise. Needs a Playwright-compatible Chromium like the layout check; without one, or
// without a built bundle, it reports that it skipped and exits 0.
//
//   node web/scripts/run-form-check.mjs
//   node web/scripts/run-form-check.mjs --verbose      print the strip and both bodies
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { LAUNCH, reachable, readBody, resolveChromium, serveDist } from "./headless.mjs";

const consoleOrigin = process.env.LAYOUT_CONSOLE ?? "http://127.0.0.1:8787/";
const distDir = fileURLToPath(new URL("../dist/", import.meta.url));
const fixturePath = fileURLToPath(new URL("../fixtures/strategy-detail.json", import.meta.url));
const verbose = process.argv.includes("--verbose");
const ROWS = ["Identity", "Data", "Limits", "Parameters"];

const runtime = await resolveChromium();
if (!runtime) {
  console.log("run-form-check: skipped (no playwright or patchright Chromium found)");
  process.exit(0);
}
if (!existsSync(join(distDir, "index.html"))) {
  console.log("run-form-check: skipped (no web/dist; run npm run build first)");
  process.exit(0);
}

const detail = JSON.parse(readFileSync(fixturePath, "utf8"));
const preset = detail.presets[0];
const costProfiles = [
  {
    id: "us-equities-default",
    name: "US equities · 1 tick + $0.005/share",
    asset_class: "US equities",
    model: "fixed_tick_per_unit",
    entry_bps: 0,
    exit_bps: 0,
    tick_size: 0.01,
    entry_slippage_ticks: 1,
    exit_slippage_ticks: 1,
    entry_commission_per_unit: 0.005,
    exit_commission_per_unit: 0.005,
    minimum_commission: 0,
    created_at: "2026-01-01T00:00:00+00:00",
    builtin: true,
  },
  {
    id: "all-in-10bps",
    name: "All-in · 10 bps round trip",
    asset_class: "Any",
    model: "all_in_bps",
    entry_bps: 5,
    exit_bps: 5,
    tick_size: 0,
    entry_slippage_ticks: 0,
    exit_slippage_ticks: 0,
    entry_commission_per_unit: 0,
    exit_commission_per_unit: 0,
    minimum_commission: 0,
    created_at: "2026-01-01T00:00:00+00:00",
    builtin: true,
  },
];
const catalog = {
  symbol: "SPY.US",
  code: "SPY",
  suffix: "US",
  name: "SPDR S&P 500 ETF Trust",
  exchange: "NYSE ARCA",
  asset_class: "ETF",
  currency: "USD",
  status: "active",
  daily: true,
  five_minute: true,
  one_minute: true,
  tick: false,
  coverage: { daily: { first: "1995-01-03", last: "2026-09-10" }, "5m": { first: "2020-10-12", last: "2026-09-09" } },
  missing_resolutions: [],
};
/** Jobs the page posted, in order. */
const posted = [];
const json = (body, status = 200) => ({ status, type: "application/json", body: JSON.stringify(body) });
const fixtureApi = async (pathname, req) => {
  if (pathname === "/api/jobs" && req.method === "POST") {
    posted.push(JSON.parse(await readBody(req)));
    return json({ id: `job-fixture-${posted.length}`, status: "queued" }, 202);
  }
  if (pathname === "/api/jobs") return json([]);
  if (pathname === "/api/dashboard") {
    return json({
      strategies: [detail.strategy],
      recent_runs: [],
      jobs: [],
      production_strategies: 0,
      archived_strategies: 0,
      historical_reports: detail.runs.length,
      active_jobs: 0,
      worker_capacity: 2,
    });
  }
  if (pathname === `/api/strategies/${detail.strategy.id}`) return json(detail);
  if (pathname === "/api/cost-profiles") return json(costProfiles);
  if (pathname === "/api/runs") return json(detail.runs);
  if (pathname === "/api/automations") return json([]);
  if (pathname === "/api/instruments") return json({ instruments: [catalog], total_matches: 1, index_size: 1, indexed_at: "" });
  if (pathname === "/api/data/status") return json({});
  return null;
};

/** JSON with keys in a stable order, so two bodies compare by content. */
const stable = (value) =>
  JSON.stringify(value, (_, v) => (v && typeof v === "object" && !Array.isArray(v) ? Object.fromEntries(Object.keys(v).sort().map((k) => [k, v[k]])) : v));
const same = (a, b) => stable(a) === stable(b);
/** `parameters.length` read from a request body. */
const dig = (body, path) => path.split(".").reduce((node, key) => (node == null ? undefined : node[key]), body);
/** Every strip item: {field: {text, value}}; `value` only when the strip claims the body carries it. */
const readStrip = (page) =>
  page.evaluate(() => {
    const out = {};
    for (const el of document.querySelectorAll(".run-strip [data-field]")) {
      const item = { text: (el.textContent ?? "").trim() };
      if (el.dataset.value !== undefined) item.value = JSON.parse(el.dataset.value);
      out[el.dataset.field] = item;
    }
    return out;
  });
/** Waits until the page has posted `count` jobs. */
async function postedJobs(count, timeout = 10000) {
  const until = Date.now() + timeout;
  while (posted.length < count && Date.now() < until) await new Promise((r) => setTimeout(r, 50));
  return posted.length >= count;
}

async function openStrategyPage(page) {
  await page.getByRole("button", { name: /Strategies$/ }).first().click();
  const name = page.locator(".catalog-name", { hasText: detail.strategy.name }).first();
  await name.waitFor({ timeout: 15000 });
  await name.click();
  await page.getByRole("button", { name: /Open strategy/ }).first().click();
  await page.locator(".run-strip").waitFor({ timeout: 15000 });
}
const dialogOpen = (page) => page.locator("dialog.run-dialog[open]");
async function openDialog(page) {
  await page.locator(".run-strip-configure").first().click();
  await dialogOpen(page).waitFor({ timeout: 10000 });
  await page.waitForTimeout(150);
}
async function applyPreset(page) {
  await dialogOpen(page).locator(".run-dialog-presets button", { hasText: preset.name }).first().click();
  await page.waitForTimeout(150);
}

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
      if (message.type() === "error" && !/Failed to load resource/.test(message.text())) errors.push(message.text());
    });
    page.on("pageerror", (error) => errors.push(String(error)));
    const fail = (text) => failures.push(`${mode}: ${text}`);
    const before = posted.length;
    try {
      await page.goto(served.url, { waitUntil: "domcontentloaded" });
      await page.locator(".app-shell").waitFor({ timeout: 15000 });
      await openStrategyPage(page);

      // The rules fold closed, its count on the bar.
      const fold = await page.evaluate(() => {
        const el = document.querySelector("details.rules-fold");
        return el ? { open: el.open, bar: el.querySelector("summary")?.textContent ?? "" } : null;
      });
      if (!fold) fail("no Production rules fold on the page");
      else {
        if (fold.open) fail("the Production rules fold opens open");
        if (!fold.bar.includes(`(${detail.rules.length})`)) fail(`the rules bar reads ${JSON.stringify(fold.bar.trim())} without the count ${detail.rules.length}`);
      }

      // The dialog: a native modal with the four rows and the presets inside it.
      if (await dialogOpen(page).count()) fail("the dialog is open before Configure run is clicked");
      await openDialog(page);
      const shape = await page.evaluate(() => {
        const dialog = document.querySelector("dialog.run-dialog[open]");
        return {
          modal: dialog?.matches(":modal") ?? false,
          rows: [...dialog.querySelectorAll(".form-section h3")].map((h) => (h.textContent ?? "").trim().split(/\s*·/)[0].trim()),
          presets: [...dialog.querySelectorAll(".run-dialog-presets button")].map((b) => (b.textContent ?? "").trim()),
          cancel: Boolean(dialog.querySelector(".run-dialog-cancel")),
          run: Boolean(dialog.querySelector(".run-dialog-run")),
          advanced: Boolean(dialog.querySelector(".mode-switch")),
        };
      });
      if (!shape.modal) fail("the dialog is not shown as a modal (showModal)");
      if (shape.rows.join("|") !== ROWS.join("|")) fail(`dialog rows are ${JSON.stringify(shape.rows)}, not ${JSON.stringify(ROWS)}`);
      if (!shape.presets.some((text) => text.includes(preset.name))) fail(`the dialog lists no preset named ${JSON.stringify(preset.name)} (found ${JSON.stringify(shape.presets)})`);
      if (!shape.cancel || !shape.run) fail("the dialog lacks its Cancel or Run button");
      if (!shape.advanced) fail("the dialog lacks the advanced toggle");

      // Apply the seeded preset, Escape: the dialog closes and the strip shows the preset.
      await applyPreset(page);
      await page.keyboard.press("Escape");
      await dialogOpen(page).waitFor({ state: "hidden", timeout: 5000 }).catch(() => fail("Escape did not close the dialog"));
      await page.waitForTimeout(150);
      const strip = await readStrip(page);
      if (verbose) console.log(`${mode}: strip ${JSON.stringify(strip)}`);
      if (strip.preset?.text !== preset.name) fail(`the strip names the preset ${JSON.stringify(strip.preset?.text)}, not ${JSON.stringify(preset.name)}`);
      for (const [key, value] of Object.entries(preset.parameters)) {
        const item = strip[`parameters.${key}`];
        if (!item || !("value" in item)) fail(`the strip shows no value for the preset parameter ${key}`);
        else if (!same(item.value, value)) fail(`the strip shows ${key} = ${stable(item.value)}, the preset says ${stable(value)}`);
      }
      for (const field of ["research_label", "start_date", "end_date"]) {
        if (!strip[field] || !("value" in strip[field])) fail(`the strip shows no ${field}`);
      }
      const claimed = Object.entries(strip).filter(([, item]) => "value" in item);
      if (claimed.length < Object.keys(preset.parameters).length + 3) fail(`the strip claims only ${claimed.length} request values`);

      // The strip's Run posts exactly what the strip shows.
      await page.locator(".run-strip-run").first().click();
      if (!(await postedJobs(before + 1))) fail("the strip's Run posted no job");
      const fromStrip = posted[before];
      if (verbose) console.log(`${mode}: strip Run posted ${stable(fromStrip)}`);
      for (const [field, item] of claimed) {
        if (!same(dig(fromStrip, field), item.value)) fail(`strip Run sent ${field} = ${stable(dig(fromStrip, field))}, the strip shows ${stable(item.value)}`);
      }
      if (fromStrip?.strategy_id !== detail.strategy.id) fail(`strip Run sent strategy_id ${JSON.stringify(fromStrip?.strategy_id)}`);

      // Back on the page: the preset again, Cancel keeps it; Run in the dialog closes and posts the same body.
      await openStrategyPage(page);
      await openDialog(page);
      await applyPreset(page);
      await dialogOpen(page).locator(".run-dialog-cancel").first().click();
      await dialogOpen(page).waitFor({ state: "hidden", timeout: 5000 }).catch(() => fail("Cancel did not close the dialog"));
      await page.waitForTimeout(150);
      const after = await readStrip(page);
      if (!same(after, strip)) fail(`after Cancel the strip reads ${stable(after)}, before it read ${stable(strip)}`);
      await openDialog(page);
      await dialogOpen(page).locator(".run-dialog-run").first().click();
      if (!(await postedJobs(before + 2))) fail("the dialog's Run posted no job");
      await dialogOpen(page).waitFor({ state: "hidden", timeout: 5000 }).catch(() => fail("Run did not close the dialog"));
      const fromDialog = posted[before + 1];
      if (verbose) console.log(`${mode}: dialog Run posted ${stable(fromDialog)}`);
      for (const [field, item] of claimed) {
        if (!same(dig(fromDialog, field), item.value)) fail(`dialog Run sent ${field} = ${stable(dig(fromDialog, field))}, the strip shows ${stable(item.value)}`);
      }
      if (fromStrip && fromDialog && !same(fromStrip, fromDialog)) fail(`the strip's Run and the dialog's Run sent different bodies:\n    strip  ${stable(fromStrip)}\n    dialog ${stable(fromDialog)}`);
      for (const error of errors) fail(`console error: ${error.split("\n")[0]}`);
    } catch (error) {
      fail(String(error).split("\n")[0]);
    }
    await context.close();
  }
} finally {
  await browser.close();
  served.server.close();
}

if (failures.length) {
  console.error(`run-form-check: ${failures.length} failure(s)`);
  for (const f of failures) console.error(`  ${f}`);
  process.exit(1);
}
console.log(
  `run-form-check: ok (web/fixtures/strategy-detail.json, terminal and modern; preset ${JSON.stringify(preset.name)} with ${Object.keys(preset.parameters).length} values read from the strip; ${posted.length} jobs posted, strip and dialog bodies equal; shell ${upstream ? "on the console" : "offline"}) via ${runtime.from}`,
);
