// Copyright 2014 The Kubernetes Authors.
// Copyright 2026 Nikola Milojević.
// SPDX-License-Identifier: Apache-2.0
// Adapted from kubectl v0.37.0 pkg/describe/describe.go and pkg/util/deployment.
// See LICENSE-APACHE.

use super::*;
use k8s_openapi::jiff::tz::TimeZone;
use serde_json::Value;
fn text(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}
fn items(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or_default()
}
fn integer(v: &Value) -> i64 {
    v.as_i64().unwrap_or_default()
}
fn scalar(v: &Value) -> String {
    v.as_str().map(str::to_owned).unwrap_or_else(|| {
        if v.is_null() {
            "<nil>".into()
        } else {
            v.to_string()
        }
    })
}

pub(super) fn supports(ar: &ApiResource) -> bool {
    (ar.group.is_empty() && ar.kind == "ReplicationController")
        || (ar.group == "apps"
            && matches!(
                ar.kind.as_str(),
                "ReplicaSet" | "DaemonSet" | "StatefulSet" | "Deployment"
            ))
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

pub(super) async fn related(
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

pub(super) fn render(
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
    let mut out = metadata(&object.metadata);
    let created = object
        .metadata
        .creation_timestamp
        .as_ref()
        .map(|t| Value::String(t.0.to_string()))
        .unwrap_or(Value::Null);
    let mut prefix = String::new();
    if matches!(kind, "Deployment" | "StatefulSet") {
        writeln!(
            prefix,
            "CreationTimestamp:\t{}",
            containers::timestamp(&created, zone)
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
    out = out.replacen("Labels:", &format!("{prefix}Labels:"), 1);
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
    workloads::template(&mut out, &spec["template"], zone);
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
        let template = without_hash(&spec["template"]);
        let new = owned
            .iter()
            .find(|rs| without_hash(&rs.data["spec"]["template"]) == template)
            .copied();
        let old: Vec<_> = owned
            .iter()
            .copied()
            .filter(|rs| new.is_none_or(|new| new.metadata.uid != rs.metadata.uid))
            .collect();
        if !old.is_empty() || new.is_some() {
            writeln!(
                out,
                "OldReplicaSets:\t{}\nNewReplicaSet:\t{}",
                replica_sets(&old),
                replica_sets(&new.into_iter().collect::<Vec<_>>())
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
fn without_hash(template: &Value) -> Value {
    let mut template = template.clone();
    if let Some(labels) = template
        .pointer_mut("/metadata/labels")
        .and_then(Value::as_object_mut)
    {
        labels.remove("pod-template-hash");
    }
    template
}
fn replica_sets(sets: &[&DynamicObject]) -> String {
    if sets.is_empty() {
        return "<none>".into();
    }
    sets.iter()
        .map(|rs| {
            format!(
                "{} ({}/{} replicas created)",
                rs.metadata.name.as_deref().unwrap_or_default(),
                integer(&rs.data["status"]["replicas"]),
                integer(&rs.data["spec"]["replicas"])
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
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
