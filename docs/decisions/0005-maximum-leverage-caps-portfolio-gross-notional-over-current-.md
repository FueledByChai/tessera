# 0005 — Maximum leverage caps portfolio gross notional over current account equity

Status: accepted
Date: 2026-09-11

## Context

Locked in `docs/PRODUCT_BACKLOG.md` under "Locked product decisions" at the initial public
release (2026-09-02), before decision records existed (HK-31); moved here by HK-33 with the
context reconstructed from the backlog and the docs. The bullet read:

> Maximum leverage is a cap on total portfolio gross notional divided by current account equity.

Why, as the docs record it: BT-402 defines gross exposure as "the sum of absolute notionals
divided by current equity" and asks for portfolio-level limits "enforced when several
instruments signal simultaneously". `docs/OPEN_SOURCE_ARCHITECTURE_DIRECTION.md` lists
"maximum gross exposure as a multiple of current portfolio equity" as one of the separate
portfolio controls, next to (not instead of) a net limit and position-count limits. The
simulated broker enforces it today as the buying-power guard: "open entry notional may not
exceed `max_gross_exposure` times total equity", with the run defaulting to the manifest's
`.max_gross_exposure(x)` (`docs/ADDING_A_STRATEGY.md`); broker guards are not optional
(`AGENTS.md`, Engine and data rules).

## Decision

"Maximum leverage" in Tessera is one number: the cap on total portfolio gross notional
divided by current account equity. It is checked by the broker at fill time against all open
positions, not by the strategy and not per position.

## Alternatives

Not recorded when the bullet was locked. Inferred from the docs, written now:

- A net-exposure limit as the only cap: `docs/OPEN_SOURCE_ARCHITECTURE_DIRECTION.md` keeps
  net as a separate control for long/short books; gross is the one that bounds what the
  account can lose, so it is the one called leverage.
- Position-count caps in place of a notional cap: they exist as entry limits (max open
  positions, max entries per day) but bound nothing when positions differ in size.
- Simulating exchange margin and liquidation: explicitly not claimed by the first
  implementation; the cap is a portfolio rule, not a margin model.

## Consequences

When requested orders would exceed the cap the engine behaves deterministically (rejects,
clips, or defers, recorded in an allocation audit, BT-402); a strategy cannot lever the
account past the cap by accident. Every run manifest carries the cap. A leverage-hungry
strategy declares what it needs in its manifest rather than assuming it.

## What would show this was wrong

A long/short strategy whose risk is misstated by gross (a hedged book shown as 2.0x levered
while its net is near zero) that the separate net limit does not resolve, or a need to model
margin calls and liquidation for a venue, would reopen this in a new record.
