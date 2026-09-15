// Copyright 2014 The Kubernetes Authors.
// Copyright 2026 sofka contributors.
// SPDX-License-Identifier: Apache-2.0
// Adapted from kubectl v0.37.0 pkg/describe/describe.go. See LICENSE-APACHE.

use super::*;
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

pub(super) async fn related(
    client: Client,
    object: &DynamicObject,
) -> Result<Vec<DynamicObject>, kube::Error> {
    let ar = ApiResource::from_gvk(&kube::core::GroupVersionKind::gvk(
        "discovery.k8s.io",
        "v1",
        "EndpointSlice",
    ));
    let selector = format!(
        "kubernetes.io/service-name={}",
        object.metadata.name.as_deref().unwrap_or_default()
    );
    list_objects(
        client,
        &ar,
        object.metadata.namespace.as_deref().unwrap_or_default(),
        ListParams::default().labels(&selector),
    )
    .await
}

pub(super) fn render(
    object: &DynamicObject,
    slices: &[DynamicObject],
    events: Option<&[Event]>,
    now: Timestamp,
) -> String {
    let spec = &object.data["spec"];
    let mut out = metadata(&object.metadata);
    let mut selector: Vec<_> = spec["selector"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(k, v)| format!("{k}={}", text(v)))
        .collect();
    selector.sort();
    writeln!(
        out,
        "Selector:\t{}\nType:\t{}",
        if selector.is_empty() {
            "<none>".into()
        } else {
            selector.join(",")
        },
        text(&spec["type"])
    )
    .unwrap();
    if !spec["ipFamilyPolicy"].is_null() {
        writeln!(out, "IP Family Policy:\t{}", text(&spec["ipFamilyPolicy"])).unwrap();
    }
    let families = items(&spec["ipFamilies"])
        .iter()
        .map(text)
        .collect::<Vec<_>>()
        .join(",");
    let ips = items(&spec["clusterIPs"])
        .iter()
        .map(text)
        .collect::<Vec<_>>()
        .join(",");
    writeln!(
        out,
        "IP Families:\t{}\nIP:\t{}\nIPs:\t{}",
        if families.is_empty() {
            "<none>"
        } else {
            &families
        },
        text(&spec["clusterIP"]),
        if ips.is_empty() { "<none>" } else { &ips }
    )
    .unwrap();
    if !items(&spec["externalIPs"]).is_empty() {
        writeln!(
            out,
            "External IPs:\t{}",
            items(&spec["externalIPs"])
                .iter()
                .map(text)
                .collect::<Vec<_>>()
                .join(",")
        )
        .unwrap();
    }
    for (field, label) in [
        ("loadBalancerIP", "Desired LoadBalancer IP"),
        ("externalName", "External Name"),
    ] {
        if !text(&spec[field]).is_empty() {
            writeln!(out, "{label}:\t{}", text(&spec[field])).unwrap();
        }
    }
    let ingress = items(&object.data["status"]["loadBalancer"]["ingress"]);
    if !ingress.is_empty() {
        let addresses = ingress
            .iter()
            .map(|i| {
                if text(&i["ip"]).is_empty() {
                    text(&i["hostname"]).into()
                } else if i["ipMode"].is_null() {
                    text(&i["ip"]).into()
                } else {
                    format!("{} ({})", text(&i["ip"]), text(&i["ipMode"]))
                }
            })
            .collect::<Vec<String>>()
            .join(", ");
        writeln!(out, "LoadBalancer Ingress:\t{addresses}").unwrap();
    }
    for port in items(&spec["ports"]) {
        let name = if text(&port["name"]).is_empty() {
            "<unset>"
        } else {
            text(&port["name"])
        };
        let protocol = text(&port["protocol"]);
        writeln!(
            out,
            "Port:\t{name}\t{}/{protocol}\nTargetPort:\t{}/{protocol}",
            integer(&port["port"]),
            port["targetPort"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| integer(&port["targetPort"]).to_string())
        )
        .unwrap();
        if let Some(protocol) = port["appProtocol"].as_str() {
            writeln!(out, "AppProtocol:\t{protocol}").unwrap();
        }
        if integer(&port["nodePort"]) != 0 {
            writeln!(
                out,
                "NodePort:\t{name}\t{}/{protocol}",
                integer(&port["nodePort"])
            )
            .unwrap();
        }
        writeln!(
            out,
            "Endpoints:\t{}",
            endpoints(slices, Some(text(&port["name"])))
        )
        .unwrap();
    }
    writeln!(out, "Session Affinity:\t{}", text(&spec["sessionAffinity"])).unwrap();
    if !text(&spec["externalTrafficPolicy"]).is_empty() {
        writeln!(
            out,
            "External Traffic Policy:\t{}",
            text(&spec["externalTrafficPolicy"])
        )
        .unwrap();
    }
    if !spec["internalTrafficPolicy"].is_null() {
        writeln!(
            out,
            "Internal Traffic Policy:\t{}",
            text(&spec["internalTrafficPolicy"])
        )
        .unwrap();
    }
    if integer(&spec["healthCheckNodePort"]) != 0 {
        writeln!(
            out,
            "HealthCheck NodePort:\t{}",
            integer(&spec["healthCheckNodePort"])
        )
        .unwrap();
    }
    if !items(&spec["loadBalancerSourceRanges"]).is_empty() {
        writeln!(
            out,
            "LoadBalancer Source Ranges:\t{}",
            items(&spec["loadBalancerSourceRanges"])
                .iter()
                .map(text)
                .collect::<Vec<_>>()
                .join(",")
        )
        .unwrap();
    }
    if !spec["trafficDistribution"].is_null() {
        writeln!(
            out,
            "Traffic Distribution:\t{}",
            text(&spec["trafficDistribution"])
        )
        .unwrap();
    }
    with_events(out, events, now)
}

pub(super) fn endpoints(slices: &[DynamicObject], port_name: Option<&str>) -> String {
    if slices.is_empty() {
        return "<none>".into();
    }
    let mut addresses = Vec::new();
    let mut count = 0;
    let mut more = false;
    for slice in slices {
        let ports = items(&slice.data["ports"]);
        let ports: Vec<Option<&Value>> = if ports.is_empty() {
            vec![None]
        } else {
            ports
                .iter()
                .filter(|p| port_name.is_none_or(|name| text(&p["name"]) == name))
                .map(Some)
                .collect()
        };
        for port in ports {
            for endpoint in items(&slice.data["endpoints"]) {
                if addresses.len() == 3 {
                    more = true;
                }
                if endpoint["conditions"]["ready"].as_bool() == Some(false) {
                    continue;
                }
                let Some(address) = items(&endpoint["addresses"])
                    .first()
                    .and_then(Value::as_str)
                else {
                    continue;
                };
                if !more {
                    addresses.push(if let Some(port) = port {
                        format!(
                            "{}:{}",
                            if address.contains(':') {
                                format!("[{address}]")
                            } else {
                                address.into()
                            },
                            integer(&port["port"])
                        )
                    } else {
                        address.into()
                    });
                }
                count += 1;
            }
        }
    }
    let rendered = addresses.join(",");
    if more {
        format!("{rendered} + {} more...", count - 3)
    } else {
        rendered
    }
}
