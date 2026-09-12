#!/usr/bin/env node
// Data page check (DS-01, decisions 0010 and 0014). Serves the built bundle with the Data
// page's APIs answered from web/fixtures/data-sources.json (/api/data/sources,
// /api/data/status, /api/automations, and /api/instruments, the last searched like the
// console does: q against code, symbol, and name, symbols exact, limit), opens the Data page in
// terminal and modern mode, and proves the three views:
//   1. the page opens at the top on Inventory, whose first panel after the tab strip is the
//      sources panel (the fixture's feeds listed), with the run-coverage fold closed;
//   2. scrolled to the bottom, switching to Instrument search lands at the top; a query typed
//      there lists the fixture's matches under the ten columns (symbol, name, venue, class,
//      currency, status, EOD, 5m, 1m, tick) and a clicked row is the selection;
//   3. after opening Studies and coming back, the Data page is still on Instrument search with
//      the same query, the same rows, and the same selection (BT-608);
//   4. scrolled to the bottom, switching to Updates & schedules lands at the top and shows the
//      update command's state and the fixture's schedules;
//   5. a fresh page in the same browser storage opens the Data page on the view last chosen.
// Every other API path proxies to the console at LAYOUT_CONSOLE when one answers and returns
// 503 otherwise. Needs a Playwright-compatible Chromium like the layout check; without one, or
// without a built bundle, it reports that it skipped and exits 0.
//
//   node web/scripts/data-page-check.mjs
//   node web/scripts/data-page-check.mjs --verbose      print every view's measurement
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { LAUNCH, reachable, resolveChromium, serveDist } from "./headless.mjs";

const consoleOrigin = process.env.LAYOUT_CONSOLE ?? "http://127.0.0.1:8787/";
const distDir = fileURLToPath(new URL("../dist/", import.meta.url));
const fixturePath = fileURLToPath(new URL("../fixtures/data-sources.json", import.meta.url));
const verbose = process.argv.includes("--verbose");
const COLUMNS = ["Symbol", "Name", "Venue", "Class", "Ccy", "Status", "EOD", "5m", "1m", "Tick"];
const QUERY = "IW";
const PICK = "IWM.US";

const runtime = await resolveChromium();
if (!runtime) {
  console.log("data-page-check: skipped (no playwright or patchright Chromium found)");
  process.exit(0);
}
if (!existsSync(join(distDir, "index.html"))) {
  console.log("data-page-check: skipped (no web/dist; run npm run build first)");
  process.exit(0);
}

const fixture = JSON.parse(readFileSync(fixturePath, "utf8"));
const expectedHits = fixture.instruments.filter((i) => [i.code, i.symbol, i.name].some((t) => t.toUpperCase().includes(QUERY))).length;
const json = (body, status = 200) => ({ status, type: "application/json", body: JSON.stringify(body) });
const list = (value) => (value ?? "").split(",").map((s) => s.trim()).filter(Boolean);
/** The console's ranking, enough of it for the fixture: exact code, code prefix, name prefix, contains. */
function searchInstruments(search) {
  const needle = (search.get("q") ?? "").trim().toUpperCase();
  const exact = search.get("symbols") ? new Set(list(search.get("symbols")).map((s) => s.toUpperCase())) : null;
  const suffixes = list(search.get("suffix")).map((s) => s.toUpperCase());
  const classes = list(search.get("asset_class")).map((s) => s.toLowerCase());
  const limit = Math.min(200, Math.max(1, Number(search.get("limit") ?? 25)));
  const scored = [];
  for (const record of fixture.instruments) {
    if (exact && !(exact.has(record.symbol) || (record.suffix === "US" && exact.has(record.code)))) continue;
    if (suffixes.length && !suffixes.includes(record.suffix)) continue;
    if (classes.length && !classes.includes(record.asset_class.toLowerCase())) continue;
    const name = record.name.toUpperCase();
    let rank;
    if (exact || !needle) rank = 5;
    else if (record.code === needle || record.symbol === needle) rank = 0;
    else if (record.code.startsWith(needle)) rank = 1;
    else if (name.startsWith(needle)) rank = 2;
    else if (name.split(/\s+/).some((w) => w.startsWith(needle))) rank = 3;
    else if (name.includes(needle) || record.symbol.includes(needle)) rank = 4;
    else continue;
    scored.push([rank, record]);
  }
  scored.sort((a, b) => a[0] - b[0] || a[1].symbol.localeCompare(b[1].symbol));
  return { instruments: scored.slice(0, limit).map(([, r]) => r), total_matches: scored.length, index_size: fixture.instruments.length, indexed_at: fixture.sources.generated_at };
}
const fixtureApi = (pathname, req) => {
  if (pathname === "/api/data/sources") return json(fixture.sources);
  if (pathname === "/api/data/status") return json(fixture.status);
  if (pathname === "/api/automations") return json(fixture.automations);
  if (pathname === "/api/instruments") return json(searchInstruments(new URL(req.url ?? "/", "http://localhost").searchParams));
  if (pathname === "/api/runs" || pathname === "/api/jobs" || pathname === "/api/cost-profiles") return json([]);
  if (pathname === "/api/studies" || pathname === "/api/features" || pathname === "/api/lake/instruments" || pathname === "/api/studies/series") return json([]);
  if (pathname === "/api/dashboard") {
    return json({ strategies: [], recent_runs: [], jobs: [], production_strategies: 0, archived_strategies: 0, historical_reports: 0, active_jobs: 0, worker_capacity: 2 });
  }
  return null;
};

