// Copyright 2014 The Kubernetes Authors.
// Copyright 2026 Nikola Milojević.
// SPDX-License-Identifier: Apache-2.0
// Adapted from kubectl v0.37.0 pkg/describe/describe.go and pkg/util/qos/qos.go.
// See LICENSE-APACHE.

use crate::events::with_events;
use crate::json::{items, text};
use crate::metadata::{annotation_section, label_section};
use crate::time::{age, value_timestamp};
use crate::{containers, node, policy, quantity, volumes};
use k8s_openapi::api::core::v1::Event;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use k8s_openapi::jiff::Timestamp;
use k8s_openapi::jiff::tz::TimeZone;
use kube::api::DynamicObject;
use serde::Deserialize;
use serde_json::Value;
use std::fmt::Write;

pub(crate) fn render(
    object: &DynamicObject,
    events: Option<&[Event]>,
    now: Timestamp,
    zone: &TimeZone,
) -> String {
    let spec = &object.data["spec"];
    let status = &object.data["status"];
    let mut out = format!(
        "Name:\t{}\nNamespace:\t{}\n",
        object.metadata.name.as_deref().unwrap_or_default(),
        object.metadata.namespace.as_deref().unwrap_or_default()
    );
    if let Some(priority) = spec["priority"].as_i64() {
        writeln!(out, "Priority:\t{priority}").unwrap();
    }
    for (field, label) in [
        ("priorityClassName", "Priority Class Name"),
        ("runtimeClassName", "Runtime Class Name"),
        ("serviceAccountName", "Service Account"),
    ] {
        if !text(&spec[field]).is_empty() {
            writeln!(out, "{label}:\t{}", text(&spec[field])).unwrap();
        }
    }
    writeln!(
        out,
        "Node:\t{}",
        if text(&spec["nodeName"]).is_empty() {
            "<none>".into()
        } else {
            format!("{}/{}", text(&spec["nodeName"]), text(&status["hostIP"]))
        }
    )
    .unwrap();
    if !status["startTime"].is_null() {
        writeln!(
            out,
            "Start Time:\t{}",
            value_timestamp(&status["startTime"], zone)
        )
        .unwrap();
    }
    out.push_str(&label_section(&object.metadata, ""));
    out.push_str(&annotation_section(&object.metadata, ""));
    if let Some(deleted) = &object.metadata.deletion_timestamp
        && !matches!(text(&status["phase"]), "Failed" | "Succeeded")
    {
        writeln!(
            out,
            "Status:\tTerminating (lasts {})\nTermination Grace Period:\t{}s",
            age(Some(deleted.0), now),
            object
                .metadata
                .deletion_grace_period_seconds
                .unwrap_or_default()
        )
        .unwrap();
    } else {
        writeln!(out, "Status:\t{}", text(&status["phase"])).unwrap();
    }
    for (field, label) in [("reason", "Reason"), ("message", "Message")] {
        if !text(&status[field]).is_empty() {
            writeln!(out, "{label}:\t{}", text(&status[field])).unwrap();
        }
    }
    let seccomp = &spec["securityContext"]["seccompProfile"];
    if !seccomp.is_null() {
        writeln!(out, "SeccompProfile:\t{}", text(&seccomp["type"])).unwrap();
        if text(&seccomp["type"]) == "Localhost" {
            writeln!(
                out,
                "LocalhostProfile:\t{}",
                text(&seccomp["localhostProfile"])
            )
            .unwrap();
        }
    }
    writeln!(out, "IP:\t{}", text(&status["podIP"])).unwrap();
    if items(&status["podIPs"]).is_empty() {
        out.push_str("IPs:\t<none>\n");
    } else {
        out.push_str("IPs:\n");
        for ip in items(&status["podIPs"]) {
            writeln!(out, "  IP:\t{}", text(&ip["ip"])).unwrap();
        }
    }
    if let Some(owner) = object
        .metadata
        .owner_references
        .iter()
        .flatten()
        .find(|o| o.controller == Some(true))
    {
        writeln!(out, "Controlled By:\t{}/{}", owner.kind, owner.name).unwrap();
    }
    if !text(&status["nominatedNodeName"]).is_empty() {
        writeln!(
            out,
            "NominatedNodeName:\t{}",
            text(&status["nominatedNodeName"])
        )
        .unwrap();
    }
    if !spec["resources"].is_null() {
        out.push_str("Resources:\n");
        containers::resources(&mut out, &spec["resources"], 1);
    }
    for (field, status_field, label) in [
        ("initContainers", "initContainerStatuses", "Init Containers"),
        ("containers", "containerStatuses", "Containers"),
        (
            "ephemeralContainers",
            "ephemeralContainerStatuses",
            "Ephemeral Containers",
        ),
    ] {
        if field == "containers" || !items(&spec[field]).is_empty() {
            containers::render(
                &mut out,
                label,
                items(&spec[field]),
                items(&status[status_field]),
                Some(object),
                "",
                zone,
            );
        }
    }
    if !items(&spec["readinessGates"]).is_empty() {
        out.push_str("Readiness Gates:\n  Type\tStatus\n");
        for gate in items(&spec["readinessGates"]) {
            let condition = items(&status["conditions"])
                .iter()
                .find(|c| c["type"] == gate["conditionType"]);
            writeln!(
                out,
                "  {} \t{} ",
                text(&gate["conditionType"]),
                condition.map(|c| text(&c["status"])).unwrap_or("<none>")
            )
            .unwrap();
        }
    }
    if !items(&status["conditions"]).is_empty() {
        out.push_str("Conditions:\n  Type\tStatus\n");
        for condition in items(&status["conditions"]) {
            writeln!(
                out,
                "  {} \t{} ",
                text(&condition["type"]),
                text(&condition["status"])
            )
            .unwrap();
        }
    }
    volumes::render(&mut out, items(&spec["volumes"]), "");
    writeln!(out, "QoS Class:\t{}", qos(spec, status)).unwrap();
    scheduling(&mut out, spec, "");
    topology(&mut out, spec, "");
    workload(&mut out, spec, "");
    with_events(out, events, now)
}

