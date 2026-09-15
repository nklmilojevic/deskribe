set unstable
set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

mod release '.just/release.just'

default:
    @just --list

# Format Rust sources.
fmt:
    cargo fmt --all

# Check Rust formatting without changing files.
fmt-check:
    cargo fmt --all -- --check

# Run clippy with the same policy as the release workflow.
clippy:
    cargo clippy --locked --all-targets -- -D warnings

# Run Rust tests and offline automation tests.
test:
    cargo test --locked
    python3 -m unittest discover -s scripts -p 'test_*.py' -v

# Check formatting, clippy, and tests.
check: fmt-check clippy test

# Verify the crate without uploading it; consumers still choose their schema.
package:
    K8S_OPENAPI_ENABLED_VERSION=1.36 cargo publish --locked --dry-run

# Release a patch version.
release-patch: release::patch

# Release a minor version.
release-minor: release::minor

# Release a major version.
release-major: release::major
