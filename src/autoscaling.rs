// Copyright 2014 The Kubernetes Authors.
// Copyright 2026 Nikola Milojević.
// SPDX-License-Identifier: Apache-2.0
// Adapted from kubectl v0.35.1 pkg/describe/describe.go. See LICENSE-APACHE.

use crate::events::with_events;
use crate::json::{integer, items, text};
use crate::metadata::metadata;
use crate::time::optional_timestamp;
use k8s_openapi::api::core::v1::Event;
use k8s_openapi::jiff::Timestamp;
use k8s_openapi::jiff::tz::TimeZone;
use kube::api::DynamicObject;
use serde_json::Value;
use std::fmt::Write;

fn quantity(v: &Value) -> String {
    v.as_str()
        .map(crate::quantity::canonical)
        .unwrap_or("<nil>".into())
}

pub(crate) fn render(
    object: &DynamicObject,
    events: Option<&[Event]>,
    now: Timestamp,
    zone: &TimeZone,
) -> String {
    let spec = &object.data["spec"];
    let status = &object.data["status"];
    let legacy = object
        .types
        .as_ref()
        .is_some_and(|t| t.api_version == "autoscaling/v1");
    let mut out = metadata(&object.metadata);
    writeln!(
        out,
        "CreationTimestamp:\t{}\nReference:\t{}/{}",
        optional_timestamp(
            object
                .metadata
                .creation_timestamp
                .as_ref()
                .map(|time| time.0),
            zone
        ),
        text(&spec["scaleTargetRef"]["kind"]),
        text(&spec["scaleTargetRef"]["name"])
    )
    .unwrap();
    if legacy {
        if !spec["targetCPUUtilizationPercentage"].is_null() {
            writeln!(
                out,
                "Target CPU utilization:\t{}%\nCurrent CPU utilization:\t{}%",
                integer(&spec["targetCPUUtilizationPercentage"]),
                status["currentCPUUtilizationPercentage"]
                    .as_i64()
                    .map(|n| n.to_string())
                    .unwrap_or("<unknown>".into())
            )
            .unwrap();
        }
    } else {
        out.push_str("Metrics:\t( current / target )\n");
        for (i, metric) in items(&spec["metrics"]).iter().enumerate() {
            let kind = text(&metric["type"]);
            let field = match kind {
                "External" => "external",
                "Pods" => "pods",
                "Object" => "object",
                "Resource" => "resource",
                "ContainerResource" => "containerResource",
                _ => {
                    writeln!(out, "  <unknown metric type {kind:?}>").unwrap();
                    continue;
                }
            };
            let source = &metric[field];
            let target = &source["target"];
            let current_metric = items(&status["currentMetrics"])
                .get(i)
                .unwrap_or(&Value::Null);
            let available = !current_metric[field].is_null();
            let current = &current_metric[field]["current"];
            let current_quantity = |field: &str| {
                if available {
                    quantity(&current[field])
                } else {
                    "<unknown>".into()
                }
            };
            match kind {
                "External" => {
                    let (label, field) = if !target["averageValue"].is_null() {
                        ("target average value", "averageValue")
                    } else {
                        ("target value", "value")
                    };
                    let current = if field == "averageValue" && current[field].is_null() {
                        "<unknown>".into()
                    } else {
                        current_quantity(field)
                    };
                    writeln!(
                        out,
                        "  {:?} ({label}):\t{current} / {}",
                        text(&source["metric"]["name"]),
                        quantity(&target[field])
                    )
                    .unwrap();
                }
                "Pods" => {
                    writeln!(
                        out,
                        "  {:?} on pods:\t{} / {}",
                        text(&source["metric"]["name"]),
                        current_quantity("averageValue"),
                        quantity(&target["averageValue"])
                    )
                    .unwrap();
                }
                "Object" => {
                    let (label, field) = if text(&target["type"]) == "AverageValue" {
                        ("target average value", "averageValue")
                    } else {
                        ("target value", "value")
                    };
                    writeln!(
                        out,
                        "  \"{}\" on {}/{} ({label}):\t{} / {}",
                        text(&source["metric"]["name"]),
                        text(&source["describedObject"]["kind"]),
                        text(&source["describedObject"]["name"]),
                        current_quantity(field),
                        quantity(&target[field])
                    )
                    .unwrap();
                }
                _ => {
                    write!(out, "  resource {}", text(&source["name"])).unwrap();
                    if kind == "ContainerResource" {
                        write!(out, " of container \"{}\"", text(&source["container"])).unwrap();
                    }
                    out.push_str(" on pods");
                    if !target["averageValue"].is_null() {
                        writeln!(
                            out,
                            ":\t{} / {}",
                            current_quantity("averageValue"),
                            quantity(&target["averageValue"])
                        )
                        .unwrap();
                    } else {
                        let current = if available && !current["averageUtilization"].is_null() {
                            format!(
                                "{}% ({})",
                                integer(&current["averageUtilization"]),
                                quantity(&current["averageValue"])
                            )
                        } else {
                            "<unknown>".into()
                        };
                        let target = target["averageUtilization"]
                            .as_i64()
                            .map(|n| format!("{n}%"))
                            .unwrap_or("<auto>".into());
                        writeln!(out, "  (as a percentage of request):\t{current} / {target}")
                            .unwrap();
                    }
                }
            }
        }
    }
    writeln!(
        out,
        "Min replicas:\t{}\nMax replicas:\t{}",
        spec["minReplicas"]
            .as_i64()
            .map(|n| n.to_string())
            .unwrap_or("<unset>".into()),
        integer(&spec["maxReplicas"])
    )
    .unwrap();
    if !legacy && !spec["behavior"].is_null() {
        out.push_str("Behavior:\n");
        for (field, label) in [("scaleUp", "Scale Up"), ("scaleDown", "Scale Down")] {
            let rules = &spec["behavior"][field];
            if rules.is_null() {
                continue;
            }
            writeln!(out, "  {label}:").unwrap();
            if !rules["stabilizationWindowSeconds"].is_null() {
                writeln!(
                    out,
                    "    Stabilization Window: {} seconds",
                    integer(&rules["stabilizationWindowSeconds"])
                )
                .unwrap();
            }
            let policies = items(&rules["policies"]);
            if !policies.is_empty() {
                writeln!(
                    out,
                    "    Select Policy: {}\n    Policies:",
                    rules["selectPolicy"].as_str().unwrap_or("Max")
                )
                .unwrap();
                for p in policies {
                    writeln!(
                        out,
                        "      - Type: {}\tValue: {}\tPeriod: {} seconds",
                        text(&p["type"]),
                        integer(&p["value"]),
                        integer(&p["periodSeconds"])
                    )
                    .unwrap();
                }
            }
        }
    }
    writeln!(
        out,
        "{} pods:\t{} current / {} desired",
        text(&spec["scaleTargetRef"]["kind"]),
        integer(&status["currentReplicas"]),
        integer(&status["desiredReplicas"])
    )
    .unwrap();
    let conditions = items(&status["conditions"]);
    if !legacy && !conditions.is_empty() {
        out.push_str(
            "Conditions:\n  Type\tStatus\tReason\tMessage\n  ----\t------\t------\t-------\n",
        );
        for c in conditions {
            writeln!(
                out,
                "  {}\t{}\t{}\t{}",
                text(&c["type"]),
                text(&c["status"]),
                text(&c["reason"]),
                text(&c["message"])
            )
            .unwrap();
        }
    }
    with_events(out, events, now)
}
