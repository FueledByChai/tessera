# Work queue

The ticket list an agent loop works through. `docs/PRODUCT_BACKLOG.md` holds product intent and
long-form acceptance criteria; this file is the executable queue. Protocol:

- One ticket per commit. The commit message starts with the id. `scripts/check.sh` must pass.
- A ticket's **Done when** line names a test, fixture, or measurable output that ships in the
  same commit. If it cannot be tested, rewrite the ticket until it can.
- Git is the record of done: a ticket is done when a commit whose subject starts with its id
  is on `main`. `scripts/backlog-status.sh` lists every ticket with its derived state, date,
  and sha; `--next` names the first `todo` whose `Blocked by` tickets have landed. This file
  carries only the claims: no state (or `todo`), `doing` while someone works it,
  `blocked <reason>`. Take what `--next` reports, never two at once, and clear the `doing`
  claim in the ticket's own commit; never write a done line.
- Anything discovered while working goes in as a new ticket, not into the current one.

## Console UI

### UI-01 Terminal theme: black panels, larger aligned form controls
The terminal theme (`web/app/globals.css`, the block "sampled from Bloomberg screens") paints
every panel navy: `--panel #0f0f3a`, `--panel-2 #181850`, lines `#2a2a6a`/`#1c1c50`, plus
literal navy on fieldsets, code, and the config template. Studies, Data, and the strategy page
read as a blue console. A Bloomberg screen's ground is black; navy is a highlight for selected
rows and a few data panels, not the panel colour. Move the theme to black panels (`#050505`,
`#0b0b0b`) with the existing gray and amber lines, keep navy only for selection and hover
highlights, and fix the form controls: inputs, selects, and textareas in one form row share a
height and top edge, dropdowns match text fields, and the control font goes from the inherited
~12 px to 18 px with labels 10 → 15 px (the table convention), without growing the controls.
Applies to the studies form, the run form, the strategy page, and the data page.
**Done when:** a stylesheet check (`web/scripts/theme-check.mjs`, run from the web step of
`scripts/check.sh`) parses the terminal-theme rules and fails on any background or border colour
with a blue hue outside the `.active`, `:hover`, and heat-map highlight selectors, and on any
terminal-theme input/select/textarea font-size below 18 px; the browser-automation check on the
studies form and the strategy run form reports every control in a row within 1 px of the same
top and bottom edge (`getBoundingClientRect`); the README screenshot is refreshed.

### UI-02 Modern mode form controls match the terminal sizes
UI-01 sized and aligned form controls only under the terminal theme; modern mode still renders
the base 10-14 px controls with the 40/42 px height mismatch between text and date fields.
**Done when:** the browser alignment measurement from UI-01 passes with modern mode selected and
`theme-check.mjs` gains a modern-mode font-size rule.

### UI-03 Pages fit a 13-inch laptop; wide tables scroll inside their panel
On a 13-inch MacBook (1440 or 1280 px wide) the run page's monthly and annual performance
table (`.monthly-panel`), the studies page's feature-by-horizon heat map and its ranked and
decile tables, and other dense grids hang off the right edge of the window instead of
scrolling. `.table-wrap` already has `overflow-x: auto`, so the likely cause is grid and flex
children whose minimum width defaults to their content (`minmax(0, 1fr)` and `min-width: 0` are
missing on some panels), plus fixed column counts and the 18/15 px grid fonts at narrow widths.
Fix so the page body never scrolls horizontally: every panel is `min-width: 0`, wide tables
scroll within their own wrapper, multi-column layouts collapse below 1280 px, and the heat map
keeps its cells readable by dropping the second line (breakeven) into the hover title under
1280 px rather than shrinking the font below 15 px.
**Done when:** a headless measurement (`web/scripts/layout-check.mjs`, run by the web step of
`scripts/check.sh` against the built bundle with fixture data, or against the running service
when present) at 1280 and 1440 px on the run overview, the strategy page, and the studies page
asserts `document.documentElement.scrollWidth <= window.innerWidth` and that no element's right
edge exceeds the viewport unless an ancestor has `overflow-x: auto`; `theme-check.mjs` fails on
any grid track wider than `minmax(0, 1fr)` without `min-width: 0` on the track's children.

### UI-04 Numeric fields are sized to their content; form grids auto-fit
Every form grid (`.field-grid`, `.field-grid.five` in `web/app/globals.css`) is a fixed
four- or five-track grid, so a field holding one to three digits is as wide as a date or a
universe select and a row holds five fields. Numeric inputs (`type="number"`, and text
inputs the SDK form renders for int and decimal parameters) get a `size`-driven width of six
characters, dates and selects keep their width, and the grids become auto-fit tracks with a
minimum per kind, so a row holds at least six fields at 1280 px. Heights and fonts do not
change: the 42 px / 18 px rule (UI-01) stands and `web/scripts/theme-check.mjs` keeps its
floor. Serves BT-1101.
**Done when:** `web/scripts/layout-check.mjs` counts the fields in the first row of the
strategy page's Data and sizing grid at 1280 px and fails under six; `theme-check.mjs` still
passes with its font and height floors untouched.

