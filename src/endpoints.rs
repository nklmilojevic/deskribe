// Copyright 2014 The Kubernetes Authors.
// Copyright 2026 Nikola Milojević.
// SPDX-License-Identifier: Apache-2.0
// Adapted from kubectl v0.35.1 pkg/describe/describe.go. See LICENSE-APACHE.

use super::*;
use serde_json::Value;

pub(super) fn supports(ar: &ApiResource) -> bool {
    matches!(
        (ar.group.as_str(), ar.kind.as_str()),
        ("", "Endpoints") | ("discovery.k8s.io", "EndpointSlice")
    )
}

fn text(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}
fn items(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or_default()
}
fn nonempty(s: &str, default: &str) -> String {
    if s.is_empty() {
        default.into()
    } else {
        s.into()
    }
}

pub(super) fn render(
    object: &DynamicObject,
    kind: &str,
    events: Option<&[Event]>,
    now: Timestamp,
) -> String {
    let v = &object.data;
    let mut out = metadata(&object.metadata);
    if kind == "Endpoints" {
        out.push_str("Subsets:\n");
        for subset in items(&v["subsets"]) {
            for (field, label) in [
                ("addresses", "Addresses"),
                ("notReadyAddresses", "NotReadyAddresses"),
            ] {
                let addresses = items(&subset[field])
                    .iter()
                    .map(|a| text(&a["ip"]))
                    .collect::<Vec<_>>()
                    .join(",");
                writeln!(out, "  {label}:\t{}", nonempty(&addresses, "<none>")).unwrap();
            }
            let ports = items(&subset["ports"]);
            if !ports.is_empty() {
                out.push_str("  Ports:\n    Name\tPort\tProtocol\n    ----\t----\t--------\n");
                for port in ports {
                    writeln!(
                        out,
                        "    {}\t{}\t{}",
                        nonempty(text(&port["name"]), "<unset>"),
                        port["port"].as_i64().unwrap_or_default(),
                        text(&port["protocol"])
                    )
                    .unwrap();
                }
            }
            out.push('\n');
        }
    } else {
        writeln!(out, "AddressType:\t{}", text(&v["addressType"])).unwrap();
        let ports = items(&v["ports"]);
        if ports.is_empty() {
            out.push_str("Ports: <unset>\n");
        } else {
            out.push_str("Ports:\n  Name\tPort\tProtocol\n  ----\t----\t--------\n");
            for port in ports {
                writeln!(
                    out,
                    "  {}\t{}\t{}",
                    nonempty(text(&port["name"]), "<unset>"),
                    port["port"]
                        .as_i64()
                        .map(|n| n.to_string())
                        .unwrap_or("<unset>".into()),
                    text(&port["protocol"])
                )
                .unwrap();
            }
        }
        let endpoints = items(&v["endpoints"]);
        if endpoints.is_empty() {
            out.push_str("Endpoints: <none>\n");
        } else {
            out.push_str("Endpoints:\n");
            for endpoint in endpoints {
                let addresses = items(&endpoint["addresses"])
                    .iter()
                    .map(text)
                    .collect::<Vec<_>>()
                    .join(", ");
                writeln!(
                    out,
                    "  - Addresses:\t{}\n    Conditions:\n      Ready:\t{}\n    Hostname:\t{}",
                    nonempty(&addresses, "<none>"),
                    endpoint["conditions"]["ready"]
                        .as_bool()
                        .map(|b| b.to_string())
                        .unwrap_or("<unset>".into()),
                    endpoint["hostname"].as_str().unwrap_or("<unset>")
                )
                .unwrap();
                if !endpoint["targetRef"].is_null() {
                    writeln!(
                        out,
                        "    TargetRef:\t{}/{}",
                        text(&endpoint["targetRef"]["kind"]),
                        text(&endpoint["targetRef"]["name"])
                    )
                    .unwrap();
                }
                writeln!(
                    out,
                    "    NodeName:\t{}\n    Zone:\t{}",
                    endpoint["nodeName"].as_str().unwrap_or("<unset>"),
                    endpoint["zone"].as_str().unwrap_or("<unset>")
                )
                .unwrap();
            }
        }
    }
    with_events(out, events, now)
}