/** Runs in the page: the tab strip's state and what follows it. */
function readWorkspace() {
  const tabs = document.querySelector("nav.data-tabs");
  const active = tabs?.querySelector("button.active");
  const workspace = document.querySelector(".data-workspace");
  const firstPanel = workspace ? [...workspace.children].find((el) => el.classList.contains("panel") || el.querySelector?.(".panel")) : null;
  const panel = firstPanel?.classList.contains("panel") ? firstPanel : firstPanel?.querySelector(".panel");
  const fold = document.querySelector("details.coverage-fold");
  return {
    scrollY: window.scrollY,
    scrollHeight: document.documentElement.scrollHeight,
    innerHeight: window.innerHeight,
    tabs: tabs ? [...tabs.querySelectorAll("button")].map((b) => b.dataset.view) : null,
    active: active?.dataset.view ?? null,
    firstPanel: panel ? `${panel.tagName.toLowerCase()}.${panel.className.trim().split(/\s+/).join(".")}` : "nothing",
    sourcesFirst: Boolean(panel?.classList.contains("data-sources-panel")),
    fold: fold ? { open: fold.open, bar: fold.querySelector("summary")?.textContent?.trim() ?? "" } : null,
  };
}
/** Runs in the page: the Instrument search view's query, columns, rows, and selection. */
function readSearch() {
  const input = document.querySelector(".instrument-search-view .instrument-search input");
  const table = document.querySelector(".instrument-table");
  return {
    query: input?.value ?? null,
    columns: table ? [...table.querySelectorAll("thead th")].map((th) => th.textContent.trim()) : null,
    rows: table ? [...table.querySelectorAll("tbody tr[data-symbol]")].map((tr) => tr.dataset.symbol) : null,
    selected: document.querySelector(".instrument-table tr.selected")?.dataset.symbol ?? null,
    detail: document.querySelector(".instrument-detail")?.textContent?.trim() ?? "",
  };
}
const scrollToBottom = (page) => page.evaluate(() => window.scrollTo({ top: document.documentElement.scrollHeight }));
const clickTab = (page, view) => page.evaluate((v) => document.querySelector(`nav.data-tabs button[data-view="${v}"]`).click(), view);
async function openDataPage(page) {
  await page.getByRole("button", { name: /Data$/ }).first().click();
  await page.locator(".data-workspace").waitFor({ timeout: 15000 });
  await page.waitForTimeout(300);
}

