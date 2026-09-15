// Copyright 2014 The Kubernetes Authors.
// Copyright 2026 Nikola Milojević.
// SPDX-License-Identifier: Apache-2.0
// Adapted from kubectl v0.35.1 pkg/describe/describe.go. See LICENSE-APACHE.

use super::*;
use k8s_openapi::jiff::tz::TimeZone;
use serde_json::Value;
fn text(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}
fn items(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or_default()
}
pub(super) fn supports(ar: &ApiResource) -> bool {
    ar.group.is_empty()
        && matches!(
            ar.kind.as_str(),
            "PersistentVolume" | "PersistentVolumeClaim"
        )
}

pub(super) async fn related(
    client: Client,
    object: &DynamicObject,
) -> Result<Vec<DynamicObject>, String> {
    let ar = ApiResource::from_gvk(&kube::core::GroupVersionKind::gvk("", "v1", "Pod"));
    list_objects(
        client,
        &ar,
        object.metadata.namespace.as_deref().unwrap_or_default(),
        ListParams::default(),
    )
    .await
    .map_err(|e| e.to_string())
}

pub(super) fn render(
    object: &DynamicObject,
    kind: &str,
    pods: &[DynamicObject],
    events: Option<&[Event]>,
    now: Timestamp,
    zone: &TimeZone,
) -> String {
    let spec = &object.data["spec"];
    let status = &object.data["status"];
    let class = object
        .metadata
        .annotations
        .as_ref()
        .and_then(|a| a.get("volume.beta.kubernetes.io/storage-class"))
        .map(String::as_str)
        .unwrap_or_else(|| text(&spec["storageClassName"]));
    let phase = object.metadata.deletion_timestamp.as_ref().map_or_else(
        || text(&status["phase"]).into(),
        |time| format!("Terminating (lasts {})", age(Some(time.0), now)),
    );
    let finalizers = format!(
        "[{}]",
        object
            .metadata
            .finalizers
            .as_deref()
            .unwrap_or_default()
            .join(" ")
    );
    let mut out;
    if kind == "PersistentVolume" {
        out = metadata_header(&object.metadata, false);
        writeln!(
            out,
            "Finalizers:\t{finalizers}\nStorageClass:\t{class}\nStatus:\t{phase}"
        )
        .unwrap();
        let claim = &spec["claimRef"];
        writeln!(
            out,
            "Claim:\t{}\nReclaim Policy:\t{}\nAccess Modes:\t{}",
            if claim.is_null() {
                String::new()
            } else {
                format!("{}/{}", text(&claim["namespace"]), text(&claim["name"]))
            },
            text(&spec["persistentVolumeReclaimPolicy"]),
            access_modes(&spec["accessModes"])
        )
        .unwrap();
        if !spec["volumeMode"].is_null() {
            writeln!(out, "VolumeMode:\t{}", text(&spec["volumeMode"])).unwrap();
        }
        writeln!(
            out,
            "Capacity:\t{}",
            quantity::canonical(spec["capacity"]["storage"].as_str().unwrap_or("0"))
        )
        .unwrap();
        out.push_str("Node Affinity:\t");
        let required = &spec["nodeAffinity"]["required"];
        if required.is_null() {
            out.push_str("<none>\n");
        } else {
            out.push_str("\n  Required Terms:\t");
            let terms = items(&required["nodeSelectorTerms"]);
            if terms.is_empty() {
                out.push_str("<none>\n");
            } else {
                out.push('\n');
                for (i, term) in terms.iter().enumerate() {
                    write!(out, "    Term {i}:\t").unwrap();
                    let expressions = items(&term["matchExpressions"]);
                    if expressions.is_empty() {
                        out.push_str("<none>\n");
                    }
                    for (j, e) in expressions.iter().enumerate() {
                        if j > 0 {
                            out.push_str("    \t");
                        }
                        write!(
                            out,
                            "{} {}",
                            text(&e["key"]),
                            text(&e["operator"]).to_lowercase()
                        )
                        .unwrap();
                        let values = items(&e["values"]);
                        if !values.is_empty() {
                            write!(
                                out,
                                " [{}]",
                                values.iter().map(text).collect::<Vec<_>>().join(", ")
                            )
                            .unwrap();
                        }
                        out.push('\n');
                    }
                }
            }
        }
        writeln!(out, "Message:\t{}\nSource:", text(&status["message"])).unwrap();
        volumes::source(&mut out, spec, true);
    } else {
        out = format!(
            "Name:\t{}\nNamespace:\t{}\nStorageClass:\t{class}\nStatus:\t{phase}\nVolume:\t{}\n",
            object.metadata.name.as_deref().unwrap_or_default(),
            object.metadata.namespace.as_deref().unwrap_or_default(),
            text(&spec["volumeName"])
        );
        let header = metadata_header(&object.metadata, false);
        out.push_str(
            header
                .split_once('\n')
                .map(|(_, tail)| tail)
                .unwrap_or_default(),
        );
        writeln!(out, "Finalizers:\t{finalizers}").unwrap();
        let bound = !text(&spec["volumeName"]).is_empty();
        writeln!(
            out,
            "Capacity:\t{}\nAccess Modes:\t{}",
            if bound {
                quantity::canonical(status["capacity"]["storage"].as_str().unwrap_or("0"))
            } else {
                String::new()
            },
            if bound {
                access_modes(&status["accessModes"])
            } else {
                String::new()
            }
        )
        .unwrap();
        if !spec["volumeMode"].is_null() {
            writeln!(out, "VolumeMode:\t{}", text(&spec["volumeMode"])).unwrap();
        }
        let source = &spec["dataSource"];
        if !source.is_null() {
            out.push_str("DataSource:\n");
            if !source["apiGroup"].is_null() {
                writeln!(out, "  APIGroup:\t{}", text(&source["apiGroup"])).unwrap();
            }
            writeln!(
                out,
                "  Kind:\t{}\n  Name:\t{}",
                text(&source["kind"]),
                text(&source["name"])
            )
            .unwrap();
        }
        let mut users = Vec::new();
        for pod in pods {
            for volume in items(&pod.data["spec"]["volumes"]) {
                if !volume["persistentVolumeClaim"].is_null()
                    && volume["persistentVolumeClaim"]["claimName"].as_str()
                        == object.metadata.name.as_deref()
                {
                    users.push(pod);
                }
            }
        }
        for owner in object
            .metadata
            .owner_references
            .as_deref()
            .unwrap_or_default()
        {
            if owner.kind == "Pod"
                && let Some(pod) = pods
                    .iter()
                    .find(|p| p.metadata.uid.as_deref() == Some(&owner.uid))
                && !users.iter().any(|p| p.metadata.uid == pod.metadata.uid)
            {
                users.push(pod);
            }
        }
        users.sort_by(|a, b| a.metadata.name.cmp(&b.metadata.name));
        writeln!(
            out,
            "Used By:\t{}",
            if users.is_empty() {
                "<none>".into()
            } else {
                users
                    .iter()
                    .map(|p| p.metadata.name.as_deref().unwrap_or_default())
                    .collect::<Vec<_>>()
                    .join("\n\t")
            }
        )
        .unwrap();
        let conditions = items(&status["conditions"]);
        if !conditions.is_empty() {
            out.push_str("Conditions:\n  Type\tStatus\tLastProbeTime\tLastTransitionTime\tReason\tMessage\n  ----\t------\t-----------------\t------------------\t------\t-------\n");
            for c in conditions {
                writeln!(
                    out,
                    "  {} \t{} \t{} \t{} \t{} \t{}",
                    text(&c["type"]),
                    text(&c["status"]),
                    containers::timestamp(&c["lastProbeTime"], zone),
                    containers::timestamp(&c["lastTransitionTime"], zone),
                    text(&c["reason"]),
                    text(&c["message"])
                )
                .unwrap();
            }
        }
    }
    with_events(out, events, now)
}

fn access_modes(value: &Value) -> String {
    [
        ("ReadWriteOnce", "RWO"),
        ("ReadOnlyMany", "ROX"),
        ("ReadWriteMany", "RWX"),
        ("ReadWriteOncePod", "RWOP"),
    ]
    .into_iter()
    .filter(|(mode, _)| items(value).iter().any(|v| v.as_str() == Some(mode)))
    .map(|(_, short)| short)
    .collect::<Vec<_>>()
    .join(",")
}
