# 0003 — Run configuration is edited in a modal dialog; a page shows a summary and a Run button

Status: accepted
Date: 2026-09-11

## Context

The strategy page (grill-me, 2026-09-11, on cleaning up the strategy pages) stacked
Production rules, Presets, and a Configure immutable run form of four five-column grids
above the Historical runs table, so launching a run and reviewing history, which the owner
does equally often, both needed scrolling. The console had no dialog of any kind; every
form lived on its page. Three shapes were on the table: folds, tabs, or a dialog.

## Decision

Run configuration is edited in a modal dialog opened from the page (Configure run). The
page itself shows a summary strip of the current configuration (identity, data, limits,
parameters, preset) with a Run button, so a repeat run needs no dialog, and the history
table sits directly below. The dialog is one form with labelled rows, no tabs; Escape or
Cancel closes it and keeps the edits in memory until the page changes; Run inside it
launches. The same pattern applies to any later page whose form crowds out its results.

## Alternatives

- Collapsible sections with the parameters open: one page, no new component, but the open
  section still pushes history down and the closed ones still cost a bar each.
- Tabs (Run, Rules, History): nothing folds, but comparing a run with the form takes two
  clicks and the tab bar is one more row.
- Leave the order and only shrink controls: not enough; the four grids remain.

## Consequences

The console gains a dialog component in the terminal look (a native dialog element, amber
title bar, the same 42 px controls); the layout check opens it at 1280 and 1440 px and
fails when it passes the viewport. Presets move inside the dialog. Production rules fold on
the page, closed by default. One more click for a new configuration; none for a repeat run.
The summary strip must be a faithful rendering of the request the Run button sends.

## What would show this was wrong

The owner opens the dialog on most runs to read a value the summary does not show, or
keeps the dialog open while scrolling history behind it: then folds with the parameters
open replace the dialog, in a superseding record.
