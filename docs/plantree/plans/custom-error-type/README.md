# Custom Error Type Migration

## Scope

Replace all `Result<_, String>` usage in the crate with a shared custom `Error` type in `src/error.rs` while preserving current CLI-facing error readability.

## Acceptance Criteria

- All fallible functions return `Result<T>` alias
- `main.rs` prints `Error: {error}` via `Display`
- No user-facing error information loss
- `cargo test` and `cargo clippy` pass
