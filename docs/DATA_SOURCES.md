# Data sources

Tessera reads two kinds of source. Both are configured in `local.toml` at the repository root
(git-ignored; copy `local.example.toml`) and shown, with file counts, sizes, and date coverage,
on the Data page's Inventory view under **Configured library**. Provider accounts and the
datasets they keep current are registered from the same view (**Data sources**, decision
0021); see [Registering a source from the console](#registering-a-source-from-the-console).

## 1. CSV bar library (daily, 5-minute, 1-minute)

One file per symbol per resolution, named `<SYMBOL>.csv`.

| Feed | Key | Row layout |
|---|---|---|
| daily | `daily_dir` | `Date,Open,High,Low,Close,Adjusted_close,Volume` |
| 5-minute | `five_minute_dir` | `Timestamp,Gmtoffset,Datetime,Open,High,Low,Close,Volume` (UTC epoch seconds) |
| 1-minute | `one_minute_dir` | same as 5-minute |

Daily prices are adjusted with `Adjusted_close / Close`; intraday prices are raw. Regular-session
filtering uses New York time. `calendar_symbol` names the daily file whose dates define the trading
calendar for screened runs (default `SPY.US`).

`catalog_dir` holds `catalog.csv` (`Code,Name,Country,Exchange,Currency,Type,...`) plus optional
universe lists `stocks.txt` and `etfs.txt` (one symbol per line) that the run form's
`universe:stocks` and `universe:etfs` selections expand to. Sub-folders and other `.txt` lists in
the catalog folder are listed on the Data page for reference.

Optional: `freshness_file` (a JSON file your refresh job writes; it is shown as *Updated*) and
`update_command` (a shell command the Data page can run and schedule).

Any vendor works once its data is exported into this layout. Rob's library is the EODHD bulk
download maintained outside the repository; the refresh script and catalog live with it.

## 2. Parquet tick lake (trades and order book)

`lake_dir` points at a folder of Hive-partitioned parquet feeds:

```
<lake>/trades/exchange=<EX>/symbol=<SYM>/date=<YYYY-MM-DD>/*.parquet
<lake>/book_snapshots/exchange=<EX>/symbol=<SYM>/date=<YYYY-MM-DD>/*.parquet
<lake>/book_events/exchange=<EX>/symbol=<SYM>/date=<YYYY-MM-DD>/*.parquet
```

| Feed | Columns used |
|---|---|
| trades | `recvTimestampMicros`, `price` (decimal), `size`, `aggressor` (BUY/SELL) |
| book_snapshots | `recvTimestampMicros`, `bookEpoch`, `bids` / `asks` as lists of `{price, size}` |
| book_events | `recvTimestampMicros`, `bookEpoch`, `side` (BID/ASK), `price`, `newSize`, `action` (CHANGE/DELETE) |

Timestamps are UTC microseconds. Instruments are addressed as `EXCHANGE:SYMBOL`. The engine builds
1s to 30s bars from trades, rebuilds the L2 book from snapshots plus deltas, and samples the book at
each bar close into `bar.book`. `funding/` and `open_interest/` are inventoried but not yet used by
the engine.

Check a new venue with:

```bash
tessera lake-diagnose --lake /path/to/lake --symbol EXCHANGE:SYMBOL --date YYYY-MM-DD
```

It reports epochs, the delta mix, and how many trades print inside the rebuilt touch. Expect 97% or
better; lower means the venue's delta semantics differ from absolute-size CHANGE/DELETE and the
reader needs adjusting.

## Providers

A provider is where a source's data comes from: an API the service talks to with the user's
own token (decision 0020). Each one is a module under `src/provider/` implementing the
`Provider` trait in `src/provider/mod.rs`; the service, the jobs, and the console see only
the trait. Its calls, the read-only ones the console shows and the download ones the jobs
make (one request each against the daily limit unless the adapter says otherwise):

| Call | Returns | Fails with |
|---|---|---|
| `verify(token)` | the account: plan, requests today, daily limit, when the counter resets (UTC) | `CredentialsRejected` on a refused token |
| `exchanges()` | code, name, country, and the bar resolutions offered there (`daily`, `1h`, `5m`, `1m`) | `Unreachable` when the provider cannot be reached, times out, answers 5xx, or rate-limits |
| `symbols(exchange, delisted)` | each listing's code, name, type (the provider's classification, verbatim), currency, country, and venue (the provider's own exchange name under the code, `NASDAQ` under `US`) | `Malformed` when the answer is not the shape the adapter reads |
| `bulk_eod(exchange, date)` | every instrument's bar on the exchange for that session (code, exchange, and the bar: date, open, high, low, close, adjusted close, volume); empty for a date the provider has no bars for (a weekend, a holiday, a session not published yet) | the same three |
| `eod_history(symbol, from)` | one symbol's daily bars from `from` on, oldest first, adjusted closes on one basis (`symbol` in the provider's full form, `AAPL.US`) | the same three |
| `splits(exchange, date)` | the splits effective on that session: code, exchange, date, and the ratio as the provider writes it | the same three |

