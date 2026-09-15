// Copyright 2014 The Kubernetes Authors.
// Copyright 2026 sofka contributors.
// SPDX-License-Identifier: Apache-2.0
// Adapted from kubectl v0.35.1 pkg/describe/describe.go. See LICENSE-APACHE.

use super::*;
use futures_util::{StreamExt, stream};
use serde_json::Value;
fn text(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}
fn items(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or_default()
}
pub(super) type Backends = BTreeMap<String, Result<(DynamicObject, Vec<DynamicObject>), String>>;
pub(super) fn supports(ar: &ApiResource) -> bool {
    matches!(ar.group.as_str(), "networking.k8s.io" | "extensions") && ar.kind == "Ingress"
}

pub(super) async fn related(client: Client, object: &DynamicObject) -> Backends {
    let mut names = std::collections::BTreeSet::new();
    let spec = &object.data["spec"];
    if let Some(name) = spec["defaultBackend"]["service"]["name"].as_str() {
        names.insert(name.to_owned());
    }
    for rule in items(&spec["rules"]) {
        for path in items(&rule["http"]["paths"]) {
            if let Some(name) = path["backend"]["service"]["name"].as_str() {
                names.insert(name.to_owned());
            }
        }
    }
    let ns = object.metadata.namespace.as_deref().unwrap_or_default();
    stream::iter(names)
        .map(|name| {
            let client = client.clone();
            async move {
                let ar =
                    ApiResource::from_gvk(&kube::core::GroupVersionKind::gvk("", "v1", "Service"));
                let reference = DynamicObject::new(&name, &ar).within(ns);
                let api = Api::<DynamicObject>::namespaced_with(client.clone(), ns, &ar);
                let (slices, service) =
                    tokio::join!(service::related(client, &reference), api.get(&name));
                let result = slices
                    .map_err(error)
                    .and_then(|slices| service.map(|service| (service, slices)).map_err(error));
                (name, result)
            }
        })
        .buffer_unordered(8)
        .collect()
        .await
}
fn error(e: kube::Error) -> String {
    match e {
        kube::Error::Api(e) => e.message,
        _ => e.to_string(),
    }
}

pub(super) fn render(
    object: &DynamicObject,
    backends: &Backends,
    events: Option<&[Event]>,
    now: Timestamp,
) -> String {
    let spec = &object.data["spec"];
    let metadata = metadata_header(&object.metadata, false);
    let (header, annotations) = metadata.split_once("Annotations:\t").unwrap();
    let mut out = header.to_owned();
    let addresses: std::collections::BTreeSet<_> =
        items(&object.data["status"]["loadBalancer"]["ingress"])
            .iter()
            .filter_map(|i| {
                let ip = text(&i["ip"]);
                let host = text(&i["hostname"]);
                if !ip.is_empty() {
                    Some(ip)
                } else if !host.is_empty() {
                    Some(host)
                } else {
                    None
                }
            })
            .collect();
    writeln!(
        out,
        "Namespace:\t{}\nAddress:\t{}\nIngress Class:\t{}",
        object.metadata.namespace.as_deref().unwrap_or_default(),
        addresses.into_iter().collect::<Vec<_>>().join(","),
        spec["ingressClassName"].as_str().unwrap_or("<none>")
    )
    .unwrap();
    let default = if spec["defaultBackend"].is_null() {
        "<default>".into()
    } else {
        backend(&spec["defaultBackend"], backends)
    };
    writeln!(out, "Default backend:\t{default}").unwrap();
    let tls = items(&spec["tls"]);
    if !tls.is_empty() {
        out.push_str("TLS:\n");
        for t in tls {
            writeln!(
                out,
                "  {} {}",
                if text(&t["secretName"]).is_empty() {
                    "SNI routes".into()
                } else {
                    format!("{} terminates", text(&t["secretName"]))
                },
                items(&t["hosts"])
                    .iter()
                    .map(text)
                    .collect::<Vec<_>>()
                    .join(",")
            )
            .unwrap();
        }
    }
    out.push_str("Rules:\n  Host\tPath\tBackends\n  ----\t----\t--------\n");
    let mut count = 0;
    for rule in items(&spec["rules"]) {
        if rule["http"].is_null() {
            continue;
        }
        count += 1;
        writeln!(
            out,
            "  {}\t",
            if text(&rule["host"]).is_empty() {
                "*"
            } else {
                text(&rule["host"])
            }
        )
        .unwrap();
        for path in items(&rule["http"]["paths"]) {
            writeln!(
                out,
                "    \t{} \t{}",
                text(&path["path"]),
                backend(&path["backend"], backends)
            )
            .unwrap();
        }
    }
    if count == 0 {
        writeln!(out, "  *\t*\t{default}").unwrap();
    }
    write!(out, "Annotations:\t{annotations}").unwrap();
    with_events(out, events, now)
}

fn backend(value: &Value, backends: &Backends) -> String {
    let service = &value["service"];
    if !service.is_null() {
        let name = text(&service["name"]);
        let port = service["port"]["number"]
            .as_i64()
            .filter(|n| *n != 0)
            .map(|n| n.to_string())
            .unwrap_or_else(|| text(&service["port"]["name"]).into());
        let display = format!("{name}:{port}");
        match backends.get(name) {
            Some(Ok((object, slices))) => {
                let mut port_name = "";
                for p in items(&object.data["spec"]["ports"]) {
                    if service["port"]["number"]
                        .as_i64()
                        .is_some_and(|n| n != 0 && Some(n) == p["port"].as_i64())
                        || (!text(&service["port"]["name"]).is_empty()
                            && text(&service["port"]["name"]) == text(&p["name"]))
                    {
                        port_name = text(&p["name"]);
                    }
                }
                format!(
                    "{display} ({})",
                    service::endpoints(slices, Some(port_name))
                )
            }
            Some(Err(error)) => format!("{display} (<error: {error}>)"),
            None => format!("{display} (<error: backend was not fetched>)"),
        }
    } else if !value["resource"].is_null() {
        let r = &value["resource"];
        format!(
            "APIGroup: {}, Kind: {}, Name: {}",
            r["apiGroup"].as_str().unwrap_or("<none>"),
            text(&r["kind"]),
            text(&r["name"])
        )
    } else {
        String::new()
    }
}
