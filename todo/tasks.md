# Tasks — Epic 1: Foundation — ValidationReport + Error Codes + Formatters

> **Branch:** `feature/art-of-possible`
> **Source:** `docs/arch/rivers-bundle-validation-spec.md` (Sections 8, 9, 11, Appendix A)
> **Goal:** Create foundational types and formatters for the 4-layer bundle validation pipeline

---

## Sprint 1.1 — ValidationReport types (`validate_result.rs`)

- [x] 1. Create `validate_result.rs` with `ValidationSeverity` enum (Error, Warning, Info)
- [x] 2. `ValidationStatus` enum (Pass, Fail, Warn, Skip) for individual results
- [x] 3. `ValidationResult` struct (status, file, message, error_code, table_path, field, suggestion, line, column, exports, etc.)
- [x] 4. `LayerResults` struct (passed, failed, skipped count + results vec)
- [x] 5. `ValidationReport` struct (bundle_name, bundle_version, layers map, summary)
- [x] 6. `ValidationSummary` struct (total_passed, total_failed, total_skipped, total_warnings, exit_code)
- [x] 7. Error code constants: S001-S010, E001-E005, X001-X013, C001-C008, L001-L005, W001-W004
- [x] 8. Builder methods: `report.add_result(layer, result)`, `report.exit_code()`, `report.has_errors()`
- [x] 9. Unit tests for report builder

## Sprint 1.2 — Text + JSON formatters (`validate_format.rs`)

- [x] 10. Text formatter matching spec section 8.1 output format
- [x] 11. JSON formatter matching spec section 8.2 contract
- [x] 12. `did_you_mean()` Levenshtein helper (distance <= 2)
- [x] 13. Unit tests for both formatters and Levenshtein helper

## Integration

- [x] 14. Export modules from `lib.rs`
- [x] 15. `cargo check -p rivers-runtime` passes
- [x] 16. `cargo test -p rivers-runtime -- validate_result validate_format` passes

---

## Validation

- `cargo check -p rivers-runtime` — compiles clean
- `cargo test -p rivers-runtime -- validate_result validate_format` — all tests pass