An error's text is safe to log: no variant carries the token, and reqwest's own errors are
stripped of the request URL (which would carry it) before they become one.

### EODHD (`src/provider/eodhd.rs`)

`Eodhd::new(base_url, token)`; `Eodhd::public(token)` uses `https://eodhd.com`. The
adapter sends the token as the `api_token` query parameter and asks every endpoint for
`fmt=json` (the list endpoints answer CSV without it). The base URL is configurable so the
tests run against a stub server: `tests/provider_eodhd.rs` serves the recorded responses
under `tests/fixtures/eodhd/` (`user.json`, `exchanges-list.json`,
`exchange-symbol-list-US.json`, `eod-bulk-last-day-US.json`,
`eod-bulk-last-day-US-splits.json`, `eod-AAPL.US.json`, token scrubbed, trimmed to a few
rows), and nothing in the repository calls eodhd.com unless the service does with a
registered token.

| Endpoint | Call | Notes |
|---|---|---|
| `/api/user` | `verify` | `subscriptionType` is the plan, `apiRequests` the count for `apiRequestsDate`, `dailyRateLimit` the limit; the counter resets at 00:00 UTC the next day |
| `/api/exchanges-list/` | `exchanges` | every exchange offers `daily`, `1h`, `5m`; `US`, `FOREX`, and `CC` add `1m` (the adapter's own table: the endpoint says nothing about resolutions) |
| `/api/exchange-symbol-list/{code}` | `symbols` | `delisted=1` asks for the instruments no longer trading instead of the active ones; `Type` is `Common Stock`, `ETF`, `Fund`, ...; `Country` and `Exchange` are the listing's country and venue |
| `/api/eod-bulk-last-day/{code}?date=` | `bulk_eod` | one row per instrument with `code`, `exchange_short_name`, and the bar; a row missing a price is dropped (nothing is invented for it) and a missing `volume` is zero |
| `/api/eod-bulk-last-day/{code}?type=splits&date=` | `splits` | `split` is the ratio text (`4.000000/1.000000`) |
| `/api/eod/{symbol}?from=&period=d` | `eod_history` | the symbol's daily rows from the date on; a symbol the provider does not know is its 404, `Malformed` |
| `/api/intraday/{symbol}?interval=&from=&to=` | `intraday` | the symbol's bars at `interval` (`1m`, `5m`, `1h`, the engine's resolution names) whose `timestamp` lies in `from..=to`, both UTC epoch seconds; a row missing its timestamp or a price is dropped and a null `volume` is zero; the `datetime` text is not read (the file derives it from the timestamp); a request costs five calls, and `intraday_window_days` is the longest span one may cover: 120 days at `1m`, 600 at `5m`, 7200 at `1h`, none at `daily` (asked anyway, `Malformed` with no call) |

HTTP 401 and 403 are `CredentialsRejected` with the provider's `message`; 429 and 5xx are
`Unreachable`; any other non-2xx (a 404 for an exchange or a symbol it does not know) is
`Malformed` with the status.

### The call budget (`src/provider/budget.rs`)

