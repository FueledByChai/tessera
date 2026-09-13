#!/usr/bin/env node
// Data page check (DS-01, DS-06; decisions 0003, 0010, 0014, 0021). Serves the built bundle
// with the Data page's APIs answered from web/fixtures/data-sources.json (/api/data/sources,
// /api/data/status, /api/automations, /api/instruments, the last searched like the console
// does: q against code, symbol, and name, symbols exact, limit; and the registered-source API,
// /api/sources with its availability, dataset, token, and scan endpoints, answered statefully
// so the page's posts show on the next load), opens the Data page in terminal and modern mode,
// and proves the three views:
//   1. the page opens at the top on Inventory, whose panels come in wireframe order (BT-1201 to
//      BT-1203): the source cards first, then Available from <source>, then the configured
//      library's feeds, and the run-coverage fold, closed (DS-11: no library metrics panel,
//      the status strip reads the calendar symbol's file); every card
//      shows its header, connection state, credits line (used / limit, "resets 00:00 UTC"),
//      datasets table (every column of the wireframe) or "no datasets yet", and Uncataloged
//      line in that order; the rejected source shows Credentials rejected with the provider's
//      message; the availability panel shows "listed <time>", the rows with datasets first, a
//      filter that narrows by code or name, and a row whose listing is not cached fetches it
//      when expanded; Add dataset, Replace token, and Add source are inline forms (no dialog),
//      Add source pre-filled with the configured library's root and catalog, the token a
//      password field posted once and then gone: after a save no element's text or value
//      carries the fixture's token or the secrets path;
//   2. scrolled to the bottom, switching to Instrument search lands at the top; a query typed
//      there lists the fixture's matches under the ten columns (symbol, name, venue, class,
//      currency, status, EOD, 5m, 1m, tick) and a clicked row is the selection;
//   3. after opening Studies and coming back, the Data page is still on Instrument search with
//      the same query, the same rows, and the same selection (BT-608);
//   4. scrolled to the bottom, switching to Updates & schedules lands at the top and shows the
//      two native tables and nothing else (DS-10, DS-11, BT-1207): the jobs
//      newest first under the wireframe's nine columns, twelve of them until Show all, the log
//      opening in a row under its job; the dataset schedules under the wireframe's columns with
//      seven marks each (o complete, ~ queued, . skipped, x failed, - none), Run now posting a
//      run and Pause posting a toggle, and Add schedule as an inline form (no dialog) listing
//      every registered dataset and posting a dataset_update schedule for the one chosen;
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

import { LAUNCH, reachable, readBody, resolveChromium, serveDist } from "./headless.mjs";

