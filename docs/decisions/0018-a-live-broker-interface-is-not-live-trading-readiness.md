# 0018 — A live-broker interface is not live-trading readiness

Status: accepted
Date: 2026-09-11

## Context

Locked in `docs/PRODUCT_BACKLOG.md` under "Locked product decisions" at the initial public
release (2026-09-02), before decision records existed (HK-31); moved here by HK-33 with the
context reconstructed from the backlog and the docs. The bullet read:

> The presence of a live-broker interface is not live-trading readiness. Connectivity,
> reconciliation, risk controls, and operational monitoring require separate acceptance.

Why, as the docs record it: `docs/STRATEGY_SDK_ARCHITECTURE.md` ships `LiveBrokerAdapter` as
"an interface only in this milestone; no order can currently reach a brokerage account", and
its deferred list (QQQ shadow mode, IBKR paper/live data, order routing, reconciliation,
safety controls) stays separate "so the presence of a live interface cannot be mistaken for
brokerage connectivity". BT-803's acceptance says "no live brokerage connection or order
routing is implied by this story"; BT-807 (deferred) names the gates: shadow mode first, then
paper with feed health, reconciliation, duplicate-order prevention, kill switches, restart
recovery, and an operational screen, and "live orders remain impossible until the explicit
paper and safety gates are accepted".

## Decision

Having a live adapter type in the engine means nothing about being able to trade. Shadow
mode, paper trading, and live orders are three separately accepted stages, each with its own
story and proof, and nothing in the code path can route a real order until the last is
accepted.

## Alternatives

Not recorded when the bullet was locked. Inferred from the docs, written now:

- Treat a working adapter as go-live: the usual way a backtester becomes a trading system by
  accident, and the reason the bullet exists.
- Never ship the interface until it is complete: leaves the boundary (0017) untested against
  a real shape and the SDK unable to prove it is broker-neutral.

## Consequences

The interface can exist, compile, and be tested without any of it implying readiness. Each
stage of BT-807 needs its own acceptance, and the console must say which stage a strategy is
at. Anyone reading `LiveBrokerAdapter` reads this record before assuming anything.

## What would show this was wrong

The gates proving so heavy that the interface rots unused for a year while trading happens
elsewhere, or a paper run showing the adapter is the wrong boundary for a real broker's
event stream. Either reopens the staging, in a new record.
