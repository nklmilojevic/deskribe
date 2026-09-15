// Copyright 2014 The Kubernetes Authors.
// Copyright 2026 Nikola Milojević.
// SPDX-License-Identifier: Apache-2.0
// Adapted from kubectl v0.37.0 pkg/describe/describe.go and pkg/util/deployment.
// See LICENSE-APACHE.

use crate::api::list_objects;
use crate::events::with_events;
use crate::json::{integer, items, text};
use crate::metadata::{annotation_section, identity, label_section};
use crate::time::optional_timestamp;
use crate::{pod, policy, quantity};
use k8s_openapi::api::core::v1::Event;
use k8s_openapi::jiff::Timestamp;
use k8s_openapi::jiff::tz::TimeZone;
use kube::Client;
use kube::api::{ApiResource, DynamicObject, ListParams};
use serde_json::Value;
use std::fmt::Write;

fn scalar(v: &Value) -> String {
    v.as_str().map(str::to_owned).unwrap_or_else(|| {
        if v.is_null() {
            "<nil>".into()
        } else {
            v.to_string()
        }
    })
}

fn selector(object: &DynamicObject, kind: &str) -> String {
    if kind == "ReplicationController" {
        labels(&object.data["spec"]["selector"])
    } else {
        policy::selector(&object.data["spec"]["selector"])
    }
}
fn labels(value: &Value) -> String {
    let mut pairs: Vec<_> = value
        .as_object()
        .into_iter()
        .flatten()
        .map(|(k, v)| format!("{k}={}", text(v)))
        .collect();
    pairs.sort();
    if pairs.is_empty() {
        "<none>".into()
    } else {
        pairs.join(",")
    }
}

pub(crate) async fn related(
    client: Client,
    object: &DynamicObject,
    kind: &str,
) -> Result<Vec<DynamicObject>, String> {
    let query = selector(object, kind);
    if query == "<error>" {
        return Err("invalid controller label selector".into());
    }
    let (group, resource_kind) = if kind == "Deployment" {
        ("apps", "ReplicaSet")
    } else {
        ("", "Pod")
    };
    let ar = ApiResource::from_gvk(&kube::core::GroupVersionKind::gvk(
        group,
        "v1",
        resource_kind,
    ));
    let params = ListParams::default().labels(if query == "<none>" { "" } else { &query });
    list_objects(
        client,
        &ar,
        object.metadata.namespace.as_deref().unwrap_or_default(),
        params,
    )
    .await
    .map_err(|e| e.to_string())
}

