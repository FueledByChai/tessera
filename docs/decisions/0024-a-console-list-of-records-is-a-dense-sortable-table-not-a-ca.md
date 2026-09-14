# 0024 — A console list of records is a dense sortable table, not a card grid

Status: accepted
Date: 2026-09-14

## Context

The Costs page (grill-me, 2026-09-14, on cleaning it up) rendered its five seeded profiles
plus every custom one as a grid of cards, each showing a name, a model, and two or three
values inside a `<dl>` whose shape changed with the model — bps here, ticks and dollars
there. Reading the library meant reading five cards of different shapes, and comparing two
profiles meant holding one in your head. The console already had the opposite convention
without ever having written it down: the strategies catalog is a nine-column table
(BT-1102), the Data page's datasets and schedules are tables (BT-1203, BT-1207), and run
history is a dense table. The Costs page was the last list drawn as cards.

## Decision

A list of records in the console is one dense, sortable table with one row per record and
the comparable values in their own columns. A cell the record's kind does not use shows a
dash, never a zero and never nothing, so every record stays one row and columns line up for
comparison. Cards are not used for lists. A record's identifiers that are for machines
rather than for reading (an id, a path) ride as a tooltip on the column that names it, not
as a column of their own. A list keeps the panel's empty state and puts the action that
creates a record in the panel's title bar.

## Alternatives

- Cards sized to their content: friendlier for two or three records, and the model-specific
  `<dl>` needs no dash rule — but a card cannot be sorted, one card's values do not line up
  with the next's, and a grid of twenty custom profiles is twenty screens tall.
- A table per kind (one for all-in bps, one for fixed tick, one for costs off): no dash
  rule, and each table shows only its own columns — but a reader scans three tables to see
  the library, and a sort across the whole set is impossible.
- One wide "assumptions" column holding each model's values in words ("1 tick +
  $0.005/unit each side"): nothing dashes and nothing is unreadable — but nothing lines up
  either, which was the point of the change.

## Consequences

Every new list page is a table, and a page that already has one keeps its conventions:
dense rows under 1500 px, headers that sort, no horizontal page scroll at 1280 px because
the table scrolls inside its own wrapper. A list page's layout-check pass measures a table,
not a grid. A value the record's kind does not have must be rendered as a dash on purpose,
so a model that gains a field later needs a rendering rule rather than appearing blank.
Decision 0003 still governs the other half of such a page: the form that creates a record
lives in a dialog.

## What would show this was wrong

A list whose records really do differ in shape — a queue where each kind wants a different
set of controls, say — becomes a table of dashes where most cells are empty on any given
row: then cards, or a table per kind with a filter above it, replace the single table, in a
superseding record.
