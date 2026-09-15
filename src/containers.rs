// Copyright 2014 The Kubernetes Authors.
// Copyright 2026 sofka contributors.
// SPDX-License-Identifier: Apache-2.0
// Adapted from kubectl v0.35.1 pkg/describe/describe.go. See LICENSE-APACHE.

use super::*;
use k8s_openapi::jiff::tz::TimeZone;
use serde_json::Value;

fn text(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}
fn items(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or_default()
}
fn boolean(v: &Value) -> bool {
    v.as_bool().unwrap_or_default()
}
fn integer(v: &Value) -> i64 {
    v.as_i64().unwrap_or_default()
}
fn scalar(v: &Value) -> String {
    v.as_str().map(str::to_owned).unwrap_or_else(|| {
        if v.is_null() {
            String::new()
        } else {
            v.to_string()
        }
    })
}

pub(super) fn timestamp(value: &Value, zone: &TimeZone) -> String {
    text(value)
        .parse::<Timestamp>()
        .ok()
        .map(|t| {
            t.to_zoned(zone.clone())
                .strftime("%a, %d %b %Y %H:%M:%S %z")
                .to_string()
        })
        .unwrap_or_else(|| "Mon, 01 Jan 0001 00:00:00 +0000".into())
}

pub(super) fn resources(out: &mut String, resources: &Value, level: usize) {
    let indent = "  ".repeat(level);
    for (field, label) in [("limits", "Limits"), ("requests", "Requests")] {
        let mut pairs: Vec<_> = resources[field].as_object().into_iter().flatten().collect();
        pairs.sort_by_key(|(k, _)| *k);
        if !pairs.is_empty() {
            writeln!(out, "{indent}{label}:").unwrap();
        }
        for (name, quantity) in pairs {
            writeln!(
                out,
                "{indent}  {name}:\t{}",
                super::quantity::canonical(&scalar(quantity))
            )
            .unwrap();
        }
    }
}

pub(super) fn render(
    out: &mut String,
    label: &str,
    containers: &[Value],
    statuses: &[Value],
    pod: Option<&DynamicObject>,
    space: &str,
    zone: &TimeZone,
) {
    writeln!(
        out,
        "{space}{label}:{}",
        if containers.is_empty() { " <none>" } else { "" }
    )
    .unwrap();
    for container in containers {
        let status = statuses.iter().find(|s| s["name"] == container["name"]);
        writeln!(
            out,
            "  {}{}:",
            if space.is_empty() { "" } else { " " },
            text(&container["name"])
        )
        .unwrap();
        if let Some(status) = status {
            writeln!(out, "    Container ID:\t{}", text(&status["containerID"])).unwrap();
        }
        writeln!(out, "    Image:\t{}", text(&container["image"])).unwrap();
        if let Some(status) = status {
            writeln!(out, "    Image ID:\t{}", text(&status["imageID"])).unwrap();
        }
        for (field, label) in [("containerPort", "Port"), ("hostPort", "Host Port")] {
            let ports = items(&container["ports"])
                .iter()
                .map(|p| {
                    let mut result = format!("{}/{}", integer(&p[field]), text(&p["protocol"]));
                    if !text(&p["name"]).is_empty() {
                        write!(result, " ({})", text(&p["name"])).unwrap();
                    }
                    result
                })
                .collect::<Vec<_>>()
                .join(", ");
            writeln!(
                out,
                "    {label}{}:\t{}",
                if ports.contains(',') { "s" } else { "" },
                if ports.is_empty() { "<none>" } else { &ports }
            )
            .unwrap();
        }
        let seccomp = &container["securityContext"]["seccompProfile"];
        if !seccomp.is_null() {
            writeln!(out, "    SeccompProfile:\t{}", text(&seccomp["type"])).unwrap();
            if text(&seccomp["type"]) == "Localhost" {
                writeln!(
                    out,
                    "      LocalhostProfile:\t{}",
                    text(&seccomp["localhostProfile"])
                )
                .unwrap();
            }
        }
        for (field, label) in [("command", "Command"), ("args", "Args")] {
            if !items(&container[field]).is_empty() {
                writeln!(out, "    {label}:").unwrap();
            }
            for arg in items(&container[field]) {
                for line in text(arg).split('\n') {
                    writeln!(out, "      {line}").unwrap();
                }
            }
        }
        if let Some(status) = status {
            state(out, "State", &status["state"], zone);
            if !status["lastState"]["terminated"].is_null() {
                state(out, "Last State", &status["lastState"], zone);
            }
            writeln!(
                out,
                "    Ready:\t{}\n    Restart Count:\t{}",
                if boolean(&status["ready"]) {
                    "True"
                } else {
                    "False"
                },
                integer(&status["restartCount"])
            )
            .unwrap();
        }
        resources(out, &container["resources"], 2);
        for (field, label) in [
            ("livenessProbe", "Liveness"),
            ("readinessProbe", "Readiness"),
            ("startupProbe", "Startup"),
        ] {
            if !container[field].is_null() {
                writeln!(out, "    {label}:\t{}", probe(&container[field])).unwrap();
            }
        }
        if !items(&container["envFrom"]).is_empty() {
            out.push_str("    Environment Variables from:\n");
            for entry in items(&container["envFrom"]) {
                let (source, kind) = if !entry["configMapRef"].is_null() {
                    (&entry["configMapRef"], "ConfigMap")
                } else {
                    (&entry["secretRef"], "Secret")
                };
                let prefix = text(&entry["prefix"]);
                writeln!(
                    out,
                    "      {}\t{kind}{}\tOptional: {}",
                    text(&source["name"]),
                    if prefix.is_empty() {
                        String::new()
                    } else {
                        format!(" with prefix '{prefix}'")
                    },
                    boolean(&source["optional"])
                )
                .unwrap();
            }
        }
        environment(out, container, pod);
        let mut mounts: Vec<_> = items(&container["volumeMounts"]).iter().collect();
        mounts.sort_by_key(|m| text(&m["mountPath"]));
        writeln!(
            out,
            "    Mounts:{}",
            if mounts.is_empty() { "\t<none>" } else { "" }
        )
        .unwrap();
        for mount in &mounts {
            let mut flags = if boolean(&mount["readOnly"]) {
                "ro"
            } else {
                "rw"
            }
            .to_string();
            if !text(&mount["subPath"]).is_empty() {
                write!(
                    flags,
                    ",path={}",
                    serde_json::to_string(text(&mount["subPath"])).unwrap()
                )
                .unwrap();
            }
            writeln!(
                out,
                "      {} from {} ({flags})",
                text(&mount["mountPath"]),
                text(&mount["name"])
            )
            .unwrap();
        }
        let mut devices: Vec<_> = items(&container["volumeDevices"]).iter().collect();
        devices.sort_by_key(|d| text(&d["devicePath"]));
        if !devices.is_empty() {
            writeln!(
                out,
                "    Devices:{}",
                if mounts.is_empty() { "\t<none>" } else { "" }
            )
            .unwrap();
        }
        for device in devices {
            writeln!(
                out,
                "      {} from {}",
                text(&device["devicePath"]),
                text(&device["name"])
            )
            .unwrap();
        }
    }
}

