# 0010 — The Data workspace describes the market data registered for future runs

Status: accepted
Date: 2026-09-11

## Context

Locked in `docs/PRODUCT_BACKLOG.md` under "Locked product decisions" at the initial public
release (2026-09-02), before decision records existed (HK-31); moved here by HK-33 with the
context reconstructed from the backlog and the docs. The bullet read:

> The Data workspace describes the market data currently registered and available to future runs.

Why, as the docs record it: the first Data page was run-centric. `docs/LOCAL_UI.md` describes
"a Data Coverage workspace that audits selected signal sessions by symbol, year, and exact
missing symbol-date", labelled signal-session coverage, and a selector over completed runs.
BT-601 wants the workspace to answer "where the application's data comes from and which
source is responsible for each dataset"; Milestone 2 says "replace the run-centric Data
screen" through BT-601 to BT-608. The question the owner brings to the page is "what data do
I have to run on", and a run's coverage answers a different one (0011).

## Decision

The Data workspace is an inventory of what is registered now and available to the next run:
sources, datasets, resolutions, coverage, and freshness. It is not organised around any
particular run.

## Alternatives

Not recorded when the bullet was locked. Inferred from the docs, written now:

- Keep the run-centric coverage page as the Data page (the original): it shows exactly what
  one run saw, which is the wrong grain for "do I have FX at five minutes".
- One page for both, inventory above run coverage: the two compete for the same screen,
  which BT-608 names as the problem.

## Consequences

Run-specific coverage moves to the run's detail page (0011). The inventory needs its own
source of truth (a catalog scan, BT-606) rather than a run's coverage artifact, and its own
freshness states (BT-605). The `Completed structured run` selector leaves the inventory.

## What would show this was wrong

The owner opening the Data page mostly to answer "what did run X use", or the inventory
never being consulted before a run because the run form already answers it. Then the page
becomes a run-coverage browser again, in a new record.
