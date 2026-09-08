#!/usr/bin/env node
// Study chart check (WB-10). Serves the built bundle with the studies API answered from the
// fixture result in web/fixtures/study-result.json (a 5-minute study of the bundled DEMO.US
// data with an accepted set, curves thinned to 60 points), opens the studies page in terminal
// and modern mode, selects the study, and fails unless every chart renders (IC by horizon,
// deciles, the costless curve, daily IC) with no console error or uncaught exception, the page
// sits at the top, and the results grid comes before the first chart. Every other API path
// proxies to the console at LAYOUT_CONSOLE when one answers and returns 503 otherwise (the shell
// shows "API offline" and carries on). Needs a Playwright-compatible Chromium like the layout
// check; without one, or without a built bundle, it reports that it skipped and exits 0.
//
//   node web/scripts/chart-check.mjs
//   node web/scripts/chart-check.mjs --verbose        print every chart's measurement
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { LAUNCH, reachable, resolveChromium, serveDist } from "./headless.mjs";

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
const json = (body) => ({ status: 200, type: "application/json", body: JSON.stringify(body) });
const fixtureApi = (pathname) => {
  if (pathname === "/api/studies") return json([study]);
  if (pathname === `/api/studies/${study.id}`) return json({ study, result });
  if (pathname === "/api/features" || pathname === "/api/lake/instruments") return json([]);
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
  `chart-check: ok (${Object.keys(CHARTS).length} charts from web/fixtures/study-result.json, terminal and modern, shell ${upstream ? "on the console" : "offline"}) via ${runtime.from}`,
);
