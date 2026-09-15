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

Deskribe is available on [crates.io](https://crates.io/crates/deskribe).
See the [API documentation](https://docs.rs/deskribe).

Add these dependencies to your application's `Cargo.toml`:

```toml
[dependencies]
deskribe = "0.1.2"
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

## Permissions

Deskribe uses the identity and permissions of the supplied client. **Permission
to `get` the selected object alone may not be sufficient.** Descriptions can also
need `get` or `list` access to related resources. No write permissions are needed.

The client needs `get` access to the selected resource in its namespace, or at
cluster scope for a resource that has no namespace. The following table lists
additional reads. Resource names use the plural names found in RBAC rules;
`core` means the empty API group (`apiGroups: [""]`).

| Description | Additional permission | Scope |
| --- | --- | --- |
| Kinds that read events, including custom resources | `list` core `events` | Object namespace; all namespaces for objects with no namespace |
| Service | `list` `discovery.k8s.io` `endpointslices` | Service namespace |
| Ingress with Service backends | `get` core `services`; `list` `discovery.k8s.io` `endpointslices` | Ingress namespace |
| Deployment | `list` `apps` `replicasets` | Deployment namespace |
| ReplicaSet, ReplicationController, DaemonSet, StatefulSet | `list` core `pods` | Object namespace |
| PersistentVolumeClaim | `list` core `pods` | Claim namespace |
| Namespace | `list` core `resourcequotas` and `limitranges` | Described namespace |
| Node | `list` core `pods` and `events` | All namespaces |
| Node | `get` `coordination.k8s.io` `leases` | `kube-node-lease` namespace; lease name matches the Node name |
| Node | `list` `resource.k8s.io` `resourceslices` | Cluster |

Secret, Namespace, ResourceQuota, LimitRange, Role, ClusterRole, RoleBinding,
ClusterRoleBinding, and NetworkPolicy descriptions do not read events. All other
supported kinds can read events. Add the event permission to the related-resource
permissions in the table where applicable.

### Denied access

The result depends on which read is denied:

- A denied read of the selected object normally makes `gather` return an error.
  The Pod exception is described below.
- A denied event list makes ConfigMap `gather` return an error. For other kinds,
  failed event reads do not stop the description.
- A denied pod list for a PersistentVolumeClaim, or a denied quota or limit list
  for a Namespace, makes `gather` return an error.
- A denied pod list for a ReplicationController, DaemonSet, or StatefulSet makes
  `render` return an error. ReplicaSet output includes the pod-list error.
  Deployment output omits related ReplicaSet data if that list fails.
- A failed Service EndpointSlice list is treated as an empty list. The output can
  show no endpoints even when access was denied. Ingress output includes an error
  for a Service backend if its Service or EndpointSlice read fails.
- For a Node, a denied pod list (HTTP 403) omits pod and resource usage sections.
  A failed lease read appears as an error in the output. A failed ResourceSlice
  list is treated as an empty list. These failures do not stop the description.

A successful description can therefore contain incomplete data. Empty or absent
sections do not always mean that no related resources exist.

## Errors and sensitive data

Errors are returned as strings. Both `gather` and `render` can return errors;
`fetch` returns errors from either step. See [denied access](#denied-access) for
permission failures.

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

### Code structure

- `resource.rs` selects the resource kind and event rules.
- `fetch.rs` gathers data through the API helpers in `api.rs`.
- `description.rs` stores the snapshot type and selects its renderer.
- Resource modules contain the related reads and text rendering for each group.
  Networking resources are in `networking/`; Jobs and CronJobs are in `batch.rs`.
- `metadata.rs`, `events.rs`, `time.rs`, `format.rs`, and `json.rs` contain shared
  functions.
- `tests/corpus.rs` checks and updates fixtures through the public gather and
  render API.

After reviewing an intentional output change, regenerate snapshots with:

```sh
DESKRIBE_UPDATE_FIXTURES=1 cargo test --locked --test corpus public_gather_and_render_match_regression_corpus
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

Each update still needs a person to review the changes, apply relevant behavior
changes in Rust, and test the result. An upstream refactor or test change may need
no Rust change. The recorded baseline does not promise exact output compatibility
with that kubectl version, or an update for every kubectl release.

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