fn state(out: &mut String, label: &str, state: &Value, zone: &TimeZone) {
    if !state["running"].is_null() {
        writeln!(
            out,
            "    {label}:\tRunning\n      Started:\t{}",
            timestamp(&state["running"]["startedAt"], zone)
        )
        .unwrap();
    } else if !state["waiting"].is_null() {
        writeln!(out, "    {label}:\tWaiting").unwrap();
        if !text(&state["waiting"]["reason"]).is_empty() {
            writeln!(out, "      Reason:\t{}", text(&state["waiting"]["reason"])).unwrap();
        }
    } else if !state["terminated"].is_null() {
        let term = &state["terminated"];
        writeln!(out, "    {label}:\tTerminated").unwrap();
        for (field, label) in [("reason", "Reason"), ("message", "Message")] {
            if !text(&term[field]).is_empty() {
                writeln!(out, "      {label}:\t{}", text(&term[field])).unwrap();
            }
        }
        writeln!(out, "      Exit Code:\t{}", integer(&term["exitCode"])).unwrap();
        if integer(&term["signal"]) > 0 {
            writeln!(out, "      Signal:\t{}", integer(&term["signal"])).unwrap();
        }
        writeln!(
            out,
            "      Started:\t{}\n      Finished:\t{}",
            timestamp(&term["startedAt"], zone),
            timestamp(&term["finishedAt"], zone)
        )
        .unwrap();
    } else {
        writeln!(out, "    {label}:\tWaiting").unwrap();
    }
}

fn environment(out: &mut String, container: &Value, pod: Option<&DynamicObject>) {
    let env = items(&container["env"]);
    writeln!(
        out,
        "    Environment:{}",
        if env.is_empty() { "\t<none>" } else { "" }
    )
    .unwrap();
    for entry in env {
        let name = text(&entry["name"]);
        let from = &entry["valueFrom"];
        if from.is_null() {
            for (i, line) in text(&entry["value"]).split('\n').enumerate() {
                writeln!(
                    out,
                    "      {}\t{line}",
                    if i == 0 {
                        format!("{name}:")
                    } else {
                        String::new()
                    }
                )
                .unwrap();
            }
        } else if !from["fieldRef"].is_null() {
            let reference = &from["fieldRef"];
            let value = pod
                .map(|p| field_value(p, text(&reference["fieldPath"])))
                .unwrap_or_default();
            writeln!(
                out,
                "      {name}:\t{value} ({}:{})",
                text(&reference["apiVersion"]),
                text(&reference["fieldPath"])
            )
            .unwrap();
        } else if !from["resourceFieldRef"].is_null() {
            let reference = &from["resourceFieldRef"];
            let resource = text(&reference["resource"]);
            let value = resource_value(container, reference);
            writeln!(
                out,
                "      {name}:\t{} ({resource})",
                if value == "0" && matches!(resource, "limits.cpu" | "limits.memory") {
                    "node allocatable"
                } else {
                    &value
                }
            )
            .unwrap();
        } else {
            for (field, kind, preposition) in [
                ("secretKeyRef", "secret", "in"),
                ("configMapKeyRef", "config map", "of"),
            ] {
                let reference = &from[field];
                if !reference.is_null() {
                    writeln!(out,"      {name}:\t<set to the key '{}' {preposition} {kind} '{}'>\tOptional: {}",text(&reference["key"]),text(&reference["name"]),boolean(&reference["optional"])).unwrap();
                    break;
                }
            }
        }
    }
}

