set unstable
set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

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

# Check, dry-run the crate, then cut a release with vesnik.
release:
    @test "$(git branch --show-current)" = main || { echo "release from main" >&2; exit 1; }
    @test -z "$(git status --porcelain)" || { echo "release from a clean tree: vesnik commits untracked files it writes to, such as CHANGELOG.md" >&2; git status --short >&2; exit 1; }
    git pull --ff-only origin main
    just check
    K8S_OPENAPI_ENABLED_VERSION=1.36 cargo publish --locked --dry-run
    vesnik release --yes
