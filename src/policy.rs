// Copyright 2014 The Kubernetes Authors.
// Copyright 2026 Nikola Milojević.
// SPDX-License-Identifier: Apache-2.0
// Adapted from kubectl v0.35.1 pkg/describe/describe.go. See LICENSE-APACHE.

use crate::events::with_events;
use crate::json::{items, text};
use crate::metadata::metadata;
use k8s_openapi::api::core::v1::Event;
use k8s_openapi::jiff::Timestamp;
use kube::api::DynamicObject;
use serde_json::Value;
use std::fmt::Write;

fn number_or_string(v: &Value) -> String {
    if let Some(s) = v.as_str() {
        s.into()
    } else if v.is_null() {
        "<nil>".into()
    } else {
        v.to_string()
    }
}

pub(crate) fn selector(value: &Value) -> String {
    if value.is_null() {
        return "<none>".into();
    }
    let mut requirements: Vec<(String, String)> = value["matchLabels"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(k, v)| (k.clone(), format!("{k}={}", text(v))))
        .collect();
    for expr in items(&value["matchExpressions"]) {
        let key = text(&expr["key"]);
        let mut values: Vec<_> = items(&expr["values"]).iter().map(text).collect();
        values.sort();
        values.dedup();
        let rendered = match text(&expr["operator"]) {
            "In" if !values.is_empty() => format!("{key} in ({})", values.join(",")),
            "NotIn" if !values.is_empty() => format!("{key} notin ({})", values.join(",")),
            "Exists" if values.is_empty() => key.into(),
            "DoesNotExist" if values.is_empty() => format!("!{key}"),
            _ => return "<error>".into(),
        };
        requirements.push((key.into(), rendered));
    }
    requirements.sort_by(|a, b| a.0.cmp(&b.0));
    if requirements.is_empty() {
        "<none>".into()
    } else {
        requirements
            .into_iter()
            .map(|(_, v)| v)
            .collect::<Vec<_>>()
            .join(",")
    }
}

pub(crate) fn render(
    object: &DynamicObject,
    kind: &str,
    events: Option<&[Event]>,
    now: Timestamp,
    zone: &k8s_openapi::jiff::tz::TimeZone,
) -> String {
    let v = &object.data;
    let spec = &v["spec"];
    let mut out;
    if kind == "PodDisruptionBudget" {
        out = format!(
            "Name:\t{}\nNamespace:\t{}\n",
            object.metadata.name.as_deref().unwrap_or_default(),
            object.metadata.namespace.as_deref().unwrap_or_default()
        );
        if !spec["minAvailable"].is_null() {
            writeln!(
                out,
                "Min available:\t{}",
                number_or_string(&spec["minAvailable"])
            )
            .unwrap();
        } else if !spec["maxUnavailable"].is_null() {
            writeln!(
                out,
                "Max unavailable:\t{}",
                number_or_string(&spec["maxUnavailable"])
            )
            .unwrap();
        }
        writeln!(
            out,
            "Selector:\t{}\nStatus:",
            if spec["selector"].is_null() {
                "<unset>".into()
            } else {
                selector(&spec["selector"])
            }
        )
        .unwrap();
        for (field, label) in [
            ("disruptionsAllowed", "Allowed disruptions"),
            ("currentHealthy", "Current"),
            ("desiredHealthy", "Desired"),
            ("expectedPods", "Total"),
        ] {
            writeln!(
                out,
                "    {label}:\t{}",
                v["status"][field].as_i64().unwrap_or_default()
            )
            .unwrap();
        }
    } else {
        let created = object
            .metadata
            .creation_timestamp
            .as_ref()
            .map(|t| {
                t.0.to_zoned(zone.clone())
                    .strftime("%Y-%m-%d %H:%M:%S %z %Z")
                    .to_string()
            })
            .unwrap_or_else(|| "0001-01-01 00:00:00 +0000 UTC".into());
        out = metadata(&object.metadata).replacen(
            "Labels:",
            &format!("Created on:\t{created}\nLabels:"),
            1,
        );
        let selected = selector(&spec["podSelector"]);
        out.push_str("Spec:\n  PodSelector:     ");
        if selected == "<none>" {
            out.push_str("<none> (Allowing the specific traffic to all pods in this namespace)\n");
        } else {
            writeln!(out, "{selected}").unwrap();
        }
        let types: Vec<_> = items(&spec["policyTypes"]).iter().map(text).collect();
        for (direction, enabled, peers) in [
            ("ingress", types.contains(&"Ingress"), "from"),
            ("egress", types.contains(&"Egress"), "to"),
        ] {
            if enabled {
                writeln!(out, "  Allowing {direction} traffic:").unwrap();
                rules(&mut out, items(&spec[direction]), direction, peers);
            } else {
                writeln!(out, "  Not affecting {direction} traffic").unwrap();
            }
        }
        writeln!(
            out,
            "  Policy Types: {}",
            if types.is_empty() {
                "<none>".into()
            } else {
                types.join(", ")
            }
        )
        .unwrap();
    }
    with_events(out, events, now)
}

fn rules(out: &mut String, rules: &[Value], direction: &str, peers_key: &str) {
    if rules.is_empty() {
        writeln!(
            out,
            "    <none> (Selected pods are isolated for {direction} connectivity)"
        )
        .unwrap();
    }
    for (index, rule) in rules.iter().enumerate() {
        let ports = items(&rule["ports"]);
        if ports.is_empty() {
            out.push_str("    To Port: <any> (traffic allowed to all ports)\n");
        }
        for port in ports {
            let protocol = port["protocol"].as_str().unwrap_or("TCP");
            if port["endPort"].is_null() {
                writeln!(
                    out,
                    "    To Port: {}/{protocol}",
                    number_or_string(&port["port"])
                )
                .unwrap();
            } else {
                writeln!(
                    out,
                    "    To Port Range: {}-{}/{protocol}",
                    number_or_string(&port["port"]),
                    number_or_string(&port["endPort"])
                )
                .unwrap();
            }
        }
        let peers = items(&rule[peers_key]);
        let (title, noun) = if peers_key == "from" {
            ("From", "source")
        } else {
            ("To", "destination")
        };
        if peers.is_empty() {
            writeln!(out, "    {title}: <any> (traffic not restricted by {noun})").unwrap();
        }
        for peer in peers {
            writeln!(out, "    {title}:").unwrap();
            if !peer["namespaceSelector"].is_null() || !peer["podSelector"].is_null() {
                for (field, label) in [
                    ("namespaceSelector", "NamespaceSelector"),
                    ("podSelector", "PodSelector"),
                ] {
                    if !peer[field].is_null() {
                        writeln!(out, "      {label}: {}", selector(&peer[field])).unwrap();
                    }
                }
            } else if !peer["ipBlock"].is_null() {
                writeln!(
                    out,
                    "      IPBlock:\n        CIDR: {}\n        Except: {}",
                    text(&peer["ipBlock"]["cidr"]),
                    items(&peer["ipBlock"]["except"])
                        .iter()
                        .map(text)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
                .unwrap();
            }
        }
        if index + 1 < rules.len() {
            out.push_str("    ----------\n");
        }
    }
}
