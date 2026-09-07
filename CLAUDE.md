# Tessera: standing instructions

Tessera is an event-driven backtesting engine (Rust) with a one-file strategy SDK and a local
Bloomberg-style research console (Vite + React bundle served by the Rust service). This file is
the contract for any agent working in this checkout. It replaces instructions that would otherwise
have to be repeated in chat.

## Layout

- This checkout is the public, AGPL repo (`FueledByChai/tessera`). Engine in `src/`, SDK in
  `src/sdk/`, service in `src/bin/tessera_ui.rs`, web app in `web/` (`web/app/page.tsx`,
  `web/app/globals.css`), docs in `docs/`, synthetic examples in `examples/`.
- `../Tessera-private` is the private repo: strategies (`strategies/*.rs`, compiled in through
  `local.toml [strategies] dirs`), frozen configs, research scripts, the legacy crate, and the
  research log. Never copy private strategies, configs, or data paths into this repo.
- Market data lives outside both repos (see `local.toml`). Never modify, delete, or commit data.
- The checkout sits in an iCloud folder. Paths contain a curly apostrophe: in shell use
  `~/Documents/Documents*/Tessera`. `web/node_modules` may stall on evicted files; if Node tools
  hang, that is why.

## Build, run, restart

- Engine: `cargo build --release --bin tessera --bin tessera-ui` (private strategies compile in).
- Web: `cd web && npm run build` (~3 s). The service serves `web/dist` from disk; no restart.
- Service: http://127.0.0.1:8787. Restart after an engine change: kill the pid in
  `data/ui/api.pid` (verify with `lsof -nP -iTCP:8787 -sTCP:LISTEN`), then
  `TESSERA_ROOT=$PWD nohup ./target/release/tessera-ui > data/ui/api.log 2>&1 &` and write the new
  pid to `data/ui/api.pid`. Do not restart while a job is `running` in the catalog.
- CLI runs: `./target/release/tessera run-strategy --config <toml> --start <date> --end <date>
  --output-dir <dir>`. Scratch outputs go under `target/` or the session scratchpad, never `artifacts/`.

## Definition of done

`scripts/check.sh` must pass: fmt, tests, build, parity of the bundled examples against
`examples/expected`, web typecheck/lint/build, and the private checks when that checkout exists.
A ticket is not done until the check passes and the commit includes the test or fixture that
proves the acceptance line in `BACKLOG.md`. If an engine change intentionally alters results,
refresh the baseline with `scripts/check.sh --refresh-baseline` and say why in the commit.

## Git

- Commit per ticket, message starts with the ticket id (`WB-03: ...`), ends with the
  `Co-Authored-By: Claude ... <noreply@anthropic.com>` trailer. Never push; the owner pushes.
- Prefer an isolated worktree per ticket. Never rewrite history on `main`.
- Update `BACKLOG.md` in the same commit: status, date, commit summary.

## Engine and data rules

- Strategies go through the SDK (`docs/ADDING_A_STRATEGY.md`), never new match arms in the service.
- Quantities are `f64`; prices reaching strategies are split-adjusted; `bar.raw_close()` is the
  unadjusted print for floors and dollar volume.
- Daily data is sanitized at load (`[data] sanitize_prices`); keep it on. Any trade exiting above
  4x its entry, or any equity swing that reverses in a day, is a data error until proven otherwise.
- Universe-sized runs (tens of thousands of symbols) must stay linear in memory: no per-row
  strings in tables that scale with symbols x sessions; coverage uses the interned column table.
- Broker guards (buying power, solvency, commission cap, tick floor, min price) are not optional.

## UI conventions

- Terminal look: black panels, amber accent, dense monospace tables, 18/15 px table fonts.
- Long tables go behind tabs or caps with a "show all"; a page must open at the top.
- Relative API origin (`import.meta.env.VITE_API_ORIGIN ?? ""`); dev server on 5173 proxies `/api`.

## Docs to keep current

`docs/ADDING_A_STRATEGY.md` (SDK contract and guards), `docs/DATA_SOURCES.md` (sources and
sanitation), `docs/LOCAL_UI.md` (service and web), `README.md` screenshot when the run view changes.
