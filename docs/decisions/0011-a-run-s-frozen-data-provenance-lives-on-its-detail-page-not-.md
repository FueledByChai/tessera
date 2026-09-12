# 0011 — A run's frozen data provenance lives on its detail page, not in the Data inventory

Status: accepted
Date: 2026-09-11

## Context

Locked in `docs/PRODUCT_BACKLOG.md` under "Locked product decisions" at the initial public
release (2026-09-02), before decision records existed (HK-31); moved here by HK-33 with the
context reconstructed from the backlog and the docs. The bullet read:

> A backtest's frozen data provenance and signal-session coverage belong on that run's detail page,
> not in the primary Data inventory.

Why, as the docs record it: BT-607 wants "the exact data used by a particular backtest
available with that result without confusing it with my current data inventory", removes the
`Completed structured run` selector from the inventory, and adds a `Data used` section to each
run's detail. BT-104 freezes dataset snapshots, resolved instruments, and coverage into the
run so later catalog changes "do not alter an existing run". The run detail page already
carries the frozen configuration and coverage warnings (`docs/LOCAL_UI.md`), so the coverage
artifact was already the run's; only its navigation lived on the wrong page. This is the
other half of 0010.

## Decision

What a run saw (source, dataset snapshot, resolution, resolved instruments, requested range,
actual coverage, missing symbol-dates) is shown on that run's detail page, from the run's own
immutable coverage artifact. The Data inventory may link to affected runs but never uses a
run as its navigation.

## Alternatives

Not recorded when the bullet was locked. Inferred from the docs, written now:

- Keep the run selector on the Data page (the original): one place for all coverage, but a
  reader cannot tell "what I have" from "what run 42 had".
- A third page for provenance: one more place to look for something that belongs with the
  result it explains.

## Consequences

Run detail grows a `Data used` section; coverage artifacts stay immutable and keep driving
partial-data warnings; the inventory loses the run selector. Cross-run questions ("which runs
were hurt by this symbol's gap") have no home until someone asks for one.

## What would show this was wrong

A recurring cross-run coverage question that the owner answers by opening runs one at a time,
or the `Data used` section going unread because the coverage warning on the run already says
enough. Either reopens where provenance is shown, in a new record.
