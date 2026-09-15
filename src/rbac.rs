// Copyright 2014, 2018 The Kubernetes Authors.
// Copyright 2026 Nikola Milojević.
// SPDX-License-Identifier: Apache-2.0
// Adapted from kubectl v0.35.1 pkg/describe/describe.go and pkg/util/rbac/rbac.go.
// See LICENSE-APACHE.

use crate::format::tabbed;
use crate::json::text;
use crate::metadata::metadata_header;
use kube::api::DynamicObject;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt::Write;

#[derive(Default)]
struct Rule {
    group: Option<String>,
    resource: Option<String>,
    name: Option<String>,
    url: Option<String>,
    verbs: Vec<String>,
}

impl Rule {
    fn sort_key(&self) -> String {
        format!(
            "&PolicyRule{{Verbs:[{}],APIGroups:{},Resources:{},ResourceNames:{},NonResourceURLs:{},}}",
            self.verbs.join(" "),
            list(self.group.as_deref()),
            list(self.resource.as_deref()),
            list(self.name.as_deref()),
            list(self.url.as_deref())
        )
    }
}

fn list(value: Option<&str>) -> String {
    format!("[{}]", value.unwrap_or_default())
}
fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(String::from)
        .collect()
}

pub(crate) fn render(object: &DynamicObject) -> String {
    let mut simple: BTreeMap<(String, String, Option<String>), Rule> = BTreeMap::new();
    let mut rules = Vec::new();
    for rule in object.data["rules"].as_array().into_iter().flatten() {
        let verbs = strings(&rule["verbs"]);
        let mut names: Vec<_> = strings(&rule["resourceNames"])
            .into_iter()
            .map(Some)
            .collect();
        if names.is_empty() {
            names.push(None);
        }
        for group in strings(&rule["apiGroups"]) {
            for resource in strings(&rule["resources"]) {
                for verb in &verbs {
                    for name in &names {
                        let entry = simple
                            .entry((group.clone(), resource.clone(), name.clone()))
                            .or_insert_with(|| Rule {
                                group: Some(group.clone()),
                                resource: Some(resource.clone()),
                                name: name.clone(),
                                ..Default::default()
                            });
                        if !entry.verbs.contains(verb) {
                            entry.verbs.push(verb.clone());
                        }
                    }
                }
            }
        }
        for url in strings(&rule["nonResourceURLs"]) {
            for verb in &verbs {
                rules.push(Rule {
                    url: Some(url.clone()),
                    verbs: vec![verb.clone()],
                    ..Default::default()
                });
            }
        }
    }
    rules.extend(simple.into_values());
    rules.sort_by_cached_key(Rule::sort_key);
    let mut out = metadata_header(&object.metadata, false);
    out.push_str("PolicyRule:\n  Resources\tNon-Resource URLs\tResource Names\tVerbs\n  ---------\t-----------------\t--------------\t-----\n");
    for rule in rules {
        let mut combined = String::new();
        if let Some(resource) = &rule.resource {
            let (base, sub) = resource
                .split_once('/')
                .map_or((resource.as_str(), None), |(a, b)| (a, Some(b)));
            combined.push_str(base);
            if let Some(group) = rule.group.as_deref().filter(|g| !g.is_empty()) {
                write!(combined, ".{group}").unwrap();
            }
            if let Some(sub) = sub {
                write!(combined, "/{sub}").unwrap();
            }
        }
        writeln!(
            out,
            "  {combined}\t{}\t{}\t[{}]",
            list(rule.url.as_deref()),
            list(rule.name.as_deref()),
            rule.verbs.join(" ")
        )
        .unwrap();
    }
    tabbed(&out)
}

pub(crate) fn render_binding(object: &DynamicObject) -> String {
    let value = &object.data;
    let mut out = metadata_header(&object.metadata, false);

    writeln!(out, "Role:\n  Kind:\t{}\n  Name:\t{}\nSubjects:\n  Kind\tName\tNamespace\n  ----\t----\t---------", text(&value["roleRef"]["kind"]), text(&value["roleRef"]["name"])).unwrap();
    for subject in value["subjects"].as_array().into_iter().flatten() {
        writeln!(
            out,
            "  {}\t{}\t{}",
            text(&subject["kind"]),
            text(&subject["name"]),
            text(&subject["namespace"])
        )
        .unwrap();
    }

    tabbed(&out)
}