Every job obeys one `CallBudget { limit, used, reserve }` built from the usage the provider
last reported for its source and the source's reserve (decision 0022): `remaining()` is the
limit less today's requests, `available()` what remains above the reserve, `can_start(n)`
admits a job whose mandatory calls fit in it (exactly filling it included) and otherwise
returns a `Refused` naming both numbers (`needs 551 calls, 550 available above the reserve
of 50`), `charge(n)` counts calls made, and `at_reserve()` is where the optional part of a
job stops. `Estimate::eod(sessions, backfills)` is two mandatory calls per session (bulk and
splits) and one optional per backfill; `Estimate::intraday(windows, symbols)` is windows
times symbols times five, all mandatory, and `Estimate::intraday_windows(increment,
backfill)` is what the intraday job runs on: the windows that extend the files that exist
times five mandatory, the windows that backfill the symbols without one times five
optional. `reserve_calls(limit, pct)` rounds the reserve up
and caps it at the limit; a limit of zero (an account the provider has not described)
admits no call and is already at its reserve. The budget never talks to a provider: the
service refreshes the figures before a job starts and after it ends.

### Registered sources (`/api/sources`)

A provider account is registered from the console (decision 0021): a row in the catalog's
`data_sources` table (id, name, kind, root, catalog_dir, reserve_pct, token_set_at,
verified_at, verify_state, verify_message, created_at, and the usage the provider last
reported: requests_today, daily_limit, resets_at, usage_checked_at) and the token in
`data/ui/secrets/<id>.token`, a file with mode 0600 in a folder the service creates with
mode 0700. The database never holds the token, and no response, log line, or error message
carries it or the file's path; a catalog restored from a backup shows its sources with
`token_set` false until each token is entered again.

| Endpoint | Does | Refuses with |
|---|---|---|
| `GET /api/sources` | the cards, plus `kinds`, the provider kinds compiled in (`eodhd`); each card's usage is refreshed through its adapter unless the source was checked within the last minute, `?refresh=1` asks regardless | a provider that cannot be reached leaves the last figures with their time on the card and sets its state |
| `POST /api/sources` | verifies the token through the kind's adapter, writes the file, inserts the row with the usage that call reported; 201 with the card | 409 for a root another source already covers (one source per root, checked before the provider is asked); 422 with the provider's message for a rejected token, 502 when the provider could not be asked; nothing saved either way. 400 for an unknown kind, a relative root or catalog folder, an empty name or token, a reserve outside 0 to 100 |
| `PUT /api/sources/{id}` | sets `reserve_pct`, the share of the daily limit jobs leave untouched (decision 0022, default 5); 200 with the card, the reserve in calls recomputed | 400 outside 0 to 100, 404 for an unknown source |
| `PUT /api/sources/{id}/token` | verifies the new token and replaces the file (written as a part file and renamed, so the old token stays until the new one is on disk), recording the usage that call reported | 422 / 502 as above, the old token untouched |
| `POST /api/sources/{id}/verify` | re-checks the token on file and records `verified_at`, `verify_state` (`connected`, `credentials_rejected`, `unreachable`), and the provider's message, plus the usage on success; 200 with the card whatever the provider said | 409 when no token file exists |
| `DELETE /api/sources/{id}` | removes the row, the token file, and the source's datasets with their scans; 204 | 409 while any regular file lies under one of the source's dataset folders, or under the root while it has no datasets: the console never deletes data files (decision 0022) |

A card also reports `root_exists` and, for a root that is mounted, the volume's
`total_bytes`, `used_bytes`, and `free_bytes` from statvfs (free is what this process may
use), and its `usage`: `requests_today`, `daily_limit`, `resets_at` (EODHD reports no reset
time, so the adapter derives 00:00 UTC after the day the count is for, and the card labels
it "resets 00:00 UTC"), `checked_at` (when the provider reported these figures; a later
failed check leaves them and this time in place), `reserve_calls` (`reserve_pct` of the
limit, rounded up), and `available_calls` (what a job may still spend); `null` until the
provider has answered once, and its datasets with their last scan (`datasets`,
`uncataloged`, `scanned_at`, `scanning`; the next sections). A verify and a usage refresh
are the same `/api/user` call, so `verified_at` is the time of the last check either way
and `verify_state` says whether it succeeded. The adapter for a card is chosen by its `kind`; `TESSERA_EODHD_BASE_URL` points
the EODHD adapter at a stub server for a scratch console, and the service tests in
`src/bin/tessera_ui.rs` (`tests::sources`) register a known placeholder token over the
DS-02 stub and fail if the token or the secrets path appears in any `/api/sources` or
`/api/data` response, or if the card's usage does not follow the stub's count on
`?refresh=1` and keep the last figures with their time while the stub answers 503.

### Availability (`/api/sources/{id}/availability`)

