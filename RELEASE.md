# Release Checklist

This document describes the release process for phi-agent. Follow these steps in order.

## Pre-release

### 1. Verify local state

```bash
# All tests pass
cargo test --features shell

# Clippy clean (ignore agent-base warnings from local patching)
cargo clippy --all-targets -- -D warnings 2>&1 | grep -v 'agent-base'

# Format check
cargo fmt --check

# Doc check (zero warnings)
cargo doc --no-deps
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps

# Benchmarks run without panicking
cargo bench --no-run  # verify they compile
```

### 2. Verify dependent crates are published

```bash
# The dependency chain must be published in this order:
# 1. agent-base    → crates.io
# 2. agent-works   → crates.io (depends on agent-base)
# 3. phi-tools     → crates.io (depends on agent-base)
# 4. phi-telemetry → crates.io
# 5. log-core      → crates.io

cargo update
cargo check  # should resolve all deps from crates.io
```

### 3. Update CHANGELOG

- [ ] All notable changes are recorded under the new version heading
- [ ] Breaking changes are in a `### Breaking` section
- [ ] Deprecated items are in a `### Deprecated` section
- [ ] Comparison links at the bottom are updated

### 4. Update version

- [ ] `Cargo.toml`: `version = "X.Y.Z"` (or `X.Y.Z-rc.N` for pre-releases)
- [ ] `README.md` / `README_CN.md`: version badges and install instructions

### 5. Ensure `path` overrides are removed

```bash
grep -r 'path =' Cargo.toml
# Should return nothing (or only in [patch] section)
```

### 6. Final review

- [ ] `grep -rn "TODO" src/` returns nothing
- [ ] `grep -rn "FIXME" src/` returns nothing
- [ ] All `pub` items in `src/lib.rs` are intentional
- [ ] `#[deprecated]` annotations have `since = "X.Y.Z"`

## Tag and release

### 7. Create a git tag

```bash
# For a stable release:
git tag -a v1.0.0 -m "v1.0.0"

# For a release candidate:
git tag -a v1.0.0-rc.1 -m "v1.0.0-rc.1"
```

### 8. Push the tag

```bash
git push origin v1.0.0
```

This triggers `.github/workflows/release.yml` which:
1. Runs the test gate (fmt + clippy + test + doc)
2. Builds release binaries for all targets
3. Uploads artifacts to the GitHub Release

### 9. Verify CI

- [ ] Release workflow completes successfully
- [ ] All four platform binaries are attached to the release
- [ ] Download and smoke-test at least one binary

## Post-release

### 10. Publish to crates.io

```bash
cargo publish
```

### 11. Update documentation

- [ ] `docs.phi-agent.dev` reflects the new version
- [ ] Trigger the deploy-docs workflow if needed

### 12. Announce

- [ ] GitHub Discussions release announcement
- [ ] Update the all-contributors table if new contributors joined

## Release candidate process

For `X.Y.Z-rc.N` releases:

1. Complete all pre-release steps above
2. Tag and push the rc
3. Wait 2–4 weeks for community testing
4. If no critical bugs are found, tag `X.Y.Z` (stable)
5. If bugs are found, fix them and tag `X.Y.Z-rc.N+1`

## Deprecation policy

- `#[deprecated]` items are kept for at least **1 minor version** before removal
- Deprecated items are tracked in CHANGELOG under `### Deprecated`
- Breaking changes require a **major version bump**

## Semver summary

| Change | Version bump |
|--------|-------------|
| Bug fix (no API change) | Patch (`1.0.0` → `1.0.1`) |
| New feature (backward-compatible) | Minor (`1.0.0` → `1.1.0`) |
| Breaking API change | Major (`1.0.0` → `2.0.0`) |
| Pre-release testing | Prerelease (`0.9.0-rc.1`) |
