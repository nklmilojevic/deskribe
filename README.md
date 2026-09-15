# deskribe

Kubernetes resource descriptions in Rust, without starting `kubectl`.

Give deskribe an existing `kube::Client` and a resource. It reads the object,
related resources, and events, then returns a text description. Applications can
reuse their current connection and authentication setup.

**Experimental.** Supports 36 specialized resource kinds and a generic format
for custom resources. The implementation is based on kubectl describers;
[`upstream.json`](upstream.json) records the reviewed source baseline. Output is
not guaranteed to match every kubectl version exactly.

## Use it

The crate is not published yet. For local development:

```toml
[dependencies]
deskribe = { path = "../deskribe" }
kube = { version = "4.2", default-features = false, features = ["client", "rustls-tls", "aws-lc-rs"] }
k8s-openapi = { version = "0.28", features = ["latest"] }
```

Your application chooses the Kubernetes schema and TLS features. The example
above uses the latest schema available in `k8s-openapi` 0.28 and rustls. Deskribe
does not choose these features for you.

```rust
use deskribe::{gather, RenderOptions};
use kube::{api::{ApiResource, DynamicObject}, Client};

async fn describe(
    client: Client,
    resource: &ApiResource,
    selected: &DynamicObject,
) -> Result<String, String> {
    let description = gather(client, resource, selected).await?;
    description.render(&RenderOptions::default())
}
```

Pass the resource's discovered `ApiResource` and the selected object, including
its name, namespace, and UID when known. Clone your existing client if you need
to keep using it after the call.

## API

- **`gather(client, resource, selected)`** fetches a fresh object and related data.
  The fresh GET uses the discovered API group, version, and plural. HPA v2 can
  fall back to v1; ServiceCIDR and IPAddress v1 can fall back to v1beta1. These
  fallbacks require the standard Kubernetes response for a missing endpoint.
  Other errors are returned without a version fallback. If the fallback fails,
  the original error is returned.
  It checks the selected UID when available, so an object recreated with the
  same name is not silently substituted. It follows paginated lists and overlaps
  independent requests.
- **`Description::render(&options)`** returns text without making network calls.
  Options control the current time and timezone. Defaults use the current time
  and the system timezone.
- **`fetch(client, resource, selected)`** gathers and renders with default options,
  returning `(DynamicObject, String)`.

You can render the same snapshot more than once. Gather again to refresh its
data. Dropping the gather future cancels the operation; requests already sent
cannot be recalled.

Deskribe does not load kubeconfig, run subprocesses, create a runtime, print to
stdout, or manage a UI. Those are the caller's responsibilities.

## Errors and sensitive data

The client needs permission to read the object and the related resources used by
its describer. Errors are returned as strings. Some optional reads are best-effort;
for example, denied or unavailable ResourceSlice access does not fail a Node
description.

If a Pod read fails but events are available, its description can show the error
and those events. In that case, `Description::object()` returns the original
selection rather than a fresh object.

**Descriptions can contain secrets.** Service-account Secret descriptions show
`data.token`, as kubectl does. Other Secret values are shown as byte counts.
Do not send descriptions to public logs or telemetry. Helm release notes belong
to the calling application, not this library.

## Development

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
python3 -m unittest discover -s scripts -p 'test_check_upstream.py' -v
```

Rust tests cover synthetic output snapshots, resource accounting, and the public
fetch/render API. They do not need a cluster or Go. Sofka keeps separate TUI
integration tests.

After reviewing an intentional output change, regenerate snapshots with:

```sh
DESKRIBE_UPDATE_FIXTURES=1 cargo test --locked render_matches_describe_regression_fixtures
```

Review every changed snapshot. Regeneration alone does not prove correctness.

## Upstream changes

The **Upstream changes** GitHub workflow runs weekly and can be started manually.
It compares the source paths in [`upstream.json`](upstream.json) with the latest
stable kubectl module tag, using the same tag for related Kubernetes repositories.

A source change marks the run as failed to request review. The run summary and
`upstream-report` artifact contain the changed file list, commit IDs, and patches.
Fetch errors are reported separately. The workflow does not run Go, edit Rust
code, advance the baseline, or open pull requests.

See [upstream maintenance](docs/upstream.md) for local commands and review steps.
This checks source changes, not Rust output parity or every upstream dependency.

## Publishing

A published GitHub Release triggers checks and a crates.io upload using trusted
publishing. Use `just release patch|minor|major` from clean `main` after the
one-time crates.io setup. This publishes only the crate, not binaries.

See [release setup and commands](docs/releases.md), including the initial publish
required before trusted publishing can be configured. `just package` verifies
the crate without uploading it.

## License

Apache-2.0. Adapted from Kubernetes and extracted from sofka.
See [LICENSE-APACHE](LICENSE-APACHE) and [NOTICE](NOTICE).
