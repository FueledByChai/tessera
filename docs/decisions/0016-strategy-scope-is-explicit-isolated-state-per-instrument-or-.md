# 0016 — Strategy scope is explicit: isolated state per instrument or one portfolio instance

Status: accepted
Date: 2026-09-11

## Context

Locked in `docs/PRODUCT_BACKLOG.md` under "Locked product decisions" at the initial public
release (2026-09-02), before decision records existed (HK-31); moved here by HK-33 with the
context reconstructed from the backlog and the docs. The bullet read:

> Strategy scope is explicit: isolated state per instrument or one synchronized portfolio instance.

Why, as the docs record it: BT-804 (complete) wants "the same SDK to support an independent
rule on each ticker or a synchronized cross-asset portfolio rule". `docs/STRATEGY_SDK_ARCHITECTURE.md`
(Strategy scopes) describes both: per instrument, "one isolated strategy state for each
selected instrument", so one source runs on QQQ alone or on QQQ, SPY, IWM, and DIA
independently; portfolio, "one strategy instance receives the synchronized slice for every
selected instrument", the scope for ranking, relative value, shared capital, and top-N. "The
host rejects a strategy whose declared scope does not match its configured host", and
instrument selection stays run configuration, not strategy code. An `EventStrategy` declares
`PerInstrument` or `Portfolio` (`docs/ADDING_A_STRATEGY.md`).

## Decision

Every strategy declares its scope. Per-instrument scope gives each selected symbol its own
state and filtered events; portfolio scope gives one instance the synchronized slice of all
of them. There is no implicit scope and no mixing inside one strategy; a mismatch between the
declared scope and the host is an error at start.

## Alternatives

Not recorded when the bullet was locked. Inferred from the docs, written now:

- Implicit scope, one instance always, the strategy looping over symbols itself: every
  strategy re-implements isolation and one bug in that loop leaks state between tickers.
- Per-instrument scope only: simpler engine, but cross-sectional and shared-capital
  strategies have nowhere to live.

## Consequences

Two hosts, each with deterministic tests; the one-file SDK defaults to per-instrument and
the lower-level `EventStrategy` carries portfolio scope. Cross-sectional work is portfolio
scope by definition. A per-instrument strategy shares the account (the broker guards and the
gross cap, 0005) but never another symbol's state.

## What would show this was wrong

A strategy that needs both at once, per-instrument signals plus a portfolio-level allocation
step, with no clean home in either scope, appearing more than once. Then a third scope or a
composition rule supersedes this.
