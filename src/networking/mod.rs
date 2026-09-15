// Copyright 2014 The Kubernetes Authors.
// Copyright 2026 Nikola Milojević.
// SPDX-License-Identifier: Apache-2.0
// Adapted from kubectl v0.35.1 pkg/describe/describe.go. See LICENSE-APACHE.

use crate::events::with_events;
use crate::json::text;
use crate::metadata::metadata_header;
use crate::time::timestamp;
use k8s_openapi::api::core::v1::Event;
use k8s_openapi::jiff::Timestamp;
use k8s_openapi::jiff::tz::TimeZone;
use kube::api::DynamicObject;
use std::fmt::Write;

pub(crate) mod endpoints;
pub(crate) mod ingress;
pub(crate) mod service;

pub(crate) fn render_ingress_class(
    object: &DynamicObject,
    events: Option<&[Event]>,
    now: Timestamp,
) -> String {
    let value = &object.data;
    let mut out = metadata_header(&object.metadata, false);

    writeln!(out, "Controller:\t{}", text(&value["spec"]["controller"])).unwrap();
    let p = &value["spec"]["parameters"];
    if !p.is_null() {
        out.push_str("Parameters:\n");
        if !p["apiGroup"].is_null() {
            writeln!(out, "  APIGroup:\t{}", text(&p["apiGroup"])).unwrap();
        }
        writeln!(
            out,
            "  Kind:\t{}\n  Name:\t{}",
            text(&p["kind"]),
            text(&p["name"])
        )
        .unwrap();
    }

    with_events(out, events, now)
}

pub(crate) fn render_ip_address(
    object: &DynamicObject,
    events: Option<&[Event]>,
    now: Timestamp,
) -> String {
    let value = &object.data;
    let mut out = metadata_header(&object.metadata, false);

    let p = &value["spec"]["parentRef"];
    if !p.is_null() {
        out.push_str("Parent Reference:\n");
        for (field, label) in [
            ("group", "Group"),
            ("resource", "Resource"),
            ("namespace", "Namespace"),
            ("name", "Name"),
        ] {
            writeln!(out, "  {label}:\t{}", text(&p[field])).unwrap();
        }
    }

    with_events(out, events, now)
}

pub(crate) fn render_service_cidr(
    object: &DynamicObject,
    events: Option<&[Event]>,
    now: Timestamp,
    zone: &TimeZone,
) -> String {
    let value = &object.data;
    let mut out = metadata_header(&object.metadata, false);

    let cidrs: Vec<_> = value["spec"]["cidrs"]
        .as_array()
        .into_iter()
        .flatten()
        .map(text)
        .collect();
    writeln!(out, "CIDRs:\t{}", cidrs.join(", ")).unwrap();
    let conditions = value["status"]["conditions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if !conditions.is_empty() {
        out.push_str("Status:\nConditions:\n  Type\tStatus\tLastTransitionTime\tReason\tMessage\n  ----\t------\t------------------\t------\t-------\n");
        for c in conditions {
            writeln!(
                out,
                "  {}\t{}\t{}\t{}\t{}",
                text(&c["type"]),
                text(&c["status"]),
                timestamp(text(&c["lastTransitionTime"]), zone),
                text(&c["reason"]),
                text(&c["message"])
            )
            .unwrap();
        }
    }

    with_events(out, events, now)
}
