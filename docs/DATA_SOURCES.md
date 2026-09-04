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

## Environment overrides

`TESSERA_DATA_ROOT` (a folder holding `eod/`, `5m/`, `1m/`, `catalog/`), `TESSERA_ENGINE`,
`TESSERA_STRATEGY_DIRS`, and `TESSERA_MEMORY_BUDGET_GB` override `local.toml`. With no `local.toml`
the console reads the synthetic dataset under `examples/data`.

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
