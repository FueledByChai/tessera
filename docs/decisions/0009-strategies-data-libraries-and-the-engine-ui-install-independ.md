# 0009 — Strategies, data libraries, and the engine/UI install independently

Status: accepted
Date: 2026-09-11

## Context

Locked in `docs/PRODUCT_BACKLOG.md` under "Locked product decisions" at the initial public
release (2026-09-02), before decision records existed (HK-31); moved here by HK-33 with the
context reconstructed from the backlog and the docs. The bullet read:

> Strategies, data libraries, and the open-source engine/UI must become independently installable.

Why, as the docs record it: `docs/OPEN_SOURCE_ARCHITECTURE_DIRECTION.md` (Goal) says the
engine and UI "must not own, compile in, or distribute the user's private strategies or
market data", and a user should "install the application, point it at one or more external
data libraries, install or author strategy packages". BT-101 (the package manifest), BT-102
(the data-library catalog), and BT-103 (a versioned worker protocol so private packages "are
not linked into the web service") are the three halves of it; Milestone 3 moves private
strategies out of the checkout. The public repo is AGPL; the private repo beside it holds
the strategies, and the market data lives outside both (`AGENTS.md`, Layout). Today the
private strategies still compile in through `local.toml [strategies] dirs`, an interim step.

## Decision

The public engine and console, a strategy package, and a data library are three things that
install separately. Strategies reach data through dataset and instrument ids, never paths;
the application can register a package or a library from outside its checkout; only sample
strategies and synthetic fixtures ship in the public repo.

## Alternatives

- One repository with the private strategies compiled in (today's interim): the fastest
  build and the reason the private crate still links; it cannot be open-sourced or shared
  without shipping the owner's strategies.
- A native Rust plugin ABI: named a near-term non-goal in
  `docs/OPEN_SOURCE_ARCHITECTURE_DIRECTION.md`; a versioned process protocol is preferred
  because it is language-neutral and a crashing package cannot take the service down.
- Publishing vendor data with the application: a non-goal there too, for licensing and size.

## Consequences

The service needs a worker protocol (BT-103) and a library catalog (BT-102) before the split
is real; until then `local.toml` bridges the gap and the private checks build the private
crate against the worktree's engine. Every strategy asks for data by id. A moved library must
fail clearly without corrupting old runs.

## What would show this was wrong

The process protocol costing more than it saves for one owner on one machine (a second
build, a second failure mode, no third party ever installing a package), or every private
strategy needing engine internals that the protocol cannot carry. Then the boundary moves
back to a compiled-in crate behind a feature flag, in a new record.
