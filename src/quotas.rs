// Copyright 2014 The Kubernetes Authors.
// Copyright 2026 Nikola Milojević.
// SPDX-License-Identifier: Apache-2.0
// Adapted from kubectl v0.35.1 pkg/describe/describe.go. See LICENSE-APACHE.

use crate::api::list_objects;
use crate::format::tabbed;
use crate::json::{items, text};
use crate::metadata::metadata_header;
use crate::time::value_timestamp;
use k8s_openapi::jiff::tz::TimeZone;
use kube::Client;
use kube::api::{ApiResource, DynamicObject, ListParams};
use serde_json::Value;
use std::fmt::Write;

fn quantity(v: &Value, default: &str) -> String {
    v.as_str()
        .map(crate::quantity::canonical)
        .unwrap_or(default.into())
}

pub(crate) async fn related(
    client: Client,
    object: &DynamicObject,
) -> Result<(Option<Vec<DynamicObject>>, Option<Vec<DynamicObject>>), String> {
    let ns = object.metadata.name.as_deref().unwrap_or_default();
    let quotas = ApiResource::from_gvk(&kube::core::GroupVersionKind::gvk(
        "",
        "v1",
        "ResourceQuota",
    ));
    let limits = ApiResource::from_gvk(&kube::core::GroupVersionKind::gvk("", "v1", "LimitRange"));
    let (quotas, limits) = tokio::join!(
        list_objects(client.clone(), &quotas, ns, ListParams::default()),
        list_objects(client, &limits, ns, ListParams::default())
    );
    let accept = |result: Result<Vec<DynamicObject>, kube::Error>| match result {
        Ok(objects) => Ok(Some(objects)),
        Err(kube::Error::Api(error)) if error.code == 404 => Ok(None),
        Err(error) => Err(error.to_string()),
    };
    Ok((accept(quotas)?, accept(limits)?))
}

pub(crate) fn render(
    object: &DynamicObject,
    kind: &str,
    quotas: Option<&[DynamicObject]>,
    limits: Option<&[DynamicObject]>,
    zone: &TimeZone,
) -> String {
    let mut out;
    if kind == "Namespace" {
        out = metadata_header(&object.metadata, false);
        writeln!(out, "Status:\t{}", text(&object.data["status"]["phase"])).unwrap();
        let conditions = items(&object.data["status"]["conditions"]);
        if !conditions.is_empty() {
            out.push_str("Conditions:\n  Type\tStatus\tLastTransitionTime\tReason\tMessage\n  ----\t------\t------------------\t------\t-------\n");
            for c in conditions {
                writeln!(
                    out,
                    "  {}\t{}\t{}\t{}\t{}",
                    text(&c["type"]),
                    text(&c["status"]),
                    value_timestamp(&c["lastTransitionTime"], zone),
                    text(&c["reason"]),
                    text(&c["message"])
                )
                .unwrap();
            }
        }
        if let Some(quotas) = quotas {
            out.push('\n');
            if quotas.is_empty() {
                out.push_str("No resource quota.\n");
            } else {
                out.push_str("Resource Quotas\n");
                let mut quotas: Vec<_> = quotas.iter().collect();
                quotas.sort_by(|a, b| a.metadata.name.cmp(&b.metadata.name));
                for q in quotas {
                    writeln!(
                        out,
                        "  Name:\t{}",
                        q.metadata.name.as_deref().unwrap_or_default()
                    )
                    .unwrap();
                    quota(&mut out, &q.data, true);
                }
            }
        }
        if let Some(limits) = limits {
            out.push('\n');
            if limits.is_empty() {
                out.push_str("No LimitRange resource.\n");
            } else {
                out.push_str("Resource Limits\n Type\tResource\tMin\tMax\tDefault Request\tDefault Limit\tMax Limit/Request Ratio\n ----\t--------\t---\t---\t---------------\t-------------\t-----------------------\n");
                for limit in limits {
                    limit_rows(&mut out, &limit.data["spec"], " ");
                }
            }
        }
    } else {
        out = format!(
            "Name:\t{}\nNamespace:\t{}\n",
            object.metadata.name.as_deref().unwrap_or_default(),
            object.metadata.namespace.as_deref().unwrap_or_default()
        );
        if kind == "ResourceQuota" {
            quota(&mut out, &object.data, false);
        } else {
            out.push_str("Type\tResource\tMin\tMax\tDefault Request\tDefault Limit\tMax Limit/Request Ratio\n----\t--------\t---\t---\t---------------\t-------------\t-----------------------\n");
            limit_rows(&mut out, &object.data["spec"], "");
        }
    }
    tabbed(&out)
}

