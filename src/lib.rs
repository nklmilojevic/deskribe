// Copyright 2014 The Kubernetes Authors.
// Copyright 2026 sofka contributors.
// SPDX-License-Identifier: Apache-2.0
//
// Adapted from kubernetes/kubectl v0.35.1 pkg/describe/describe.go and
// pkg/util/event/sorted_event_list.go; duration formatting from
// kubernetes/apimachinery v0.35.1 pkg/util/duration/duration.go (Copyright 2018).
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy at http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License. See NOTICE and README.md.

//! Native Kubernetes descriptions with caller-owned clients and separate gathering/rendering.
//! See the README for the supported behavior baseline and compatibility notes.

mod description;
mod fetch;
pub use description::{Description, RenderOptions};
pub use fetch::{fetch, gather};

mod autoscaling;
mod certificate;
mod classes;
mod containers;
mod controllers;
mod endpoints;
mod generic;
mod ingress;
mod node;
mod pod;
mod policy;
mod quantity;
mod quotas;
mod rbac;
mod service;
mod storage;
mod volumes;
mod workloads;

use std::collections::BTreeMap;
use std::fmt::Write;

use k8s_openapi::api::core::v1::{ConfigMap, Event, Secret};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use k8s_openapi::jiff::Timestamp;
use kube::Client;
use kube::api::{Api, ApiResource, DynamicObject, ListParams};

pub fn supports(ar: &ApiResource) -> bool {
    if classes::supports(ar)
        || rbac::supports(ar)
        || endpoints::supports(ar)
        || policy::supports(ar)
        || workloads::supports(ar)
        || controllers::supports(ar)
        || quotas::supports(ar)
        || storage::supports(ar)
        || autoscaling::supports(ar)
        || ingress::supports(ar)
        || (ar.group == "certificates.k8s.io" && ar.kind == "CertificateSigningRequest")
        || (ar.group.is_empty() && matches!(ar.kind.as_str(), "Pod" | "Service" | "Node"))
        || !has_upstream_specialized(ar)
    {
        return true;
    }
    ar.group.is_empty()
        && ar.version == "v1"
        && matches!(
            (ar.kind.as_str(), ar.plural.as_str()),
            ("ConfigMap", "configmaps") | ("Secret", "secrets")
        )
}

/// Keep unported specialized kinds on kubectl rather than silently replacing
/// their resource-specific information with a generic representation.
fn has_upstream_specialized(ar: &ApiResource) -> bool {
    match ar.group.as_str() {
        "" => matches!(
            ar.kind.as_str(),
            "Pod"
                | "ReplicationController"
                | "Secret"
                | "Service"
                | "ServiceAccount"
                | "Node"
                | "LimitRange"
                | "ResourceQuota"
                | "PersistentVolume"
                | "PersistentVolumeClaim"
                | "Namespace"
                | "Endpoints"
                | "ConfigMap"
                | "PriorityClass"
        ),
        "discovery.k8s.io" => ar.kind == "EndpointSlice",
        "autoscaling" => ar.kind == "HorizontalPodAutoscaler",
        "extensions" => ar.kind == "Ingress",
        "networking.k8s.io" => matches!(
            ar.kind.as_str(),
            "Ingress" | "IngressClass" | "ServiceCIDR" | "IPAddress" | "NetworkPolicy"
        ),
        "batch" => matches!(ar.kind.as_str(), "Job" | "CronJob"),
        "apps" => matches!(
            ar.kind.as_str(),
            "StatefulSet" | "Deployment" | "DaemonSet" | "ReplicaSet"
        ),
        "certificates.k8s.io" => ar.kind == "CertificateSigningRequest",
        "storage.k8s.io" => matches!(
            ar.kind.as_str(),
            "StorageClass" | "CSINode" | "VolumeAttributesClass"
        ),
        "policy" => ar.kind == "PodDisruptionBudget",
        "rbac.authorization.k8s.io" => matches!(
            ar.kind.as_str(),
            "Role" | "ClusterRole" | "RoleBinding" | "ClusterRoleBinding"
        ),
        "scheduling.k8s.io" => ar.kind == "PriorityClass",
        _ => false,
    }
}