pub(crate) fn template(out: &mut String, template: &Value, zone: &TimeZone) {
    out.push_str("Pod Template:\n");
    if template.is_null() {
        out.push_str("  <unset>");
        return;
    }
    // Borrow the metadata rather than cloning the subtree into `from_value`.
    let meta = ObjectMeta::deserialize(&template["metadata"]).unwrap_or_default();
    // Indent section titles only. Keep continuation lines at their current level.
    out.push_str(&label_section(&meta, "  "));
    if meta.annotations.as_ref().is_some_and(|a| !a.is_empty()) {
        out.push_str(&annotation_section(&meta, "  "));
    }
    let spec = &template["spec"];
    if !text(&spec["serviceAccountName"]).is_empty() {
        writeln!(
            out,
            "  Service Account:\t{}",
            text(&spec["serviceAccountName"])
        )
        .unwrap();
    }
    if !items(&spec["initContainers"]).is_empty() {
        containers::render(
            out,
            "Init Containers",
            items(&spec["initContainers"]),
            &[],
            None,
            "  ",
            zone,
        );
    }
    containers::render(
        out,
        "Containers",
        items(&spec["containers"]),
        &[],
        None,
        "  ",
        zone,
    );
    volumes::render(out, items(&spec["volumes"]), "  ");
    topology(out, spec, "  ");
    if !text(&spec["priorityClassName"]).is_empty() {
        writeln!(
            out,
            "  Priority Class Name:\t{}",
            text(&spec["priorityClassName"])
        )
        .unwrap();
    }
    scheduling(out, spec, "  ");
    workload(out, spec, "  ");
}

