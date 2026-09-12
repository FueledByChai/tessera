# 0020 — Provider adapters are native Rust modules in the service behind one trait

Status: accepted
Date: 2026-09-11

## Context

The service knows nothing about where its market data comes from. `local.toml` names folders
and one opaque `update_command`; the whole EODHD pipeline (universe lists, bulk EOD, intraday
increments, crypto, FX, bond yields, a dashboard) is Python outside both repositories, run by
three launchd jobs, and the console can only launch the one command and read one freshness
file. The crate has no HTTP client. The owner asked (grill-me, 2026-09-11, Epic L) to register
EODHD as a data source from the console, see what the provider offers against what is on
disk, see today's API usage, schedule nightly downloads in the service (record 0001), and add
other providers later; other users of the application must be able to do the same with their
own accounts (record 0009). Three shapes were on the table: a native adapter in Rust, an
external tool per provider with a command contract the service runs and parses, and a hybrid
where Rust does the read-only calls and declared commands do the downloads.

## Decision

A provider is a Rust module in the library crate (`src/provider/`) implementing one
`Provider` trait: verify a token and report the account's usage and limits, list exchanges
and their listings, and fetch the data the download jobs need (bulk EOD, history, splits,
intraday windows). EODHD is the first adapter; another provider is another module behind the
same trait and nothing else. The crate gains an HTTP client (reqwest with rustls) for it.
The adapters, the jobs, and the scheduler are the service's own code, tested against a stub
server and recorded fixtures.

## Alternatives

- An external tool per provider with a fixed CLI contract (list, status, download, usage,
  printing JSON): cheapest in Rust and keeps the Python pipeline, but every provider needs a
  tool that only its author can maintain, the console can show nothing the tool did not
  print, and a user installing the application has no tool.
- Hybrid, Rust for the read-only calls and declared commands for the downloads: fastest to
  the visible panels, but the download stays a black box (no progress, no per-symbol
  outcome, no budget the service can enforce), and the cut-over from launchd would have to
  happen twice.

## Consequences

The Python pipeline's coverage has to be rebuilt in Rust before its launchd jobs can go
(BT-1205 to BT-1207 do it one job at a time); until then the old path stays. The service
owns an HTTP dependency and a stub-server test pattern (`tests/fixtures/<provider>/`,
recorded responses with the token scrubbed). Each provider's quirks (call costs per request,
window limits, bulk endpoints, split lists) live in its adapter, and the jobs stay
provider-neutral. A provider that offers something the trait cannot express extends the
trait for everyone.

## What would show this was wrong

A second provider whose API cannot fit the trait without a special case in every job, or the
native downloads never reaching the Python pipeline's coverage so both keep running. Then the
boundary moves to a tool contract, in a superseding record.
