// Copyright 2014 The Kubernetes Authors.
// Copyright 2026 Nikola Milojević.
// SPDX-License-Identifier: Apache-2.0
// Adapted from kubectl v0.37.0 pkg/describe/describe.go. See LICENSE-APACHE.

use super::*;
use serde_json::Value;

pub(super) fn render(object: &DynamicObject, events: Option<&[Event]>, now: Timestamp) -> String {
    let mut out = metadata(&object.metadata);
    let value = serde_json::to_value(object).expect("DynamicObject is JSON serializable");
    content(&mut out, &value, 0, "");
    with_events(out, events, now)
}

fn content(out: &mut String, value: &Value, level: usize, prefix: &str) {
    let Some(fields) = value.as_object() else {
        return;
    };
    let mut keys: Vec<_> = fields.keys().collect();
    keys.sort();
    for key in keys {
        let path = format!("{prefix}.{key}");
        if matches!(
            path.as_str(),
            ".metadata.managedFields"
                | ".metadata.name"
                | ".metadata.namespace"
                | ".metadata.labels"
                | ".metadata.annotations"
        ) {
            continue;
        }
        let indent = "  ".repeat(level);
        let label = smart_label(key);
        match &fields[key] {
            Value::Object(_) => {
                writeln!(out, "{indent}{label}:").unwrap();
                content(out, &fields[key], level + 1, &path);
            }
            Value::Array(items) => {
                writeln!(out, "{indent}{label}:").unwrap();
                for item in items {
                    if item.is_object() {
                        content(out, item, level + 1, &path);
                    } else {
                        writeln!(out, "{indent}  {}", go_value(item)).unwrap();
                    }
                }
            }
            value => writeln!(out, "{indent}{label}:\t{}", go_value(value)).unwrap(),
        }
    }
}

fn go_value(value: &Value) -> String {
    match value {
        Value::Null => "<nil>".into(),
        Value::String(s) => s.clone(),
        Value::Array(a) => format!("[{}]", a.iter().map(go_value).collect::<Vec<_>>().join(" ")),
        Value::Object(m) => {
            let mut pairs: Vec<_> = m.iter().collect();
            pairs.sort_by_key(|(k, _)| *k);
            format!(
                "map[{}]",
                pairs
                    .into_iter()
                    .map(|(k, v)| format!("{k}:{}", go_value(v)))
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        }
        _ => value.to_string(),
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
    use super::*;

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
