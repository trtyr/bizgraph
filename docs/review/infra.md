# Infra Review — bizgraph

> Evaluated: 2026-06-05
> Scope: build, deploy, dependency health, CI/CD, config, documentation, release process

## Summary

| Criterion | Score |
|-----------|-------|
| Dependency Hygiene | B |
| Build Reproducibility | C |
| CI/CD | F |
| Environment Config | C |
| Documentation | B |
| Release Process | D |
| **Overall** | **C** |

---

## 1. Dependency Hygiene — B

**Score: B**

| Aspect | Detail |
|--------|--------|
| Direct deps | 10 in `Cargo.toml` (lines 18-28) |
| Transitive deps | ~100 locked in `Cargo.lock` (2102 lines) |
| Version specs | Semver ranges (`"1"`, `"4"`, `"0.12"`, etc.) — not pinned to patch |
| Lock file | `Cargo.lock` committed ✓ — resolved versions pinned |
| Unused deps | None detected — all 10 direct deps have clear usage paths |
| `cargo audit` | Not installed — cannot verify CVE status |
| `cargo-outdated` | Not installed — cannot verify freshness |

**Evidence:**
- `Cargo.toml:18-28` — all deps listed with semver ranges
- `Cargo.lock` — committed, lockfile version 4
- `cargo tree --depth 1` confirms 10 direct deps, no orphans

**Risks:**
- `reqwest 0.12` pulls `hyper 1.9.0`, `native-tls 0.2.18` — large TLS/HTTP surface, CVE-prone
- `rusqlite 0.31` bundles `libsqlite3-sys 0.28.0` (C code) — upstream SQLite CVEs apply
- No automated vulnerability scanning

**Recommendations:**
- Install `cargo-audit` and run in CI (or locally before releases)
- Install `cargo-outdated` to track dep freshness quarterly
- Consider pinning `reqwest` and `rusqlite` to patch versions in `Cargo.toml` to avoid surprise breaking changes from transitive C deps

---

## 2. Build Reproducibility — C

**Score: C**

| Aspect | Detail |
|--------|--------|
| `Cargo.lock` committed | ✓ |
| `rust-toolchain.toml` | ✗ Missing — relies on system rustc |
| MSRV in `Cargo.toml` | ✗ Not specified (`rust-version` field absent) |
| `.cargo/config.toml` | ✗ Not present |
| Dev machine rustc | 1.93.0 (per `docs/context/tech-stack.md:14`) |
| Effective minimum | Likely ≥ 1.70+ (dep features), but unverified |
| SQLite | Bundled via `rusqlite/bundled` ✓ — no system dependency |
| TLS | System `native-tls` — platform-dependent behavior |

**Evidence:**
- `Cargo.toml` — no `rust-version` field
- No `rust-toolchain.toml` found via fffind
- `docs/context/tech-stack.md:16-17` — explicitly notes "None" for both

**Risks:**
- Builds on different machines may use different rustc versions → potential compile failures or behavioral differences
- `native-tls` uses platform TLS stacks (OpenSSL / Secure Transport / SChannel) — TLS behavior varies by OS
- No MSRV means downstream packagers can't know minimum supported version

**Recommendations:**
- Add `rust-version = "1.75"` (or whatever the actual minimum is) to `Cargo.toml`
- Add `rust-toolchain.toml` pinning to a specific stable version for contributors
- Consider `rustls` feature for `reqwest` to eliminate platform TLS variance (tradeoff: no system cert store integration)

---

## 3. CI/CD — F

**Score: F**

| Aspect | Detail |
|--------|--------|
| GitHub Actions | ✗ No `.github/workflows/` |
| GitLab CI | ✗ No `.gitlab-ci.yml` |
| Jenkinsfile | ✗ Not present |
| Makefile / justfile | ✗ Not present |
| Dockerfile | ✗ Not present |
| Any CI config | ✗ None found |

**Evidence:**
- fffind for `.github/workflows/`, `.gitlab-ci.yml`, `Jenkinsfile`, `Makefile`, `justfile`, `Taskfile.yml`, `Dockerfile`, `docker-compose.yml` — all returned "No files found"
- `docs/context/deploy.md:113` — "No CI pipeline exists"

**Risks:**
- No automated test gate — broken code can be committed without detection
- No automated lint/format check — `cargo clippy` and `cargo fmt` are manual-only
- No cross-platform testing — Linux and macOS behavior unverified in CI
- No automated release builds — binary distribution relies on manual `install.sh`

**Recommendations:**
- Add `.github/workflows/ci.yml` with: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, `cargo build --release`
- Add dependency caching (`actions/cache` for `~/.cargo/registry` and `target/`)
- Add matrix builds for `ubuntu-latest` + `macos-latest`
- Add `cargo audit` step if `cargo-audit` is installed
- Optional: add release workflow triggered on tag push → build binaries → create GitHub release

---

## 4. Environment Config — C

**Score: C**

