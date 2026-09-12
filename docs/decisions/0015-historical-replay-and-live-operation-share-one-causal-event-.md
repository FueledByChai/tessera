# 0015 — Historical replay and live operation share one causal event contract

Status: accepted
Date: 2026-09-11

## Context

Locked in `docs/PRODUCT_BACKLOG.md` under "Locked product decisions" at the initial public
release (2026-09-02), before decision records existed (HK-31); moved here by HK-33 with the
context reconstructed from the backlog and the docs. The bullet read:

> Historical replay and future live operation use the same causal market and broker event contract;
> strategies must not contain separate historical and live signal logic.

Why, as the docs record it: `docs/STRATEGY_SDK_ARCHITECTURE.md` states it as the product
goal: "Strategy code must not branch on whether it is running historically or live.
Historical files and a future broker feed are adapters that emit the same market events."
BT-801 (complete) defines the causal events and forbids a callback from seeing "a bar's high,
low, close, or volume at that bar's open"; BT-802 (complete) gives the engine one
deterministic replay loop so "each strategy does not reimplement time sequencing". The
pre-SDK strategies each owned their replay plumbing (BT-808), which is the duplication this
rule ends. `docs/ADDING_A_STRATEGY.md` says the same callbacks "are intended for both
historical replay and future live feeds".

## Decision

There is one event contract: session start, bar open, bar close, session end, and broker
events, delivered causally. A strategy implements its callbacks once; the historical engine
and any live adapter are feeds behind that contract, and a strategy never asks which one it
is running under.

## Alternatives

- Separate backtest and live code paths per strategy (what the legacy strategies had):
  quick for one strategy, and the thing that makes a live result differ from its backtest in
  ways nobody can explain.
- Vectorized backtests over whole arrays: much faster for research, but there is no live
  equivalent, so the strategy is rewritten for production and the parity is lost.

## Consequences

The engine owns the loop, symbol ordering, and follow-up order processing; strategies cannot
peek ahead, and the parity checks (`examples/expected/`, the ETF ORB byte-identical
comparison in BT-806) hold because of it. Replay is slower than a vectorized run. A live
adapter has to reproduce the event sequence, including partial sessions, before it counts.

## What would show this was wrong

A live feed whose real sequence (partial bars, out-of-order ticks, late corrections) cannot
be mapped onto the contract without strategy-side branching, or research that needs
vectorized speed so badly that a second, non-causal path is written anyway. Then the
contract widens or splits, in a new record.
