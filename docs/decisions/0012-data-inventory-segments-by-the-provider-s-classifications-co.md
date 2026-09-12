# 0012 — Data inventory segments by the provider's classifications, Common Stock and ETF distinct

Status: accepted
Date: 2026-09-11

## Context

Locked in `docs/PRODUCT_BACKLOG.md` under "Locked product decisions" at the initial public
release (2026-09-02), before decision records existed (HK-31); moved here by HK-33 with the
context reconstructed from the backlog and the docs. The bullet read:

> Data inventory segmentation follows the provider's instrument classifications, with Common Stock
> and ETF kept distinct. Equity industry/sector classifications are not required initially.

Why, as the docs record it: BT-602 wants an inventory "by market, asset type, and resolution"
that can "distinguish US common stocks from US ETFs and spot FX from crypto spot or
perpetuals". The provider's catalog already carries a kind per instrument, and the service's
`classify_instrument` (`src/bin/tessera_ui.rs`) maps that kind and the symbol suffix to the
inventory's classes: Common Stock, ETF (ETC folded in), Fund, Preferred, Index, Crypto, FX,
Government bond. The stock and ETF universes are separate lists (`stocks.txt`, `etfs.txt`,
`docs/DATA_SOURCES.md`) and strategies are tested on one or the other, so the two must not
be summed. Sector and industry data is a second dataset the library does not hold.

## Decision

The inventory's segments are the provider's instrument classifications, mapped once in the
service, with Common Stock and ETF always shown as separate rows. No sector or industry
taxonomy is required for the inventory to be complete.

## Alternatives

Not recorded when the bullet was locked. Inferred from the docs, written now:

- Tessera's own taxonomy: consistent across providers, but a second thing to maintain and to
  reconcile against every provider's kinds.
- Sector and industry (GICS-like) segments from the start: useful for sector-neutral studies,
  but it needs a classification dataset the library does not have, and the inventory question
  ("do I have ETFs at one minute") does not need it.

## Consequences

Inventory rows follow the provider's kinds; a second provider with a different scheme needs
its own mapping into the same classes. The stock/ETF distinction survives every summary.
Sector grouping, when a study needs it, is a new dataset and a new record.

## What would show this was wrong

A study that needs sector neutrality or industry grouping and cannot get it from the
inventory, or two providers disagreeing on the same symbol's kind so the mapping has to
choose. Then a Tessera-owned taxonomy supersedes this.
