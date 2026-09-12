# 0006 — Fixed position targets: 100% is 1.0x of equity, and unused capacity is not reassigned

Status: accepted
Date: 2026-09-11

## Context

Locked in `docs/PRODUCT_BACKLOG.md` under "Locked product decisions" at the initial public
release (2026-09-02), before decision records existed (HK-31); moved here by HK-33 with the
context reconstructed from the backlog and the docs. Three bullets spelled out the arithmetic
that follows from 0004 and 0005 as worked examples; BT-403 carries the same three as its
acceptance criteria, so they are one record. The bullets read:

> A 100% position target means 1.0x account equity, regardless of how many instruments were selected.

> With two 100% positions and a 2.0x gross limit, two simultaneous signals produce 2.0x gross
> exposure. One signal produces 1.0x; unused capacity is not reassigned automatically.

> A single instrument may target 200% with a 2.0x gross limit.

Why, as the docs record it: `docs/OPEN_SOURCE_ARCHITECTURE_DIRECTION.md` (Portfolio semantics)
gives the example of two perpetual-futures instruments each targeting 100% with a 2.0x gross
cap and "active-position rescaling disabled": both signal, each gets 1.0x and gross is 2.0x;
one signals, gross "remains 1.0x; the engine does not silently double that position". BT-403
states the user story: "unused exposure remains unused unless I explicitly choose a rescaling
rule", and "the default engine never enlarges a position merely because another instrument has
no signal".

## Decision

A position target is fixed per instrument and does not depend on how many instruments were
selected or how many are currently signalling: 100% is 1.0x of equity whether one or fifty
symbols are in the run. Capacity left under the gross cap by instruments without a signal
stays unused. A single instrument may target the whole cap (200% under 2.0x). Any rescaling
of active positions is an explicit, named run option, never the default.

## Alternatives

- Equal weight among currently active signals: the natural "use the capital" mode, but it
  makes one instrument's size depend on another's silence, which hides risk in the count of
  signals. `docs/OPEN_SOURCE_ARCHITECTURE_DIRECTION.md` lists it as a candidate allocation
  mode to be chosen explicitly.
- Fixed sleeves, ranked top-N with a maximum count, volatility weighting: the same doc lists
  these as explicit modes; each must appear in the run manifest with the inactive cash.
- Dividing the target by the number of selected instruments: rejected by the first bullet;
  a universe of 18,000 symbols would size each position at nothing.

## Consequences

Idle capital is common and visible: a two-instrument run at 100% each shows 1.0x gross most
of the time. Results are comparable across runs that differ only in the symbol list. The
engine needs a rescaling mode only when someone asks for one, and that mode is a manifest
field, not a side effect. The parity baselines in `examples/expected/` assume fixed targets.

## What would show this was wrong

Most runs being configured with a rescaling rule, or exposure histories showing that idle
capacity, not the signals, explains the difference between two strategies the owner is
comparing. Then the default mode changes, in a record that supersedes this one.
