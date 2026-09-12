# 0013 — Coverage reads a persistent fast cache; deeper completeness analysis is requested explicitly

Status: accepted
Date: 2026-09-11

## Context

Locked in `docs/PRODUCT_BACKLOG.md` under "Locked product decisions" at the initial public
release (2026-09-02), before decision records existed (HK-31); moved here by HK-33 with the
context reconstructed from the backlog and the docs. The bullet read:

> The default coverage experience uses a persistent fast cache; deeper completeness analysis is
> requested explicitly rather than repeated on every page load.

Why, as the docs record it: the daily library covers about 18,000 US common stocks
(`README.md`), and universe-sized work "must stay linear in memory" (`AGENTS.md`, Engine and
data rules). BT-603 wants coverage by year "without loading an entire large file into the
browser"; BT-606 wants scans "incremental by default" in a worker, with "a failed scan
preserves the last valid catalog snapshot"; BT-604 wants coverage snapshots timestamped "so a
later rescan cannot rewrite what an old run observed". The original Data Coverage workspace
audited the selected sessions on request (`docs/LOCAL_UI.md`), which is the deep analysis,
not something to redo on every visit.

## Decision

Opening the Data workspace reads a persisted coverage cache (the catalog's last scan) and
renders from it. Anything that walks the files again, a full rescan or a completeness audit
of missing and duplicate rows, is an explicit action with its own progress and a timestamp
on its result.

## Alternatives

Not recorded when the bullet was locked. Inferred from the docs, written now:

- Compute coverage on every page load: always current, and minutes of file reading on a
  universe-sized library every time the page opens.
- Always run the full completeness audit in the scan: the cache would be honest to the row,
  at the cost of scans that never finish before the next data update.

## Consequences

The cache has to say when it was taken and the page has to show it; a stale cache is a
visible state, not a silent one. Rescan and audit are separate actions (BT-606) that run in
a worker and report progress. A run's own coverage artifact (0011) is never derived from the
cache.

## What would show this was wrong

A run that failed or misled because the cache was stale and nothing said so, or rescans that
take so long the cache is never current when the owner looks. Either reopens the split
between the cache and the audit, in a new record.