### UI-05 The strategy page is a summary strip, a Run button, and history; configuration in a dialog — Blocked by UI-04
`StrategyWorkspace` (`web/app/page.tsx`) reorders to: hero (title, badges, View source,
Configure run); Production rules as a fold closed by default with the rule count on its bar;
a summary strip of the current configuration (research label, window, universe, resolution,
capital, entry limits, every strategy parameter with its value, preset name) with Configure
run and Run buttons; then `RunHistoryTable`. Configure run opens a modal dialog (a native
`dialog` element styled in the terminal look, the console's first) holding the whole form
(`SdkForm` and the identity fields) as four labelled rows (Identity, Data, Limits,
Parameters), the presets panel inside it, the advanced toggle, Cancel and Run; Escape or
Cancel closes it keeping the edits in memory; Run launches and closes. The strip's Run sends
the same request body the dialog's Run would with no edits. `docs/LOCAL_UI.md` describes the
page and the dialog. Serves BT-1101. Wireframe: BT-1101. Decisions: 0003.
**Done when:** `web/scripts/layout-check.mjs` opens the dialog on the strategy page at 1280
and 1440 px in both display modes and fails when the dialog passes the viewport or the page
scrolls horizontally, and fails when the history table is not the first panel after the
summary strip; `web/scripts/chart-check.mjs` (or a new `run-form-check.mjs` run by
`scripts/check.sh`) applies a seeded preset, reads the strip's values, intercepts the POST
from the strip's Run and from the dialog's Run, and fails unless both bodies match the strip.

### UI-06 The strategies catalog shows each strategy's last completed run, in nine dense columns
`StrategyCatalog` (`web/app/page.tsx`) shows #, name with version and status as badges under
it, asset, runs, CAGR, Sharpe, max DD, last run date, open; each metric column sorts
(`orderCatalogRows`). The catalog endpoint in `src/bin/tessera_ui.rs` adds the last completed
run's cached metrics (`metrics_json` on `runs`) and its date to each strategy row; no run or
no cached metrics gives nulls, rendered as a dash. Rows go dense under 1500 px like the run
history (`web/app/globals.css`). `docs/LOCAL_UI.md` names the columns. Serves BT-1102.
Wireframe: BT-1102.
**Done when:** a service test seeds a strategy with two completed runs (one without cached
metrics) and one with none and asserts the catalog rows carry the last completed run's
metrics, a null-metrics row, and a no-run row; `web/scripts/layout-check.mjs` measures the
strategies page at 1280 px with the nine columns and fails on horizontal scroll.

### UI-07 The Configure run dialog fits an 800 px viewport for every strategy — Blocked by UI-05
The dialog (UI-05) fits 1280×800 in both display modes for the fixture strategy, but against
the console's catalog `web/scripts/layout-check.mjs --verbose` reports its rows scrolling
inside by a few pixels in modern mode for the first strategy there (783 px of content in
776): modern mode's taller head and footer and the two-line captions in the Limits row
(`web/app/globals.css`, `SdkForm` in `web/app/page.tsx`) cost the difference, and a strategy
declaring many parameters scrolls in either mode. Decide whether the rows may scroll for such a
strategy or the Parameters row goes two-column and the modern head and footer as dense as the
terminal's, and make the check hold it. Serves BT-1101.
**Done when:** `web/scripts/layout-check.mjs` fails when the dialog's rows scroll inside at
1280×800 in either mode for a strategy declaring up to eight parameters, and passes against
the console and the scratch console.

### UI-08 The console's left menu collapses to an animated icon rail
Today the sidebar (`web/app/page.tsx`, the `<aside className="sidebar">` in `Home` with
`navItems` beside it) is a fixed column — 224 px, 216 px in terminal mode (`web/app/globals.css`,
`.app-shell` and `.app-shell[data-theme="terminal"]`) — rendering a glyph and a label per item,
with nothing to collapse it. Add a toggle button in the brand row, a `collapsed` state on
`Home`, and the collapsed styles: a 56 px rail in both display modes, the brand mark and the
engine dot kept, the labels and the foot's text not rendered, a themed CSS tooltip on hover for
each nav item plus the brand and the dot, and the active item's amber colour and inset bar kept.
The width and the label fade animate over 150 ms; the expanded sidebar is pixel-identical to
today. Serves BT-1103. Wireframe: BT-1103. Decisions: 0019.
**Done when:** `web/scripts/layout-check.mjs` gains a collapsed pass — it clicks the toggle on
the strategies page at 1280 and 1440 px in terminal and modern mode and fails unless the sidebar
measures 56 px, no nav-item label text is visible, the active item still carries its highlight,
hovering a nav icon surfaces a tooltip whose text is that item's label, and the page body does
not scroll horizontally; `theme-check.mjs` still passes.

### UI-09 The collapsed left menu is remembered per browser — Blocked by UI-08
UI-08 gives the sidebar a collapsed state that resets on every page load. Store it in browser
storage beside the display-mode preference (`window.localStorage`, the `bt-display-mode` read
and write in `Home`, `web/app/page.tsx`) under `bt-sidebar` with the values `collapsed` and
`expanded`: read on mount, written on every toggle, so a reload and a new tab keep the rail.
A fresh browser and unavailable storage both start expanded. Serves BT-1103. Wireframe:
BT-1103. Decisions: 0019.
**Done when:** the collapsed pass in `web/scripts/layout-check.mjs` reloads the strategies page
after collapsing and fails unless the rail is still collapsed, then expands and reloads and
fails unless it is expanded again; a run with browser storage blocked still renders the
expanded sidebar.

### UI-10 The dialog fit check covers choice and boolean parameters — Blocked by UI-07
UI-07's rule that the Configure run dialog shows its whole form at 1280x800 for up to eight
parameters was proven with numeric parameters, which take one 92 px track each; a `choice`
parameter takes two tracks and a `bool` its own control, so eight parameters with several of
those wrap the Parameters grid to a second row and the dialog scrolls inside itself.
`web/scripts/layout-check.mjs`'s eight-parameter fixture strategy gets two `choice` parameters
and one `bool` among the eight, and the dialog's rules (`web/app/globals.css`, `SdkForm` in
`web/app/page.tsx`) adapt until it fits at both widths in both modes, or the bound in
`docs/LOCAL_UI.md` is restated in tracks rather than parameters. Serves BT-1101. Decisions: 0003.
**Done when:** `web/scripts/layout-check.mjs` measures the mixed eight-parameter fixture at 1280
and 1440 px in both display modes and fails when the dialog's rows scroll inside it.

### UI-11 A cost profile's name is unique; a second one with the same name is refused
`validate_cost_profile_request` (`src/bin/tessera_ui.rs`, line 1398) checks the name's
length, the asset class, the model, the basis-point bounds, the tick size, the slippage
ceiling, and the commission bounds — but not whether the name is taken, so two POSTs to
`/api/cost-profiles` (line 771) with the same name both succeed and the cost-profile select
in the run form then offers two rows with the same label and nothing to tell them apart,
which is exactly what "Duplicate as new version" (UI-13) would produce every time.
`create_cost_profile` (line 3775) gains the check: a name already present in
`cost_profiles`, compared case-insensitively on the trimmed name, is refused with 400 and
the error "a cost profile with that name already exists". `seed_cost_profiles` (line 1241)
is unaffected, inserting as it does with INSERT OR IGNORE. Serves BT-1105.
**Done when:** an async test in the `mod tests` at the foot of `src/bin/tessera_ui.rs`
builds an in-memory `AppState` (as the feature-preset tests do), calls `create_cost_profile`
twice with the same name in a different case, and asserts the second answers 400 with that
message and leaves one row while a new name answers 200 and leaves two; and
`validate_cost_profile_request` still refuses an empty name, an unknown asset class, and
501 bps.

