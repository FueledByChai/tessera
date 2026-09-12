# Data sources

Tessera reads two kinds of source. Both are configured in `local.toml` at the repository root
(git-ignored; copy `local.example.toml`) and shown, with file counts, sizes, and date coverage,
on the Data page under **Data sources**.

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
the trait. Its read-only calls:

| Call | Returns | Fails with |
|---|---|---|
| `verify(token)` | the account: plan, requests today, daily limit, when the counter resets (UTC) | `CredentialsRejected` on a refused token |
| `exchanges()` | code, name, country, and the bar resolutions offered there (`daily`, `1h`, `5m`, `1m`) | `Unreachable` when the provider cannot be reached, times out, answers 5xx, or rate-limits |
| `symbols(exchange, delisted)` | each listing's code, name, type (the provider's classification, verbatim), currency | `Malformed` when the answer is not the shape the adapter reads |

An error's text is safe to log: no variant carries the token, and reqwest's own errors are
stripped of the request URL (which would carry it) before they become one.

### EODHD (`src/provider/eodhd.rs`)

`Eodhd::new(base_url, token)`; `Eodhd::public(token)` uses `https://eodhd.com`. The
adapter sends the token as the `api_token` query parameter and asks every endpoint for
`fmt=json` (the list endpoints answer CSV without it). The base URL is configurable so the
tests run against a stub server: `tests/provider_eodhd.rs` serves the recorded responses
under `tests/fixtures/eodhd/` (`user.json`, `exchanges-list.json`,
`exchange-symbol-list-US.json`, token scrubbed, trimmed to a few rows), and nothing in the
repository calls eodhd.com unless the service does with a registered token.

| Endpoint | Call | Notes |
|---|---|---|
| `/api/user` | `verify` | `subscriptionType` is the plan, `apiRequests` the count for `apiRequestsDate`, `dailyRateLimit` the limit; the counter resets at 00:00 UTC the next day |
| `/api/exchanges-list/` | `exchanges` | every exchange offers `daily`, `1h`, `5m`; `US`, `FOREX`, and `CC` add `1m` (the adapter's own table: the endpoint says nothing about resolutions) |
| `/api/exchange-symbol-list/{code}` | `symbols` | `delisted=1` asks for the instruments no longer trading instead of the active ones; `Type` is `Common Stock`, `ETF`, `Fund`, ... |

HTTP 401 and 403 are `CredentialsRejected` with the provider's `message`; 429 and 5xx are
`Unreachable`; any other non-2xx (a 404 for an exchange it does not know) is `Malformed`
with the status. The download calls (bulk EOD, history, splits, intraday windows) come with
the tickets that build the native download jobs.

### Registered sources (`/api/sources`)

A provider account is registered from the console (decision 0021): a row in the catalog's
`data_sources` table (id, name, kind, root, catalog_dir, reserve_pct, token_set_at,
verified_at, verify_state, verify_message, created_at) and the token in
`data/ui/secrets/<id>.token`, a file with mode 0600 in a folder the service creates with
mode 0700. The database never holds the token, and no response, log line, or error message
carries it or the file's path; a catalog restored from a backup shows its sources with
`token_set` false until each token is entered again.

| Endpoint | Does | Refuses with |
|---|---|---|
| `GET /api/sources` | the cards, plus `kinds`, the provider kinds compiled in (`eodhd`) | |
| `POST /api/sources` | verifies the token through the kind's adapter, writes the file, inserts the row; 201 with the card | 422 with the provider's message for a rejected token, 502 when the provider could not be asked; nothing saved either way. 400 for an unknown kind, a relative root or catalog folder, an empty name or token, a reserve outside 0 to 100 |
| `PUT /api/sources/{id}/token` | verifies the new token and replaces the file (written as a part file and renamed, so the old token stays until the new one is on disk) | 422 / 502 as above, the old token untouched |
| `POST /api/sources/{id}/verify` | re-checks the token on file and records `verified_at`, `verify_state` (`connected`, `credentials_rejected`, `unreachable`), and the provider's message; 200 with the card whatever the provider said | 409 when no token file exists |
| `DELETE /api/sources/{id}` | removes the row and the token file; 204 | 409 while any regular file lies under the source's root: the console never deletes data files (decision 0022) |

A card also reports `root_exists` and, for a root that is mounted, the volume's
`total_bytes`, `used_bytes`, and `free_bytes` from statvfs (free is what this process may
use). The adapter for a card is chosen by its `kind`; `TESSERA_EODHD_BASE_URL` points the
EODHD adapter at a stub server for a scratch console, and the service test in
`src/bin/tessera_ui.rs` (`tests::sources`) registers a known placeholder token over the
DS-02 stub and fails if the token or the secrets path appears in any `/api/sources` or
`/api/data` response.

### Availability (`/api/sources/{id}/availability`)

What a source's provider offers is cached in the catalog with the time it was fetched
(decision 0013) and served from there; nothing calls the provider on a page load. Two
tables hold it: `provider_exchanges` (source_id, code, name, country, resolutions as a
JSON list, fetched_at) and `provider_listings` (source_id, exchange, code, name, type,
currency, delisted, fetched_at), with `provider_refreshes` (source_id, attempted_at,
error) recording the last refresh attempt. A refresh fetches the exchange list and then
the active listing of every exchange with a dataset registered against the source (the
`datasets` table, once it exists), or of the one exchange the body names; a provider call
that fails leaves every row fetched before in place, its time included, and records the
failure as the table's `unreachable` note, so the console shows a stale table with a note,
never an empty one. The tables go with their source when it is deleted.

| Endpoint | Does | Refuses with |
|---|---|---|
| `GET /api/sources/{id}/availability` | the cached table: `fetched_at` (the exchange list's), `refreshed_at` (the last attempt), `unreachable` (why it failed, else null), and `exchanges`, each with code, name, country, resolutions, `fetched_at`, `listings_fetched_at` (null until its listing is cached), `listed`, and `types`, the active listing counted per type under the provider's own type names, largest first | 404 for an unknown source |
| `POST /api/sources/{id}/availability/refresh` | fetches the exchange list and the listings (`{"exchange": "US"}` names one; an empty body means every exchange with a dataset), caches them, and answers 200 with the table whatever the provider said | 409 when no token file exists; 400 for an exchange the provider does not list, the cache untouched |

The service test in `src/bin/tessera_ui.rs` (`tests::sources`) refreshes over the DS-02 stub,
asserts the four exchange rows and the US type counts, then makes the stub answer 503 and
asserts the rows and `fetched_at` unchanged with the note set.

## Environment overrides

`TESSERA_DATA_ROOT` (a folder holding `eod/`, `5m/`, `1m/`, `catalog/`), `TESSERA_ENGINE`,
`TESSERA_STRATEGY_DIRS`, and `TESSERA_MEMORY_BUDGET_GB` override `local.toml`. With no `local.toml`
the console reads the synthetic dataset under `examples/data`. `TESSERA_EODHD_BASE_URL` points
the EODHD adapter of every registered source at another base URL (a stub server for a scratch
console) instead of `https://eodhd.com`.

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
