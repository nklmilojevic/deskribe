# Releases

Deskribe follows sofka's release flow, but publishes only a crate. There are no
binary archives, Homebrew updates, or Nix cache jobs.

Publishing a GitHub Release runs `.github/workflows/release.yaml`. It checks that
the `vMAJOR.MINOR.PATCH` tag matches `Cargo.toml`, runs Rust formatting, clippy,
tests, and a publish dry run, then publishes to crates.io using a short-lived
OIDC token. Pushing a tag alone does not start publishing.

## One-time setup

1. Create the GitHub environment **release** in `nklmilojevic/deskribe`. Add
   required reviewers and restrict deployments to release tags (`v*`) as needed.
2. Bootstrap the crate on crates.io if it has never been published. The current
   [trusted publishing requirements](https://crates.io/docs/trusted-publishing)
   require an existing crate and an owner; the first publish needs an API token.
   From a reviewed, clean `main`, run `just check` and `just package`, then publish
   the current manifest version with your own short-lived crates.io token:

   ```sh
   # Supply CARGO_REGISTRY_TOKEN securely for this command; do not commit it.
   K8S_OPENAPI_ENABLED_VERSION=1.36 cargo publish --locked
   ```

   This uploads a real release and requires explicit maintainer approval. Do not
   run it as part of testing this setup. A GitHub Release is not needed for this
   bootstrap; creating one for the already-published version would try to upload
   that version again.

3. In the crate's crates.io **Settings → Trusted Publishing**, configure:
   - Repository owner: `nklmilojevic`
   - Repository name: `deskribe`
   - Workflow filename: `release.yaml`
   - Environment: `release`
4. Revoke the bootstrap token. Routine releases do not need a stored
   `CARGO_REGISTRY_TOKEN` repository secret.

## Routine releases

Install Rust through rustup, Python 3.11+, Git, GitHub CLI, `just`, and `vesnik`.
`rust-toolchain.toml` pins Rust 1.97.0 with rustfmt and clippy. Rustup uses this
file locally, and both release jobs read the same version instead of floating
`stable`. Update this file and run the checks when upgrading Rust. If your local
Rust comes from Nix or another non-rustup installation, keep that toolchain at
the same version too.

Authenticate GitHub CLI with permission to push and create releases.

Merge reviewed changes first. Start on a clean `main` with no untracked files:

```sh
just release
```

The recipe:

1. Fast-forwards `main` from `origin`, so the checks see the commit that ships.
   Then runs `just check` and a publish dry run of the crate. Both run before the
   version changes, so the dry run checks the current manifest, not the new
   version. The dry run also refuses a dirty tree.
2. Runs `vesnik release --yes`. vesnik picks the next version from the commits
   since the latest tag. Below `1.0.0`, breaking changes bump the minor version.
   It updates `Cargo.toml` and `Cargo.lock`, writes `CHANGELOG.md`, commits
   `chore(release): vX.Y.Z`, pushes `main`, tags that commit, and creates a
   GitHub Release.
3. The release workflow validates and publishes the crate after any environment
   approval.

These commands push commits and create releases. They are not dry runs. Review
any failure before retrying; the recipe does not roll back a version bump or a
published release. For local verification without publishing, use:

```sh
just check
just package
```

Package verification and publishing set `K8S_OPENAPI_ENABLED_VERSION=1.36` because
Cargo does not build dev-dependencies during package verification. This is a
build-only schema choice, not a default feature imposed on consumers. Keep it
in sync with the supported schemas when upgrading `k8s-openapi`.

## Failed releases

Inspect the GitHub Actions logs before retrying. Authentication failures usually
mean the crates.io repository, workflow filename, or environment does not match.
Fix setup and rerun the failed job if nothing was uploaded.

Versions on crates.io are immutable. If an upload succeeded but a later step
failed, verify the crates.io version before rerunning: the workflow does not
silently skip duplicate uploads. Never move an existing release tag to different
source. Fix source problems in a new version instead.