### UI-12 The cost-profile form moves into a dialog whose fields follow the model — Blocked by UI-11
`CostsWorkspace` (`web/app/page.tsx`, line 4524) ends with the "Create cost profile" panel:
a `field-grid` of eleven fields, six of them inert for any one model, sitting below the
profile cards so the results are always under the form. It becomes the console's second
modal dialog after UI-05's Configure run (decision 0003), opened from a "New profile"
button in the workspace head: a native `dialog` element in the terminal look, amber title
bar, the same 42 px controls, Cancel and Save. Profile name and Asset class always render;
the rest follow the chosen model — all-in basis points adds Entry bps and Exit bps, fixed
tick + per unit adds tick size, entry and exit slippage ticks, entry and exit commission
per unit, and minimum commission, costs off adds nothing — and switching model swaps the set
while keeping each model's typed values. A name matching one already loaded disables Save
with the reason beside it; a save the service refuses shows the service's message inline
above the buttons with the dialog open and every field intact. Escape or Cancel keeps the
edits in memory until the page changes; Save creates the immutable version and closes.
Serves BT-1105. Wireframe: BT-1105. Decisions: 0003, 0024.
**Done when:** a new `web/scripts/costs-check.mjs` in the `web/scripts/run-form-check.mjs`
style (serving `web/dist` with `/api/cost-profiles` and its POST answered from a new
`web/fixtures/cost-profiles.json`, run by `scripts/check.sh`) fails unless the page shows no
create form; New profile opens a dialog; All-in basis points shows no tick size, slippage,
commission, or minimum field, Fixed tick + per unit shows exactly those six, and Costs off
shows only name, asset class, and model; switching model away and back restores the values
typed; a duplicate name disables Save with the reason beside it; and a POST answered 400
renders the message above the buttons with the dialog still open and every field holding
what was typed. `web/scripts/layout-check.mjs` gains a Costs pass that opens the dialog at
1280×800 in both display modes for each of the three field sets and fails when it passes
the viewport or the page behind it scrolls horizontally.

### UI-13 The cost profiles are a sortable table — Blocked by UI-12
The `cost-profile-grid` of cards (`web/app/page.tsx`, line 4547), each holding a
model-shaped `<dl>`, becomes one table (decision 0024): one row per profile with the
columns NAME, ORIGIN, ASSET CLASS, MODEL, ENTRY, EXIT, ROUND TRIP, TICK, MIN COMM, and a
Duplicate control; ENTRY and EXIT in the model's own unit and ROUND TRIP their sum; a cell
the model does not use rendering a dash and never a zero, so a costs-off row keeps its
name, asset class, and model over five dashes; every header sorting, with the order
`/api/cost-profiles` returns as the default; the profile id as the NAME cell's tooltip
rather than a column; the panel's empty state when there is no profile. Each row's Duplicate
opens UI-12's dialog pre-filled from that row with the name as "<name> copy" and its text
selected, and the panel's title bar holds New profile. Nothing on a row edits or deletes.
Serves BT-1104. Wireframe: BT-1104. Decisions: 0003, 0024.
**Done when:** `web/scripts/costs-check.mjs` fails unless the fixture's seven profiles (two
all-in, three fixed tick, one costs off, one custom) render as seven rows in wireframe
order; a costs-off row's five value cells read a dash while a fixed-tick row's TICK reads
"$0.01"; clicking NAME sorts by name and clicking ROUND TRIP sorts by round trip, a second
click reversing it; Duplicate on a row opens the dialog carrying that row's model, asset
class, and every value with its name as "<name> copy"; the NAME cell's title attribute is
that profile's id; and an empty list shows the panel's empty state with New profile still
present. `web/scripts/layout-check.mjs`'s Costs pass measures the table at 1280 and 1440 px
in both display modes and fails on horizontal page scroll; `theme-check.mjs` still passes.

### UI-14 The dialog prices the assumption it is defining — Blocked by UI-12
The dialog from UI-12 prices what it is defining, so a basis-point figure and a
ticks-and-commission figure can be told apart before either is saved as an immutable
version. A line at its foot reads "A $100,000 round trip at $100.00/share costs about
$30.00 (3.00 bps)", recomputed on every change: shares are 100,000 divided by the reference
price, which is editable and defaults to 100; a side's commission is the greater of the
minimum commission and shares times its per-unit commission; a side's cost is that plus
shares times its slippage in ticks times the tick size; all-in basis points is 100,000
times entry plus exit over 10,000 and needs no price, so the reference-price control is not
rendered for it; costs off reads "No modeled cost." Serves BT-1105. Wireframe: BT-1105.
**Done when:** `web/scripts/costs-check.mjs` fails unless, with tick size 0.01, one tick of
slippage each side, $0.005 per unit each side, and a $100.00 reference price, the line
reads $30.00 and 3.00 bps; at $50.00 it reads $60.00 and 6.00 bps; with All-in basis points
at 5 and 5 it reads $100.00 and 10.00 bps with no reference-price control; with Costs off
it reads "No modeled cost."; and a minimum commission above the per-unit figure is the
figure the line uses.

### UI-15 A created record survives the poll that was already in flight
The console re-reads `/api/cost-profiles` every three seconds (`refresh` in
`web/app/page.tsx`) and a save puts the created profile at the front of the list before the
service is asked again. A poll whose request was already in flight when the save landed is
answered from the library as it was, and its answer replaces the list the save had just
updated, so for up to three seconds the profile the user created is not in the table — the
row comes back with the poll after that. Found while proving UI-13, whose check counts the
rows after a save and saw seven instead of eight in one run of two. The same shape covers
anything else the console both polls and writes: either keep the record a save created when a
poll that began before it is applied, or ignore a poll that began before a local write.
Serves BT-1104.
**Done when:** `web/scripts/costs-check.mjs` fails unless, with `/api/cost-profiles` answered
only after a delay long enough for a save to land while the poll is in flight, the row that
save added is still in the table once the answer is applied — the fixture holding the list a
request in flight was answered from, and the check reading the row count after the poll has
landed rather than straight after the save.

## Feature workbench (Studies page)

### WB-01 Feature expression grammar
Replace the fixed feature list in `src/study.rs` with expressions: a base series followed by
transforms, e.g. `trade_count | rate 1 | ratio_to sma 300`, `signed_volume | zscore 30`,
`obi_l1 | diff 1`, `a | times b`. Bases: every `BookFeatures` field plus trade count, volume,
buy/sell volume, per-side depth. Transforms: `ema n`, `sma n`, `zscore n`, `diff n`, `lag n`,
`rate n`, `ratio_to`, `pct_rank n`, `abs`, `sign`, `clip lo hi`, `times <expr>`. Streaming state
per symbol; unknown names produce a clear error listing what exists.
**Done when:** unit tests cover parsing (including nested `times` and error text), each transform
against hand-computed values, and a study over the existing eight names produces identical IC to
the current implementation (fixture from a 1-day SOL grid checked into `target/`-free test data).
Note: the parity fixture is a deterministic synthetic 1-second grid generated in the test rather
than a checked-in SOL day; it exercises book gaps and all eight names.

