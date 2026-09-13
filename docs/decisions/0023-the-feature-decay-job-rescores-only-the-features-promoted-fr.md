# 0023 — The feature-decay job rescores only the features promoted from a study

Status: accepted
Date: 2026-09-13

## Context

WB-17 rescores the promoted features nightly so their IC can be watched for decay. It says a
promoted feature "whose frozen study no longer exists" is skipped as `study missing`, but WB-16
recorded only the study's *shape* on the promotion — `promoted_grid`, `promoted_symbols`,
`promoted_horizon`, `promoted_target`, and `baseline_ic` — and no study id, so nothing names a
row that could go missing. WB-16 also said a promotion made from the library alone "leaves them
null for WB-17's first run to fill", which reads as though such a feature is still scored.

The two readings disagree about what happens to a feature promoted from the library panel with no
study behind it: skipped, or scored against something.

## Decision

The provenance WB-16 recorded **is** the frozen study. A promotion that carries a grid, symbols,
and a horizon is rescored on exactly that; a promotion with no provenance — one made from the
library alone — has no study to rescore against and is skipped with the reason `study missing`,
never scored. No study id is added to `feature_presets`. `baseline_ic` stays nullable because a
promotion made from a study sets it on the job's first run against that feature.

## Alternatives

- Add `promoted_study_id` to `feature_presets` and skip when that `studies` row is gone: the most
  literal reading of "no longer exists", and it would notice a study deleted after promotion. It
  also makes a library promotion permanently `study missing` until someone re-promotes it, which
  is the same behaviour as the decision above for a new column and a foreign key.
- Score a library promotion on a configured default grid (daily, the catalog's symbols) and fill
  its baseline from that first run: the closest reading of "for WB-17's first run to fill", but it
  invents a study the user never ran, and the resulting IC would be compared against nothing.

## Consequences

A promoted feature is only watched once it came from a study, so promoting from the library
remains a way to keep an expression without claiming a baseline. The job needs no join to
`studies` and no new column. The study engine scores no cell at all under a hundred
observations, so a twenty-session window over a single symbol produces no cell and the feature is
skipped; a universe of a few symbols clears it. WB-18 shows the history beside the feature, and
it will have to render `skipped` rows with their reason rather than a number.

## What would show this was wrong

The owner promotes a feature from the library and expects it to appear in the decay history with
a baseline, or wants the history to survive a study being deleted after promotion — both of which
need the study id. Then a superseding record adds one and the library promotion is given a grid.
