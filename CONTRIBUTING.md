# Contributing

Thanks for taking a look. The engine, SDK, and UI are open for contributions; strategies
themselves usually belong in your own private folder (see "Private strategies" in the README).

## Before opening a pull request

```bash
cargo fmt --all
cargo test
cd web && npm run lint && npm run build
```

CI runs the same commands plus one example strategy against the bundled synthetic data.

## What makes a good change

- **Engine and SDK**: keep simulation deterministic. Anything that touches fills, stops,
  entry arbitration, or costs needs a test in `src/event_engine.rs` or `src/sdk/` that pins
  the behaviour.
- **Indicators**: add them to `src/sdk/indicators.rs` with a unit test and export them from
  the prelude.
- **UI**: the terminal look is deliberate. Match the existing palette and density rather
  than introducing new component styles.
- **Docs**: `docs/ADDING_A_STRATEGY.md` is the user-facing SDK reference; update it when the
  authoring surface changes.

## Reporting bugs

Include the strategy file (or a minimal one that reproduces the issue), the run
configuration, the date range, and the engine output. Runs against the bundled
`examples/data` are the easiest to reproduce.

## License

Contributions are accepted under the repository's AGPL-3.0 license.
