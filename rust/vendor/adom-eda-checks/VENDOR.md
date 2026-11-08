# Vendored: adom-eda-checks

The EDA Bridge Standard's shared checks engine, copied unchanged from adom/eda-bridge-standard `engine/` (the wiki
has no git protocol, so Cargo cannot fetch it).

- Standard version: 0.30.0
- Standard commit: 25dd7d1e87e961f5fa197a728a1f2b2c3025175a
- Crate version: 0.24.0
- Copied: 2026-10-30, by fusion-lane

To update: clone the standard fresh (`adom-wiki repo clone adom/eda-bridge-standard`), copy `engine/src`,
`engine/Cargo.toml` and `engine/README.md` here, record the new version and commit above, and run the bridge's tests.
Never edit the copy: fixes go to the standard (an issue titled `fusion-lane: ...`).