What a source's provider offers is cached in the catalog with the time it was fetched
(decision 0013) and served from there; nothing calls the provider on a page load. Two
tables hold it: `provider_exchanges` (source_id, code, name, country, resolutions as a
JSON list, fetched_at) and `provider_listings` (source_id, exchange, code, name, type,
currency, delisted, fetched_at), with `provider_refreshes` (source_id, attempted_at,
error) recording the last refresh attempt. A refresh fetches the exchange list and then
the active listing of every exchange with a dataset registered against the source (the
`datasets` table), or of the one exchange the body names, and the delisted listing as well
(one more call per exchange, cached apart with `delisted = 1`) where the body says
`"delisted": true` or, when it says nothing, where a dataset of the exchange includes
delisted symbols; a provider call that fails leaves every row fetched before in place, its
time included, and records the failure as the table's `unreachable` note, so the console
shows a stale table with a note, never an empty one. The tables go with their source when
it is deleted.

| Endpoint | Does | Refuses with |
|---|---|---|
| `GET /api/sources/{id}/availability` | the cached table: `fetched_at` (the exchange list's), `refreshed_at` (the last attempt), `unreachable` (why it failed, else null), and `exchanges`, each with code, name, country, resolutions, `fetched_at`, `listings_fetched_at` (null until its listing is cached), `listed`, `types`, the active listing counted per type under the provider's own type names, largest first, and `delisted` with `delisted_fetched_at` (the delisted listing's count and time, 0 and null until it is cached) | 404 for an unknown source |
| `POST /api/sources/{id}/availability/refresh` | fetches the exchange list and the listings (`{"exchange": "US"}` names one; an empty body means every exchange with a dataset; `"delisted": true` or `false` asks for or skips the delisted listing, else it follows the datasets), caches them, and answers 200 with the table whatever the provider said | 409 when no token file exists; 400 for an exchange the provider does not list, the cache untouched |

The service test in `src/bin/tessera_ui.rs` (`tests::sources`) refreshes over the DS-02 stub,
asserts the four exchange rows and the US type counts, then makes the stub answer 503 and
asserts the rows and `fetched_at` unchanged with the note set.

### Datasets and the scan cache (`/api/sources/{id}/datasets`, `/api/sources/{id}/scan`)

A dataset is what a source keeps current: one exchange, a set of the provider's types
(decision 0012: Common Stock and ETF stay distinct), a resolution, a from-date, a folder,
and whether delisted symbols are included. It is a row of `datasets` (id, source_id,
exchange, types_json, resolution, from_date, folder, include_delisted, created_at,
min_bulk_rows, the row count a bulk day must reach before the EOD job accepts it, default
10,000) and its figures are a row of `dataset_scans` (dataset_id, scanned_at, listed,
on_disk, latest_date, current_count, bytes, uncataloged_json, state, error), written only by
a scan (decision 0013: the page reads the cache and shows its time; walking the files is an
explicit action). Both go with their source, as do the dataset's download jobs.

| Endpoint | Does | Refuses with |
|---|---|---|
| `POST /api/sources/{id}/datasets` | registers a dataset (`exchange`, `types`, `resolution`, `from_date`, optional `folder`, absolute or relative to the root, default `<root>/eod` for daily bars and `<root>/<resolution>` otherwise, optional `include_delisted`, default on for daily bars, optional `min_bulk_rows`, default 10,000) against the cached availability; 201 with the row, state Unknown until a scan runs | 400 for an exchange not in the source's list, a resolution the provider does not offer there, a type not in the exchange's cached listing, a from-date that is not a date, or no types; 409 while the exchange's listing is not cached, and for a dataset of the source already covering one of the types on that exchange at that resolution; 404 for an unknown source |
| `DELETE /api/datasets/{id}` | removes the registration, its scan row, and its job records; 200 with the source card | 409 while a file exists for any of its listed symbols (with no listing cached, while any file lies in its folder): files are never deleted (decision 0022); 409 while a job runs on it |
| `POST /api/sources/{id}/scan` | starts the source's scan job in the background; 202 with `{"source_id", "scanning": true}`, and the card says `scanning` until every dataset's row is written | 409 while a scan of the source runs; 404 for an unknown source |

The scan reads each dataset's listing under one short catalog lock (the codes of its types
on its exchange in `provider_listings`, the delisted ones included when the dataset includes
them) and then walks the disk without it. Per dataset it counts the files named for those
symbols in its folder (`<code>.<exchange>.csv`; part files and anything else are not among
them), their bytes, the latest of their last dates (each tail-read through
`last_csv_row_date`), and how many are current through the latest expected session: for
daily bars the calendar symbol's last date (`[data] calendar_symbol`, its file looked for
in the dataset's folder, then the source's daily dataset folders, then `daily_dir`); for
intraday bars that session's close in New York, which a file reaches when its last bar
starts at the close less one bar. Files under the root that no dataset's folder (nor the
catalog folder) is at or under are counted per folder as Uncataloged, loose files directly
under the root under `.`, and stored on every row of the scan; the card serves the newest
row's list with its time. A dataset whose root or folder is missing is Unavailable and a
folder that cannot be read is Failed with the reason on the row: either keeps the previous
figures under the new time, and a dataset never scanned before gets zeros.