fn field_value(pod: &DynamicObject, path: &str) -> String {
    match path {
        "metadata.name" => pod.metadata.name.clone().unwrap_or_default(),
        "metadata.namespace" => pod.metadata.namespace.clone().unwrap_or_default(),
        "metadata.uid" => pod.metadata.uid.clone().unwrap_or_default(),
        "metadata.labels" | "metadata.annotations" => {
            let map = if path == "metadata.labels" {
                &pod.metadata.labels
            } else {
                &pod.metadata.annotations
            };
            map.iter()
                .flatten()
                .map(|(k, v)| format!("{k}={}", serde_json::to_string(v).unwrap()))
                .collect::<Vec<_>>()
                .join("\n")
        }
        _ => {
            for (prefix, map) in [
                ("metadata.labels['", &pod.metadata.labels),
                ("metadata.annotations['", &pod.metadata.annotations),
            ] {
                if let Some(key) = path.strip_prefix(prefix).and_then(|s| s.strip_suffix("']")) {
                    return map
                        .as_ref()
                        .and_then(|m| m.get(key))
                        .cloned()
                        .unwrap_or_default();
                }
            }
            String::new()
        }
    }
}

fn resource_value(container: &Value, reference: &Value) -> String {
    let Some((category, name)) = text(&reference["resource"]).split_once('.') else {
        return String::new();
    };
    if !matches!(category, "limits" | "requests")
        || !(matches!(name, "cpu" | "memory" | "ephemeral-storage")
            || name.starts_with("hugepages-"))
    {
        return String::new();
    }
    let value =
        super::quantity::parse(text(&container["resources"][category][name])).unwrap_or_default();
    let divisor = super::quantity::parse(text(&reference["divisor"]))
        .filter(|q| *q != 0)
        .unwrap_or(1_000_000_000);
    let scale = if name == "cpu" {
        1_000_000
    } else {
        1_000_000_000
    };
    let value = (value + scale - 1) / scale;
    let divisor = (divisor + scale - 1) / scale;
    if divisor <= 0 {
        return String::new();
    }
    ((value + divisor - 1) / divisor).to_string()
}

fn probe(probe: &Value) -> String {
    let attrs = format!(
        "delay={}s timeout={}s period={}s successThreshold={} failureThreshold={}",
        integer(&probe["initialDelaySeconds"]),
        integer(&probe["timeoutSeconds"]),
        integer(&probe["periodSeconds"]),
        integer(&probe["successThreshold"]),
        integer(&probe["failureThreshold"])
    );
    let kind = if !probe["exec"].is_null() {
        format!(
            "exec [{}]",
            items(&probe["exec"]["command"])
                .iter()
                .map(text)
                .collect::<Vec<_>>()
                .join(" ")
        )
    } else if !probe["httpGet"].is_null() {
        let http = &probe["httpGet"];
        let host = text(&http["host"]);
        let host = if host.contains(':') {
            format!("[{host}]")
        } else {
            host.into()
        };
        let port = scalar(&http["port"]);
        let path = text(&http["path"]);
        // Go url.URL escapes path bytes but leaves path separators intact.
        let path = path
            .bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || b"/-._~!$&'()*+,;=:@".contains(&b) {
                    (b as char).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect::<String>();
        let host = if port.is_empty() {
            host
        } else {
            format!("{host}:{port}")
        };
        let path = if !host.is_empty() && !path.is_empty() && !path.starts_with('/') {
            format!("/{path}")
        } else {
            path
        };
        let scheme = text(&http["scheme"]).to_lowercase();
        format!(
            "http-get {}//{host}{path}",
            if scheme.is_empty() {
                String::new()
            } else {
                format!("{scheme}:")
            }
        )
    } else if !probe["tcpSocket"].is_null() {
        format!(
            "tcp-socket {}:{}",
            text(&probe["tcpSocket"]["host"]),
            scalar(&probe["tcpSocket"]["port"])
        )
    } else if !probe["grpc"].is_null() {
        format!(
            "grpc <pod>:{} {}",
            integer(&probe["grpc"]["port"]),
            text(&probe["grpc"]["service"])
        )
    } else {
        "unknown".into()
    };
    format!("{kind} {attrs}")
}
