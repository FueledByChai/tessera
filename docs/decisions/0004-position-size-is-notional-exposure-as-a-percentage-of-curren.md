# 0004 — Position size is notional exposure as a percentage of current account equity

Status: accepted
Date: 2026-09-11

## Context

Locked in `docs/PRODUCT_BACKLOG.md` under "Locked product decisions" at the initial public
release (2026-09-02), before decision records existed (HK-31); moved here by HK-33 with the
context reconstructed from the backlog and the docs. The bullet read:

> Position size is notional exposure as a percentage of current account equity.

Why, as the docs record it: BT-401 wants every position target "expressed as a percentage of
current account equity so sizing remains understandable across one or many selected
instruments". `docs/OPEN_SOURCE_ARCHITECTURE_DIRECTION.md` (Portfolio semantics) separates
"target notional per position as a percentage of current portfolio equity" from the gross
cap (0005), and notes that notional exposure "is distinct from an exchange's margin-leverage
setting". The SDK already works this way: `Size::Default` uses the run's `position_percent`
(`docs/ADDING_A_STRATEGY.md`), and the run form's position size control is the same number
for a one-symbol run and an 18,000-symbol universe.

## Decision

A position target is a percentage of the account's current equity at decision time, and it
means notional exposure: 100% is notional equal to equity, 200% is 2.0x. It is not a share
count, a dollar amount, or an exchange margin setting.

## Alternatives

Not recorded when the bullet was locked. Inferred from the docs, written now:

- Fixed unit or share counts: familiar from single-instrument scripts, but meaningless across
  instruments of different prices and useless for a universe run.
- Fixed dollar notional: stable across instruments, but it stops tracking equity, so a run
  that halves its capital keeps sizing as if it had not.
- The exchange's margin-leverage setting as the size: `docs/OPEN_SOURCE_ARCHITECTURE_DIRECTION.md`
  keeps margin and liquidation rules out of the first implementation deliberately.
- Weighting modes (equal weight among active signals, volatility-weighted): listed there as
  candidate allocation modes for later, not as the meaning of a position size.

## Consequences

Sizing reads the configured decision-time equity with no look-ahead (BT-401), so every fill's
notional depends on the equity path before it. Rounding to whole units is asset-aware and
recorded. Every strategy, preset, and study speaks the same unit, and the position size
control means the same thing on every run form. A strategy that sizes in units must convert.

## What would show this was wrong

A class of strategy whose sizing cannot be stated as a share of equity (contract-based
futures sizing, volatility targeting in units) that has to fight the run form to express
itself, or the owner routinely converting a dollar figure into a percentage by hand before
every run. Either would be a new record, superseding this one.
