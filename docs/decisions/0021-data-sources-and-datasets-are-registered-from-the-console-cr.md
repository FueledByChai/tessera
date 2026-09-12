# 0021 — Data sources and datasets are registered from the console; credentials live in a 0600 file per source, never in the database

Status: accepted
Date: 2026-09-11

## Context

Today the one data library is `local.toml [data]`: folders, a provider name, a freshness
file, and an update command, read at start and shown on the Data page. Adding or changing a
source means editing the file and restarting the service. The owner wants to add a source
from the console with no restart and wants other users of the application to register their
own provider accounts the same way (grill-me, 2026-09-11, BT-1201). That puts an API token in
the console's hands, and BT-601 says credentials are never displayed. Three homes for the
registration were on the table (the config file, a manifest inside the library folder, the
catalog database through the console) and three for the token (the database, a file the
service owns, the macOS keychain).

## Decision

Sources and datasets are records in the catalog database, created, edited, and removed from
the Inventory view with inline forms (the run form stays the console's one modal, record
0003); the service reads them live, so no restart. The API token is written to a file per
source under `data/ui` with mode 0600; the database holds only that a token is set and when
it was last verified; no API response carries the token or the file's path, and the console
shows "token set, verified <date>" with a Replace control. The engine's data roots stay in
`local.toml` until strategies ask for data by id (BT-102); the first EODHD source adopts that
library's folders in place.

## Alternatives

- Keep everything in `local.toml`: one file, no new tables, but a restart per change and no
  path for another user of the application except editing a file by hand.
- A `tessera-source.toml` manifest inside the library root: the library describes itself and
  travels with the drive (attractive for BT-704 and BT-712), but a user still edits a file,
  and a manifest on a drive is the wrong place for a credential.
- The token as a column in the catalog database: simplest, but the token travels with every
  copy of `tessera_ui.sqlite3` and any endpoint that dumps a row risks showing it.
- The macOS keychain: best on the Mac mini, but users on Linux have none and the service would
  need two code paths.

## Consequences

New tables (`data_sources`, `datasets`, their scans and jobs) and endpoints under
`/api/sources`; a secrets folder the service creates with restricted modes; a test that
registers a known token and greps every response for it. Backups of the catalog database do
not carry credentials, so a restored catalog shows sources whose tokens must be re-entered.
A library moved to another machine has to be registered again there. The `local.toml` data
keys for the update command and the freshness file become redundant once the native jobs run
and are removed (BT-1207).

## What would show this was wrong

A token found in any response, log, or fixture; or the owner wanting a library on an external
drive to carry its own registration between machines. The first is a defect to fix under this
record; the second reopens the manifest alternative in a superseding record.
