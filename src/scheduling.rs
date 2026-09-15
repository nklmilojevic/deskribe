// Copyright 2014 The Kubernetes Authors.
// Copyright 2026 Nikola Milojević.
// SPDX-License-Identifier: Apache-2.0
// Adapted from kubectl v0.35.1 pkg/describe/describe.go. See LICENSE-APACHE.

use crate::events::with_events;
use crate::json::text;
use crate::metadata::inline_annotations;
use k8s_openapi::api::core::v1::Event;
use k8s_openapi::jiff::Timestamp;
use kube::api::DynamicObject;
use std::fmt::Write;

pub(crate) fn render(object: &DynamicObject, events: Option<&[Event]>, now: Timestamp) -> String {
    let value = &object.data;
    let mut out = format!(
        "Name:\t{}\n",
        object.metadata.name.as_deref().unwrap_or_default()
    );
    writeln!(
        out,
        "Value:\t{}\nGlobalDefault:\t{}\nPreemptionPolicy:\t{}\nDescription:\t{}\nAnnotations:\t{}",
        value["value"].as_i64().unwrap_or_default(),
        value["globalDefault"].as_bool().unwrap_or_default(),
        text(&value["preemptionPolicy"]),
        text(&value["description"]),
        inline_annotations(object)
    )
    .unwrap();

    with_events(out, events, now)
}