### WB-02 Costless equity curve and breakeven cost — Blocked by WB-01
For each feature and horizon: position = clipped z-score of the feature (and a `sign` variant),
P&L = position x forward return with zero costs; report cumulative P&L series, Sharpe, turnover
(mean absolute position change per bar), and breakeven cost in bps = mean P&L per unit turnover.
Add the series to `StudyResult` and CSV output; UI shows the curve per selected cell.
**Done when:** a synthetic feature equal to the forward return plus noise yields the expected
Sharpe within tolerance in a unit test, a pure-noise feature yields breakeven near zero, and the
results grid in the UI sorts by breakeven cost.

### WB-03 Study targets — Blocked by WB-01
Add a `target` field: `return` (current), `realized_variance` (sum of squared mid returns over the
horizon), `abs_move`, `spread_change`, `fair_value_residual` (mid minus a 60 s EMA, and mid minus
microprice). IC, deciles, and the equity curve all run against the chosen target.
**Done when:** unit tests compute each target on a fixture, and a study of `spread_bps` against
`realized_variance` on the SOL fixture reports a positive IC.

### WB-04 Panel loader over every bar resolution — Blocked by WB-01
The study reads its panel through the SDK loader: 1-minute, 5-minute, and daily CSV bars as well
as tick-built lake bars. Book-only bases report "unavailable on this grid" instead of failing.
OHLCV bases: `return_n`, `range_bps`, `gap_bps`, `volume | zscore n`, `high_252_distance`.
**Done when:** a daily study of `return_1 | zscore 20` on `examples/data` runs end to end and a
test asserts the same IC from the CSV path and from an equivalent in-memory panel.

### WB-05 Exogenous series registry with availability times — Blocked by WB-04
`local.toml [[data.series]]`: name, path (CSV or parquet), kind `level` or `event`, optional
symbol column, and an `available_at` column or a fixed publication lag. Studies join series as-of
the bar's time using availability, never the nominal date. Funding and open interest from the
lake register automatically.
**Done when:** a test with a series whose `available_at` is after its nominal date proves the
bar before availability does not see the value and the bar after does; funding rate is usable as
a base in a SOL study.

### WB-06 Cross-sectional mode — Blocked by WB-04
For daily panels across many symbols: rank the feature across symbols per date, IC per date,
mean and t across dates, and a costless long-short decile portfolio with its equity curve.
**Done when:** a synthetic panel where the feature is the next-day return gives IC near 1 and the
long-short curve is monotone; the UI offers `time-series` and `cross-sectional` modes.

### WB-07 Aggregation transforms and event studies — Blocked by WB-05
`agg daily sum|mean|last|realized_var` lifts intraday series to the daily grid; slow series ride
fast grids by forward fill. For `event` series: average forward and backward return path around
events with counts and t per offset.
**Done when:** a test builds a daily realized variance from 1-minute fixture bars and matches a
direct computation; an event-study fixture with a known post-event drift reproduces it.

### WB-08 Diagnostics: incremental IC, stability, regime buckets — Blocked by WB-02
Incremental IC against an "accepted" feature set (regress out, score the residual); IC per day with
sign-consistency count; IC by spread tercile, realized-vol tercile, and hour of day; feature
autocorrelation.
**Done when:** a feature that is a linear copy of an accepted feature reports incremental IC near
zero in a test; the UI shows the per-day IC strip and the regime table.

### WB-09 Presets and promotion — Blocked by WB-02
Named feature expressions saved in SQLite, a "promote to accepted" action, and a parquet export of
accepted features plus targets for model fitting.
**Done when:** a preset survives a service restart, promotion changes what WB-08 orthogonalizes
against, and the export round-trips through `tessera parquet-schema`.

### WB-10 Study charts — Blocked by WB-02
IC decay across horizons, decile bars, the costless equity curve, and daily IC, as inline SVG in
the terminal style (see `EquityChart`).
**Done when:** each chart renders from a fixture result in the browser without console errors and
the study page opens at the top with the results grid first.

### WB-11 Non-overlapping costless curve for slow features — Blocked by WB-02
The WB-02 curve pays every bar the forward return of an `h`-bar hold, so a feature whose position
barely changes (spread_bps: turnover 0.002/bar on the SOL day) reports a breakeven of tens of bps
that no non-overlapping execution would earn. Add a variant that rebalances every `h` bars (or
holds until the position flips) and report its Sharpe and breakeven next to the per-bar ones.
**Done when:** a unit test shows the per-bar and rebalanced variants agree for `h = 1` and the
rebalanced breakeven of a constant-position feature is finite and far below the per-bar figure.

### WB-12 Studies form knows the grid's features and symbols — Blocked by WB-04
On a CSV grid the form still offers the order-book feature checkboxes (they come back as
"unavailable on this grid") and takes symbols as typed text. Hide or grey the book features when
a CSV grid is chosen, pick CSV symbols from the catalog with the run form's instrument picker,
and pre-tick the OHLCV set (`return_1`, `range_bps`, `gap_bps`, `high_252_distance`).
**Done when:** the browser check selects the daily grid and finds no order-book checkbox and a
catalog-backed symbol picker; a submitted daily study reports nothing unavailable.

### WB-13 Studies form lists the registered series — Blocked by WB-05
Series from `[[data.series]]` and the lake side feeds (funding_rate, funding_annualized,
open_interest, open_interest_usd) are usable as bases but only appear in the expression hint;
the form should list them as feature checkboxes with their kind and availability rule, from a
`GET /api/studies/series` endpoint, and grey out lake feeds on CSV grids.
**Done when:** the browser check on the studies form finds a checkbox per registered series and
a lake study submitted from it with `funding_rate` ticked produces that cell.

### WB-14 Realized vol and rolling std transforms, with second windows on the lake grid
`src/feature_expr.rs` has no volatility transform: `rv` for the regime buckets lives in
`study.rs` (`trailing_realized_variance`) and is not an expression. Add two transforms with
streaming state per symbol like the others: `rv n`, the square root of the sum of squared
bar-to-bar returns (bps) of the incoming series over the last n bars, gaps (NaN or
non-positive prices) skipped the way `trailing_realized_variance` skips them, NaN until n
returns are seen; and `std n`, the sample standard deviation of the last n values of any
series, NaN until the window is full. Windows become `Window::Bars(n) | Window::Seconds(n)`
(`30` or `30s`) for every windowed transform (`ema`, `sma`, `zscore`, `diff`, `lag`, `rate`,
`pct_rank`, `rv`, `std`), resolved against the grid's step when the study evaluates them; a
seconds window on a non-lake grid fails with a message naming the lake grid and the step.
Bars without a trade on the lake grid repeat the last close, so their zero returns count.
`docs/ADDING_A_STRATEGY.md`'s grammar section lists both transforms and the suffix. Serves
BT-1001.
**Done when:** unit tests in `src/feature_expr.rs` check `rv` and `std` against hand-computed
values on a fixture with a gap; show `rv 30s` at step 5 equal to `rv 6` and at step 1 equal to
`rv 30`; show `ema 60s` on the daily grid failing with text that names the lake grid; and a
synthetic 1-second grid with two vol regimes gives a mean `mid | rv 30` in the high regime
more than twice the low regime's. WB-01's parity test still passes unchanged.