const consoleOrigin = process.env.LAYOUT_CONSOLE ?? "http://127.0.0.1:8787/";
const distDir = fileURLToPath(new URL("../dist/", import.meta.url));
const fixturePath = fileURLToPath(new URL("../fixtures/data-sources.json", import.meta.url));
const verbose = process.argv.includes("--verbose");
const COLUMNS = ["Symbol", "Name", "Venue", "Class", "Ccy", "Status", "EOD", "5m", "1m", "Tick"];
const DATASET_COLUMNS = ["Exchange", "Types", "Res", "From", "Folder", "Listed", "On disk", "Latest", "Current", "Size", "State"];
const AVAILABILITY_COLUMNS = ["Exchange", "Name", "Country", "Types (listed)", "Res", "Here"];
const PANEL_ORDER = ["data-sources-panel", "availability-panel", "data-library-feeds", "coverage-fold"];
const CARD_ORDER = ["source-card-head", "source-state", "source-credits", "dataset-table", "source-uncataloged"];
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
const SECRET = fixture.secret;
const REJECTED = fixture.registered.sources.find((s) => s.verify_state === "credentials_rejected");
const CONNECTED = fixture.registered.sources.find((s) => s.verify_state === "connected");
const clone = (value) => JSON.parse(JSON.stringify(value));
/** The registered-source API's state for one browser context, reset per display mode. */
let sources;
let availability;
/** The schedules (DS-10): the fixture's, plus what the page adds, runs, and toggles. */
let automations;
/** What the page posted, by endpoint, in order. */
let posted;
function resetSources() {
  sources = clone(fixture.registered.sources);
  availability = clone(fixture.availability);
  automations = clone(fixture.automations);
  posted = { sources: [], tokens: [], datasets: [], refreshes: [], scans: [], reserves: [], automations: [], runs: [], toggles: [] };
}
const DATASET_SCHEDULES = fixture.automations.filter((a) => a.kind === "dataset_update");
const JOB_COLUMNS = ["Dataset", "Kind", "State", "Started", "Calls", "Added", "Updated", "Error", "Log"];
const SCHEDULE_COLUMNS = ["Dataset", "Time PT", "Days", "On", "Last run", "Last status", "Last 7", ""];
const JOBS_SHOWN = 12;
const UPDATES_ORDER = ["dataset-jobs-panel", "dataset-schedules-panel"];
/** The console's mark for a run's status, and a schedule's seven marks padded on the left. */
const markOf = (status) => (status === "complete" ? "o" : status.startsWith("failed") ? "x" : status.startsWith("skipped") ? "." : status.startsWith("queued") ? "~" : "?");
const marksOf = (runs) => `${"-".repeat(Math.max(0, 7 - runs.length))}${runs.slice(-7).map((r) => markOf(r.status)).join("")}`;
const datasetLabel = (d) => `${d.exchange} ${d.resolution === "daily" ? "EOD" : d.resolution}`;
/** The schedules API, stateful: a created schedule is listed, Run now appends a run, toggle flips enabled. */
async function automationsApi(pathname, req) {
  const method = req.method ?? "GET";
  if (pathname === "/api/automations") {
    if (method === "GET") return json(automations);
    if (method === "POST") {
      const request = JSON.parse(await readBody(req));
      posted.automations.push(request);
      const dataset = sources.flatMap((s) => s.datasets).find((d) => d.id === request.dataset_id);
      if (request.kind === "dataset_update" && !dataset) return json({ error: `no dataset ${request.dataset_id}` }, 404);
      const label = dataset ? datasetLabel(dataset) : null;
      const created = {
        id: `automation-fixture-${posted.automations.length}`,
        name: request.name || label,
        kind: request.kind,
        enabled: Boolean(request.enabled),
        local_time: request.local_time,
        weekdays: request.weekdays,
        last_run_date: null,
        last_status: null,
        created_at: "2026-09-11T20:40:00+00:00",
        dataset_id: dataset?.id ?? null,
        dataset: label,
        runs: [],
      };
      automations.push(created);
      return json(created, 201);
    }
  }
  const match = pathname.match(/^\/api\/automations\/([^/]+)\/(run|toggle)$/);
  if (!match || method !== "POST") return json({ error: `the fixture does not answer ${method} ${pathname}` }, 405);
  const schedule = automations.find((a) => a.id === match[1]);
  if (!schedule) return json({ error: `no schedule ${match[1]}` }, 404);
  if (match[2] === "toggle") {
    posted.toggles.push(schedule.id);
    schedule.enabled = !schedule.enabled;
  } else {
    posted.runs.push(schedule.id);
    const run = { ran_at: "2026-09-11T20:41:00+00:00", status: `queued job-fixture-${posted.runs.length}`, job_id: `job-fixture-${posted.runs.length}` };
    schedule.runs = [...(schedule.runs ?? []), run].slice(-7);
    schedule.last_status = run.status;
    schedule.last_run_date = "2026-09-11";
  }
  return json(schedule);
}
resetSources();
const findSource = (id) => sources.find((s) => s.id === id);
/** A card for a source the page registers: the request's settings, no token, no datasets. */
function cardOf(request, index) {
  return {
    ...clone(CONNECTED),
    id: `source-fixture-${index}`,
    name: request.name,
    kind: request.kind,
    root: request.root,
    catalog_dir: request.catalog_dir,
    reserve_pct: request.reserve_pct ?? 5,
    datasets: [],
    uncataloged: [],
    scanned_at: null,
    scanning: false,
  };
}
/** The LSE listing the fixture provider answers when the row is expanded. */
const LSE_LISTING = { listings_fetched_at: "2026-09-11T20:30:00+00:00", listed: 3162, types: [{ type: "Common Stock", count: 1912 }, { type: "ETF", count: 1250 }] };
const sourcePath = (pathname) => pathname.match(/^\/api\/sources\/([^/]+)(?:\/(.*))?$/);
async function sourcesApi(pathname, req) {
  const method = req.method ?? "GET";
  if (pathname === "/api/sources") {
    if (method === "GET") return json({ kinds: fixture.registered.kinds, sources });
    if (method === "POST") {
      const request = JSON.parse(await readBody(req));
      posted.sources.push(request);
      const card = cardOf(request, posted.sources.length);
      sources.push(card);
      availability[card.id] = { source_id: card.id, fetched_at: null, refreshed_at: null, unreachable: null, exchanges: [] };
      return json(card, 201);
    }
  }
  const match = sourcePath(pathname);
  if (!match) return null;
  const [, id, rest] = match;
  const source = findSource(id);
  if (!source) return json({ error: `no source ${id}` }, 404);
  if (rest === undefined) {
    if (method === "PUT") {
      const request = JSON.parse(await readBody(req));
      posted.reserves.push({ id, ...request });
      source.reserve_pct = request.reserve_pct;
      return json(source);
    }
    if (method === "DELETE") {
      sources = sources.filter((s) => s.id !== id);
      return { status: 204, body: "" };
    }
  }
  if (rest === "token" && method === "PUT") {
    const request = JSON.parse(await readBody(req));
    posted.tokens.push({ id, ...request });
    source.token_set_at = "2026-09-11T20:31:00+00:00";
    source.verified_at = "2026-09-11T20:31:00+00:00";
    source.verify_state = "connected";
    source.verify_message = null;
    return json(source);
  }
  if (rest === "verify" && method === "POST") return json(source);
  if (rest === "availability" && method === "GET") return json(availability[id]);
  if (rest === "availability/refresh" && method === "POST") {
    const text = await readBody(req);
    const request = text.trim() ? JSON.parse(text) : {};
    posted.refreshes.push({ id, ...request });
    const table = availability[id];
    if (request.exchange === "LSE") {
      const row = table.exchanges.find((e) => e.code === "LSE");
      if (row) Object.assign(row, LSE_LISTING);
    }
    return json(table);
  }
  if (rest === "datasets" && method === "POST") {
    const request = JSON.parse(await readBody(req));
    posted.datasets.push({ id, ...request });
    const dataset = {
      id: `dataset-fixture-${posted.datasets.length}`,
      source_id: id,
      exchange: request.exchange,
      types: request.types,
      resolution: request.resolution,
      from_date: request.from_date,
      folder: request.folder ?? `${source.root}/${request.resolution === "daily" ? "eod" : request.resolution}`,
      include_delisted: request.include_delisted ?? request.resolution === "daily",
      created_at: "2026-09-11T20:32:00+00:00",
      state: "Unknown",
      scan: null,
    };
    source.datasets.push(dataset);
    return json(dataset, 201);
  }
  if (rest === "scan" && method === "POST") {
    posted.scans.push({ id });
    return json({ source_id: id, scanning: true }, 202);
  }
  return json({ error: `the fixture does not answer ${method} ${pathname}` }, 405);
}
const fixtureApi = async (pathname, req) => {
  if (pathname === "/api/data/sources") return json(fixture.sources);
  if (pathname === "/api/data/status") return json(fixture.status);
  if (pathname.startsWith("/api/automations")) return automationsApi(pathname, req);
  if (pathname === "/api/instruments") return json(searchInstruments(new URL(req.url ?? "/", "http://localhost").searchParams));
  if (pathname.startsWith("/api/sources")) return sourcesApi(pathname, req);
  if (pathname === "/api/datasets/jobs" && req.method === "GET") return json(fixture.jobs);
  if (/^\/api\/datasets\/jobs\/[^/]+\/log$/.test(pathname) && req.method === "GET") return { status: 200, type: "text/plain; charset=utf-8", body: fixture.job_log };
  if (pathname.startsWith("/api/datasets/") && req.method === "DELETE") {
    const dataset = pathname.slice("/api/datasets/".length);
    for (const source of sources) source.datasets = source.datasets.filter((d) => d.id !== dataset);
    return json(sources[0]);
  }
  if (pathname === "/api/runs" || pathname === "/api/jobs" || pathname === "/api/cost-profiles") return json([]);
  if (pathname === "/api/studies" || pathname === "/api/features" || pathname === "/api/lake/instruments" || pathname === "/api/studies/series") return json([]);
  if (pathname === "/api/dashboard") {
    return json({ strategies: [], recent_runs: [], jobs: [], production_strategies: 0, archived_strategies: 0, historical_reports: 0, active_jobs: 0, worker_capacity: 2 });
  }
  return null;
};
/** Waits until `posted[kind]` holds `count` requests. */
async function postedCount(kind, count, timeout = 10000) {
  const until = Date.now() + timeout;
  while (posted[kind].length < count && Date.now() < until) await new Promise((r) => setTimeout(r, 50));
  return posted[kind].length >= count;
}

