# Contributing

Thanks for looking. voidflare is small on purpose: every converter is a thin app over an existing engine.

## Reporting a problem

Open an issue with:

- what you dropped (type and, if you can share it, a sample or a description),
- what you chose,
- what you expected and what happened,
- your Windows version.

For a conversion that looks wrong, say whether the same file converts correctly in the engine's own program. Many problems come from the engine, and then the right place for the report is its project.

## Translations

English is the source. Texts live in tab-separated catalogs (`strings.tsv` per app and `vf-ui`'s shell catalog), one row per key with the columns `key en it es fr de`. A missing translation falls back to English. Native-speaker corrections are the most useful contribution we can get.

## Code

- `cd src` and `cargo test --release -p vf-core` must pass; tests that need an engine skip themselves when none is installed.
- Keep the apps offline: no network access without asking the user first.
- Match the surrounding code: naming, comment density, idiom.
- By contributing you agree that your work is licensed under MIT OR Apache-2.0, like the rest of the project.