### WB-15 realized_vol target and a seeded vol feature set — Blocked by WB-14
Add `Target::RealizedVol` in `src/study.rs`: the square root of `Target::RealizedVariance`,
name `realized_vol`, unit bps, in `Target::ALL` and `parse`; `STUDY_TARGETS` in
`web/app/page.tsx` lists it as "Realized vol (bps)". Add `seed_feature_presets` beside the
other seeders in `src/bin/tessera_ui.rs`: on start, insert into `feature_presets` by name
when absent: "rv 5s" (`mid | rv 5s`), "rv 30s", "rv 60s", "vol ratio 5s/60s" (`mid | rv 5s |
ratio_to rv 60s`), and "vol of vol 60s" (`mid | rv 5s | std 60s`). `docs/LOCAL_UI.md` and
`docs/ADDING_A_STRATEGY.md` name the target and the seeded set. Serves BT-1002.
**Done when:** a unit test computes `realized_vol` on a fixture as the root of
`realized_variance`; a synthetic 1-second panel with persistent vol regimes, studied with
`mid | rv 30` at horizon 30, gives IC above 0.5 against `realized_vol` and below 0.1 against
`return`; a service test shows the five presets after a fresh catalog and no duplicates after
a second start; `web/scripts/chart-check.mjs` finds the `realized_vol` option in the form's
target select.

### WB-16 Promotion freezes the study and the IC a feature was promoted with
A promoted preset stores only its expression, so nothing can rescore it apples to apples.
`feature_presets` gains `promoted_grid`, `promoted_symbols`, `promoted_horizon`,
`promoted_target`, and `baseline_ic` (nullable); `POST /api/features/{id}/promote` and the
results-grid promotion fill them from the study the cell came from, and a library promotion
without a study leaves them null for WB-17's first run to fill. The library panel shows the
promoted study beside a promoted feature. Serves BT-1003. Decisions: 0001.
**Done when:** a service test promotes a cell from a stored study and reads the five fields
back; promoting from the library leaves them null;
`feature_presets_survive_reopening_the_catalog` still passes; `web/scripts/chart-check.mjs`
finds the promoted-study text on an accepted row of the fixture library.

### WB-17 A nightly job rescores promoted features and keeps their IC history — Blocked by WB-16
Add the automation kind `feature_decay` to `automation_schedules` (seeded disabled, local
time 02:30, weekdays all, like the other kinds) and the table `feature_ic_history(preset_id,
date, ic, observations, status, reason)` with status in {baseline, ok, watch, alert, skipped}.
A promoted feature whose frozen study no longer exists gets a skipped row with reason
`study missing` (grill-me, HK-34 run).
The job runs each promoted feature on its frozen study over the trailing 20 sessions: a null
baseline is set from this run and recorded as `baseline`; trailing IC under half the
baseline or of opposite sign is `watch` when the previous row was not, and `alert` when it
was `watch` or `alert`; no bars for a symbol in the window, a base the grid cannot supply, or
a horizon beyond the window is `skipped` with the reason, never watch or alert. The
schedule's `last_status` says how many were scored and skipped. Serves BT-1003. Decisions:
0001.
**Done when:** a service test against an in-memory catalog builds a synthetic panel whose
feature predicts the target for 40 sessions and then stops, with one quiet blip earlier, and
runs the job day by day: `baseline` on day one, `ok` through the blip, `watch` on the first
breach, `alert` on the second, and a `skipped` row with its reason on a day with no bars, on
a day whose base the grid lacks, and on a day whose horizon exceeds the window; the schedule
row's `last_status` reads "scored N, skipped M".

### WB-18 The studies page shows decayed features and the research log records them — Blocked by WB-17
The studies page (`StudiesWorkspace` in `web/app/page.tsx`) gains a `DCY FEATURE DECAY`
panel directly under the `LIB` feature library, from `GET /api/features/decay`: the title
carries the counts and the last run date; one row per promoted feature with feature, study,
baseline IC, trailing IC, observations, status, and reason, in the terminal look, dense under
1500 px. Alerts come first, then watches, then skipped rows (`no base`, `horizon > window`,
`data gap`, `study missing`); ok and baseline rows sit behind a show-all, and alerts and
watches cap at twenty behind the same show-all. Clicking a row opens the frozen promotion
study's results (the study WB-16 froze); a `study missing` row opens nothing. With nothing
promoted, or before the job's first run, one line names the next step and links the
automation page. Each nightly run that produced a new alert appends one dated line per alert
to the private research log (`../Tessera-private/docs/research-log.md`, the file
`/nightly-studies` writes) naming the feature, the study, and the two ICs. Serves BT-1003.
Wireframe: BT-1003. Decisions: 0002.
**Done when:** `web/fixtures/feature-decay.json` carries an alert, a watch, a skipped, a
baseline-set, and an ok row, and `web/scripts/chart-check.mjs` fails unless the first three
render with their numbers under the library panel at 1280 px, the last two appear only after
show-all, clicking the alert row puts that study's IC-by-horizon chart on the page, and no
console error appears; a service test shows one log line per new alert and none for a
repeat; `docs/LOCAL_UI.md` names the panel.

## Data sources

### DS-01 The Data page is three views: Inventory, Instrument search, Updates & schedules
`view === "data"` in `web/app/page.tsx` renders `DataSourcesPanel`, the DATA LIBRARY metrics,
`DataCoverage`, and `AutomationsWorkspace` in one column. A tab strip under the title selects a
`dataView` (inventory, instruments, updates) remembered in browser storage; Inventory keeps the
sources panel and the library metrics with the run-coverage panel folded closed at the bottom
(BT-607 moves it later); Instrument search is a query box over `GET /api/instruments` listing
symbol, name, venue, class, currency, status, and the resolution flags, with query and
selection held in app state so they survive leaving the workspace; Updates & schedules holds
`AutomationsWorkspace`. Each view opens at the top. `docs/LOCAL_UI.md` describes the three
views. Serves BT-608. Wireframe: BT-608. Decisions: 0010, 0014.
**Done when:** `web/scripts/data-page-check.mjs` (new; `npm run data-page-check`, run by
`scripts/check.sh`) serves the bundle with `/api/data/sources`, `/api/data/status`,
`/api/automations`, and `/api/instruments` answered from `web/fixtures/data-sources.json` and
fails unless each view opens at the top, Inventory shows the sources panel first, and a query
typed in Instrument search is still there after opening Studies and coming back;
`web/scripts/layout-check.mjs` measures the three views at 1280 and 1440 px in both modes.