The states (BT-605), as the scan assigns them:

| State | Meaning |
|---|---|
| `Current` | files reach the latest expected session and exist for at least 95% of the listed symbols (a listing always carries a few symbols with no history) |
| `Updating` | a download job on the dataset is queued or running (its `last_job`); the scan itself never assigns it, and the job ends with a rescan that replaces it |
| `Stale` | files exist but none reaches the latest expected session: the last update did not run or did not finish |
| `Partial` | files exist for under 95% of the listed symbols; a dataset with no files yet is Partial |
| `Failed` | the last scan could not read the folder (`error` says why); the figures are the previous scan's. A download job that failed says so on the dataset's `last_job`, and the rescan after it judges the files as they are |
| `Unknown` | never scanned, or nothing to judge against: no listing cached for the exchange, or no calendar file to set the expected session |
| `Unavailable` | the root or the dataset's folder was not there when scanned (an unmounted drive); the figures are the previous scan's |

`GET /api/sources` returns each card with `datasets` (the row and its `scan`: `scanned_at`,
`listed`, `on_disk`, `latest_date`, `current_count`, `bytes`, `error`, null until one has
run, the `state`, and `last_job`, the newest download job's record, null before one),
`uncataloged` (`folder`, `files`, `bytes` per folder), `scanned_at`, and `scanning`. The service test in `src/bin/tessera_ui.rs` (`tests::sources::datasets`)
builds a root with `eod/` holding files for three listed symbols, a part file, a stray
folder, and a loose file, registers a US EOD dataset over the stub's listing of five symbols
(three active, two delisted), scans, and asserts listed 5, on disk 3, the latest date, the
bytes of the three files, the part file excluded, the stray folder Uncataloged with its
count, and state Partial; removes the folder, rescans, and asserts Unavailable with the
previous counts kept; and asserts the dataset's DELETE is 409 with files and 200 without,
that a second source over the same root is 409, and that the source's guard is its dataset
folders, not the root.

### The EOD download job (`/api/datasets/{id}/update`)

The job that brings a daily dataset up to date (BT-1205) is `src/provider/jobs/eod.rs` in
the library crate, written against the `Provider` trait alone: it knows the dataset (its
folder, its symbols from the cached listing with their delisted flag, its from-date, its
`min_bulk_rows`, the calendar symbol's code when that symbol is listed on the dataset's
exchange) and a `CallBudget`, nothing of the catalog. `plan` reads the folder and says what
a run would do; `run` does it, in this order, and every rule below is decision 0022's:

1. The folder is refused without a call when it is missing, not a folder, or not writable;
   the job never creates it. The mandatory calls (one bulk and one splits call per session)
   are judged against the budget: over what is available above the reserve, the job is
   Failed with both numbers (`needs 4 calls, 3 available above the reserve of 5`) and no
   call is made.
2. The sessions are the weekdays after the dataset's latest date (the calendar symbol's
   file's last date, else the latest last date across the dataset's files; a dataset with no
   files has none) through today in New York. Every session's bulk bars and splits are
   fetched before anything is written. A session the provider has no bars for is not a
   session (a weekend the job never asks about, a holiday, a session not published yet) and
   is skipped with one call. One with bars but under `min_bulk_rows`, or without the
   calendar symbol, fails the job whole with the row count and nothing is written.