async fn fetch_object(
    client: Client,
    selected: &ApiResource,
    namespace: &str,
    name: &str,
) -> Result<DynamicObject, kube::Error> {
    let resource = if has_upstream_specialized(selected) {
        let group = match selected.kind.as_str() {
            "Ingress" => "networking.k8s.io",
            "PriorityClass" => "scheduling.k8s.io",
            _ => selected.group.as_str(),
        };
        ApiResource::from_gvk(&kube::core::GroupVersionKind::gvk(
            group,
            "v1",
            &selected.kind,
        ))
    } else {
        selected.clone()
    };
    let api: Api<DynamicObject> = if namespace.is_empty() {
        Api::all_with(client.clone(), &resource)
    } else {
        Api::namespaced_with(client.clone(), namespace, &resource)
    };
    let result = api.get(name).await;
    if result.is_err()
        && resource.group == "networking.k8s.io"
        && matches!(resource.kind.as_str(), "ServiceCIDR" | "IPAddress")
    {
        let fallback = ApiResource::from_gvk(&kube::core::GroupVersionKind::gvk(
            &resource.group,
            "v1beta1",
            &resource.kind,
        ));
        Api::<DynamicObject>::all_with(client, &fallback)
            .get(name)
            .await
    } else {
        result
    }
}

async fn list_objects(
    client: Client,
    ar: &ApiResource,
    namespace: &str,
    mut params: ListParams,
) -> Result<Vec<DynamicObject>, kube::Error> {
    let api = if namespace.is_empty() {
        Api::<DynamicObject>::all_with(client, ar)
    } else {
        Api::namespaced_with(client, namespace, ar)
    };
    let mut objects = Vec::new();
    loop {
        let page = api.list(&params).await?;
        objects.extend(page.items);
        match page.metadata.continue_.filter(|s| !s.is_empty()) {
            Some(token) => params = params.continue_token(&token),
            None => return Ok(objects),
        }
    }
}

async fn fetch_events(client: Client, meta: &ObjectMeta, kind: &str) -> Result<Vec<Event>, String> {
    let ns = meta.namespace.as_deref().unwrap_or_default();
    let name = meta.name.as_deref().unwrap_or_default();
    let mut selector = format!("involvedObject.name={name},involvedObject.namespace={ns}");
    if !kind.is_empty() {
        write!(selector, ",involvedObject.kind={kind}").unwrap();
    }
    if let Some(uid) = meta.uid.as_deref().filter(|s| !s.is_empty()) {
        write!(selector, ",involvedObject.uid={uid}").unwrap();
    }
    let api: Api<Event> = if ns.is_empty() {
        Api::all(client)
    } else {
        Api::namespaced(client, ns)
    };
    let mut params = ListParams::default().fields(&selector);
    let mut events = Vec::new();
    loop {
        let page = api
            .list(&params)
            .await
            .map_err(|e| format!("describe events failed: {e}"))?;
        events.extend(page.items);
        match page.metadata.continue_.filter(|s| !s.is_empty()) {
            Some(token) => params = params.continue_token(&token),
            None => return Ok(events),
        }
    }
}

fn metadata(meta: &ObjectMeta) -> String {
    metadata_header(meta, true)
}

