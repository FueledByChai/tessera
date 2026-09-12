# 0022 — A provider download job is budgeted, exclusive per source, and fails closed on its folder

Status: accepted
Date: 2026-09-11

## Context

EODHD meters requests per day (the owner's plan: 100,000, reset at 00:00 UTC; intraday
requests cost five). The Python pipeline stops at a `--call-budget` and refuses a bulk day
that omits SPY or is short of rows, and it takes a lock so its jobs do not overlap. The native
jobs (BT-1204 to BT-1206) need the same rules stated once. The owner chose (grill-me,
2026-09-11) that jobs are governed by the usage the provider reports, not merely shown it;
that a second job on a busy source, a job whose folder is missing, unwritable, or under an
unmounted root, and a rejected token are refused outright; and that the console never
deletes data files. BT-712's rule that an updater must never create a folder at an absent
mount or fall back to the internal disk applies here before Epic G lands.

## Decision

Before it starts, a job estimates its calls and refuses to start when the mandatory part
(the increment) exceeds the provider's remaining requests minus a per-source reserve (default
five percent of the daily limit); the optional part (backfill) stops at the reserve, records
that it did, and resumes on the next run. One job runs per source at a time; a second is
refused naming the running one, and a due schedule on a busy source is skipped, never queued
twice. A job never creates its dataset folder and never writes anywhere else: a missing,
unwritable, or unmounted folder is refused before the first call. Every file is written as a
part file and renamed, a bulk day that omits the calendar symbol or is under the dataset's
minimum row count is refused whole, and no job or console action deletes a data file.

## Alternatives

- Show usage and warn only: simpler, but a nightly that runs into the provider's refusal
  leaves a half-done dataset and burns the calls the next job needed.
- Run to the provider's refusal and treat the error as the stop: the same outcome with the
  provider deciding when, and no reserve left for the console's own reads.
- Allow concurrent jobs per source (one per dataset): faster nights, but they would share one
  budget and one rate limit and the scan cache would see each other's part files.

## Consequences

Every job goes through one `CallBudget` and one per-source lock; the estimate is part of each
job's record and the console can say why a job did not start. Backfills of a large dataset
take several nights by design. A dataset on a drive that is not mounted stays Unavailable
rather than silently refilling the internal disk. The reserve is a field on the source card;
the minimum row count is a field on the dataset.

## What would show this was wrong

Nightlies that never finish their increments within the reserve, or the per-source lock
holding up a small dataset every night behind a large one. Then the budget splits per dataset
or the lock narrows, in a superseding record.
