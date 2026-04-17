# Rivers Filesystem Driver — Implementation Changelog

### 2026-04-16 — OperationDescriptor framework baseline
- Files: crates/rivers-driver-sdk/src/{operation_descriptor.rs,traits.rs,lib.rs}
- Summary: new types (OpKind, OperationDescriptor, Param, ParamType) + opt-in DatabaseDriver::operations() method; all existing drivers build and test without modification.
- Spec: rivers-filesystem-driver-spec.md §2.
- Test delta: +1016 passing (0 failures, 17 ignored), backward compatible.
