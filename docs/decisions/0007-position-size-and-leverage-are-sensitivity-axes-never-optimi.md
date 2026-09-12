# 0007 — Position size and leverage are sensitivity axes, never optimizer-selected by default

Status: accepted
Date: 2026-09-11

## Context

Locked in `docs/PRODUCT_BACKLOG.md` under "Locked product decisions" at the initial public
release (2026-09-02), before decision records existed (HK-31); moved here by HK-33 with the
context reconstructed from the backlog and the docs. The bullet read:

> Position size and leverage are editable and eligible for deliberate sensitivity studies, but are
> not optimizer-selected by default.

Why, as the docs record it: BT-404 wants to compare sizing assumptions "without letting an
optimizer quietly select the riskiest result"; BT-303 lists position size, gross leverage,
costs, research dates, and holdout definitions as "default to not optimizer-eligible", with
size or leverage allowed "only in a clearly labeled sensitivity study".
`docs/OPEN_SOURCE_ARCHITECTURE_DIRECTION.md` says transaction costs, data membership, research
dates, and the final holdout "must not become accidental optimization variables". Leverage
scales any positive-expectancy result, so an unconstrained optimizer always picks the most.

## Decision

Position size and gross leverage can be edited on any run and swept in a study labelled
Sensitivity, but no optimizer, sweep ranking, or walk-forward selection chooses them unless
the study says so in its label. They are excluded from ordinary optimization by default.

## Alternatives

Not recorded when the bullet was locked. Inferred from the docs, written now:

- Treat them as ordinary parameters: simplest, and the reason the rule exists; the optimizer
  would report the highest leverage as the best configuration every time.
- Forbid sweeping them at all: loses the honest question "how much does this depend on
  size", which BT-404 wants answered with normalized risk metrics.

## Consequences

The Sweeps workspace and any future optimizer treat size and leverage as ineligible unless
the run is labelled a sensitivity study; walk-forward selection never picks them. Sensitivity
reports normalize risk metrics and show gross and net exposure histories (BT-404). A study
that wants leverage as an axis has to say so where the reader sees it.

## What would show this was wrong

A research question where leverage is the thing being chosen (a volatility-targeting rule
with the target as a real parameter) that the labelled sensitivity study proves too blunt for,
or the label being applied to every study to get around the rule. Either reopens this.