3. The files: a symbol that split has its whole history refetched from the from-date and
   its file replaced, so its adjusted closes are on one basis; every other symbol with a
   file has the session rows appended, never a date already in the file; a delisted symbol
   is never incremented; a bulk row for a symbol without a file makes no file (the backfill
   fetches its history instead). Every write is a part file (`<name>.part`) renamed over
   the target, and a failed write removes its part file.
4. The backfill: each symbol with no file, delisted ones included, gets one history call
   from the from-date and a new file, in listing order, until the budget is at its reserve;
   the job then records how many were left and the next run, planning from the files again,
   continues from them. A symbol the provider answers without a history (an empty list, or
   its 404) is skipped with the reason; a provider that stops answering fails the job on
   that call, the files written before it whole.
5. `catalog.csv` (`Code,Name,Country,Exchange,Currency,Type`, the exchange's active
   listing sorted by code), `stocks.txt` (its `Common Stock` codes as `CODE.EXCHANGE`),
   and `etfs.txt` (its `ETF` codes) are regenerated in the source's catalog folder, so the
   run form's universes and the instrument index keep working.

The service keeps one job per source at a time, records each in `dataset_jobs` (id,
dataset_id, kind `eod`, state Queued, Running, Complete, or Failed, percent, created_at,
started_at, finished_at, calls, added: files created, updated: files appended to or
replaced, skipped_json: the symbols with their reasons, estimate_json, error, log_path),
writes the log under `data/ui/logs/<job id>.log` (never under the dataset folder), and,
after the job whatever its outcome, refreshes the source's usage from the provider and
rescans the source so the dataset's figures follow the files. A job left Queued or Running
by a service restart is marked Failed at the next start; its files are whole and the next
run continues from them.

| Endpoint | Does | Refuses with |
|---|---|---|
| `POST /api/datasets/{id}/update` | refreshes the source's usage, queues the job, and answers 202 with its record; the dataset says `Updating` until the job ends | 409 while a job runs on the source, naming the running id and its dataset; 409, before any call, for a root that is not mounted or a folder that is missing, not a folder, or not writable; 409 when no token is on file, and when the provider has not reported usage for the source (a job starts only from reported usage); 400 for a dataset at a resolution the source serves no intraday bars at (a daily dataset gets the EOD job, any other the intraday job below); 404 for an unknown dataset |
| `GET /api/datasets/jobs/{id}` | the record | 404 |
| `GET /api/datasets/jobs/{id}/log` | the log as a text download (`Content-Disposition: attachment`); empty until the job has started writing it | 404 |

`tests/provider_eod_job.rs` drives the job through the EODHD adapter over a stub server
whose answers the test sets per date and symbol, and a temp root of three seeded files,
and proves each clause of the ticket's done line: two sessions appended to the three files
with one bulk and one splits call each and a fourth listed symbol backfilled from the
from-date in one call; a seeded split rewriting that symbol's whole file with the
refetched history; a bulk day without the calendar symbol, or under the minimum, writing
nothing and failing with the row count, the splits call never made; a rerun through a
weekend and a holiday adding and updating zero; a reserve that admits one backfill making
that call, leaving no part file, and the rerun continuing from the symbol left, with a
budget short of the mandatory calls refusing before any call; a removed or read-only
folder refused with no call made and not created; the catalog files matching the listing;
and a provider that goes away mid-run failing the job on that call with the files written
before it whole. The service test in `src/bin/tessera_ui.rs` (`tests::sources::datasets`)
posts an update over the stub with its first bulk call held, asserts the 202 record, the
409 naming the job for a second post on the source, `Updating` on the card, the 409 on
deleting the dataset or the source meanwhile, then releases the stub and asserts the
Complete record (files added, updated, the delisted symbol without a history skipped with
its reason), the files, the catalog files from the cached listing, the log download from
under `data/ui/`, the source's usage checked after the job finished, the rescan, and the
409 with no call for a removed folder and for an unmounted root.