fn metadata_header(meta: &ObjectMeta, namespaced: bool) -> String {
    let mut out = format!("Name:\t{}\n", meta.name.as_deref().unwrap_or_default());
    if namespaced {
        writeln!(
            out,
            "Namespace:\t{}",
            meta.namespace.as_deref().unwrap_or_default()
        )
        .unwrap();
    }
    let empty = BTreeMap::new();
    out.push_str("Labels:\t");
    let labels = meta.labels.as_ref().unwrap_or(&empty);
    if labels.is_empty() {
        out.push_str("<none>\n");
    }
    for (i, (key, value)) in labels.iter().enumerate() {
        if i > 0 {
            out.push('\t');
        }
        writeln!(out, "{key}={value}").unwrap();
    }
    out.push_str("Annotations:\t");
    let annotations: Vec<_> = meta
        .annotations
        .as_ref()
        .unwrap_or(&empty)
        .iter()
        .filter(|(key, _)| key.as_str() != "kubectl.kubernetes.io/last-applied-configuration")
        .collect();
    if annotations.is_empty() {
        out.push_str("<none>\n");
    }
    for (i, (key, value)) in annotations.into_iter().enumerate() {
        if i > 0 {
            out.push('\t');
        }
        let value = value.strip_suffix('\n').unwrap_or(value);
        if value.len() + key.len() + 2 > 140 || value.contains('\n') {
            writeln!(out, "{key}:").unwrap();
            for line in value.split('\n') {
                // Go truncates bytes, including in the middle of UTF-8. The UI
                // already decodes kubectl stdout lossily, so do the same here.
                let short = String::from_utf8_lossy(&line.as_bytes()[..line.len().min(138)]);
                writeln!(
                    out,
                    "\t  {short}{}",
                    if line.len() > 138 { "..." } else { "" }
                )
                .unwrap();
            }
        } else {
            writeln!(out, "{key}: {value}").unwrap();
        }
    }
    out
}

pub fn render_secret(secret: &Secret) -> String {
    let mut out = metadata(&secret.metadata);
    write!(
        out,
        "\nType:\t{}\n\nData\n====\n",
        secret.type_.as_deref().unwrap_or_default()
    )
    .unwrap();
    for (key, value) in secret.data.iter().flatten() {
        // Intentional upstream parity: service-account tokens are displayed.
        // All other Secret data (including bootstrap/TLS/docker credentials)
        // is represented only by decoded byte length, never by its value.
        if key == "token" && secret.type_.as_deref() == Some("kubernetes.io/service-account-token")
        {
            writeln!(out, "{key}:\t{}", String::from_utf8_lossy(&value.0)).unwrap();
        } else {
            writeln!(out, "{key}:\t{} bytes", value.0.len()).unwrap();
        }
    }
    tabbed(&out)
}

pub fn render_config_map(cm: &ConfigMap, events: &[Event], now: Timestamp) -> String {
    let mut out = metadata(&cm.metadata);
    out.push_str("\nData\n====\n");
    for (key, value) in cm.data.iter().flatten() {
        writeln!(out, "{key}:\n----\n{value}\n").unwrap();
    }
    out.push_str("\nBinaryData\n====\n");
    for (key, value) in cm.binary_data.iter().flatten() {
        writeln!(out, "{key}: {} bytes", value.0.len()).unwrap();
    }
    out.push('\n');
    with_events(out, Some(events), now)
}

fn with_events(mut out: String, events: Option<&[Event]>, now: Timestamp) -> String {
    let Some(events) = events else {
        return tabbed(&out);
    };
    if events.is_empty() {
        out.push_str("Events:\t<none>\n");
        return tabbed(&out);
    }
    let mut out = tabbed(&out); // Upstream flushes before the Events table.
    let mut rows = String::from(
        "Events:\n  Type\tReason\tAge\tFrom\tMessage\n  ----\t------\t----\t----\t-------\n",
    );
    let mut events: Vec<_> = events.iter().collect();
    events.sort_by_key(|e| e.last_timestamp.as_ref().map(|t| t.0));
    for e in events {
        let first = age(
            e.event_time
                .as_ref()
                .map(|t| t.0)
                .or_else(|| e.first_timestamp.as_ref().map(|t| t.0)),
            now,
        );
        let interval = if let Some(series) = &e.series {
            format!(
                "{} (x{} over {first})",
                age(series.last_observed_time.as_ref().map(|t| t.0), now),
                series.count.unwrap_or_default()
            )
        } else if e.count.unwrap_or_default() > 1 {
            format!(
                "{} (x{} over {first})",
                age(e.last_timestamp.as_ref().map(|t| t.0), now),
                e.count.unwrap()
            )
        } else {
            first
        };
        let source = e
            .source
            .as_ref()
            .and_then(|s| s.component.as_deref())
            .filter(|s| !s.is_empty())
            .or(e.reporting_component.as_deref())
            .unwrap_or_default();
        let message = e.message.as_deref().unwrap_or_default().trim();
        let message = match e
            .involved_object
            .field_path
            .as_deref()
            .filter(|s| !s.is_empty())
        {
            Some(field) => format!("{field}: {message}"),
            None => message.into(),
        };
        writeln!(
            rows,
            "  {}\t{}\t{interval}\t{source}\t{message}",
            e.type_.as_deref().unwrap_or_default(),
            e.reason.as_deref().unwrap_or_default()
        )
        .unwrap();
    }
    out.push_str(&tabbed(&rows));
    out
}

