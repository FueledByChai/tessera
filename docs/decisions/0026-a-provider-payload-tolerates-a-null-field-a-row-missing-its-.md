# 0026 — A provider payload tolerates a null field; a row missing its key is dropped and counted

Status: accepted
Date: 2026-09-15

## Context

Expanding the GBOND exchange on the Inventory page failed outright: `malformed provider
response: /api/exchange-symbol-list/GBOND: invalid type: null, expected a string at line 1
column 6006`. The adapter's payload structs declare their string fields as `String` with
`#[serde(default)]`, and `serde`'s `default` covers a field that is **absent** — not one that is
explicitly `null`. So a single null anywhere in a six-thousand-row listing rejected the entire
exchange, and the same hazard sits in every payload the adapter parses: `ListingRow` (6 string
fields), `SplitRow` (4), `ExchangeRow` (3), `BulkRow` (2), `BarRow` (1). A provider that omits one
field on one instrument could therefore cost a whole exchange on the page, and in the unattended
jobs a whole night's download.

The repository already had the right instinct for numbers — `BarRow`'s comment says "a row
missing a price is dropped by the caller: nothing is invented for it" — but it had never been
stated as a rule for the payloads generally, and the string fields were left strict by omission
rather than by choice.

## Decision

A `null` and an absent field are the same thing: **the provider did not supply that value**, and
neither is a reason to fail a response. Beyond that, what a null means depends on what the field
is for:

- A row whose **key** is null or absent is dropped, with a reason, and counted — `code` on a
  listing, bulk, or split row; `date` on a bar or split row; `split` on a split row. A row with no
  key names nothing and cannot be used.
- A **descriptive** field that is null or absent becomes blank — `name`, `country`, `currency`,
  `Type`, `exchange`, `exchange_short_name`. The row is still usable without it.
- A body that is **not a JSON list** — an object, `null`, an HTML page, a truncated response —
  still fails loudly as `malformed provider response`. A real outage is never rendered as an empty
  exchange.
- An **empty string** is not a null. An empty `code` drops the row; an empty `name` is kept as a
  blank name.

Dropped rows are never silent. The trait's listing call returns the rows **and** the rows it
dropped, each with its reason, so a synchronous refresh can show the count on the exchange row
that asked for it; the download jobs keep reusing the `outcome.skipped` path they already use for
a bar missing its price, so the job record and its log carry the loss.

## Alternatives

- Blank every null and drop nothing: keeps every row, but an empty-symbol instrument enters the
  catalog and the instrument index, which is worse than losing it.
- Drop any row carrying any null: never invents a blank, but discards a whole instrument because
  the provider omitted its name — precisely the exchanges most likely to be sparse.
- Tolerate only the fields known to have been null so far: the smallest diff, and the same failure
  returns the next time a provider omits a different field, in a job nobody is watching.
- Keep a bare dropped count instead of the rows and reasons: smaller signature, but the job record
  and the log could then only say how many were lost, never what was wrong with them.

## Consequences

The `Provider` trait's listing call no longer returns `Vec<Listing>` alone; it carries the dropped
rows with their reasons, which is a signature change every adapter and its callers share. Adding a
field to a payload struct is now a decision — key or description — rather than a default. The
console gains a dropped-row count on the exchange row, and the Updates table gains skipped entries
it did not have. Nothing here weakens the loud failure for a body that is not a list, so a genuine
provider outage still reads as one.

## What would show this was wrong

A provider whose payloads use `null` to mean something other than "not supplied" — an explicit
"no value" that should be refused or surfaced rather than blanked, or a null that distinguishes
two states a blank cannot. Then the rule needs a per-field reading of null rather than one rule
for the adapter, in a superseding record.
