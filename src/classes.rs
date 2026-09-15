// Copyright 2014 The Kubernetes Authors.
// Copyright 2026 sofka contributors.
// SPDX-License-Identifier: Apache-2.0
// Adapted from kubectl v0.35.1 pkg/describe/describe.go. See LICENSE-APACHE.

use super::*;
use serde_json::Value;

pub(super) fn supports(ar: &ApiResource) -> bool {
    matches!(
        (ar.group.as_str(), ar.kind.as_str()),
        ("", "ServiceAccount")
            | (
                "storage.k8s.io",
                "StorageClass" | "VolumeAttributesClass" | "CSINode"
            )
            | (
                "networking.k8s.io",
                "IngressClass" | "ServiceCIDR" | "IPAddress"
            )
            | ("" | "scheduling.k8s.io", "PriorityClass")
            | (
                "rbac.authorization.k8s.io",
                "RoleBinding" | "ClusterRoleBinding"
            )
    )
}

pub(super) fn events(ar: &ApiResource) -> bool {
    ar.group != "rbac.authorization.k8s.io"
}

pub(super) fn render(
    object: &DynamicObject,
    kind: &str,
    events: Option<&[Event]>,
    now: Timestamp,
    zone: &k8s_openapi::jiff::tz::TimeZone,
) -> String {
    let value = &object.data;
    let mut out = format!(
        "Name:\t{}\n",
        object.metadata.name.as_deref().unwrap_or_default()
    );
    match kind {
        "ServiceAccount" => {
            out = metadata(&object.metadata);
            let names: Vec<_> = value["imagePullSecrets"].as_array().into_iter().flatten().map(|s| text(&s["name"])).collect();
            if names.is_empty() { out.push_str("Image pull secrets:\t<none>\n"); }
            else {
                for (i, name) in names.iter().enumerate() {
                    writeln!(out, "{}\t{name}", if i == 0 { "Image pull secrets:" } else { "                   " }).unwrap();
                }
            }
        }
        "RoleBinding" | "ClusterRoleBinding" => {
            out = metadata_header(&object.metadata, false);
            writeln!(out, "Role:\n  Kind:\t{}\n  Name:\t{}\nSubjects:\n  Kind\tName\tNamespace\n  ----\t----\t---------", text(&value["roleRef"]["kind"]), text(&value["roleRef"]["name"])).unwrap();
            for subject in value["subjects"].as_array().into_iter().flatten() {
                writeln!(out, "  {}\t{}\t{}", text(&subject["kind"]), text(&subject["name"]), text(&subject["namespace"])).unwrap();
            }
        }
        "StorageClass" => {
            let default = object.metadata.annotations.as_ref().is_some_and(|a| ["storageclass.kubernetes.io/is-default-class", "storageclass.beta.kubernetes.io/is-default-class"].iter().any(|key| a.get(*key).is_some_and(|v| v == "true")));
            writeln!(out, "IsDefaultClass:\t{}\nAnnotations:\t{}\nProvisioner:\t{}\nParameters:\t{}\nAllowVolumeExpansion:\t{}", if default { "Yes" } else { "No" }, annotations(object), text(&value["provisioner"]), labels(&value["parameters"]), value["allowVolumeExpansion"].as_bool().map(|b| if b { "True" } else { "False" }).unwrap_or("<unset>")).unwrap();
            let options = value["mountOptions"].as_array().cloned().unwrap_or_default();
            if options.is_empty() { out.push_str("MountOptions:\t<none>\n"); }
            else {
                out.push_str("MountOptions:\n");
                for option in options { writeln!(out, "  {}", text(&option)).unwrap(); }
            }
            for (field, label) in [("reclaimPolicy", "ReclaimPolicy"), ("volumeBindingMode", "VolumeBindingMode")] {
                if !value[field].is_null() { writeln!(out, "{label}:\t{}", text(&value[field])).unwrap(); }
            }
            if let Some(terms) = value["allowedTopologies"].as_array() {
                out.push_str("AllowedTopologies:\t");
                if terms.is_empty() { out.push_str("<none>\n"); }
                else {
                    out.push('\n');
                    for (i, term) in terms.iter().enumerate() {
                        write!(out, "  Term {i}:\t").unwrap();
                        let reqs = term["matchLabelExpressions"].as_array().cloned().unwrap_or_default();
                        if reqs.is_empty() { out.push_str("<none>\n"); }
                        for (j, req) in reqs.iter().enumerate() {
                            if j > 0 { out.push_str("  \t"); }
                            write!(out, "{} in", text(&req["key"])).unwrap();
                            let values: Vec<_> = req["values"].as_array().into_iter().flatten().map(text).collect();
                            if !values.is_empty() { write!(out, " [{}]", values.join(", ")).unwrap(); }
                            out.push('\n');
                        }
                    }
                }
            }
        }
        "VolumeAttributesClass" => writeln!(out, "Annotations:\t{}\nDriverName:\t{}\nParameters:\t{}", annotations(object), text(&value["driverName"]), labels(&value["parameters"])).unwrap(),
        "PriorityClass" => writeln!(out, "Value:\t{}\nGlobalDefault:\t{}\nPreemptionPolicy:\t{}\nDescription:\t{}\nAnnotations:\t{}", value["value"].as_i64().unwrap_or_default(), value["globalDefault"].as_bool().unwrap_or_default(), text(&value["preemptionPolicy"]), text(&value["description"]), annotations(object)).unwrap(),
        "IngressClass" => {
            out = metadata_header(&object.metadata, false);
            writeln!(out, "Controller:\t{}", text(&value["spec"]["controller"])).unwrap();
            let p = &value["spec"]["parameters"];
            if !p.is_null() {
                out.push_str("Parameters:\n");
                if !p["apiGroup"].is_null() { writeln!(out, "  APIGroup:\t{}", text(&p["apiGroup"])).unwrap(); }
                writeln!(out, "  Kind:\t{}\n  Name:\t{}", text(&p["kind"]), text(&p["name"])).unwrap();
            }
        }
        "IPAddress" => {
            out = metadata_header(&object.metadata, false);
            let p = &value["spec"]["parentRef"];
            if !p.is_null() {
                out.push_str("Parent Reference:\n");
                for (field, label) in [("group", "Group"), ("resource", "Resource"), ("namespace", "Namespace"), ("name", "Name")] {
                    writeln!(out, "  {label}:\t{}", text(&p[field])).unwrap();
                }
            }
        }
        "ServiceCIDR" => {
            out = metadata_header(&object.metadata, false);
            let cidrs: Vec<_> = value["spec"]["cidrs"].as_array().into_iter().flatten().map(text).collect();
            writeln!(out, "CIDRs:\t{}", cidrs.join(", ")).unwrap();
            let conditions = value["status"]["conditions"].as_array().cloned().unwrap_or_default();
            if !conditions.is_empty() {
                out.push_str("Status:\nConditions:\n  Type\tStatus\tLastTransitionTime\tReason\tMessage\n  ----\t------\t------------------\t------\t-------\n");
                for c in conditions {
                    writeln!(out, "  {}\t{}\t{}\t{}\t{}", text(&c["type"]), text(&c["status"]), timestamp(text(&c["lastTransitionTime"]), zone), text(&c["reason"]), text(&c["message"])).unwrap();
                }
            }
        }
        "CSINode" => {
            out = metadata_header(&object.metadata, false);
            let created = object.metadata.creation_timestamp.as_ref().map(|t| t.0.to_string()).unwrap_or_default();
            writeln!(out, "CreationTimestamp:\t{}\nSpec:", timestamp(&created, zone)).unwrap();
            if let Some(drivers) = value["spec"]["drivers"].as_array() {
                out.push_str("  Drivers:\n");
                for driver in drivers {
                    writeln!(out, "    {}:\n      Node ID:\t{}", text(&driver["name"]), text(&driver["nodeID"])).unwrap();
                    if let Some(count) = driver["allocatable"]["count"].as_i64() { writeln!(out, "      Allocatables:\n        Count:\t{count}").unwrap(); }
                    if let Some(keys) = driver["topologyKeys"].as_array() { writeln!(out, "      Topology Keys:\t[{}]", keys.iter().map(text).collect::<Vec<_>>().join(" ")).unwrap(); }
                }
            }
        }
        _ => unreachable!("routing limits class renderer"),
    }
    with_events(out, events, now)
}

fn timestamp(value: &str, zone: &k8s_openapi::jiff::tz::TimeZone) -> String {
    value
        .parse::<Timestamp>()
        .ok()
        .map(|t| {
            t.to_zoned(zone.clone())
                .strftime("%a, %d %b %Y %H:%M:%S %z")
                .to_string()
        })
        .unwrap_or_else(|| "Mon, 01 Jan 0001 00:00:00 +0000".into())
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}

fn annotations(object: &DynamicObject) -> String {
    let pairs: Vec<_> = object
        .metadata
        .annotations
        .iter()
        .flatten()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    if pairs.is_empty() {
        "<none>".into()
    } else {
        pairs.join(",")
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
