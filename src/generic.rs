// Copyright 2014 The Kubernetes Authors.
// Copyright 2026 Nikola Milojević.
// SPDX-License-Identifier: Apache-2.0
// Adapted from kubectl v0.37.0 pkg/describe/describe.go. See LICENSE-APACHE.

use crate::events::with_events;
use crate::metadata::metadata;
use k8s_openapi::api::core::v1::Event;
use k8s_openapi::jiff::Timestamp;
use kube::api::DynamicObject;
use serde_json::Value;
use std::fmt::Write;

pub(crate) fn render(object: &DynamicObject, events: Option<&[Event]>, now: Timestamp) -> String {
    let mut out = metadata(&object.metadata);
    // `DynamicObject::data` already is the JSON body. Serializing the complete
    // object deep-copied every custom-resource field merely to add type and
    // metadata back at the top level. Merge those three small fields into the
    // sorted traversal while borrowing the potentially large body in place.
    let metadata = serde_json::to_value(&object.metadata).expect("ObjectMeta is JSON serializable");
    let mut keys: Vec<_> = object
        .data
        .as_object()
        .into_iter()
        .flatten()
        .map(|(key, _)| key.as_str())
        .filter(|key| !matches!(*key, "apiVersion" | "kind" | "metadata"))
        .collect();
    if object.types.is_some() {
        keys.extend(["apiVersion", "kind"]);
    }
    keys.push("metadata");
    keys.sort_unstable();
    for key in keys {
        match key {
            "apiVersion" => writeln!(
                out,
                "API Version:\t{}",
                object
                    .types
                    .as_ref()
                    .map(|types| types.api_version.as_str())
                    .unwrap_or_default()
            )
            .unwrap(),
            "kind" => writeln!(
                out,
                "Kind:\t{}",
                object
                    .types
                    .as_ref()
                    .map(|types| types.kind.as_str())
                    .unwrap_or_default()
            )
            .unwrap(),
            "metadata" => field(&mut out, key, &metadata, 0, false),
            _ => field(&mut out, key, &object.data[key], 0, false),
        }
    }
    with_events(out, events, now)
}

fn content(out: &mut String, value: &Value, level: usize, omit_metadata_fields: bool) {
    let Some(fields) = value.as_object() else {
        return;
    };
    let mut keys: Vec<_> = fields.keys().collect();
    keys.sort();
    for key in keys {
        field(out, key, &fields[key], level, omit_metadata_fields);
    }
}

fn field(out: &mut String, key: &str, value: &Value, level: usize, omit_metadata_fields: bool) {
    if omit_metadata_fields
        && matches!(
            key,
            "managedFields" | "name" | "namespace" | "labels" | "annotations"
        )
    {
        return;
    }
    let indent = Indent(level);
    let label = smart_label(key);
    match value {
        Value::Object(_) => {
            writeln!(out, "{indent}{label}:").unwrap();
            content(out, value, level + 1, level == 0 && key == "metadata");
        }
        Value::Array(items) => {
            writeln!(out, "{indent}{label}:").unwrap();
            for item in items {
                if item.is_object() {
                    content(out, item, level + 1, false);
                } else {
                    write!(out, "{indent}  ").unwrap();
                    write_go_value(out, item);
                    out.push('\n');
                }
            }
        }
        value => {
            write!(out, "{indent}{label}:\t").unwrap();
            write_go_value(out, value);
            out.push('\n');
        }
    }
}

fn write_go_value(out: &mut String, value: &Value) {
    match value {
        Value::Null => out.push_str("<nil>"),
        Value::String(value) => out.push_str(value),
        Value::Array(values) => {
            out.push('[');
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    out.push(' ');
                }
                write_go_value(out, value);
            }
            out.push(']');
        }
        Value::Object(values) => {
            let mut pairs: Vec<_> = values.iter().collect();
            pairs.sort_by_key(|(key, _)| *key);
            out.push_str("map[");
            for (index, (key, value)) in pairs.into_iter().enumerate() {
                if index != 0 {
                    out.push(' ');
                }
                write!(out, "{key}:").unwrap();
                write_go_value(out, value);
            }
            out.push(']');
        }
        value => write!(out, "{value}").unwrap(),
    }
}

struct Indent(usize);

impl std::fmt::Display for Indent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for _ in 0..self.0 {
            f.write_str("  ")?;
        }
        Ok(())
    }
}

fn smart_label(field: &str) -> String {
    if field.chars().any(|c| !c.is_alphabetic() && c != '-') {
        return field.into();
    }
    let chars: Vec<_> = field.chars().collect();
    let mut parts = Vec::new();
    let mut start = 0;
    for i in 1..chars.len() {
        let prev = chars[i - 1];
        let current = chars[i];
        if (prev.is_lowercase() && current.is_uppercase())
            || (prev.is_uppercase()
                && current.is_uppercase()
                && chars.get(i + 1).is_some_and(|c| c.is_lowercase()))
            || ((prev == '-') != (current == '-'))
        {
            parts.push(chars[start..i].iter().collect::<String>());
            start = i;
        }
    }
    if start < chars.len() {
        parts.push(chars[start..].iter().collect());
    }
    let mut merged = Vec::new();
    let mut parts = parts.into_iter().peekable();
    while let Some(mut part) = parts.next() {
        if part.len() >= 2
            && part.chars().all(|c| !c.is_alphabetic() || c.is_uppercase())
            && parts
                .peek()
                .is_some_and(|next| next.chars().next().is_some_and(char::is_uppercase))
        {
            part.push_str(&parts.next().unwrap());
        }
        merged.push(part);
    }
    merged
        .into_iter()
        .map(|p| {
            let upper = p.to_uppercase();
            if matches!(upper.as_str(), "API" | "URL" | "UID" | "GUID") {
                upper
            } else if p.to_lowercase() != p {
                p
            } else {
                let mut letters = p.chars();
                match letters.next() {
                    Some(first) => {
                        first.to_uppercase().collect::<String>() + &letters.as_str().to_lowercase()
                    }
                    None => String::new(),
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::{render, smart_label};
    use k8s_openapi::jiff::Timestamp;

    #[test]
    fn labels_preserve_special_fields_and_split_acronyms() {
        for (input, expected) in [
            ("apiVersion", "API Version"),
            ("externalURL", "External URL"),
            ("someUID", "Some UID"),
            ("osbGUID", "Osb GUID"),
            ("HTTPProxy", "HTTPProxy"),
            ("IPAddress", "IPAddress"),
            ("APIVersion", "APIVersion"),
            ("TLSConfig", "TLSConfig"),
            ("key.example", "key.example"),
            ("field2", "field2"),
        ] {
            assert_eq!(smart_label(input), expected);
        }
    }

    #[test]
    fn generic_omits_duplicate_metadata_and_keeps_nested_fields() {
        let object = serde_json::from_value(serde_json::json!({"apiVersion":"example.com/v1","kind":"Widget","metadata":{"name":"demo","namespace":"default","managedFields":[{"manager":"hidden"}]},"spec":{"enabled":true,"members":[{"name":"one"}],"nullable":null}})).unwrap();
        let output = render(&object, Some(&[]), Timestamp::now());
        assert!(!output.contains("hidden"));
        assert_eq!(output.matches("Name:").count(), 2);
        assert!(output.contains("API Version:"));
        assert!(output.contains("<nil>"));
        assert!(output.contains("Events:"));
    }
}