/** Runs in the page: the tab strip's state and what follows it. */
function readWorkspace() {
  const tabs = document.querySelector("nav.data-tabs");
  const active = tabs?.querySelector("button.active");
  const workspace = document.querySelector(".data-workspace");
  const firstPanel = workspace ? [...workspace.children].find((el) => el.classList.contains("panel") || el.querySelector?.(".panel")) : null;
  const panel = firstPanel?.classList.contains("panel") ? firstPanel : firstPanel?.querySelector(".panel");
  const fold = document.querySelector("details.coverage-fold");
  const known = ["data-sources-panel", "availability-panel", "data-library-feeds", "coverage-fold"];
  const panels = workspace
    ? [...workspace.querySelectorAll(":scope > section, :scope > details")].map((el) => known.find((k) => el.classList.contains(k)) ?? el.className).filter(Boolean)
    : null;
  return {
    scrollY: window.scrollY,
    panels,
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
/** Runs in the page: every source card, its lines in DOM order, and what they say. */
function readCards() {
  const text = (el) => el?.textContent?.replace(/\s+/g, " ").trim() ?? "";
  const known = ["source-card-head", "source-state", "source-credits", "dataset-table", "dataset-empty", "source-uncataloged", "dataset-form", "token-form"];
  return [...document.querySelectorAll(".data-sources-panel article.source-card")].map((card) => {
    const table = card.querySelector("table.dataset-table");
    return {
      id: card.dataset.sourceId,
      order: [...card.querySelectorAll("*")].map((el) => known.find((k) => el.classList.contains(k))).filter(Boolean).filter((k, i, all) => all.indexOf(k) === i),
      head: text(card.querySelector(".source-card-head")),
      state: text(card.querySelector(".source-state")),
      rejected: card.querySelector(".source-state")?.classList.contains("rejected") ?? false,
      credits: text(card.querySelector(".source-credits")),
      reserve: card.querySelector(".source-credits input")?.value ?? null,
      columns: table ? [...table.querySelectorAll("thead th")].map((th) => th.textContent.trim()) : null,
      rows: table ? [...table.querySelectorAll("tbody tr")].map((tr) => [...tr.querySelectorAll("td")].map((td) => td.textContent.replace(/\s+/g, " ").trim())) : null,
      empty: text(card.querySelector(".dataset-empty")),
      uncataloged: text(card.querySelector(".source-uncataloged")),
    };
  });
}
/** Runs in the page: the availability panel's title, note, columns, and rows. */
function readAvailability() {
  const panel = document.querySelector(".availability-panel");
  const text = (el) => el?.textContent?.replace(/\s+/g, " ").trim() ?? "";
  return {
    title: text(panel?.querySelector(".terminal-panel-title")),
    note: text(panel?.querySelector(".availability-note")),
    columns: panel ? [...panel.querySelectorAll("table thead th")].map((th) => th.textContent.trim()) : null,
    rows: panel
      ? [...panel.querySelectorAll("table tbody tr[data-exchange]")].map((tr) => ({
          code: tr.dataset.exchange,
          cells: [...tr.querySelectorAll("td")].map((td) => td.textContent.replace(/\s+/g, " ").trim()),
          fetch: Boolean(tr.querySelector("button.fetch-listing")),
        }))
      : null,
  };
}
/** Runs in the page: the Updates & schedules view's panels in order, its jobs table, its schedules table, and the open log. */
function readUpdates() {
  const text = (el) => el?.textContent?.replace(/\s+/g, " ").trim() ?? "";
  const workspace = document.querySelector(".data-workspace");
  const known = ["dataset-jobs-panel", "dataset-schedules-panel"];
  const jobsTable = document.querySelector("table.dataset-jobs-table");
  const schedulesTable = document.querySelector("table.dataset-schedules-table");
  return {
    panels: workspace ? [...workspace.querySelectorAll(":scope > section")].map((el) => known.find((k) => el.classList.contains(k)) ?? el.className) : null,
    title: text(document.querySelector(".dataset-jobs-panel .terminal-panel-title")),
    jobColumns: jobsTable ? [...jobsTable.querySelectorAll("thead th")].map((th) => th.textContent.trim()) : null,
    jobs: jobsTable ? [...jobsTable.querySelectorAll("tbody tr[data-job-id]")].map((tr) => ({ id: tr.dataset.jobId, cells: [...tr.querySelectorAll("td")].map((td) => text(td)) })) : null,
    log: text(document.querySelector("tr.job-log-row pre")),
    scheduleColumns: schedulesTable ? [...schedulesTable.querySelectorAll("thead th")].map((th) => th.textContent.trim()) : null,
    schedules: schedulesTable
      ? [...schedulesTable.querySelectorAll("tbody tr[data-schedule-id]")].map((tr) => ({
          id: tr.dataset.scheduleId,
          cells: [...tr.querySelectorAll("td")].map((td) => text(td)),
          marks: [...tr.querySelectorAll(".run-marks i")].map((i) => i.textContent).join(""),
          buttons: [...tr.querySelectorAll("button")].map((b) => b.textContent.trim()),
        }))
      : null,
    form: Boolean(document.querySelector("form.schedule-form")),
  };
}
/** Runs in the page: whether the Data workspace fits the viewport, and which tables or elements do not. */
function measureFit() {
  const innerWidth = window.innerWidth;
  const wide = [];
  for (const table of document.querySelectorAll(".data-workspace table")) {
    const wrapper = table.parentElement?.clientWidth ?? 0;
    if (table.scrollWidth > wrapper + 1) wide.push({ table: table.className || "unnamed", width: table.scrollWidth, wrapper });
  }
  const past = [];
  for (const el of document.querySelectorAll(".data-workspace .panel, .data-workspace .panel > *")) {
    const right = el.getBoundingClientRect().right;
    if (right > innerWidth + 1) past.push({ element: `${el.tagName.toLowerCase()}.${el.className.trim().split(/\s+/).join(".")}`, right: Math.round(right) });
  }
  return { innerWidth, scrollWidth: document.documentElement.scrollWidth, wide, past: past.slice(0, 5) };
}
/** Runs in the page: the elements whose own text or value carries `needle`. */
function leaks(needle) {
  const hits = [];
  for (const el of document.querySelectorAll("body *")) {
    if (el.children.length === 0 && (el.textContent ?? "").includes(needle)) hits.push(`${el.tagName.toLowerCase()}.${el.className}`);
    if ("value" in el && typeof el.value === "string" && el.value.includes(needle)) hits.push(`${el.tagName.toLowerCase()}[name=${el.name}]`);
  }
  return hits;
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

      // 1. Inventory: at the top, the panels in wireframe order, the source cards first, the
      //    coverage fold closed.
      resetSources();
      await openDataPage(page);
      await page.locator(".data-sources-panel article.source-card table.dataset-table tbody tr").first().waitFor({ timeout: 15000 });
      await page.locator(".availability-panel table tbody tr[data-exchange]").first().waitFor({ timeout: 15000 });
      const inventory = await page.evaluate(readWorkspace);
      note(`inventory ${JSON.stringify(inventory)}`);
      if (!inventory.tabs) fail("no tab strip (nav.data-tabs) on the Data page");
      else if (inventory.tabs.join(",") !== "inventory,instruments,updates") fail(`the tab strip offers ${JSON.stringify(inventory.tabs)}`);
      if (inventory.active !== "inventory") fail(`the Data page opened on ${JSON.stringify(inventory.active)}, not Inventory`);
      if (inventory.scrollY !== 0) fail(`Inventory opened scrolled (scrollY ${inventory.scrollY})`);
      if (!inventory.sourcesFirst) fail(`the first panel on Inventory is ${inventory.firstPanel}, not the sources panel`);
      if (JSON.stringify(inventory.panels) !== JSON.stringify(PANEL_ORDER)) fail(`Inventory's panels are ${JSON.stringify(inventory.panels)}, the wireframes order them ${JSON.stringify(PANEL_ORDER)}`);
      if (!inventory.fold) fail("Inventory has no run-coverage fold (details.coverage-fold)");
      else if (inventory.fold.open) fail("the run-coverage fold opens open");
      const feeds = await page.locator(".data-library-feeds td.source-path").allTextContents();
      for (const feed of fixture.sources.csv_library.feeds) {
        if (!feeds.includes(feed.path)) fail(`the library feeds panel does not list the fixture feed ${feed.path}`);
      }
      if (await page.locator(".data-workspace .data-library-panel, .data-workspace .data-library-metrics").count()) fail("Inventory still shows the DATA LIBRARY panel (DS-11 retired it)");
      if (await page.locator(".data-workspace .dataset-schedules-panel").count()) fail("Inventory shows the schedules");
      if (await page.locator("dialog[open]").count()) fail("Inventory opened a dialog");

      // The cards: header, state, credits, datasets, Uncataloged, in that order; the rejected
      // source with its state and message; every column of the wireframe's datasets table.
      const cards = await page.evaluate(readCards);
      note(`cards ${JSON.stringify(cards)}`);
      if (cards.length !== fixture.registered.sources.length) fail(`Inventory shows ${cards.length} source cards, the fixture registers ${fixture.registered.sources.length}`);
      const first = cards.find((c) => c.id === CONNECTED.id);
      const rejected = cards.find((c) => c.id === REJECTED.id);
      if (!first) fail(`no card for the connected source ${CONNECTED.id}`);
      else {
        if (JSON.stringify(first.order) !== JSON.stringify(CARD_ORDER)) fail(`the connected card's lines are ${JSON.stringify(first.order)}, the wireframe orders them ${JSON.stringify(CARD_ORDER)}`);
        for (const part of [CONNECTED.name, CONNECTED.kind, CONNECTED.root, "token set", "verified 09-11"]) {
          if (!first.head.includes(part)) fail(`the connected card's header ${JSON.stringify(first.head)} lacks ${JSON.stringify(part)}`);
        }
        if (!/^Connected 19:02/.test(first.state)) fail(`the connected card's state reads ${JSON.stringify(first.state)}, not "Connected 19:02 ..."`);
        if (!first.state.includes("1.21 TB used, 0.79 TB free of 2.00 TB")) fail(`the connected card's volume line reads ${JSON.stringify(first.state)}`);
        if (!first.credits.includes("41,430 / 100,000")) fail(`the credits line ${JSON.stringify(first.credits)} does not read used / limit`);
        if (!first.credits.includes("resets 00:00 UTC")) fail(`the credits line ${JSON.stringify(first.credits)} does not say "resets 00:00 UTC"`);
        if (first.reserve !== "5") fail(`the reserve field reads ${JSON.stringify(first.reserve)}, not the source's 5`);
        if (!first.columns) fail("the connected card has no datasets table");
        else if (first.columns.slice(0, DATASET_COLUMNS.length).join("|") !== DATASET_COLUMNS.join("|")) fail(`the datasets table's columns are ${JSON.stringify(first.columns)}, not ${JSON.stringify(DATASET_COLUMNS)}`);
        if ((first.rows?.length ?? 0) !== CONNECTED.datasets.length) fail(`the connected card lists ${first.rows?.length ?? 0} datasets, the fixture registers ${CONNECTED.datasets.length}`);
        const us = first.rows?.[0] ?? [];
        const expect = ["US", "Common Stock, ETF", "EOD", "2000-01-01", "eod/", "23,765", "23,748", "09-10", "23,700", "41.2 GB", "Current"];
        if (us.slice(0, expect.length).join("|") !== expect.join("|")) fail(`the first dataset row reads ${JSON.stringify(us)}, not ${JSON.stringify(expect)}`);
        for (const part of ["Uncataloged", "1m/", "9 files", "12.4 GB", "scanned 09-11 02:10"]) {
          if (!first.uncataloged.includes(part)) fail(`the Uncataloged line ${JSON.stringify(first.uncataloged)} lacks ${JSON.stringify(part)}`);
        }
      }
      if (!rejected) fail(`no card for the rejected source ${REJECTED.id}`);
      else {
        if (!rejected.rejected || !rejected.state.startsWith("Credentials rejected")) fail(`the rejected source's state reads ${JSON.stringify(rejected.state)}`);
        if (!rejected.state.includes(REJECTED.verify_message)) fail(`the rejected source's state does not carry the provider's message ${JSON.stringify(REJECTED.verify_message)}`);
        if (rejected.columns) fail("the rejected source, with no datasets, shows a datasets table");
        if (!rejected.empty.includes("no datasets yet, add one to scan")) fail(`a source with no datasets says ${JSON.stringify(rejected.empty)}, not "no datasets yet, add one to scan"`);
        for (const part of ["Uncataloged", "eod/", "3 files"]) {
          if (!rejected.uncataloged.includes(part)) fail(`the rejected source's Uncataloged line ${JSON.stringify(rejected.uncataloged)} lacks ${JSON.stringify(part)}`);
        }
      }

      // Available from <source>: listed <time>, the rows with datasets first, the filter, and
      // a row whose listing is not cached fetched when expanded.
      const avail = await page.evaluate(readAvailability);
      note(`availability ${JSON.stringify(avail)}`);
      if (!avail.title.includes(`AVAILABLE FROM ${CONNECTED.name.toUpperCase()}`)) fail(`the availability panel's title reads ${JSON.stringify(avail.title)}`);
      if (!avail.title.includes("listed 09-11 02:00")) fail(`the availability panel's title ${JSON.stringify(avail.title)} does not say "listed 09-11 02:00"`);
      if (!avail.columns || avail.columns.join("|") !== AVAILABILITY_COLUMNS.join("|")) fail(`the availability table's columns are ${JSON.stringify(avail.columns)}, not ${JSON.stringify(AVAILABILITY_COLUMNS)}`);
      const table = fixture.availability[CONNECTED.id];
      if ((avail.rows?.length ?? 0) !== table.exchanges.length) fail(`the availability table lists ${avail.rows?.length ?? 0} exchanges, the fixture caches ${table.exchanges.length}`);
      const withDatasets = new Set(CONNECTED.datasets.map((d) => d.exchange));
      const firstWithout = avail.rows?.findIndex((r) => !withDatasets.has(r.code)) ?? -1;
      const lastWith = avail.rows?.map((r) => withDatasets.has(r.code)).lastIndexOf(true) ?? -1;
      if (firstWithout !== -1 && lastWith > firstWithout) fail(`the availability rows are ${JSON.stringify(avail.rows?.map((r) => r.code))}; the exchanges with datasets sort first`);
      const usRow = avail.rows?.find((r) => r.code === "US");
      if (!usRow) fail("no US row in the availability table");
      else {
        if (!usRow.cells[3].includes("Common Stock 17,906") || !usRow.cells[3].includes("ETF 5,859")) fail(`the US row's types read ${JSON.stringify(usRow.cells[3])}`);
        if (usRow.cells[5] !== "2") fail(`the US row's HERE reads ${JSON.stringify(usRow.cells[5])}, two datasets are registered against it`);
      }
      const lseRow = avail.rows?.find((r) => r.code === "LSE");
      if (!lseRow) fail("no LSE row in the availability table");
      else if (!lseRow.fetch) fail("the LSE row, whose listing is not cached, offers no expand-to-fetch control");

      // The seeded Inventory fits a 13-inch laptop: at 1280 and 1440 px nothing scrolls
      // horizontally and no table is wider than its wrapper (the layout check measures the
      // console's own Inventory, which may hold no source; this one holds the fixture's).
      for (const width of [1280, 1440]) {
        await page.setViewportSize({ width, height: 800 });
        await page.waitForTimeout(200);
        const fit = await page.evaluate(measureFit);
        note(`${width}px ${JSON.stringify(fit)}`);
        if (fit.scrollWidth > fit.innerWidth) fail(`at ${width}px Inventory scrolls horizontally (${fit.scrollWidth}px in a ${fit.innerWidth}px viewport)`);
        for (const wide of fit.wide) fail(`at ${width}px the ${wide.table} table is ${wide.width}px in a ${wide.wrapper}px wrapper`);
        for (const past of fit.past) fail(`at ${width}px ${past.element} passes the viewport (right edge ${past.right}px)`);
      }
      await page.setViewportSize({ width: 1440, height: 700 });
      await page.waitForTimeout(100);
      await page.locator(".availability-panel .availability-filter input").fill("lond");
      await page.waitForTimeout(150);
      const filtered = await page.evaluate(readAvailability);
      if (JSON.stringify(filtered.rows?.map((r) => r.code)) !== JSON.stringify(["LSE"])) fail(`the filter "lond" leaves ${JSON.stringify(filtered.rows?.map((r) => r.code))}, not LSE alone`);
      await page.locator(".availability-panel tr[data-exchange='LSE'] button.fetch-listing").click();
      if (!(await postedCount("refreshes", 1))) fail("expanding the LSE row posted no availability refresh");
      else if (posted.refreshes[0].exchange !== "LSE" || posted.refreshes[0].id !== CONNECTED.id) fail(`expanding the LSE row posted ${JSON.stringify(posted.refreshes[0])}`);
      await page.waitForTimeout(300);
      const fetched = await page.evaluate(readAvailability);
      const lseAfter = fetched.rows?.find((r) => r.code === "LSE");
      if (!lseAfter?.cells[3].includes("Common Stock 1,912")) fail(`after the fetch the LSE row's types read ${JSON.stringify(lseAfter?.cells[3])}`);
      await page.locator(".availability-panel .availability-filter input").fill("");

      // Add dataset, inline on the card: the exchange from the cached list, the types from
      // its listing, the resolution from what the provider offers there, the folder defaulted.
      await page.locator(`article.source-card[data-source-id='${CONNECTED.id}'] button.add-dataset-toggle`).click();
      const datasetForm = page.locator(`article.source-card[data-source-id='${CONNECTED.id}'] form.dataset-form`);
      await datasetForm.waitFor({ timeout: 5000 });
      if (await page.locator("dialog[open]").count()) fail("Add dataset opened a dialog");
      await datasetForm.locator("select[name='exchange']").selectOption("US");
      await page.waitForTimeout(100);
      const typeBoxes = await datasetForm.locator("input[type='checkbox'][name='types']").evaluateAll((els) => els.map((el) => el.value));
      const usTypes = table.exchanges.find((e) => e.code === "US").types.map((t) => t.type);
      if (JSON.stringify(typeBoxes) !== JSON.stringify(usTypes)) fail(`Add dataset offers the types ${JSON.stringify(typeBoxes)}, US lists ${JSON.stringify(usTypes)}`);
      const resolutions = await datasetForm.locator("select[name='resolution'] option").evaluateAll((els) => els.map((el) => el.value));
      if (JSON.stringify(resolutions) !== JSON.stringify(["daily", "1h", "5m", "1m"])) fail(`Add dataset offers the resolutions ${JSON.stringify(resolutions)}, US offers daily, 1h, 5m, 1m`);
      const folder = await datasetForm.locator("input[name='folder']").inputValue();
      if (folder !== `${CONNECTED.root}/eod`) fail(`Add dataset's folder defaults to ${JSON.stringify(folder)}, not ${CONNECTED.root}/eod`);
      if (!(await datasetForm.locator("input[name='include_delisted']").isChecked())) fail("Add dataset's include delisted is off for daily bars");
      await datasetForm.locator("input[type='checkbox'][name='types'][value='Preferred Stock']").check();
      await datasetForm.locator("input[name='from_date']").fill("2010-01-04");
      await datasetForm.locator("button[type='submit']").click();
      if (!(await postedCount("datasets", 1))) fail("saving Add dataset posted nothing");
      else {
        const body = posted.datasets[0];
        const want = { id: CONNECTED.id, exchange: "US", types: ["Preferred Stock"], resolution: "daily", from_date: "2010-01-04", folder: `${CONNECTED.root}/eod`, include_delisted: true };
        if (JSON.stringify(body) !== JSON.stringify(want)) fail(`Add dataset posted ${JSON.stringify(body)}, not ${JSON.stringify(want)}`);
      }
      await page.waitForTimeout(300);
      const afterDataset = await page.evaluate(readCards);
      if ((afterDataset.find((c) => c.id === CONNECTED.id)?.rows?.length ?? 0) !== CONNECTED.datasets.length + 1) fail("the card does not show the dataset just added");
      if (await datasetForm.count()) fail("the Add dataset form is still open after the save");

      // Replace token on the rejected source: a password field, posted once, then gone.
      await page.locator(`article.source-card[data-source-id='${REJECTED.id}'] button.replace-token-toggle`).click();
      const tokenForm = page.locator(`article.source-card[data-source-id='${REJECTED.id}'] form.token-form`);
      await tokenForm.waitFor({ timeout: 5000 });
      if ((await tokenForm.locator("input[name='token']").getAttribute("type")) !== "password") fail("the Replace token field is not a password input");
      await tokenForm.locator("input[name='token']").fill(SECRET.token);
      await tokenForm.locator("button[type='submit']").click();
      if (!(await postedCount("tokens", 1))) fail("saving Replace token posted nothing");
      else if (posted.tokens[0].token !== SECRET.token || posted.tokens[0].id !== REJECTED.id) fail(`Replace token posted ${JSON.stringify({ ...posted.tokens[0], token: "<redacted>" })}`);
      await page.waitForTimeout(300);
      if (await tokenForm.count()) fail("the Replace token form is still open after the save");
      const afterToken = await page.evaluate(readCards);
      if (!afterToken.find((c) => c.id === REJECTED.id)?.state.startsWith("Connected")) fail("after the new token the source's state is not Connected");

      // Add source, inline: pre-filled with the configured library's root and catalog, the
      // token a password field; after the save the token and the secrets path are nowhere.
      await page.locator(".data-sources-panel button.add-source-toggle").click();
      const sourceForm = page.locator(".data-sources-panel form.source-form");
      await sourceForm.waitFor({ timeout: 5000 });
      if (await page.locator("dialog[open]").count()) fail("Add source opened a dialog");
      const kinds = await sourceForm.locator("select[name='kind'] option").evaluateAll((els) => els.map((el) => el.value));
      if (JSON.stringify(kinds) !== JSON.stringify(fixture.registered.kinds)) fail(`Add source offers the kinds ${JSON.stringify(kinds)}, the service compiles in ${JSON.stringify(fixture.registered.kinds)}`);
      const libraryRoot = "/srv/market";
      if ((await sourceForm.locator("input[name='root']").inputValue()) !== libraryRoot) fail(`Add source's root is not pre-filled with the configured library's root ${libraryRoot}`);
      if ((await sourceForm.locator("input[name='catalog_dir']").inputValue()) !== fixture.sources.csv_library.catalog.path) fail(`Add source's catalog is not pre-filled with the configured library's catalog ${fixture.sources.csv_library.catalog.path}`);
      if ((await sourceForm.locator("input[name='token']").getAttribute("type")) !== "password") fail("the Add source token field is not a password input");
      await sourceForm.locator("input[name='name']").fill("Library");
      await sourceForm.locator("input[name='token']").fill(SECRET.token);
      await sourceForm.locator("button[type='submit']").click();
      if (!(await postedCount("sources", 1))) fail("saving Add source posted nothing");
      else {
        const body = posted.sources[0];
        if (body.kind !== "eodhd" || body.name !== "Library" || body.root !== libraryRoot || body.catalog_dir !== fixture.sources.csv_library.catalog.path || body.token !== SECRET.token) {
          fail(`Add source posted ${JSON.stringify({ ...body, token: body.token === SECRET.token ? "<the token>" : "<something else>" })}`);
        }
      }
      await page.waitForTimeout(300);
      if (await sourceForm.count()) fail("the Add source form is still open after the save");
      if (await page.locator("input[type='password']").count()) fail("a password field is still rendered after the save");
      const afterSource = await page.evaluate(readCards);
      if (!afterSource.some((c) => c.head.includes("Library"))) fail("the source just added has no card");
      for (const [what, needle] of [["token", SECRET.token], ["secrets path", SECRET.secrets_path]]) {
        const hits = await page.evaluate(leaks, needle);
        if (hits.length) fail(`the ${what} is on the page after the save: ${hits.join(", ")}`);
      }
      const pageText = await page.evaluate(() => document.body.innerText);
      if (pageText.includes(SECRET.token)) fail("the token is in the page's text after the save");

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

      // 4. Updates & schedules: opens at the top with the jobs and the dataset schedules.
      await scrollToBottom(page);
      await page.waitForTimeout(100);
      await clickTab(page, "updates");
      await page.locator(".data-workspace .dataset-schedules-panel").waitFor({ timeout: 10000 });
      await page.waitForTimeout(300);
      const updates = await page.evaluate(readWorkspace);
      note(`updates ${JSON.stringify(updates)}`);
      if (updates.active !== "updates") fail(`after the click the active tab is ${JSON.stringify(updates.active)}`);
      if (updates.scrollY !== 0) fail(`Updates & schedules opened scrolled (scrollY ${updates.scrollY})`);
      if (await page.locator(".data-workspace .instrument-search-view").count()) fail("Updates & schedules still shows the instrument search");
      if (await page.locator(".data-workspace .data-updates-panel, .data-workspace .automation-panel").count()) fail("Updates & schedules still shows the legacy update command panel or its schedules (DS-11 retired them)");

      // The native tables (DS-10, BT-1207): the jobs newest first under the nine columns,
      // twelve until Show all, the log opening under its row; the schedules with their seven
      // marks, Run now, Pause; Add schedule inline listing the registered datasets; and no
      // other panel (DS-11).
      await page.locator("table.dataset-jobs-table tbody tr[data-job-id]").first().waitFor({ timeout: 10000 }).catch(() => fail("no jobs table (table.dataset-jobs-table) with rows on Updates & schedules"));
      await page.locator("table.dataset-schedules-table tbody tr[data-schedule-id]").first().waitFor({ timeout: 10000 }).catch(() => fail("no schedules table (table.dataset-schedules-table) with rows on Updates & schedules"));
      const native = await page.evaluate(readUpdates);
      note(`native ${JSON.stringify(native)}`);
      if (JSON.stringify(native.panels) !== JSON.stringify(UPDATES_ORDER)) fail(`Updates & schedules' panels are ${JSON.stringify(native.panels)}, the wireframe has ${JSON.stringify(UPDATES_ORDER)} and nothing else`);
      if (!native.jobColumns) fail("no jobs table");
      else if (native.jobColumns.join("|") !== JOB_COLUMNS.join("|")) fail(`the jobs table's columns are ${JSON.stringify(native.jobColumns)}, not ${JSON.stringify(JOB_COLUMNS)}`);
      const capped = Math.min(JOBS_SHOWN, fixture.jobs.length);
      if ((native.jobs?.length ?? 0) !== capped) fail(`the jobs table lists ${native.jobs?.length ?? 0} rows, the cap is ${capped} of the fixture's ${fixture.jobs.length}`);
      if (JSON.stringify(native.jobs?.map((j) => j.id)) !== JSON.stringify(fixture.jobs.slice(0, capped).map((j) => j.id))) fail(`the jobs are not the fixture's newest first: ${JSON.stringify(native.jobs?.map((j) => j.id))}`);
      if (!native.title.includes(`(${fixture.jobs.length}`) || !/show all/i.test(native.title)) fail(`the jobs title reads ${JSON.stringify(native.title)}, not "(${fixture.jobs.length}, show all)"`);
      const running = native.jobs?.[0]?.cells ?? [];
      const expectRunning = ["US EOD", "scheduled", "running 62%", "09-12 02:15", "1,204", "0", "11,350", "-"];
      if (running.slice(0, expectRunning.length).join("|") !== expectRunning.join("|")) fail(`the first job row reads ${JSON.stringify(running)}, not ${JSON.stringify(expectRunning)}`);
      const failed = native.jobs?.find((j) => j.id === fixture.jobs.find((f) => f.state === "Failed").id);
      if (!failed?.cells[1].includes("manual") || !failed?.cells[7].includes("not mounted")) fail(`the failed manual job's row reads ${JSON.stringify(failed?.cells)}`);
      if (fixture.jobs.length > JOBS_SHOWN) {
        await page.locator(".dataset-jobs-panel button.show-all-jobs").click();
        await page.waitForTimeout(200);
        const all = await page.evaluate(readUpdates);
        if ((all.jobs?.length ?? 0) !== fixture.jobs.length) fail(`after Show all the jobs table lists ${all.jobs?.length ?? 0} rows, the fixture holds ${fixture.jobs.length}`);
      }
      await page.locator(`table.dataset-jobs-table tr[data-job-id='${fixture.jobs[0].id}'] button.job-log`).click();
      await page.locator("tr.job-log-row pre").waitFor({ timeout: 5000 }).catch(() => fail("clicking a job's log opened no log row under it"));
      const withLog = await page.evaluate(readUpdates);
      if (!withLog.log.includes(fixture.job_log.split("\n")[0])) fail(`the opened log reads ${JSON.stringify(withLog.log.slice(0, 80))}, not the fixture's log`);
      if (await page.locator("dialog[open]").count()) fail("the log opened a dialog");
      if (!native.scheduleColumns) fail("no schedules table");
      else if (native.scheduleColumns.join("|") !== SCHEDULE_COLUMNS.join("|")) fail(`the schedules table's columns are ${JSON.stringify(native.scheduleColumns)}, not ${JSON.stringify(SCHEDULE_COLUMNS)}`);
      if ((native.schedules?.length ?? 0) !== DATASET_SCHEDULES.length) fail(`the schedules table lists ${native.schedules?.length ?? 0} rows, the fixture holds ${DATASET_SCHEDULES.length} dataset schedules`);
      for (const schedule of DATASET_SCHEDULES) {
        const row = native.schedules?.find((r) => r.id === schedule.id);
        if (!row) {
          fail(`no row for the schedule ${schedule.id}`);
          continue;
        }
        const want = marksOf(schedule.runs ?? []);
        if (row.marks.length !== 7) fail(`the ${schedule.name} row shows ${row.marks.length} marks, not seven`);
        if (row.marks !== want) fail(`the ${schedule.name} row's marks are ${JSON.stringify(row.marks)}, its runs make ${JSON.stringify(want)}`);
        if (row.cells[0] !== schedule.dataset) fail(`the ${schedule.name} row names ${JSON.stringify(row.cells[0])}`);
        if (row.cells[1] !== schedule.local_time) fail(`the ${schedule.name} row's time reads ${JSON.stringify(row.cells[1])}`);
        if (row.cells[3] !== (schedule.enabled ? "x" : "-")) fail(`the ${schedule.name} row's ON reads ${JSON.stringify(row.cells[3])}`);
        if (!row.cells[5].includes(schedule.last_status ?? "never run")) fail(`the ${schedule.name} row's status reads ${JSON.stringify(row.cells[5])}`);
        if (JSON.stringify(row.buttons) !== JSON.stringify(["Run now", schedule.enabled ? "Pause" : "Enable"])) fail(`the ${schedule.name} row's actions are ${JSON.stringify(row.buttons)}`);
      }
      const usEod = native.schedules?.find((r) => r.cells[0] === "US EOD");
      if (usEod && usEod.cells[2] !== "mon-fri") fail(`the US EOD row's days read ${JSON.stringify(usEod.cells[2])}, not mon-fri`);
      // Run now posts a run for that schedule; Pause posts a toggle and the button flips.
      const firstSchedule = DATASET_SCHEDULES[0];
      await page.locator(`tr[data-schedule-id='${firstSchedule.id}'] button.run-now`).click();
      if (!(await postedCount("runs", 1))) fail("Run now posted nothing");
      else if (posted.runs[0] !== firstSchedule.id) fail(`Run now posted a run for ${posted.runs[0]}, not ${firstSchedule.id}`);
      await page.waitForTimeout(300);
      const afterRun = await page.evaluate(readUpdates);
      if (!afterRun.schedules?.find((r) => r.id === firstSchedule.id)?.cells[5].includes("queued job-fixture-1")) fail("after Run now the row does not show the run the service answered with");
      await page.locator(`tr[data-schedule-id='${firstSchedule.id}'] button.pause`).click();
      if (!(await postedCount("toggles", 1))) fail("Pause posted nothing");
      else if (posted.toggles[0] !== firstSchedule.id) fail(`Pause posted a toggle for ${posted.toggles[0]}, not ${firstSchedule.id}`);
      await page.waitForTimeout(300);
      const afterPause = await page.evaluate(readUpdates);
      if (!afterPause.schedules?.find((r) => r.id === firstSchedule.id)?.buttons.includes("Enable")) fail("after Pause the row's button does not read Enable");
      // Add schedule: inline, the datasets from every registered source, the chosen one posted.
      await page.locator(".dataset-schedules-panel button.add-schedule-toggle").click();
      const scheduleForm = page.locator("form.schedule-form");
      await scheduleForm.waitFor({ timeout: 5000 });
      if (await page.locator("dialog[open]").count()) fail("Add schedule opened a dialog");
      await scheduleForm.locator("select[name='dataset_id'] option").nth(CONNECTED.datasets.length).waitFor({ state: "attached", timeout: 5000 }).catch(() => fail("Add schedule's dataset select did not fill from the registered sources"));
      const options = await scheduleForm.locator("select[name='dataset_id'] option").evaluateAll((els) => els.map((el) => [el.value, el.textContent.trim()]));
      const registered = sources.flatMap((s) => s.datasets.map((d) => d.id));
      if (JSON.stringify(options.map(([v]) => v)) !== JSON.stringify(registered)) fail(`Add schedule offers the datasets ${JSON.stringify(options.map(([v]) => v))}, the sources register ${JSON.stringify(registered)}`);
      const eodOption = options.find(([v]) => v === CONNECTED.datasets[0].id);
      if (!eodOption?.[1].startsWith("US EOD")) fail(`the first dataset's option reads ${JSON.stringify(eodOption?.[1])}, not "US EOD ..."`);
      const chosen = CONNECTED.datasets[1];
      await scheduleForm.locator("select[name='dataset_id']").selectOption(chosen.id);
      await scheduleForm.locator("input[name='local_time']").fill("20:30");
      await scheduleForm.locator("button[type='submit']").click();
      if (!(await postedCount("automations", 1))) fail("saving Add schedule posted nothing");
      else {
        const body = posted.automations[0];
        const want = { kind: "dataset_update", dataset_id: chosen.id, local_time: "20:30", weekdays: "mon,tue,wed,thu,fri", enabled: true };
        if (JSON.stringify(body) !== JSON.stringify(want)) fail(`Add schedule posted ${JSON.stringify(body)}, not ${JSON.stringify(want)}`);
      }
      await page.waitForTimeout(300);
      if (await scheduleForm.count()) fail("the Add schedule form is still open after the save");
      const afterAdd = await page.evaluate(readUpdates);
      if ((afterAdd.schedules?.length ?? 0) !== DATASET_SCHEDULES.length + 1) fail(`after Add schedule the table lists ${afterAdd.schedules?.length ?? 0} rows, not ${DATASET_SCHEDULES.length + 1}`);
      if (!afterAdd.schedules?.some((r) => r.id === "automation-fixture-1" && r.cells[0] === datasetLabel(chosen) && r.marks === "-------")) fail("the schedule just added has no row with its dataset and seven empty marks");
      // The seeded view fits a 13-inch laptop like Inventory does.
      for (const width of [1280, 1440]) {
        await page.setViewportSize({ width, height: 800 });
        await page.waitForTimeout(200);
        const fit = await page.evaluate(measureFit);
        note(`updates ${width}px ${JSON.stringify(fit)}`);
        if (fit.scrollWidth > fit.innerWidth) fail(`at ${width}px Updates & schedules scrolls horizontally (${fit.scrollWidth}px in a ${fit.innerWidth}px viewport)`);
        for (const wide of fit.wide) fail(`at ${width}px the ${wide.table} table is ${wide.width}px in a ${wide.wrapper}px wrapper`);
        for (const past of fit.past) fail(`at ${width}px ${past.element} passes the viewport (right edge ${past.right}px)`);
      }
      await page.setViewportSize({ width: 1440, height: 700 });

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
  `data-page-check: ok (web/fixtures/data-sources.json, terminal and modern; three views each at the top, Inventory's panels in wireframe order with ${fixture.registered.sources.length} source cards (one Credentials rejected, ${CONNECTED.datasets.length} datasets and an Uncataloged folder on the first), the availability table listed 09-11 02:00 with LSE fetched on expand, Add dataset, Replace token, and Add source posted inline with the token nowhere after, ${JSON.stringify(QUERY)} listing ${expectedHits} of ${fixture.instruments.length} instruments with ${PICK} selected and kept across Studies, ${fixture.jobs.length} jobs on Updates (${JOBS_SHOWN} then Show all, the log opened from a row) over ${DATASET_SCHEDULES.length} dataset schedules with seven marks each, Run now and Pause posted, Add schedule posted inline for one of ${CONNECTED.datasets.length + 1} registered datasets, no legacy panel under them, the last view remembered; shell ${upstream ? "on the console" : "offline"}) via ${runtime.from}`,
);