fn quota(out: &mut String, value: &Value, nested: bool) {
    let indent = if nested { "  " } else { "" };
    let mut scopes: Vec<_> = items(&value["spec"]["scopes"]).iter().map(text).collect();
    scopes.sort();
    if !scopes.is_empty() {
        writeln!(out, "{indent}Scopes:\t{}", scopes.join(", ")).unwrap();
    }
    for scope in scopes {
        let help = match scope {
            "Terminating" => {
                "Matches all pods that have an active deadline. These pods have a limited lifespan on a node before being actively terminated by the system."
            }
            "NotTerminating" => {
                "Matches all pods that do not have an active deadline. These pods usually include long running pods whose container command is not expected to terminate."
            }
            "BestEffort" => {
                "Matches all pods that do not have resource requirements set. These pods have a best effort quality of service."
            }
            "NotBestEffort" => {
                "Matches all pods that have at least one resource requirement set. These pods have a burstable or guaranteed quality of service."
            }
            _ => continue,
        };
        writeln!(out, "{}* {help}", if nested { "  " } else { " " }).unwrap();
    }
    writeln!(
        out,
        "{indent}Resource\tUsed\tHard\n{indent}--------\t{}\t{}",
        if nested { "---" } else { "----" },
        if nested { "---" } else { "----" }
    )
    .unwrap();
    let mut hard: Vec<_> = value["status"]["hard"]
        .as_object()
        .into_iter()
        .flatten()
        .collect();
    hard.sort_by_key(|(k, _)| *k);
    for (name, capacity) in hard {
        let used = &value["status"]["used"][name];
        let mut used_text = quantity(used, "0");
        if !nested && text(capacity).ends_with('i') && !text(used).ends_with('i') {
            let nanos = crate::quantity::parse(text(used)).unwrap_or_default();
            let mut units = (nanos + 999_999_999) / 1_000_000_000;
            let suffixes = ["", "Ki", "Mi", "Gi", "Ti", "Pi", "Ei"];
            let mut index = 0;
            while units != 0 && units % 1024 == 0 && index + 1 < suffixes.len() {
                units /= 1024;
                index += 1;
            }
            used_text = format!("{units}{}", suffixes[index]);
        } else if !nested && !text(capacity).ends_with('i') && text(used).ends_with('i') {
            let units = (crate::quantity::parse(text(used)).unwrap_or_default() + 999_999_999)
                / 1_000_000_000;
            used_text = crate::quantity::canonical(&units.to_string());
        }
        writeln!(
            out,
            "{indent}{name}\t{used_text}\t{}",
            quantity(capacity, "0")
        )
        .unwrap();
    }
}

fn limit_rows(out: &mut String, spec: &Value, prefix: &str) {
    for limit in items(&spec["limits"]) {
        let mut names = std::collections::BTreeSet::new();
        for field in [
            "max",
            "min",
            "default",
            "defaultRequest",
            "maxLimitRequestRatio",
        ] {
            names.extend(
                limit[field]
                    .as_object()
                    .into_iter()
                    .flatten()
                    .map(|(k, _)| k),
            );
        }
        for name in names {
            write!(out, "{prefix}{}\t{name}", text(&limit["type"])).unwrap();
            for field in [
                "min",
                "max",
                "defaultRequest",
                "default",
                "maxLimitRequestRatio",
            ] {
                write!(out, "\t{}", quantity(&limit[field][name], "-")).unwrap();
            }
            out.push('\n');
        }
    }
}
