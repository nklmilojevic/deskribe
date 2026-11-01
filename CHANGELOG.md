# Changelog

## v0.2.0 - 2026-11-01

This release moves deskribe to kube 5 and k8s-openapi 0.29. Code that uses deskribe must change to keep working.

### Other changes

- The `deskribe::fetch` function now takes a `kube::Client` from kube 5, with k8s-openapi 0.29. This change breaks the public API, so callers must upgrade.
- The README now shows how to install the published `deskribe` crate. Users can install the released version from crates.io.
- The docs now list the Kubernetes permissions that a `describe` call needs. They also explain upstream maintenance.