fn age(time: Option<Timestamp>, now: Timestamp) -> String {
    let Some(time) = time.filter(|t| t.as_second() != -62135596800) else {
        return "<unknown>".into();
    };
    human_duration((now.as_nanosecond() - time.as_nanosecond()) / 1_000_000_000)
}

fn human_duration(seconds: i128) -> String {
    let minutes = seconds / 60;
    let hours = minutes / 60;
    let days = hours / 24;
    let years = days / 365;
    let pair = |n, suffix, rest, tail| {
        if rest == 0 {
            format!("{n}{suffix}")
        } else {
            format!("{n}{suffix}{rest}{tail}")
        }
    };
    match seconds {
        ..=-2 => "<invalid>".into(),
        -1 => "0s".into(),
        0..120 => format!("{seconds}s"),
        _ if minutes < 10 => pair(minutes, "m", seconds % 60, "s"),
        _ if minutes < 180 => format!("{minutes}m"),
        _ if hours < 8 => pair(hours, "h", minutes % 60, "m"),
        _ if hours < 48 => format!("{hours}h"),
        _ if days < 8 => pair(days, "d", hours % 24, "h"),
        _ if days < 730 => format!("{days}d"),
        _ if days < 2920 => pair(years, "y", days % 365, "d"),
        _ => format!("{years}y"),
    }
}

// Go text/tabwriter's contiguous-column alignment (minwidth=0, padding=2).
// Column widths count Unicode code points, not terminal display width.
fn tabbed(text: &str) -> String {
    let escaped = text.replace('\x1b', "^[").replace('\r', "\\r");
    let mut output = String::new();
    let mut sections = escaped.split('\x0c').peekable();
    while let Some(section) = sections.next() {
        let mut rows: Vec<Vec<String>> = section
            .split_terminator('\n')
            .map(|line| line.split(['\t', '\x0b']).map(String::from).collect())
            .collect();
        align(&mut rows, 0);
        for row in rows {
            output.push_str(&row.concat());
            output.push('\n');
        }
        if sections.peek().is_some() && (section.is_empty() || section.ends_with('\n')) {
            output.push('\n');
        }
    }
    output
}

