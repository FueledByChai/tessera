# 0008 — Signal parameters declare whether they are editable and optimizer-eligible

Status: accepted
Date: 2026-09-11

## Context

Locked in `docs/PRODUCT_BACKLOG.md` under "Locked product decisions" at the initial public
release (2026-09-02), before decision records existed (HK-31); moved here by HK-33 with the
context reconstructed from the backlog and the docs. The bullet read:

> Signal parameters may declare whether they are editable and optimizer-eligible.

Why, as the docs record it: BT-303 wants a strategy author to "state which parameters are
reasonable to search and over what domain so research tools do not optimize arbitrary
configuration values", with "editable and optimizer-eligible" as separate flags and a
recommended optimization domain narrower than the validation domain. BT-301 (complete) put
typed parameter declarations in the manifest with defaults, bounds, steps, and tiers;
`docs/OPEN_SOURCE_ARCHITECTURE_DIRECTION.md` adds that `optimizable` "means that the parameter
may be included in a sweep or walk-forward search; it does not mean the application
automatically optimizes it". Today's Sweeps workspace can grid "any numeric manifest
parameter" (`docs/LOCAL_UI.md`), which is what the flags are meant to narrow.

## Decision

The parameter manifest is where a strategy says, per parameter, whether a user may edit it on
a run and whether a sweep or walk-forward search may include it, with a search domain when it
may. The UI, presets, sweeps, and optimizer read those flags; nothing else decides.

## Alternatives

Not recorded when the bullet was locked. Inferred from the docs, written now:

- Every numeric parameter sweepable (the status quo of the Sweeps workspace): no authoring
  work, but the optimizer can search a warm-up length or a calendar offset as if it were a
  signal parameter.
- A research configuration listing the searchable parameters outside the manifest: keeps the
  strategy file smaller, but splits one contract across two places that drift.

## Consequences

The manifest grows two flags and an optional recommended domain per parameter; the run form
renders non-editable parameters read-only; sweeps refuse ineligible parameters; the author
owns the domains. Presets stay valid across the flags since they hold values, not flags.

## What would show this was wrong

Authors marking every parameter eligible so the flag stops meaning anything, or research tools
that never consult it. Then eligibility moves to the research definition, in a new record.
