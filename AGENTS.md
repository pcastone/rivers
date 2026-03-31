# Repository Guidelines

## Project Structure & Module Organization
`Cargo.toml` defines a Rust workspace with crates under `crates/`. The main binaries live in `crates/riversd` (server) and `crates/riversctl` (CLI); shared runtime APIs live in `crates/rivers-runtime`; plugins and engines follow the `rivers-plugin-*` and `rivers-engine-*` patterns. Keep implementation code in `crates/<crate>/src` and integration tests in `crates/<crate>/tests`.

Supporting assets are organized by purpose: `address-book-bundle/` and `examples/` hold sample bundles, `config/` and `packaging/` contain config and packaging assets, `scripts/` contains release helpers, and `docs/` stores plans and notes.

## Build, Test, and Development Commands
- `just build`: release static build for the workspace.
- `just build-debug`: fast debug build while iterating.
- `just check`: compile-check all crates without producing release binaries.
- `just test`: run the full test suite with `cargo test`.
- `just build-dynamic`: build thin binaries plus shared libraries in `release/dynamic/`.
- `just package-deb`, `just package-rpm`, `just package-windows`, `just package-tarball`: create distribution artifacts.

For focused work, use Cargo directly, for example `cargo test -p riversd --test websocket_tests`.

## Coding Style & Naming Conventions
Use Rust 2021 defaults with four-space indentation and standard `rustfmt` formatting; run `cargo fmt --all` before opening a PR. Follow existing Rust naming conventions: `snake_case` for files, modules, and functions; `CamelCase` for structs/enums/traits; `SCREAMING_SNAKE_CASE` for constants and environment variables.

Match the repository’s modular style: keep `lib.rs` and `mod.rs` thin, and split large subsystems into focused sibling modules instead of growing monolithic files.

## Testing Guidelines
Place unit tests close to the code they cover and integration tests under `crates/*/tests`. Async tests typically use `#[tokio::test]`. CI currently mirrors `cargo test --workspace --lib`, so run that locally.

Some driver tests are live/integration checks and depend on local services plus `RIVERS_TEST_*` host variables such as `RIVERS_TEST_PG_HOST` or `RIVERS_TEST_REDIS_HOST`. Document any new test-only environment variables in `README.md`.

## Commit & Pull Request Guidelines
Recent history follows Conventional Commit-style messages like `refactor(rivers-runtime): split config_tests.rs into 4 test modules`; keep that format. Scope commits to a single crate or subsystem when practical.

Pull requests should summarize behavior changes, list affected crates, note config or packaging impacts, and include the commands you ran to validate the change. Link the relevant issue or plan doc when one exists.

## Security & Configuration Tips
Do not commit secrets, local keystores, generated release artifacts, or runtime logs. When changing runtime paths, service names, or default config, review both `packaging/` and `scripts/` so release, tarball, and system package layouts stay aligned.
