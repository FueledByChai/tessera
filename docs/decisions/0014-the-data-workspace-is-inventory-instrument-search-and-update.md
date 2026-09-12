# 0014 — The Data workspace is Inventory, Instrument search, and Updates & schedules

Status: accepted
Date: 2026-09-11

## Context

Locked in `docs/PRODUCT_BACKLOG.md` under "Locked product decisions" at the initial public
release (2026-09-02), before decision records existed (HK-31); moved here by HK-33 with the
context reconstructed from the backlog and the docs. The bullet read:

> The Data workspace is organized into `Inventory`, `Instrument search`, and `Updates & schedules`.

Why, as the docs record it: BT-608 wants "inventory, instrument search, and update operations
organized clearly so broad coverage questions and operational maintenance do not compete on
one screen". Today's Data page holds all three at once: the sources inventory with file
counts and a Rescan button, the coverage workspace, the `update_command` launcher and its
schedules (`docs/LOCAL_UI.md`), and the instrument picker's index (`docs/DATA_SOURCES.md`).
Pages must fit a 13-inch laptop and open at the top (`AGENTS.md`, UI conventions), which one
screen with all of it cannot do.

## Decision

The Data workspace has three views and no more: `Inventory` (source cards and a filterable
dataset coverage table), `Instrument search` (one instrument, every dataset and resolution it
has), and `Updates & schedules` (rescans, provider updates, their schedules and their state).

## Alternatives

Not recorded when the bullet was locked. Inferred from the docs, written now:

- One screen (the status quo): nothing to navigate, and the inventory question, the "does
  IWM have one-minute bars" question, and "did last night's update run" all scroll past each
  other.
- More pages (freshness, sources, coverage each on its own): more places to look for what
  is one workspace.

## Consequences

Three views to build and keep under the layout check; instrument search keeps its query and
selection when the owner navigates away and back (BT-608); update actions show source, scope,
and state in their own view. Freshness (BT-605) belongs to Inventory, not to Updates.

## What would show this was wrong

`Updates & schedules` opened so rarely it belongs in settings, or the owner needing the
search box inside Inventory on most visits. Then the views merge or split again, in a new
record.