const upstream = (await reachable(consoleOrigin)) ? consoleOrigin : null;
const served = await serveDist(distDir, upstream, fixtureApi);
const browser = await runtime.chromium.launch(LAUNCH);
const failures = [];
try {
  for (const mode of ["terminal", "modern"]) {
    const context = await browser.newContext({ viewport: { width: 1440, height: 700 } });
    await context.addInitScript((m) => window.localStorage.setItem("bt-display-mode", m), mode);
    const page = await context.newPage();
    const errors = [];
    page.on("console", (message) => {
      if (message.type() === "error" && !/Failed to load resource/.test(message.text())) errors.push(message.text());
    });
    page.on("pageerror", (error) => errors.push(String(error)));
    const fail = (text) => failures.push(`${mode}: ${text}`);
    const note = (text) => verbose && console.log(`${mode}: ${text}`);
    try {
      await page.goto(served.url, { waitUntil: "domcontentloaded" });
      await page.locator(".app-shell").waitFor({ timeout: 15000 });

      // 1. Inventory: at the top, the sources panel first, the coverage fold closed.
      await openDataPage(page);
      await page.locator(".data-sources-panel tbody tr").first().waitFor({ timeout: 15000 });
      const inventory = await page.evaluate(readWorkspace);
      note(`inventory ${JSON.stringify(inventory)}`);
      if (!inventory.tabs) fail("no tab strip (nav.data-tabs) on the Data page");
      else if (inventory.tabs.join(",") !== "inventory,instruments,updates") fail(`the tab strip offers ${JSON.stringify(inventory.tabs)}`);
      if (inventory.active !== "inventory") fail(`the Data page opened on ${JSON.stringify(inventory.active)}, not Inventory`);
      if (inventory.scrollY !== 0) fail(`Inventory opened scrolled (scrollY ${inventory.scrollY})`);
      if (!inventory.sourcesFirst) fail(`the first panel on Inventory is ${inventory.firstPanel}, not the sources panel`);
      if (!inventory.fold) fail("Inventory has no run-coverage fold (details.coverage-fold)");
      else if (inventory.fold.open) fail("the run-coverage fold opens open");
      const feeds = await page.locator(".data-sources-panel td.source-path").allTextContents();
      for (const feed of fixture.sources.csv_library.feeds) {
        if (!feeds.includes(feed.path)) fail(`the sources panel does not list the fixture feed ${feed.path}`);
      }
      const latest = await page.locator(".data-library-metrics").textContent().catch(() => "");
      if (!latest.includes(fixture.status.latest_market_date)) fail(`the library metrics do not show the fixture's latest market date ${fixture.status.latest_market_date}`);
      if (await page.locator(".data-workspace .automation-panel").count()) fail("Inventory shows the schedules");

      // 2. Instrument search: opens at the top; the query lists the fixture's matches; a row selects.
      await scrollToBottom(page);
      await page.waitForTimeout(100);
      note(`inventory scrolled to ${await page.evaluate(() => window.scrollY)} of ${inventory.scrollHeight - inventory.innerHeight}`);
      await clickTab(page, "instruments");
      await page.locator(".instrument-search-view").waitFor({ timeout: 10000 });
      await page.waitForTimeout(300);
      const searchView = await page.evaluate(readWorkspace);
      if (searchView.active !== "instruments") fail(`after the click the active tab is ${JSON.stringify(searchView.active)}`);
      if (searchView.scrollY !== 0) fail(`Instrument search opened scrolled (scrollY ${searchView.scrollY})`);
      if (await page.locator(".data-workspace .data-sources-panel").count()) fail("Instrument search still shows the sources panel");
      await page.locator(".instrument-search-view .instrument-search input").fill(QUERY);
      await page.locator(`.instrument-table tr[data-symbol="${PICK}"]`).waitFor({ timeout: 10000 }).catch(() => fail(`typing ${JSON.stringify(QUERY)} listed no ${PICK} row`));
      await page.evaluate((symbol) => document.querySelector(`.instrument-table tr[data-symbol="${symbol}"]`)?.click(), PICK);
      await page.waitForTimeout(200);
      const search = await page.evaluate(readSearch);
      note(`search ${JSON.stringify(search)}`);
      if (search.query !== QUERY) fail(`the search box reads ${JSON.stringify(search.query)} after typing ${JSON.stringify(QUERY)}`);
      if (!search.columns) fail("no instrument table");
      else if (search.columns.join("|") !== COLUMNS.join("|")) fail(`the instrument table's columns are ${JSON.stringify(search.columns)}, not ${JSON.stringify(COLUMNS)}`);
      if ((search.rows?.length ?? 0) !== expectedHits) fail(`${JSON.stringify(QUERY)} lists ${search.rows?.length ?? 0} rows, the fixture holds ${expectedHits} matches`);
      if (search.selected !== PICK) fail(`clicking the ${PICK} row selected ${JSON.stringify(search.selected)}`);
      if (!search.detail.includes(PICK)) fail(`the selection strip does not name ${PICK} (${JSON.stringify(search.detail)})`);
      const picked = fixture.instruments.find((i) => i.symbol === PICK);
      if (picked && !search.detail.includes(picked.coverage.daily.first)) fail(`the selection strip does not show ${PICK}'s daily coverage from ${picked.coverage.daily.first}`);

      // 3. Away to Studies and back: the query, the rows, and the selection are still there.
      await page.getByRole("button", { name: /Studies$/ }).first().click();
      await page.locator(".studies-workspace").waitFor({ timeout: 15000 });
      if (await page.locator(".data-workspace").count()) fail("the Data workspace is still mounted on the Studies page");
      await openDataPage(page);
      const back = await page.evaluate(readWorkspace);
      const searchBack = await page.evaluate(readSearch);
      note(`back ${JSON.stringify(searchBack)}`);
      if (back.active !== "instruments") fail(`back from Studies the Data page opened on ${JSON.stringify(back.active)}, not Instrument search`);
      if (back.scrollY !== 0) fail(`back from Studies the Data page opened scrolled (scrollY ${back.scrollY})`);
      if (searchBack.query !== QUERY) fail(`back from Studies the search box reads ${JSON.stringify(searchBack.query)}, the query ${JSON.stringify(QUERY)} is gone`);
      if (JSON.stringify(searchBack.rows) !== JSON.stringify(search.rows)) fail(`back from Studies the rows are ${JSON.stringify(searchBack.rows)}, before they were ${JSON.stringify(search.rows)}`);
      if (searchBack.selected !== PICK) fail(`back from Studies the selection is ${JSON.stringify(searchBack.selected)}, not ${PICK}`);

      // 4. Updates & schedules: opens at the top with the update command's state and the schedules.
      await scrollToBottom(page);
      await page.waitForTimeout(100);
      await clickTab(page, "updates");
      await page.locator(".data-workspace .automation-panel").waitFor({ timeout: 10000 });
      await page.waitForTimeout(300);
      const updates = await page.evaluate(readWorkspace);
      note(`updates ${JSON.stringify(updates)}`);
      if (updates.active !== "updates") fail(`after the click the active tab is ${JSON.stringify(updates.active)}`);
      if (updates.scrollY !== 0) fail(`Updates & schedules opened scrolled (scrollY ${updates.scrollY})`);
      if (await page.locator(".data-workspace .instrument-search-view").count()) fail("Updates & schedules still shows the instrument search");
      const schedules = await page.locator(".data-workspace .automation-list article").allTextContents();
      for (const schedule of fixture.automations) {
        if (!schedules.some((text) => text.includes(schedule.name))) fail(`Updates & schedules does not list the schedule ${JSON.stringify(schedule.name)}`);
      }
      const jobs = await page.locator(".data-updates-panel").textContent().catch(() => null);
      if (jobs == null) fail("Updates & schedules has no updates panel (.data-updates-panel)");
      else {
        if (!jobs.includes(fixture.sources.csv_library.update_command)) fail("the updates panel does not name the update command");
        if (!jobs.includes(fixture.status.update_job.status)) fail(`the updates panel does not show the last update's state ${JSON.stringify(fixture.status.update_job.status)}`);
      }

      // 5. A fresh page in the same storage opens the Data page on the last view.
      const fresh = await context.newPage();
      await fresh.goto(served.url, { waitUntil: "domcontentloaded" });
      await fresh.locator(".app-shell").waitFor({ timeout: 15000 });
      await openDataPage(fresh);
      const remembered = await fresh.evaluate(readWorkspace);
      if (remembered.active !== "updates") fail(`a fresh page opened the Data page on ${JSON.stringify(remembered.active)}; Updates & schedules was the last view chosen`);
      await fresh.close();

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
  console.error(`data-page-check: ${failures.length} failure(s)`);
  for (const f of failures) console.error(`  ${f}`);
  process.exit(1);
}
console.log(
  `data-page-check: ok (web/fixtures/data-sources.json, terminal and modern; three views each at the top, sources panel first on Inventory, ${JSON.stringify(QUERY)} listing ${expectedHits} of ${fixture.instruments.length} instruments with ${PICK} selected and kept across Studies, ${fixture.automations.length} schedules on Updates, the last view remembered; shell ${upstream ? "on the console" : "offline"}) via ${runtime.from}`,
);
