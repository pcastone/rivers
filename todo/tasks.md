# Tasks — cargo-deploy

> **Branch:** `deployment`
> **Goal:** Create `cargo deploy <path>` subcommand that builds and deploys Rivers to a target directory.

---

## T1: Create cargo-deploy crate

- [x] **T1.1** Add `crates/cargo-deploy/Cargo.toml` to workspace
- [x] **T1.2** Create `crates/cargo-deploy/src/main.rs` — arg parsing, usage/help

## T2: Dynamic deploy (default)

- [x] **T2.1–T2.7** Build binaries (no-default-features), engines, plugins; copy to bin/, lib/, plugins/; generate TLS cert; write VERSION

## T3: Static deploy (--static flag)

- [x] **T3.1–T3.5** Build with default features (all static); copy binaries to bin/; generate TLS cert; write VERSION

## T4: Commit & PR

- [ ] **T4.1** Commit and open PR to main
