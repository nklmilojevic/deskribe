// Copyright 2014 The Kubernetes Authors.
// Copyright 2026 Nikola Milojević.
// SPDX-License-Identifier: Apache-2.0
// Adapted from kubectl v0.35.1 pkg/describe/describe.go. See LICENSE-APACHE.

use crate::events::with_events;
use crate::json::text;
use crate::metadata::metadata;
use k8s_openapi::api::core::v1::Event;
use k8s_openapi::jiff::Timestamp;
use kube::api::DynamicObject;
use std::fmt::Write;

pub(crate) fn render(object: &DynamicObject, events: Option<&[Event]>, now: Timestamp) -> String {
    let value = &object.data;
    let mut out = metadata(&object.metadata);

    let names: Vec<_> = value["imagePullSecrets"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| text(&s["name"]))
        .collect();
    if names.is_empty() {
        out.push_str("Image pull secrets:\t<none>\n");
    } else {
        for (i, name) in names.iter().enumerate() {
            writeln!(
                out,
                "{}\t{name}",
                if i == 0 {
                    "Image pull secrets:"
                } else {
                    "                   "
                }
            )
            .unwrap();
        }
    }

    with_events(out, events, now)
}
