# Decisions

One file per decision, never edited in place: a change is a new record that supersedes
the old one. Cite a decision by its number. `scripts/decisions.sh new "<title>"` adds one.

- [0001](0001-scheduled-research-work-runs-inside-the-service-s-scheduler.md) Scheduled research work runs inside the service's scheduler — accepted
- [0002](0002-alerts-surface-in-the-console-and-the-research-log-never-out.md) Alerts surface in the console and the research log, never outbound — accepted
- [0003](0003-run-configuration-is-edited-in-a-modal-dialog-a-page-shows-a.md) Run configuration is edited in a modal dialog; a page shows a summary and a Run button — accepted
- [0004](0004-position-size-is-notional-exposure-as-a-percentage-of-curren.md) Position size is notional exposure as a percentage of current account equity — accepted
- [0005](0005-maximum-leverage-caps-portfolio-gross-notional-over-current-.md) Maximum leverage caps portfolio gross notional over current account equity — accepted
- [0006](0006-fixed-position-targets-100-is-1-0x-of-equity-and-unused-capa.md) Fixed position targets: 100% is 1.0x of equity, and unused capacity is not reassigned — accepted
- [0007](0007-position-size-and-leverage-are-sensitivity-axes-never-optimi.md) Position size and leverage are sensitivity axes, never optimizer-selected by default — accepted
- [0008](0008-signal-parameters-declare-whether-they-are-editable-and-opti.md) Signal parameters declare whether they are editable and optimizer-eligible — accepted
- [0009](0009-strategies-data-libraries-and-the-engine-ui-install-independ.md) Strategies, data libraries, and the engine/UI install independently — accepted
- [0010](0010-the-data-workspace-describes-the-market-data-registered-for-.md) The Data workspace describes the market data registered for future runs — accepted
- [0011](0011-a-run-s-frozen-data-provenance-lives-on-its-detail-page-not-.md) A run's frozen data provenance lives on its detail page, not in the Data inventory — accepted
- [0012](0012-data-inventory-segments-by-the-provider-s-classifications-co.md) Data inventory segments by the provider's classifications, Common Stock and ETF distinct — accepted
- [0013](0013-coverage-reads-a-persistent-fast-cache-deeper-completeness-a.md) Coverage reads a persistent fast cache; deeper completeness analysis is requested explicitly — accepted
- [0014](0014-the-data-workspace-is-inventory-instrument-search-and-update.md) The Data workspace is Inventory, Instrument search, and Updates & schedules — accepted
- [0015](0015-historical-replay-and-live-operation-share-one-causal-event-.md) Historical replay and live operation share one causal event contract — accepted
- [0016](0016-strategy-scope-is-explicit-isolated-state-per-instrument-or-.md) Strategy scope is explicit: isolated state per instrument or one portfolio instance — accepted
- [0017](0017-strategies-emit-broker-neutral-order-intents-adapters-own-ex.md) Strategies emit broker-neutral order intents; adapters own execution and accounting — accepted
- [0018](0018-a-live-broker-interface-is-not-live-trading-readiness.md) A live-broker interface is not live-trading readiness — accepted
- [0019](0019-the-console-s-left-menu-collapses-to-an-icon-rail-the-choice.md) The console's left menu collapses to an icon rail; the choice is per-browser — accepted
- [0020](0020-provider-adapters-are-native-rust-modules-in-the-service-beh.md) Provider adapters are native Rust modules in the service behind one trait — accepted
- [0021](0021-data-sources-and-datasets-are-registered-from-the-console-cr.md) Data sources and datasets are registered from the console; credentials live in a 0600 file per source, never in the database — accepted
- [0022](0022-a-provider-download-job-is-budgeted-exclusive-per-source-and.md) A provider download job is budgeted, exclusive per source, and fails closed on its folder — accepted
