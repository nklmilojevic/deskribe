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

use crate::json::text;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::api::DynamicObject;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt::Write;

pub(crate) fn metadata(meta: &ObjectMeta) -> String {
    metadata_header(meta, true)
}

pub(crate) fn metadata_header(meta: &ObjectMeta, namespaced: bool) -> String {
    let mut out = identity(meta, namespaced);
    out.push_str(&label_section(meta, ""));
    out.push_str(&annotation_section(meta, ""));
    out
}

pub(crate) fn identity(meta: &ObjectMeta, namespaced: bool) -> String {
    let mut out = format!("Name:\t{}\n", meta.name.as_deref().unwrap_or_default());
    if namespaced {
        writeln!(
            out,
            "Namespace:\t{}",
            meta.namespace.as_deref().unwrap_or_default()
        )
        .unwrap();
    }
    out
}

pub(crate) fn label_section(meta: &ObjectMeta, prefix: &str) -> String {
    let empty = BTreeMap::new();
    let mut out = format!("{prefix}Labels:\t");
    let labels = meta.labels.as_ref().unwrap_or(&empty);
    if labels.is_empty() {
        out.push_str("<none>\n");
    }
    for (i, (key, value)) in labels.iter().enumerate() {
        if i > 0 {
            out.push('\t');
        }
        writeln!(out, "{key}={value}").unwrap();
    }
    out
}

pub(crate) fn annotation_section(meta: &ObjectMeta, prefix: &str) -> String {
    let empty = BTreeMap::new();
    let mut out = format!("{prefix}Annotations:\t");
    let annotations: Vec<_> = meta
        .annotations
        .as_ref()
        .unwrap_or(&empty)
        .iter()
        .filter(|(key, _)| key.as_str() != "kubectl.kubernetes.io/last-applied-configuration")
        .collect();
    if annotations.is_empty() {
        out.push_str("<none>\n");
    }
    for (i, (key, value)) in annotations.into_iter().enumerate() {
        if i > 0 {
            out.push('\t');
        }
        let value = value.strip_suffix('\n').unwrap_or(value);
        if value.len() + key.len() + 2 > 140 || value.contains('\n') {
            writeln!(out, "{key}:").unwrap();
            for line in value.split('\n') {
                // Go truncates bytes, including in the middle of UTF-8. The UI
                // already decodes kubectl stdout lossily, so do the same here.
                let short = String::from_utf8_lossy(&line.as_bytes()[..line.len().min(138)]);
                writeln!(
                    out,
                    "\t  {short}{}",
                    if line.len() > 138 { "..." } else { "" }
                )
                .unwrap();
            }
        } else {
            writeln!(out, "{key}: {value}").unwrap();
        }
    }
    out
}

pub(crate) fn inline_annotations(object: &DynamicObject) -> String {
    let pairs: Vec<_> = object
        .metadata
        .annotations
        .iter()
        .flatten()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    if pairs.is_empty() {
        "<none>".into()
    } else {
        pairs.join(",")
    }
}

pub(crate) fn inline_labels(value: &Value) -> String {
    let mut pairs: Vec<_> = value
        .as_object()
        .into_iter()
        .flatten()
        .map(|(k, v)| format!("{k}={}", text(v)))
        .collect();
    pairs.sort();
    if pairs.is_empty() {
        "<none>".into()
    } else {
        pairs.join(",")
    }
}
