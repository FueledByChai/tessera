#!/usr/bin/env node
// Laptop layout check (UI-03). Drives the console headlessly at 1280 and 1440 px wide, in
// terminal and modern mode, through the run overview, a strategy page, and the studies page,
// and fails when the page body scrolls horizontally or an element's right edge passes the
// viewport without a scrolling ancestor (a wide table must scroll inside its own wrapper).
//
//   node web/scripts/layout-check.mjs                  this checkout's web/dist, API from the
//                                                      console at LAYOUT_CONSOLE (127.0.0.1:8787)
//   LAYOUT_URL=http://127.0.0.1:5173/ node ...         a served app as-is (the vite dev server)
//   node web/scripts/layout-check.mjs --verbose        print every page's measurement
//
// The bundle under test is the one just built (`npm run build`), served by this script with
// /api, /artifacts, and /reports proxied to the running console, so the check sees the current
// stylesheet with real runs, strategies, and studies. Needs a Playwright-compatible Chromium:
// `playwright` or `patchright` resolvable from web/, or the CodeGPT VS Code extension's bundled
// copy. Without one, without a built bundle, or without a reachable console, the check reports
// that it skipped and exits 0; the theme check's static rules still apply there.
import { createRequire } from "node:module";
import { existsSync, readdirSync } from "node:fs";
import { readFile, stat } from "node:fs/promises";
import http from "node:http";
import { homedir } from "node:os";
import { extname, join } from "node:path";
import { fileURLToPath } from "node:url";

const consoleOrigin = process.env.LAYOUT_CONSOLE ?? "http://127.0.0.1:8787/";
const servedUrl = process.env.LAYOUT_URL;
const distDir = fileURLToPath(new URL("../dist/", import.meta.url));
const verbose = process.argv.includes("--verbose");

/** Serves the built bundle on an ephemeral port, proxying the API paths to the console. */
async function serveDist(dir, apiOrigin) {
  const types = {
    ".html": "text/html; charset=utf-8",
    ".js": "text/javascript",
    ".css": "text/css",
    ".svg": "image/svg+xml",
    ".json": "application/json",
    ".png": "image/png",
    ".ico": "image/x-icon",
    ".woff2": "font/woff2",
  };
  const server = http.createServer(async (req, res) => {
    const requested = new URL(req.url ?? "/", "http://localhost");
    if (/^\/(api|artifacts|reports)(\/|$)/.test(requested.pathname)) {
      const target = new URL(requested.pathname + requested.search, apiOrigin);
      const upstream = http.request(
        target,
        { method: req.method, headers: { ...req.headers, host: target.host } },
        (reply) => {
          res.writeHead(reply.statusCode ?? 502, reply.headers);
          reply.pipe(res);
        },
      );
      upstream.on("error", () => {
        res.writeHead(502);
        res.end();
      });
      req.pipe(upstream);
      return;
    }
    let file = join(dir, decodeURIComponent(requested.pathname));
    try {
      if ((await stat(file)).isDirectory()) file = join(file, "index.html");
    } catch {
      file = join(dir, "index.html"); // the console is a single page
    }
    try {
      const body = await readFile(file);
      res.writeHead(200, { "content-type": types[extname(file)] ?? "application/octet-stream" });
      res.end(body);
    } catch {
      res.writeHead(404);
      res.end();
    }
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  return { server, url: `http://127.0.0.1:${server.address().port}/` };
}
const WIDTHS = [1280, 1440];
const HEIGHT = 800;

function resolveChromium() {
  const roots = [process.cwd() + "/", join(process.cwd(), "web") + "/"];
  for (const base of [join(homedir(), ".vscode/extensions"), join(homedir(), ".vscode-server/extensions")]) {
    if (!existsSync(base)) continue;
    const dirs = readdirSync(base)
      .filter((d) => d.startsWith("danielsanmedium.dscodegpt-"))
      .sort((a, b) => a.localeCompare(b, undefined, { numeric: true }));
    if (dirs.length) roots.push(join(base, dirs[dirs.length - 1], "standalone") + "/");
  }
  for (const root of roots) {
    for (const name of ["playwright", "patchright"]) {
      try {
        const mod = createRequire(root)(name);
        const chromium = mod?.chromium ?? mod?.default?.chromium;
        if (chromium) return { chromium, from: `${name} (${root})` };
      } catch {
        /* try the next candidate */
      }
    }
  }
  return null;
}

async function reachable(target) {
  try {
    const response = await fetch(new URL("/api/jobs", target), { signal: AbortSignal.timeout(3000) });
    return response.ok;
  } catch {
    return false;
  }
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

/** The three pages, reached by clicking, since the console has no routes. */
const PAGES = {
  "run overview": async (page) => {
    await page.getByRole("button", { name: /Runs$/ }).first().click();
    const row = page.locator("table tbody tr").first();
    await row.waitFor({ timeout: 15000 });
    await row.locator("td").nth(1).click();
    await page.locator(".equity-chart svg, .run-failure").first().waitFor({ timeout: 30000 });
    await page.getByRole("button", { name: /^Overview/ }).first().click().catch(() => {});
  },
  "strategy page": async (page) => {
    await page.getByRole("button", { name: /Strategies$/ }).first().click();
    const name = page.locator(".catalog-name").first();
    await name.waitFor({ timeout: 15000 });
    await name.click();
    await page.getByRole("button", { name: /Open strategy/ }).first().click();
    await page.locator(".field-grid").first().waitFor({ timeout: 15000 });
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
};

const runtime = resolveChromium();
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
if (!servedUrl) served = await serveDist(distDir, consoleOrigin);
const url = servedUrl ?? served.url;

// The full Chromium build in new headless mode: the separate headless shell is often absent.
const browser = await runtime.chromium.launch({
  headless: true,
  channel: "chromium",
  args: ["--no-sandbox", "--disable-dev-shm-usage"],
});
const failures = [];
try {
  for (const mode of ["terminal", "modern"]) {
    for (const width of WIDTHS) {
      const context = await browser.newContext({ viewport: { width, height: HEIGHT } });
      await context.addInitScript((m) => window.localStorage.setItem("bt-display-mode", m), mode);
      const page = await context.newPage();
      for (const [name, open] of Object.entries(PAGES)) {
        await page.goto(url, { waitUntil: "domcontentloaded" });
        await page.locator(".app-shell").waitFor({ timeout: 15000 });
        try {
          await open(page);
        } catch (error) {
          failures.push(`${mode} ${width}px ${name}: could not open (${String(error).split("\n")[0]})`);
          continue;
        }
        await page.waitForTimeout(400);
        const result = await page.evaluate(measure);
        const label = `${mode} ${width}px ${name}`;
        const wide = result.scrollWidth > result.innerWidth;
        const notes = [...result.offenders.map((o) => `past the viewport: ${o}`), ...result.clipped.map((c) => `cut off in its wrapper: ${c}`)];
        if (verbose || wide || notes.length) {
          console.log(`${label}: scrollWidth ${result.scrollWidth} / ${result.innerWidth}${notes.length ? "\n  " + notes.join("\n  ") : ""}`);
        }
        if (wide) failures.push(`${label}: page scrolls horizontally (${result.scrollWidth} > ${result.innerWidth})`);
        for (const note of notes) failures.push(`${label}: ${note}`);
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
console.log(
  `layout-check: ok (${WIDTHS.join("/")} px, terminal and modern, ${Object.keys(PAGES).length} pages, ${served ? "built bundle with the console's data" : servedUrl}) via ${runtime.from}`,
);
