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

Install Rust (with rustfmt and clippy), Python 3.11+, Git, GitHub CLI, and `just`.
Authenticate GitHub CLI with permission to push and create releases. Like sofka,
the recipes use `op plugin run -- gh` when the 1Password CLI is installed.

Merge reviewed changes first. Start on a clean `main` with no untracked files:

```sh
just release patch
# or: just release minor
# or: just release major
```

The aliases `just release-patch`, `just release-minor`, and `just release-major`
do the same thing. The recipe:

1. Checks authentication and cleanliness, fetches tags, and fast-forwards `main`.
   It refuses local commits that are not on `origin/main`.
2. Computes the next version from the latest non-draft, non-prerelease GitHub
   Release. If none exists, it bumps the current manifest version. For example,
   after bootstrapping `0.1.0`, the first `just release patch` makes `0.1.1`.
3. Updates `Cargo.toml` and lets Cargo update `Cargo.lock`. Runs `just check` and
   a publish dry run. The dry run permits the deliberate uncommitted version bump.
4. Commits the version change as `chore(release): vX.Y.Z` and pushes `main`.
5. Creates a GitHub Release with generated notes at that exact commit. The release
   workflow validates and publishes the crate after any environment approval.

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
