// Copyright 2014 The Kubernetes Authors.
// Copyright 2026 Nikola Milojević.
// SPDX-License-Identifier: Apache-2.0
//
// Adapted from kubernetes/kubectl v0.35.1 pkg/describe/describe.go and
// pkg/util/event/sorted_event_list.go; duration formatting from
// kubernetes/apimachinery v0.35.1 pkg/util/duration/duration.go (Copyright 2018).
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy at http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License. See NOTICE and README.md.

use crate::events::with_events;
use crate::format::tabbed;
use crate::metadata::metadata;
use k8s_openapi::api::core::v1::{ConfigMap, Event, Secret};
use k8s_openapi::jiff::Timestamp;
use std::fmt::Write;

pub fn render_secret(secret: &Secret) -> String {
    let mut out = metadata(&secret.metadata);
    write!(
        out,
        "\nType:\t{}\n\nData\n====\n",
        secret.type_.as_deref().unwrap_or_default()
    )
    .unwrap();
    for (key, value) in secret.data.iter().flatten() {
        // Intentional upstream parity: service-account tokens are displayed.
        // All other Secret data (including bootstrap/TLS/docker credentials)
        // is represented only by decoded byte length, never by its value.
        if key == "token" && secret.type_.as_deref() == Some("kubernetes.io/service-account-token")
        {
            writeln!(out, "{key}:\t{}", String::from_utf8_lossy(&value.0)).unwrap();
        } else {
            writeln!(out, "{key}:\t{} bytes", value.0.len()).unwrap();
        }
    }
    tabbed(&out)
}

pub fn render_config_map(cm: &ConfigMap, events: &[Event], now: Timestamp) -> String {
    let mut out = metadata(&cm.metadata);
    out.push_str("\nData\n====\n");
    for (key, value) in cm.data.iter().flatten() {
        writeln!(out, "{key}:\n----\n{value}\n").unwrap();
    }
    out.push_str("\nBinaryData\n====\n");
    for (key, value) in cm.binary_data.iter().flatten() {
        writeln!(out, "{key}: {} bytes", value.0.len()).unwrap();
    }
    out.push('\n');
    with_events(out, Some(events), now)
}