The intraday increment job (BT-1206, DS-09) is `src/provider/jobs/intraday.rs`, the same
shape against the same trait, and `POST /api/datasets/{id}/update` runs it for every
dataset whose resolution is not `daily` (kind `intraday` in `dataset_jobs`; the record,
the log, the one-job-per-source lock, the usage refresh, and the rescan are the EOD job's).
It knows the dataset's exchange, resolution, folder, from-date, and symbols, and the instant
it fetches through (now, from the service). `plan` reads the folder and cuts every fetch to
windows of the span the provider allows per request at the resolution
(`intraday_window_days`: 120 days at `1m`, 600 at `5m`): for each symbol with a file, the
windows from the file's last timestamp through the instant, the first window starting at
that bar so a current file gets it back and nothing else; for each symbol without one, the
windows from midnight UTC of the from-date. `run` then: refuses without a call a folder that
is missing, not a folder, or not writable (never created), a resolution the provider has no
intraday bars at, and mandatory calls (the increments, every window at five calls) over what
the budget has above its reserve, with both numbers; extends every file, the windows fetched
oldest first and the bars after the file's last timestamp appended in order, never one
already in the file, through a part file renamed over the target, a delisted symbol's file
never touched and a file whose last row carries no timestamp left as it is and skipped;
backfills the symbols with no file, delisted ones included, from the from-date, a new file
each, stopping before the request that would reach the reserve: a symbol cut off between
its windows keeps the file its fetched windows made (the next run extends it from its last
bar), the symbols left are counted on the record and in the log (`stopped at reserve`), and
the next run, planning from the files again, picks them up. A symbol the provider answers
with no bars in any of its windows, or whose request it answers with something other than
bars (its 404), is skipped with the reason and asked nothing more in that run; the next run
asks again. A provider that stops answering, or rejects the token, fails the job on that
request with the files written before it whole. Files keep the intraday layout
(`Timestamp,Gmtoffset,Datetime,Open,High,Low,Close,Volume`, the timestamp as UTC epoch
seconds, `Datetime` as that instant in UTC, one file per symbol named `<CODE>.<EXCHANGE>.csv`)
and the catalog files are not touched (they are the EOD job's). `tests/provider_intraday_job.rs`
drives the job through the EODHD adapter over a stub answering `intraday/{symbol}` from rows
set per symbol, cut to each request's span, and proves each clause of the ticket's done line:
a 5m file 700 days behind extended across two 600-day windows with two requests (ten calls)
and one a bar behind with one, the bar each file holds asked back and not written twice; a
missing symbol backfilled from the from-date across two windows; a symbol with no bars
skipped with the reason after its windows; a rerun at the same instant asking each file's
last bar back and writing nothing, an instant before the from-date asking nothing; the
reserve stopping the backfill before its first request or between a symbol's windows with
the file holding what was fetched and no part file, the rerun continuing from the symbol
left, and a budget short of the increments refusing before any call; a removed folder and a
daily resolution refused with no call; a rejected token and an outage failing the job with
the files whole and a 404 symbol skipped beside them. The service test
(`tests::sources::datasets::a_5m_datasets_update_routes_to_the_intraday_job`) posts an
update on a 5m dataset over the stub and asserts the record's kind `intraday`, the seeded
file extended in the intraday layout, the backfilled file, the symbol skipped with its
reason, the calls at five a request, no part file, and the job as the dataset's `last_job`.

## Environment overrides

`TESSERA_DATA_ROOT` (a folder holding `eod/`, `5m/`, `1m/`, `catalog/`), `TESSERA_ENGINE`,
`TESSERA_STRATEGY_DIRS`, and `TESSERA_MEMORY_BUDGET_GB` override `local.toml`. With no `local.toml`
the console reads the synthetic dataset under `examples/data`. `TESSERA_EODHD_BASE_URL` points
the EODHD adapter of every registered source at another base URL (a stub server for a scratch
console) instead of `https://eodhd.com`.

## Registering a source from the console

The Inventory view of the Data page (DS-06; decisions 0003, 0014, 0021) is where a provider
account and its datasets are registered, with no restart. Its panels, top to bottom:

1. **Data sources**: one card per registered source. The header carries the name, the kind,
   the root, and "token set, verified <time>" with *Replace token*, *Verify*, *Rescan*, and
   *Remove*; the next line the connection state with the time checked (Connected, Credentials
   rejected with the provider's message, Unreachable) and the root volume's used, free, and
   total space; then the credits line (requests used / daily limit, "resets 00:00 UTC", what
   is left above the reserve, and the reserve as an editable field, decision 0022); then the
   datasets table (exchange, types, resolution, from, folder, listed, on disk, latest, current,
   size, state; `remove` on each row); then the Uncataloged line (files under the root no
   dataset claims, per folder, with the scan's time) and *Add dataset*. A source with no
   datasets says "no datasets yet, add one to scan" and still shows its Uncataloged folders.
   Every time is UTC, the service's own stamp.
2. **Available from <source>**: the provider's cached exchange list (BT-1202): code, name,
   country, the listed count per instrument type under the provider's own names (the two
   largest, then `+n`, the full list on hover, and the delisted count once fetched), the
   resolutions offered, and *Here*, the datasets registered against the exchange. The title
   says "listed <time>"; *Refresh* asks the provider again; an unreachable provider leaves the
   rows with an Unreachable note; the filter narrows by code or name; rows with datasets sort
   first; an exchange whose listing is not cached shows `+`, which fetches that one listing.
   With several sources a select in the title picks the one shown.
3. **Configured library**: the `local.toml` library and lake as before, then the library
   metrics and the run-coverage fold.

**Add source** (a button on the Data sources title) opens an inline form, never a dialog (the
run form is the console's one, decision 0003): kind (the adapters compiled in), name, library
root, catalog folder, API token (a password field), reserve %. Root and catalog are pre-filled
from the configured library when no source covers that root yet, so the first EODHD source
adopts today's folders in place and nothing is downloaded again. *Save* posts the token once;
the service verifies it with the provider before writing anything, a rejected token is refused
with the provider's message and nothing is saved, and the field is cleared either way. The
token is never rendered again: the card shows only "token set, verified <time>", and
`web/scripts/data-page-check.mjs` fails if the fixture's token or the secrets path appears in
any element's text or value after a save.

**Add dataset** (on the card) is an inline form as well: the exchange from the cached list,
the types as checkboxes from that exchange's listing (with a *fetch it* link when the listing
is not cached yet), the resolution from what the provider offers there (EOD, 1h, 5m, 1m), the
from-date, include delisted (on by default for EOD), and the folder, defaulted to
`<root>/eod` for daily bars and `<root>/<resolution>` otherwise. The new row is Unknown until
*Rescan* runs the scan in the background; the card polls while the scan runs.

*Remove* on a source or `remove` on a dataset asks the service, which refuses while files lie
under the folders concerned: the console never deletes data files (decision 0022).

The fixture `web/fixtures/data-sources.json` seeds the view for the check with two sources
(one Credentials rejected), five datasets, an Uncataloged folder, the US and LSE availability
rows, and the usage object; the check drives Add dataset, Replace token, and Add source
against a stateful stand-in for the API and measures the seeded Inventory at 1280 and 1440 px.

## Adding a source

1. **Same shape, new location.** Export the vendor's data into one of the layouts above and point
   the matching key at it. Relaunch and press *Rescan* on the Data page.
2. **A different shape.** Add a reader in the engine. The CSV readers are `load_daily` and
   `load_intraday` in `src/sdk/runner.rs`; the lake readers and bar builder are in `src/lake.rs`.
   A reader has to produce `Bar` values (date, time, OHLCV, adjustment, optional book features)
   in ascending time for a symbol and window, and the standard-mode loader in `plan_standard`
   decides which reader a symbol routes to (today: `EXCHANGE:SYMBOL` or a second resolution goes
   to the lake, everything else to CSV). Register the new keys in `src/local_config.rs`, add them
   to the inventory in `build_data_sources` in `src/bin/tessera_ui.rs`, and document the layout
   here.
3. **Instrument search.** `build_instrument_index` in the API decides what the picker shows and
   which resolutions each record satisfies; a new source should add its records there so runs can
   be validated before they are queued.

## Sanitation before replay

Vendor daily files contain prints that are not market data: closes on market holidays, one-bar
spikes that revert the next session, and segments where the whole price scale changes overnight
and stays changed. The SDK runner cleans daily series as it loads them (`[data] sanitize_prices`,
default on; the calendar symbol's own dates define the session calendar), drops the first two
kinds, skips symbols with the third, prints the counts in the run log, and writes them to
`sanitation.json` beside the report (off-calendar rows, spikes, and every skipped symbol with
its reason, including symbols with no bars in the window). The run page shows the counts on
the Overview tab and lists the skipped symbols on the Symbols tab. For a standalone
audit of a library, `research/scripts/scan_bad_prints.py` in the private repo lists every
suspect file with its first bad date.