pub(crate) fn render(
    object: &DynamicObject,
    kind: &str,
    related: Result<&[DynamicObject], &str>,
    events: Option<&[Event]>,
    now: Timestamp,
    zone: &TimeZone,
) -> Result<String, String> {
    if !matches!(kind, "Deployment" | "ReplicaSet")
        && let Err(error) = related
    {
        return Err(error.into());
    }
    let spec = &object.data["spec"];
    let status = &object.data["status"];
    let selected = selector(object, kind);
    if selected == "<error>" {
        return Err("invalid controller label selector".into());
    }
    let raw_selector = if selected == "<none>" { "" } else { &selected };
    let mut out = identity(&object.metadata, true);
    let mut prefix = String::new();
    if matches!(kind, "Deployment" | "StatefulSet") {
        writeln!(
            prefix,
            "CreationTimestamp:\t{}",
            optional_timestamp(
                object
                    .metadata
                    .creation_timestamp
                    .as_ref()
                    .map(|time| time.0),
                zone
            )
        )
        .unwrap();
    }
    if kind != "Deployment" {
        writeln!(
            prefix,
            "Selector:\t{}",
            if matches!(kind, "ReplicationController" | "ReplicaSet") {
                &selected
            } else {
                raw_selector
            }
        )
        .unwrap();
    }
    if kind == "DaemonSet" {
        writeln!(
            prefix,
            "Node-Selector:\t{}",
            labels(&spec["template"]["spec"]["nodeSelector"])
        )
        .unwrap();
    }
    out.push_str(&prefix);
    out.push_str(&label_section(&object.metadata, ""));
    out.push_str(&annotation_section(&object.metadata, ""));
    if kind == "ReplicaSet"
        && let Some(owner) = object
            .metadata
            .owner_references
            .iter()
            .flatten()
            .find(|o| o.controller == Some(true))
    {
        writeln!(out, "Controlled By:\t{}/{}", owner.kind, owner.name).unwrap();
    }
    match kind {
        "ReplicationController" | "ReplicaSet" => writeln!(
            out,
            "Replicas:\t{} current / {} desired",
            integer(&status["replicas"]),
            integer(&spec["replicas"])
        )
        .unwrap(),
        "DaemonSet" => {
            for (field, label) in [
                (
                    "desiredNumberScheduled",
                    "Desired Number of Nodes Scheduled",
                ),
                (
                    "currentNumberScheduled",
                    "Current Number of Nodes Scheduled",
                ),
                (
                    "updatedNumberScheduled",
                    "Number of Nodes Scheduled with Up-to-date Pods",
                ),
                (
                    "numberAvailable",
                    "Number of Nodes Scheduled with Available Pods",
                ),
                ("numberMisscheduled", "Number of Nodes Misscheduled"),
            ] {
                writeln!(out, "{label}: {}", integer(&status[field])).unwrap();
            }
        }
        "StatefulSet" => {
            for (field, label) in [
                ("serviceName", "Service Name"),
                ("podManagementPolicy", "Pod Management Policy"),
            ] {
                if !text(&spec[field]).is_empty() {
                    writeln!(out, "{label}:\t{}", text(&spec[field])).unwrap();
                }
            }
            writeln!(
                out,
                "Replicas:\t{} desired | {} total\nUpdate Strategy:\t{}",
                integer(&spec["replicas"]),
                integer(&status["replicas"]),
                text(&spec["updateStrategy"]["type"])
            )
            .unwrap();
            let rolling = &spec["updateStrategy"]["rollingUpdate"];
            if !rolling["partition"].is_null() {
                writeln!(out, "  Partition:\t{}", integer(&rolling["partition"])).unwrap();
                if !rolling["maxUnavailable"].is_null() {
                    writeln!(
                        out,
                        "  MaxUnavailable:\t{}",
                        scalar(&rolling["maxUnavailable"])
                    )
                    .unwrap();
                }
            }
            let retention = &spec["persistentVolumeClaimRetentionPolicy"];
            if !retention.is_null() {
                writeln!(out, "Persistent Volume Claim Retention Policy:\n  WhenDeleted:\t{}\n  WhenScaled:\t{}", text(&retention["whenDeleted"]), text(&retention["whenScaled"])).unwrap();
            }
        }
        "Deployment" => {
            writeln!(out,"Selector:\t{raw_selector}\nReplicas:\t{} desired | {} updated | {} total | {} available | {} unavailable\nStrategyType:\t{}\nMinReadySeconds:\t{}",integer(&spec["replicas"]),integer(&status["updatedReplicas"]),integer(&status["replicas"]),integer(&status["availableReplicas"]),integer(&status["unavailableReplicas"]),text(&spec["strategy"]["type"]),integer(&spec["minReadySeconds"])).unwrap();
            let rolling = &spec["strategy"]["rollingUpdate"];
            if !rolling.is_null() {
                writeln!(
                    out,
                    "RollingUpdateStrategy:\t{} max unavailable, {} max surge",
                    scalar(&rolling["maxUnavailable"]),
                    scalar(&rolling["maxSurge"])
                )
                .unwrap();
            }
        }
        _ => unreachable!(),
    }
    if kind != "Deployment" {
        out.push_str("Pods Status:\t");
        match related {
            Err(error) => writeln!(out, "error in fetching pods: {error}").unwrap(),
            Ok(pods) => {
                let mut counts = [0; 4];
                for pod in pods.iter().filter(|p| owned_by(p, object)) {
                    if let Some(index) = ["Running", "Pending", "Succeeded", "Failed"]
                        .iter()
                        .position(|phase| *phase == text(&pod.data["status"]["phase"]))
                    {
                        counts[index] += 1;
                    }
                }
                writeln!(
                    out,
                    "{} Running / {} Waiting / {} Succeeded / {} Failed",
                    counts[0], counts[1], counts[2], counts[3]
                )
                .unwrap();
            }
        }
    }
    pod::template(&mut out, &spec["template"], zone);
    if matches!(kind, "Deployment" | "ReplicaSet" | "ReplicationController")
        && !items(&status["conditions"]).is_empty()
    {
        out.push_str("Conditions:\n  Type\tStatus\tReason\n  ----\t------\t------\n");
        for c in items(&status["conditions"]) {
            writeln!(
                out,
                "  {} \t{}\t{}",
                text(&c["type"]),
                text(&c["status"]),
                text(&c["reason"])
            )
            .unwrap();
        }
    }
    if kind == "StatefulSet" {
        claims(&mut out, items(&spec["volumeClaimTemplates"]));
    }
    if kind == "Deployment"
        && let Ok(sets) = related
    {
        let mut owned: Vec<_> = sets.iter().filter(|rs| owned_by(rs, object)).collect();
        owned.sort_by(|a, b| {
            a.metadata
                .creation_timestamp
                .cmp(&b.metadata.creation_timestamp)
                .then_with(|| a.metadata.name.cmp(&b.metadata.name))
        });
        let template = &spec["template"];
        let new = owned
            .iter()
            .find(|rs| templates_match(&rs.data["spec"]["template"], template))
            .copied();
        let is_old =
            |rs: &&DynamicObject| new.is_none_or(|new| new.metadata.uid != rs.metadata.uid);
        if owned.iter().any(is_old) || new.is_some() {
            writeln!(
                out,
                "OldReplicaSets:\t{}\nNewReplicaSet:\t{}",
                replica_sets(owned.iter().copied().filter(is_old)),
                replica_sets(new)
            )
            .unwrap();
        }
    }
    Ok(with_events(out, events, now))
}
fn owned_by(child: &DynamicObject, parent: &DynamicObject) -> bool {
    child
        .metadata
        .owner_references
        .iter()
        .flatten()
        .find(|o| o.controller == Some(true))
        .is_some_and(|o| Some(&o.uid) == parent.metadata.uid.as_ref())
}
/// The label the Deployment controller stamps onto each ReplicaSet's template,
/// and the only reason a current ReplicaSet's template differs from its
/// Deployment's.
const POD_TEMPLATE_HASH: &str = "pod-template-hash";

