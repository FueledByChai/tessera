// Shared scaffolding for the headless browser checks (layout-check, chart-check): serving the
// built bundle with the API proxied or answered from fixtures, finding a Chromium, and probing
// the console.
import { createRequire } from "node:module";
import { existsSync, readdirSync } from "node:fs";
import { readFile, stat } from "node:fs/promises";
import http from "node:http";
import { homedir } from "node:os";
import { extname, join } from "node:path";

/**
 * Serves the built bundle on an ephemeral port. `/api`, `/artifacts`, and `/reports` go to
 * `override(pathname, req)` first when it returns `{ status, type, body }`, then proxy to
 * `apiOrigin`, and answer 503 when there is no origin. Unknown paths fall back to index.html,
 * as the console is a single page.
 */
export async function serveDist(dir, apiOrigin, override = () => null) {
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
      const answer = override(requested.pathname, req);
      if (answer) {
        res.writeHead(answer.status ?? 200, { "content-type": answer.type ?? "application/json" });
        res.end(answer.body ?? "");
        return;
      }
      if (!apiOrigin) {
        res.writeHead(503, { "content-type": "application/json" });
        res.end(JSON.stringify({ error: "no console behind this check" }));
        return;
      }
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
      file = join(dir, "index.html");
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

/** A Playwright-compatible Chromium: playwright or patchright from web/ or the repo root, or the CodeGPT VS Code extension's bundled copy. */
export function resolveChromium() {
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

/** Whether a console answers at `target`. */
export async function reachable(target) {
  try {
    const response = await fetch(new URL("/api/jobs", target), { signal: AbortSignal.timeout(3000) });
    return response.ok;
  } catch {
    return false;
  }
}

/** Launch options shared by the checks: the full Chromium build in new headless mode, since the separate headless shell is often absent. */
export const LAUNCH = {
  headless: true,
  channel: "chromium",
  args: ["--no-sandbox", "--disable-dev-shm-usage"],
};