pub(crate) fn scheduling(out: &mut String, spec: &Value, space: &str) {
    write!(out, "{space}Node-Selectors:\t").unwrap();
    let mut selectors: Vec<_> = spec["nodeSelector"]
        .as_object()
        .into_iter()
        .flatten()
        .collect();
    selectors.sort_by_key(|(k, _)| *k);
    if selectors.is_empty() {
        out.push_str("<none>\n");
    }
    for (i, (k, v)) in selectors.iter().enumerate() {
        if i > 0 {
            write!(out, "{space}\t").unwrap();
        }
        writeln!(out, "{k}={}", text(v)).unwrap();
    }
    write!(out, "{space}Tolerations:\t").unwrap();
    let mut tolerations: Vec<_> = items(&spec["tolerations"]).iter().collect();
    tolerations.sort_by_key(|t| text(&t["key"]));
    if tolerations.is_empty() {
        out.push_str("<none>\n");
    }
    for (i, t) in tolerations.iter().enumerate() {
        if i > 0 {
            write!(out, "{space}\t").unwrap();
        }
        out.push_str(text(&t["key"]));
        if !text(&t["value"]).is_empty() {
            write!(out, "={}", text(&t["value"])).unwrap();
        }
        if !text(&t["effect"]).is_empty() {
            write!(out, ":{}", text(&t["effect"])).unwrap();
        }
        if text(&t["operator"]) == "Exists" && text(&t["value"]).is_empty() {
            if !text(&t["key"]).is_empty() || !text(&t["effect"]).is_empty() {
                out.push(' ');
            }
            out.push_str("op=Exists");
        }
        if let Some(seconds) = t["tolerationSeconds"].as_i64() {
            write!(out, " for {seconds}s").unwrap();
        }
        out.push('\n');
    }
}

pub(crate) fn topology(out: &mut String, spec: &Value, space: &str) {
    let mut constraints: Vec<_> = items(&spec["topologySpreadConstraints"]).iter().collect();
    constraints.sort_by_key(|t| text(&t["topologyKey"]));
    if !constraints.is_empty() {
        write!(out, "{space}Topology Spread Constraints:\t").unwrap();
    }
    for (i, c) in constraints.iter().enumerate() {
        if i > 0 {
            write!(out, "{space}\t").unwrap();
        }
        write!(
            out,
            "{}:{} when max skew {} is exceeded",
            text(&c["topologyKey"]),
            text(&c["whenUnsatisfiable"]),
            c["maxSkew"].as_i64().unwrap_or_default()
        )
        .unwrap();
        if !c["labelSelector"].is_null() {
            write!(
                out,
                " for selector {}",
                policy::selector(&c["labelSelector"])
            )
            .unwrap();
        }
        out.push('\n');
    }
}

pub(crate) fn workload(out: &mut String, spec: &Value, space: &str) {
    if let Some(group) = spec["schedulingGroup"].as_object() {
        writeln!(
            out,
            "{space}SchedulingGroup:\n  PodGroupName:\t{}",
            group
                .get("podGroupName")
                .and_then(Value::as_str)
                .unwrap_or_default()
        )
        .unwrap();
        return;
    }
    // Retain the earlier API's workload reference when describing older objects.
    let workload = &spec["workloadRef"];
    if !workload.is_null() {
        writeln!(
            out,
            "{space}WorkloadRef:\n  Name:\t{}\n  PodGroup:\t{}",
            text(&workload["name"]),
            text(&workload["podGroup"])
        )
        .unwrap();
        if !text(&workload["podGroupReplicaKey"]).is_empty() {
            writeln!(
                out,
                "  PodGroupReplicaKey:\t{}",
                text(&workload["podGroupReplicaKey"])
            )
            .unwrap();
        }
    }
}

fn qos<'a>(spec: &Value, status: &'a Value) -> &'a str {
    if !text(&status["qosClass"]).is_empty() {
        return text(&status["qosClass"]);
    }
    let resources: Vec<_> = if ["requests", "limits"].iter().any(|field| {
        spec["resources"][field]
            .as_object()
            .is_some_and(|r| r.keys().any(|name| node::pod_level_resource(name)))
    }) {
        vec![&spec["resources"]]
    } else {
        items(&spec["containers"])
            .iter()
            .chain(items(&spec["initContainers"]))
            .map(|c| &c["resources"])
            .collect()
    };
    let mut qos = None;
    for r in resources {
        for name in ["cpu", "memory"] {
            let request = quantity::parse(text(&r["requests"][name])).unwrap_or_default();
            let limit = quantity::parse(text(&r["limits"][name])).unwrap_or_default();
            let resource_qos = if request != limit {
                "Burstable"
            } else if request == 0 {
                "BestEffort"
            } else {
                "Guaranteed"
            };
            if resource_qos == "Burstable" || qos.is_some_and(|previous| previous != resource_qos) {
                return "Burstable";
            }
            qos = Some(resource_qos);
        }
    }
    qos.unwrap_or("BestEffort")
}