| Aspect | Detail |
|--------|--------|
| Config format | TOML (`~/.config/bizgraph/config.toml`) |
| Env var support | ✗ None — TOML-only |
| Secrets handling | API key stored in plaintext config file |
| Config discovery | `try_load_config()` returns `Option` if missing; `load_config()` errors if key absent |
| DB location | `~/.config/bizgraph/bizgraph.db` (auto-created) |
| `.gitignore` | Excludes `*.db` ✓ |

**Evidence:**
- `docs/context/deploy.md:55` — "no environment variables"
- `docs/context/conventions.md:92` — "TOML-only, no environment variables"
- `src/lib.rs:105-134` — `load_config()` / `try_load_config()` functions
- `.gitignore` — `*.db` excluded

**Risks:**
- API key in plaintext on disk — no encryption, no keychain integration
- No env var fallback — can't inject config in CI/CD or containerized environments without writing a file
- No config validation beyond "key exists" — invalid URL or model name silently accepted at parse time

**Recommendations:**
- Add `BIZGRAPH_API_KEY` env var as alternative to config file (takes precedence)
- Add `BIZGRAPH_API_URL` and `BIZGRAPH_MODEL` env vars for CI/container use
- Document that the config file contains a secret and should be `chmod 600`
- Add config validation (URL format, model name non-empty) at load time

---

## 5. Documentation — B

**Score: B**

| Aspect | Detail |
|--------|--------|
| README.md | ✓ Good — quick start, CLI ref, architecture, building |
| CONTRIBUTING.md | ✗ Missing |
| CHANGELOG | ✗ Missing |
| License | MIT ✓ |
| `docs/context/` | ✓ Excellent — architecture, modules, tech-stack, conventions, api, deploy |
| Inline docs | Convention-driven, behavior-descriptive test names |
| Setup instructions | ✓ In README and `docs/context/deploy.md` |

**Evidence:**
- `README.md` — 130 lines, covers quick start, CLI reference, architecture diagram, building
- `docs/context/` — 6 detailed docs (architecture, modules, tech-stack, conventions, api, deploy)
- No `CONTRIBUTING.md` or `CHANGELOG.md` found via fffind

**Risks:**
- No contributing guide — external contributors won't know conventions, test expectations, or PR process
- No changelog — users can't see what changed between versions
- README mentions "Rust ≥ 1.56" but actual minimum is likely higher — misleading

**Recommendations:**
- Add `CONTRIBUTING.md` with: setup steps, test commands, format/lint expectations, PR process
- Add `CHANGELOG.md` (or use `git-cliff` / `cargo-release` to auto-generate)
- Fix README to reflect actual MSRV once `rust-version` is set in `Cargo.toml`

---

## 6. Release Process — D

**Score: D**

| Aspect | Detail |
|--------|--------|
| Versioning | Manual bump in `Cargo.toml` |
| Current version | 0.1.1 |
| Release checklist | Documented in `docs/context/deploy.md:117-125` |
| Automated release | ✗ None |
| Changelog | ✗ None |
| Rollback | Manual — no versioned artifacts, no binary releases |
| Distribution | `install.sh` — copies binary to `~/.local/bin/` |
| Binary releases | ✗ No GitHub releases, no package manager (brew, cargo install from registry) |

**Evidence:**
- `docs/context/deploy.md:115-125` — manual release checklist
- `install.sh` — 37 lines, `cargo build --release` + `cp` + `chmod`
- No GitHub Actions release workflow
- No `cargo-release` or `release-please` config

**Risks:**
- No reproducible release artifacts — each `install.sh` run builds from source on the local machine
- No rollback path — if a bad version is installed, user must manually rebuild an older commit
- No binary distribution — users must have Rust toolchain installed

**Recommendations:**
- Add a GitHub Actions release workflow: tag push → build Linux/macOS binaries → attach to GitHub release
- Use `cargo-release` or manual checklist with `cargo publish` for crates.io
- Add `CHANGELOG.md` and update it with each release
- Consider Homebrew tap or `cargo install bizgraph` for easier distribution

---

## Appendix: Tooling Gaps

| Tool | Status | Priority |
|------|--------|----------|
| `cargo-audit` | Not installed | High — security |
| `cargo-outdated` | Not installed | Medium — freshness |
| `cargo-release` | Not installed | Medium — release automation |
| `git-cliff` | Not installed | Low — changelog generation |
| `rustfmt.toml` | Not present (uses defaults) | Low — fine for solo project |
| `clippy.toml` | Not present (uses defaults) | Low — fine for solo project |

---

## Appendix: Files Reviewed

| File | Lines | Purpose |
|------|-------|---------|
| `Cargo.toml` | 28 | Dependencies, metadata |
| `Cargo.lock` | 2102 | Pinned transitive deps |
| `README.md` | 130 | Project documentation |
| `install.sh` | 37 | Build + install script |
| `.gitignore` | 5 | Git exclusions |
| `docs/context/tech-stack.md` | 104 | Tech stack reference |
| `docs/context/deploy.md` | 146 | Deploy/build/test reference |
| `docs/context/conventions.md` | 99 | Code conventions |
