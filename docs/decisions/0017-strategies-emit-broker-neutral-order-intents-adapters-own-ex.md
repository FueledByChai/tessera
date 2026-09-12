# 0017 — Strategies emit broker-neutral order intents; adapters own execution and accounting

Status: accepted
Date: 2026-09-11

## Context

Locked in `docs/PRODUCT_BACKLOG.md` under "Locked product decisions" at the initial public
release (2026-09-02), before decision records existed (HK-31); moved here by HK-33 with the
context reconstructed from the backlog and the docs. The bullet read:

> Strategies emit broker-neutral order intents; execution, positions, cash, leverage, and costs are
> owned by broker and portfolio adapters.

Why, as the docs record it: BT-803 (complete) puts execution "behind an adapter so strategy
logic does not calculate fills or account balances"; the simulated adapter "owns orders,
bracket execution, costs, positions, realized equity, and trades".
`docs/STRATEGY_SDK_ARCHITECTURE.md` (Broker boundary) has `BrokerAdapter` own market-event
processing, submission, fills, positions, and the portfolio snapshot, and "a simulated broker
and a future live broker consume the same order intents". `docs/ADDING_A_STRATEGY.md` says a
strategy "must not load vendor files, inspect future bars, calculate broker fills, maintain
the account", and the broker guards (buying power, solvency, commission cap, tick floor, min
price) "are not optional" (`AGENTS.md`). The legacy engines each carried their own fill and
accounting code, which is what BT-808 retires.

## Decision

A strategy's only output is `OrderIntent` values: entries, exits, brackets, stop changes,
sized in the run's units. Fills, positions, cash, leverage, and costs are computed by the
broker and portfolio adapters, once, with the guards applied there. No strategy reads or
writes account state except through the snapshot it is handed.

## Alternatives

- Strategies compute their own fills and P&L (the legacy engines): every strategy models
  execution slightly differently, the guards are optional in practice, and parity between
  strategies means nothing.
- A thin adapter with strategy-supplied fill hooks: keeps exotic execution close to the
  strategy, at the cost of the same divergence through a side door.

## Consequences

One fill model and one cost library, frozen into every run manifest; the guards apply to
every strategy equally; a live adapter slots in behind the same intents (0015). A strategy
cannot express an order type `OrderIntent` does not have, so the intent vocabulary grows
only by engine change with parity proof.

## What would show this was wrong

An order type more than one strategy needs (exchange-native conditional orders, iceberg,
venue-specific time-in-force) that cannot be stated neutrally, or a fill model a strategy
must own to be honest (an auction, a market where the strategy is the maker). Then the
boundary moves, in a new record.