/// Compare two pod templates while ignoring `pod-template-hash`.
///
/// A pod template carries every container, env var, probe and volume of the
/// workload, and the previous approach deep-copied one per ReplicaSet just to
/// drop a single label before `==`. Walking both in place reaches the same
/// answer without allocating.
fn templates_match(a: &Value, b: &Value) -> bool {
    object_match(a, b, |key, x, y| {
        if key == "metadata" {
            metadata_match(x, y)
        } else {
            x == y
        }
    })
}

fn metadata_match(a: &Value, b: &Value) -> bool {
    object_match(a, b, |key, x, y| {
        if key == "labels" {
            labels_match(x, y)
        } else {
            x == y
        }
    })
}

fn labels_match(a: &Value, b: &Value) -> bool {
    let (Some(x), Some(y)) = (a.as_object(), b.as_object()) else {
        return a == b;
    };
    let kept = |m: &serde_json::Map<String, Value>| {
        m.len() - usize::from(m.contains_key(POD_TEMPLATE_HASH))
    };
    kept(x) == kept(y)
        && x.iter()
            .filter(|(key, _)| key.as_str() != POD_TEMPLATE_HASH)
            .all(|(key, value)| y.get(key) == Some(value))
}

/// Compare two JSON objects key by key, deferring to `compare` for the values.
/// Non-objects fall back to plain equality, matching what a clone-and-compare
/// did for a missing or malformed template.
fn object_match(a: &Value, b: &Value, compare: impl Fn(&str, &Value, &Value) -> bool) -> bool {
    let (Some(x), Some(y)) = (a.as_object(), b.as_object()) else {
        return a == b;
    };
    x.len() == y.len()
        && x.iter().all(|(key, value)| match y.get(key) {
            Some(other) => compare(key, value, other),
            None => false,
        })
}
fn replica_sets<'a>(sets: impl IntoIterator<Item = &'a DynamicObject>) -> String {
    let mut out = String::new();
    for rs in sets {
        if !out.is_empty() {
            out.push_str(", ");
        }
        write!(
            out,
            "{} ({}/{} replicas created)",
            rs.metadata.name.as_deref().unwrap_or_default(),
            integer(&rs.data["status"]["replicas"]),
            integer(&rs.data["spec"]["replicas"])
        )
        .unwrap();
    }
    if out.is_empty() { "<none>".into() } else { out }
}
fn claims(out: &mut String, claims: &[Value]) {
    if claims.is_empty() {
        out.push_str("Volume Claims:\t<none>\n");
        return;
    }
    out.push_str("Volume Claims:\n");
    for claim in claims {
        let spec = &claim["spec"];
        let meta = &claim["metadata"];
        let class = meta["annotations"]["volume.beta.kubernetes.io/storage-class"]
            .as_str()
            .unwrap_or_else(|| text(&spec["storageClassName"]));
        writeln!(
            out,
            "  Name:\t{}\n  StorageClass:\t{class}",
            text(&meta["name"])
        )
        .unwrap();
        for (field, label) in [("labels", "Labels"), ("annotations", "Annotations")] {
            write!(out, "  {label}:\t").unwrap();
            let mut pairs: Vec<_> = meta[field].as_object().into_iter().flatten().collect();
            pairs.sort_by_key(|(k, _)| *k);
            if pairs.is_empty() {
                out.push_str("<none>\n");
            }
            for (i, (k, v)) in pairs.iter().enumerate() {
                if i > 0 {
                    out.push_str("  \t");
                }
                writeln!(out, "{k}={}", text(v)).unwrap();
            }
        }
        writeln!(
            out,
            "  Capacity:\t{}\n  Access Modes:\t[{}]",
            spec["resources"]["requests"]["storage"]
                .as_str()
                .map(quantity::canonical)
                .unwrap_or("<default>".into()),
            items(&spec["accessModes"])
                .iter()
                .map(text)
                .collect::<Vec<_>>()
                .join(" ")
        )
        .unwrap();
    }
}