fn align(rows: &mut [Vec<String>], col: usize) {
    let mut start = 0;
    while start < rows.len() {
        if rows[start].len() <= col + 1 {
            start += 1;
            continue;
        }
        let end = (start..rows.len())
            .find(|&i| rows[i].len() <= col + 1)
            .unwrap_or(rows.len());
        let width = rows[start..end]
            .iter()
            .map(|r| r[col].chars().count() + 2)
            .max()
            .unwrap();
        for row in &mut rows[start..end] {
            let padding = width - row[col].chars().count();
            row[col].push_str(&" ".repeat(padding));
        }
        align(&mut rows[start..end], col + 1);
        start = end;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_matches_describe_regression_fixtures() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/describe");
        for path in std::fs::read_dir(&root)
            .unwrap()
            .map(|p| p.unwrap().path())
            .filter(|p| p.extension().is_some_and(|ext| ext == "json"))
        {
            let name = path.file_stem().unwrap().to_str().unwrap();
            let fixture: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let object: DynamicObject = serde_json::from_value(fixture["object"].clone()).unwrap();
            let types = object.types.as_ref().unwrap();
            let (group, version) = types
                .api_version
                .split_once('/')
                .unwrap_or(("", types.api_version.as_str()));
            let ar = ApiResource::from_gvk(&kube::core::GroupVersionKind::gvk(
                group,
                version,
                &types.kind,
            ));
            let events: Vec<Event> = serde_json::from_value(fixture["events"].clone()).unwrap();
            let now = "2026-01-01T00:00:00Z".parse().unwrap();
            let output = if ar.group.is_empty() && ar.kind == "Node" {
                let related = node::Related {
                    pods: Some(serde_json::from_value(fixture["related"].clone()).unwrap()),
                    lease: Ok(serde_json::from_value(fixture["nodeLease"].clone()).unwrap()),
                    events: Some(events.iter().chain(&events).cloned().collect()),
                    resource_slices: serde_json::from_value(
                        fixture
                            .get("nodeResourceSlices")
                            .cloned()
                            .unwrap_or_else(|| serde_json::json!([])),
                    )
                    .unwrap(),
                };
                node::render(
                    &object,
                    &related,
                    now,
                    &k8s_openapi::jiff::tz::TimeZone::UTC,
                )
            } else if autoscaling::supports(&ar) {
                autoscaling::render(
                    &object,
                    Some(&events),
                    now,
                    &k8s_openapi::jiff::tz::TimeZone::UTC,
                )
            } else if ingress::supports(&ar) {
                let backends: ingress::Backends = fixture["ingressBackends"]
                    .as_object()
                    .into_iter()
                    .flatten()
                    .map(|(name, backend)| {
                        (
                            name.clone(),
                            Ok((
                                serde_json::from_value(backend["service"].clone()).unwrap(),
                                serde_json::from_value(backend["slices"].clone()).unwrap(),
                            )),
                        )
                    })
                    .collect();
                ingress::render(&object, &backends, Some(&events), now)
            } else if storage::supports(&ar) {
                let related: Vec<DynamicObject> = serde_json::from_value(
                    fixture
                        .get("related")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!([])),
                )
                .unwrap();
                storage::render(
                    &object,
                    &ar.kind,
                    &related,
                    Some(&events),
                    now,
                    &k8s_openapi::jiff::tz::TimeZone::UTC,
                )
            } else if ar.group == "certificates.k8s.io" && ar.kind == "CertificateSigningRequest" {
                certificate::render(
                    &object,
                    Some(&events),
                    now,
                    &k8s_openapi::jiff::tz::TimeZone::UTC,
                )
                .unwrap()
            } else if quotas::supports(&ar) {
                let quota: Vec<DynamicObject> = serde_json::from_value(
                    fixture
                        .get("namespaceQuotas")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!([])),
                )
                .unwrap();
                let limits: Vec<DynamicObject> = serde_json::from_value(
                    fixture
                        .get("namespaceLimits")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!([])),
                )
                .unwrap();
                quotas::render(
                    &object,
                    &ar.kind,
                    Some(&quota),
                    Some(&limits),
                    &k8s_openapi::jiff::tz::TimeZone::UTC,
                )
            } else if ar.group.is_empty() && ar.kind == "Pod" {
                pod::render(
                    &object,
                    Some(&events),
                    now,
                    &k8s_openapi::jiff::tz::TimeZone::UTC,
                )
            } else if ar.group.is_empty() && ar.kind == "Service" {
                let related: Vec<DynamicObject> = serde_json::from_value(
                    fixture
                        .get("related")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!([])),
                )
                .unwrap();
                service::render(&object, &related, Some(&events), now)
            } else if controllers::supports(&ar) {
                let related: Vec<DynamicObject> = serde_json::from_value(
                    fixture
                        .get("related")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!([])),
                )
                .unwrap();
                controllers::render(
                    &object,
                    &ar.kind,
                    Ok(&related),
                    Some(&events),
                    now,
                    &k8s_openapi::jiff::tz::TimeZone::UTC,
                )
                .unwrap()
            } else if workloads::supports(&ar) {
                workloads::render(
                    &object,
                    &ar.kind,
                    Some(&events),
                    now,
                    &k8s_openapi::jiff::tz::TimeZone::UTC,
                )
            } else if policy::supports(&ar) {
                policy::render(
                    &object,
                    &ar.kind,
                    policy::events(&ar).then_some(events.as_slice()),
                    now,
                    &k8s_openapi::jiff::tz::TimeZone::UTC,
                )
            } else if endpoints::supports(&ar) {
                endpoints::render(&object, &ar.kind, Some(&events), now)
            } else if classes::supports(&ar) {
                classes::render(
                    &object,
                    &ar.kind,
                    classes::events(&ar).then_some(events.as_slice()),
                    now,
                    &k8s_openapi::jiff::tz::TimeZone::UTC,
                )
            } else if rbac::supports(&ar) {
                rbac::render(&object)
            } else if !has_upstream_specialized(&ar) {
                generic::render(&object, Some(&events), now)
            } else if name.starts_with("cm-") {
                render_config_map(
                    &serde_json::from_value(fixture["object"].clone()).unwrap(),
                    &serde_json::from_value::<Vec<Event>>(fixture["events"].clone()).unwrap(),
                    "2026-01-01T00:00:00Z".parse().unwrap(),
                )
            } else {
                render_secret(&serde_json::from_value(fixture["object"].clone()).unwrap())
            };
            let expected_path = root.join(format!("{name}.txt"));
            if std::env::var("DESKRIBE_UPDATE_FIXTURES").as_deref() == Ok("1") {
                std::fs::write(&expected_path, &output).unwrap();
            }
            assert_eq!(
                output,
                std::fs::read_to_string(expected_path).unwrap(),
                "{name}"
            );
            assert!(!output.contains("MUST NOT SHOW"));
            assert!(!output.contains("SYNTHETIC-DO-NOT-PRINT"));
            if name == "secret-token" {
                assert!(output.contains("SYNTHETIC-TOKEN-NOT-A-CREDENTIAL"));
            }
        }
    }

    #[test]
    fn event_age_matches_upstream_duration_boundaries() {
        for (seconds, expected) in [
            (-2, "<invalid>"),
            (-1, "0s"),
            (0, "0s"),
            (119, "119s"),
            (120, "2m"),
            (121, "2m1s"),
            (600, "10m"),
            (10800, "3h"),
            (10860, "3h1m"),
            (28800, "8h"),
            (172800, "2d"),
            (176400, "2d1h"),
            (691200, "8d"),
            (63072000, "2y"),
            (63158400, "2y1d"),
            (252288000, "8y"),
        ] {
            assert_eq!(human_duration(seconds), expected);
        }
    }

    #[test]
    fn events_sort_by_last_timestamp_and_render_intervals() {
        let cm = ConfigMap::default();
        let events: Vec<Event> = serde_json::from_value(serde_json::json!([
            {"metadata":{},"involvedObject":{},"reason":"Later","firstTimestamp":"2026-01-01T00:00:00Z","lastTimestamp":"2026-01-01T00:04:00Z","count":4},
            {"metadata":{},"involvedObject":{"fieldPath":"data.key"},"reason":"Earlier","eventTime":"2026-01-01T00:01:00.000000Z","lastTimestamp":"2026-01-01T00:02:00Z","series":{"count":2,"lastObservedTime":"2026-01-01T00:03:00.000000Z"},"reportingComponent":"controller","message":"  hello  "}
        ])).unwrap();
        let text = render_config_map(&cm, &events, "2026-01-01T00:05:00Z".parse().unwrap());
        assert!(text.find("Earlier").unwrap() < text.find("Later").unwrap());
        assert!(text.contains("2m (x2 over 4m)"));
        assert!(text.contains("60s (x4 over 5m)"));
        assert!(text.contains("controller"));
        assert!(text.contains("data.key: hello\n"));
    }

    #[test]
    fn routing_requires_exact_supported_core_resource() {
        use kube::core::GroupVersionKind;
        for (group, version, kind, supported) in [
            ("", "v1", "ConfigMap", true),
            ("", "v1", "Secret", true),
            ("", "v1", "Pod", true),
            ("example.com", "v1", "Secret", true),
            ("", "v2", "Secret", false),
            ("example.com", "v1", "Widget", true),
        ] {
            assert_eq!(
                supports(&ApiResource::from_gvk(&GroupVersionKind::gvk(
                    group, version, kind
                ))),
                supported
            );
        }
    }
}
