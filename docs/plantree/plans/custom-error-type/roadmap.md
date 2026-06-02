# Roadmap

## Done

- Scoped custom error migration and verification targets
- Implemented `src/error.rs`
- Migrated parser / graph / db / ai / lib / main to shared `Result<T>`
- Passed `cargo test`
- Passed `cargo clippy --all-targets --all-features -- -D warnings`

## In Progress

- None

## Next

- Optional follow-up: evaluate whether `Error` should be boxed internally if enum size becomes a concern elsewhere

## Deferred

- None