### DS-02 A Provider trait and the EODHD adapter's read-only calls
New `src/provider/mod.rs` in the library crate: `trait Provider` with `verify(token)` returning
an account (requests today, daily limit, resets at, plan), `exchanges()` returning code, name,
country, and resolutions, and `symbols(exchange, delisted)` returning code, name, type, and
currency; errors distinguish CredentialsRejected, Unreachable, and Malformed.
`src/provider/eodhd.rs` implements it over reqwest (rustls) with a configurable base URL
(the user, exchanges-list, and exchange-symbol-list endpoints). No service wiring yet.
`docs/DATA_SOURCES.md` gains a Providers section. Decisions: 0020.
**Done when:** `tests/provider_eodhd.rs` starts a stub axum server on a free port serving
`tests/fixtures/eodhd/{user,exchanges-list,exchange-symbol-list-US}.json` (recorded once,
token scrubbed, trimmed to a few rows) and asserts the parsed account, exchanges, and
listings; a 401 body yields CredentialsRejected, a 503 and a dropped connection yield
Unreachable, and the token travels as a query parameter that no log line prints.

### DS-03 Sources are registered from the console; the token lives in a 0600 file — Blocked by DS-02
Table `data_sources` (id, name, kind, root, catalog_dir, reserve_pct, token_set_at,
verified_at, verify_state, verify_message, created_at) in `src/bin/tessera_ui.rs`.
`POST /api/sources` verifies the token through the adapter and refuses a rejected one (422
with the provider's message, nothing saved), writes `data/ui/secrets/<id>.token` with mode
0600, then inserts; `GET /api/sources` lists cards with the root volume's used, free, and
total (statvfs on the root) and the connection state; `PUT /api/sources/{id}/token`
replaces; `POST /api/sources/{id}/verify` re-checks; `DELETE /api/sources/{id}` refuses (409)
when any file lies under the root's dataset folders. The token and its path appear in no
response. Serves BT-1201. Decisions: 0020, 0021.
**Done when:** a `#[cfg(test)]` service test against an in-memory catalog and the DS-02 stub
registers a source with a known token, fetches every `/api/sources` and `/api/data` response,
and fails if the token string or the secrets path appears; asserts the file mode is 0600;
asserts a rejected token leaves no row and no file; asserts DELETE returns 409 with a file
under the root and removes row and file without one.

### DS-04 The provider's availability is cached and served — Blocked by DS-03
Tables `provider_exchanges` (source_id, code, name, country, resolutions, fetched_at) and
`provider_listings` (source_id, exchange, code, name, type, currency, delisted, fetched_at).
`POST /api/sources/{id}/availability/refresh` fetches the exchange list and the listings of
every exchange with a dataset (or the one named in the body); `GET
/api/sources/{id}/availability` returns the cached table with per-type counts, `fetched_at`,
and an unreachable note when the last refresh failed, keeping the previous rows. Serves
BT-1202. Decisions: 0013, 0020. The `SourceAdapter` enum from DS-03 forwards `exchanges` and `symbols` and gains a helper that builds the adapter from the token on file; the environment-overrides paragraph of `docs/DATA_SOURCES.md` lists `TESSERA_EODHD_BASE_URL`.
**Done when:** a service test seeds a source over the stub, refreshes, and asserts the
exchange rows and the US type counts; makes the stub return 503, refreshes again, and asserts
the rows and `fetched_at` are unchanged with the note set.

### DS-05 Datasets, the scan cache, and disk usage — Blocked by DS-04
Table `datasets` (id, source_id, exchange, types_json, resolution, from_date, folder,
include_delisted, created_at) and `dataset_scans` (dataset_id, scanned_at, listed, on_disk,
latest_date, current_count, bytes, uncataloged_json, state, error). `POST
/api/sources/{id}/datasets` validates against the cached listing; `DELETE /api/datasets/{id}`
refuses (409) with files present. `POST /api/sources/{id}/scan` runs a background scan job:
per dataset, the files for its listed symbols (last date tail-read through
`last_csv_row_date`), bytes, the count current through the latest expected session (the
calendar symbol's last date for daily, the last session's close for intraday), Uncataloged
files per folder under the root, and the BT-605 state; a failed scan keeps the previous row.
`GET /api/sources` returns each dataset with its last scan. Serves BT-1203. Decisions: 0012,
0013, 0021. A second source over the same root is refused (409), and `holds_files` from DS-03 narrows to the source's dataset folders once datasets exist. DS-04's refresh caches active listings only: it gains a `delisted` flag on the body (one extra call per exchange, cached with `delisted = 1`) so a dataset with include delisted can count and backfill them.
**Done when:** a service test builds a temp root with `eod/` holding files for three listed
symbols, one `.part`, and a stray folder, registers a US EOD dataset over the stub listing of
five symbols, scans, and asserts listed 5, on disk 3, the latest date, bytes, the `.part`
excluded, the stray folder Uncataloged with its count, and state Partial; removes the folder,
rescans, and asserts Unavailable with the previous counts kept; asserts DELETE is 409 with
files and 200 without.

### DS-06 The Inventory view: source cards, datasets, add forms, and the availability panel — Blocked by DS-05
`DataSourcesPanel` in `web/app/page.tsx` becomes source cards from `GET /api/sources`: header,
connection state, volume line, credits line (a placeholder until DS-07), the datasets table
(dense under 1500 px), the Uncataloged line, Add dataset inline; Add source inline (kind,
name, root, catalog, token as a password field); Replace token; Rescan per source; the
"Available from <source>" panel with filter, expand-to-fetch, and Refresh. The token field is
cleared after save and never rendered again. `docs/LOCAL_UI.md` and `docs/DATA_SOURCES.md`
describe registering a source and adding a dataset. Serves BT-1201, BT-1202, BT-1203.
Wireframes: BT-1201, BT-1202, BT-1203. Decisions: 0003, 0014, 0021. Add source pre-fills root and catalog from `/api/data/sources` so the first EODHD source adopts the existing library (BT-1201's last criterion); the library metrics tile shows the freshness time as a date and time, not the raw ISO string, so it fits at 1280 px.
**Done when:** `web/fixtures/data-sources.json` seeds two sources (one Credentials rejected),
five datasets, an Uncataloged folder, and the US and LSE availability rows;
`web/scripts/data-page-check.mjs` fails unless the panels appear in wireframe order, the
rejected source shows its state, the datasets table shows every column, the availability
panel shows "listed <time>", and no element's text contains the fixture's token or secrets
path; `web/scripts/layout-check.mjs` passes at 1280 and 1440 px.

### DS-07 Credits: usage on the card and a call budget every job obeys — Blocked by DS-03
`src/provider/budget.rs`: `CallBudget { limit, used, reserve }` with estimate helpers,
`can_start(mandatory_calls)`, `charge(n)`, and `at_reserve()`; the source record stores
requests today, daily limit, resets at, and when usage was checked, refreshed by `GET
/api/sources` (through the adapter, cached for a minute) and after every job; `PUT
/api/sources/{id}` sets `reserve_pct`. The card renders used / limit, the reset time, and the
reserve field. Serves BT-1204. Decisions: 0022. EODHD returns no reset time: the adapter derives 00:00 UTC after the usage date, and the card labels it "resets 00:00 UTC".
**Done when:** unit tests in `budget.rs` cover `can_start` at the boundary, `charge` past the
reserve, and a limit of zero; a service test asserts `GET /api/sources` shows the stub's usage
and, with the stub down, the last value with its time; `data-page-check.mjs` asserts the
credits line.

### DS-08 The native EOD download job — Blocked by DS-05, DS-07
`Provider` gains `bulk_eod(exchange, date)`, `eod_history(symbol, from)`, and
`splits(exchange, date)`; `src/provider/jobs/eod.rs` runs a dataset: the sessions after its
latest date (bulk, one call each, refused when the calendar symbol is missing or the rows are
under the dataset's minimum), one appended row per file, backfill of missing symbols
(delisted once), refetch and replace of split symbols, regeneration of `catalog.csv`,
`stocks.txt`, and `etfs.txt` in the source's catalog folder, part-then-rename throughout,
progress and counts on the record. Table `dataset_jobs` (id, dataset_id, kind, state,
percent, started_at, finished_at, calls, added, updated, skipped_json, error, log_path);
`POST /api/datasets/{id}/update` queues one; a second on the same source is 409 naming the
running id; a missing, unwritable, or unmounted folder is refused before any call. Serves
BT-1205. Decisions: 0020, 0022.
**Done when:** `tests/provider_eod_job.rs` over the stub and a temp root proves: two sessions
appended to three files; a fourth listed symbol backfilled from the from-date; a seeded split
rewrites that symbol's whole file on the new basis; a bulk day without SPY writes nothing and
the job is Failed with the count; a rerun adds and updates zero; a reserve that admits only
one backfill leaves no `.part` visible and the rerun continues from the missing symbol; a
removed folder is refused with no call made; the catalog files match the listing.

### DS-09 The native intraday increment job — Blocked by DS-08
`Provider` gains `intraday(symbol, resolution, from, to)`; `src/provider/jobs/intraday.rs`
extends every existing file from its last timestamp in the provider's window for the
resolution (five calls a request), then backfills missing symbols, budgeted, part-then-rename,
skipping a symbol with no rows for the rest of the job, writing today's intraday columns.
Serves BT-1206. Decisions: 0020, 0022.
**Done when:** `tests/provider_intraday_job.rs` over the stub proves a 5m file is extended
across two windows with the right call count, a missing symbol is backfilled, a no-rows symbol
is skipped with its reason, a rerun writes nothing, and the reserve stops the backfill cleanly.

### DS-10 Per-dataset schedules and the Updates & schedules view — Blocked by DS-08
`automation_schedules` gains `dataset_id`; kind `dataset_update` queues the dataset's job
from `execute_automation`, or records "skipped: job <id> running" when the source is busy;
`automation_runs` (schedule_id, ran_at, status) keeps the last seven outcomes served on each
schedule; `GET /api/datasets/jobs` lists jobs newest first with a cap. The view renders the
Updates table and the Schedules table with Run now, Pause, the seven marks, and Add schedule
inline listing the registered datasets; the log opens from the row. `docs/LOCAL_UI.md`
describes the view. Serves BT-1207. Wireframe: BT-1207. Decisions: 0001, 0014.
**Done when:** a service test with an in-memory catalog seeds a `dataset_update` schedule,
runs `execute_automation` with the source idle and asserts a queued job and a mark, then
with a running job and asserts the skipped status and no second job; `data-page-check.mjs`
asserts both tables, the seven marks, and that Add schedule lists the fixture's datasets.

### DS-11 Retire the update command, the freshness file, and the DATA LIBRARY panel — Blocked by DS-10
Remove `update_command`, `freshness_file`, and `provider` from `LocalConfig`
(`src/local_config.rs`) and `local.example.toml`, the `data_updates` table and
`queue_eod_update`, `run_eod_update`, and `start_eod_update`, the `data_update` schedule kind
and its seed, and the DATA LIBRARY panel; the status strip's latest EOD date comes from the
calendar symbol's file. `docs/DATA_SOURCES.md` and `docs/LOCAL_UI.md` describe sources only.
Serves BT-1207. Decisions: 0001, 0021. A `dataset_update` schedule whose dataset is deleted is removed with it (DS-10 leaves the row with a null dataset and runs that fail), and `DELETE /api/datasets/{id}` says how many schedules went with it.
**Done when:** `grep` finds none of the removed keys under `src/`, `web/app/`, `docs/`, or
`local.example.toml`; the config tests in `local_config.rs` pass without them;
`data-page-check.mjs` no longer expects the panel. No new test: removal only.

### DS-12 Exchange resolutions come from the provider, not from a table in the adapter — Blocked by DS-04
DS-02's EODHD adapter fills each exchange's resolutions from a table written from EODHD's public
docs (daily, 1h, and 5m everywhere; 1m for US, FOREX, and CC), because the exchanges-list
endpoint says nothing about intraday coverage. The availability panel then shows resolutions the
account may not have. When an exchange row is expanded or a dataset is added against it, the
service probes each intraday resolution once (one call per resolution, charged to the budget of
DS-07) and stores what the provider actually served in `provider_exchanges`, with the probe time;
a probe that returns no data removes that resolution from the row. Serves BT-1202. Decisions:
0020, 0022.
**Done when:** a service test over the stub lets the exchange list claim 1m for LSE, answers the
1m probe with no data and the 5m probe with rows, and asserts the cached row lists 5m and not 1m
with a probe time; a second expansion makes no further call.

### DS-13 The usage refresh has a short timeout and runs across sources at once — Blocked by DS-07
`GET /api/sources` (DS-07) refreshes each source's usage in turn through the adapter's `/api/user`
call, whose request timeout is the adapter's general 60 s; a provider that hangs rather than
fails fast holds the Inventory listing for up to a minute per source. The usage call gets its
own short timeout (5 s) and the refresh runs the sources concurrently (`join_all`), each
recording `unreachable` on its own; the listing returns as soon as the slowest answers or times
out. Serves BT-1204. Decisions: 0020.
**Done when:** a service test registers two sources over two stubs, one of which never
answers, and asserts `GET /api/sources` returns within 10 s with the answering source
`connected` and the other `unreachable`, both with their last usage and `checked_at` intact.

### DS-14 Catalog files are regenerated per exchange, delisted apart — Blocked by DS-08
DS-08's EOD job regenerates `catalog.csv`, `stocks.txt`, and `etfs.txt` in the source's catalog
folder root from the dataset's exchange, so a source with datasets on several exchanges makes
each job overwrite the others' files, and the instrument index (`build_instrument_index` in
`src/bin/tessera_ui.rs`) expects the non-US exchanges as sub-catalogs (`CC/catalog.csv`,
`FOREX/catalog.csv`) and the delisted list under `delisted/`, the layout `docs/DATA_SOURCES.md`
describes. The job writes the US exchange's files at the root and every other exchange's under
`<EXCHANGE>/`, and writes `delisted/catalog.csv` and its symbol list from the cached delisted
listing, all part-then-rename. Serves BT-1205. Decisions: 0012, 0022.
**Done when:** `tests/provider_eod_job.rs` runs the job for a US dataset and a CC dataset on one
source and asserts the root files hold only US rows, `CC/catalog.csv` holds the CC rows, and
`delisted/catalog.csv` holds the delisted US rows with today's columns; a service test asserts
the instrument index built from that folder lists a CC and a delisted symbol.

### DS-15 The EOD nightly's call discipline before the cut-over — Blocked by DS-08
Three things DS-08 left that would waste calls or misjudge a night once the job runs on a US
dataset with delisted included: a symbol whose history comes back empty is skipped with its
reason but retried on every run, one call each, so thousands of delisted names would burn the
budget nightly; `Estimate::eod` (`src/provider/budget.rs`) counts no split refetches, so the
mandatory estimate understates a night with splits; and the job's through-date is New York's
today for every exchange. The job keeps a per-dataset skip list (`dataset_skips`: symbol, reason,
first seen, retry after) and retries a skipped symbol only after a configurable interval
(default 30 days); the estimate adds a split allowance (the dataset's average splits per session
over its recorded jobs, at least one); the through-date is the exchange's own local date from
the cached exchange row. Serves BT-1204, BT-1205. Decisions: 0022. DS-09's intraday job has the same two costs: every existing file spends one request (five calls) per run just to read its last bar back, about 50,000 calls a night for a US-sized 5m dataset that is already current, so files whose last bar reaches the latest expected session close (the scan's expected session) are skipped without a call; and a no-bars symbol is skipped for one run only, so it joins the same skip list with the same retry interval.
**Done when:** `tests/provider_eod_job.rs` proves a no-history symbol costs one call on the first
run and none on the second, and a call again after the interval; a budget unit test proves the
split allowance is in the mandatory estimate; a job test on an exchange whose local date is ahead
of New York plans the extra session.; `tests/provider_intraday_job.rs` proves a current 5m file costs no request and a no-bars symbol is not asked again on the next run.

## Housekeeping

### HK-43 The scratch console from a worktree finds the engine that was just built
`scripts/scratch-console.sh` links the scratch root's `target` to the checkout's own `target/`,
and the service resolves the engine for the SDK strategy sync at `<root>/target/release/tessera`.
From a worktree, `scripts/check.sh` builds into the shared `target-worktrees/` (HK-27), so the
worktree's `target/` holds no binaries: the scratch console starts, logs `SDK strategy sync
failed: tessera engine is not built`, its catalog stays empty, and `start` times out waiting
for it (found while working UI-06; copying the two binaries into the worktree's
`target/release/` was the workaround). The script should give the scratch root the engine
from `$CARGO_TARGET_DIR/release` when that is set (a link per binary, or the service taking
the engine path from an environment variable the script sets).
**Done when:** `CARGO_TARGET_DIR=<shared> scripts/scratch-console.sh start --port 8793` from a
worktree whose own `target/` is empty seeds the strategy catalog and the run; the CI web job,
which starts the scratch console from the main checkout, is unaffected.

### HK-45 The coverage fold loads the newest completed run with no run id in the code
`openCoverage` in `web/app/page.tsx` (from DS-01, inherited from the old `openData`) prefers a
hard-coded run id from the owner's private catalog before falling back to the newest completed
run; a private run id has no place in the public repo (AGENTS.md, Layout), and the fold should
simply take the newest completed run from `/api/runs`. Remove the literal and the preference.
Serves BT-607. Decisions: 0011.
**Done when:** `grep -n 'run-2026' web/app/page.tsx` finds nothing, and
`web/scripts/data-page-check.mjs` opens the coverage fold against the fixture's runs and asserts
it loaded the newest completed one.

### HK-46 The proof gate counts `#[tokio::test]` as a test line
`proof_pattern` in `.loop.toml` is `#\[test\]|#\[cfg\(test\)\]`, so a change under `src/`
proven by an async service test (`#[tokio::test]`, the norm in `src/bin/tessera_ui.rs` since
DS-03) fails the gate until a plain `#[test]` is added beside it (DS-04 hit this). The pattern
becomes `#\[(tokio::)?test\]|#\[cfg\(test\)\]` here, and the kit's default and its self-test
learn the same in `coding-agent-loop`, tagged and synced through `kit_ref` as `AGENTS.md`
requires.
**Done when:** `scripts/check.sh --self-test` (the proof gate's) passes a fixture commit whose
only proof line is `#[tokio::test]`, and `scripts/loop-kit-sync.sh --check` is clean at the new
`kit_ref`.

### HK-48 Coverage runs from two worktrees do not clobber each other
`scripts/coverage.sh` builds the instrumented crate into one shared
`target-worktrees/llvm-cov-target`, so two lanes running the check at once corrupt each other's
profiling data: DS-09's and DS-10's `scripts/coverage-ratchet.sh --set` each failed once with an
empty JSON from `cargo llvm-cov`, whose stderr the script hides behind `2>/dev/null`, and passed
on a rerun. The coverage build goes into a per-worktree subdirectory
(`target-worktrees/llvm-cov-<worktree name>`, the dependency crates still shared through
`CARGO_TARGET_DIR` for the ordinary build), and llvm-cov's stderr is kept in the check's log.
**Done when:** `scripts/coverage.sh --self-test` (new, run by `scripts/check.sh`) starts two
coverage runs from two scratch worktrees at once and both report a percentage; a run whose
`cargo llvm-cov` fails prints its stderr instead of an empty result.
