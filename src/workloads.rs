// Copyright 2014 The Kubernetes Authors.
// Copyright 2026 Nikola Milojević.
// SPDX-License-Identifier: Apache-2.0
// Adapted from kubectl v0.37.0 pkg/describe/describe.go. See LICENSE-APACHE.

use super::*;
use k8s_openapi::jiff::tz::TimeZone;
use serde_json::Value;
fn text(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}
fn items(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or_default()
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
fn integer(v: &Value) -> i64 {
    v.as_i64().unwrap_or_default()
}

pub(super) fn supports(ar: &ApiResource) -> bool {
    ar.group == "batch" && matches!(ar.kind.as_str(), "Job" | "CronJob")
}

pub(super) fn template(out: &mut String, template: &Value, zone: &TimeZone) {
    out.push_str("Pod Template:\n");
    if template.is_null() {
        out.push_str("  <unset>");
        return;
    }
    let meta =
        serde_json::from_value::<ObjectMeta>(template["metadata"].clone()).unwrap_or_default();
    let header = metadata_header(&meta, false);
    let labels = header
        .split_once('\n')
        .map(|(_, tail)| tail)
        .unwrap_or_default();
    // Upstream indents section titles but not continuation lines in templates.
    let labels =
        labels
            .replacen("Labels:", "  Labels:", 1)
            .replacen("Annotations:", "  Annotations:", 1);
    if meta.annotations.as_ref().is_none_or(|a| a.is_empty()) {
        out.push_str(
            labels
                .split_once("  Annotations:")
                .map(|(head, _)| head)
                .unwrap_or(&labels),
        );
    } else {
        out.push_str(&labels);
    }
    let spec = &template["spec"];
    if !text(&spec["serviceAccountName"]).is_empty() {
        writeln!(
            out,
            "  Service Account:\t{}",
            text(&spec["serviceAccountName"])
        )
        .unwrap();
    }
    if !items(&spec["initContainers"]).is_empty() {
        containers::render(
            out,
            "Init Containers",
            items(&spec["initContainers"]),
            &[],
            None,
            "  ",
            zone,
        );
    }
    containers::render(
        out,
        "Containers",
        items(&spec["containers"]),
        &[],
        None,
        "  ",
        zone,
    );
    volumes::render(out, items(&spec["volumes"]), "  ");
    pod::topology(out, spec, "  ");
    if !text(&spec["priorityClassName"]).is_empty() {
        writeln!(
            out,
            "  Priority Class Name:\t{}",
            text(&spec["priorityClassName"])
        )
        .unwrap();
    }
    pod::scheduling(out, spec, "  ");
    pod::workload(out, spec, "  ");
}

pub(super) fn render(
    object: &DynamicObject,
    kind: &str,
    events: Option<&[Event]>,
    now: Timestamp,
    zone: &TimeZone,
) -> String {
    let spec = &object.data["spec"];
    let status = &object.data["status"];
    let mut out = metadata(&object.metadata);
    if kind == "Job" {
        let selector = policy::selector(&spec["selector"]);
        out = out.replacen(
            "Labels:",
            &format!(
                "Selector:\t{}\nLabels:",
                if selector == "<none>" { "" } else { &selector }
            ),
            1,
        );
        if let Some(owner) = object
            .metadata
            .owner_references
            .iter()
            .flatten()
            .find(|o| o.controller == Some(true))
        {
            writeln!(out, "Controlled By:\t{}/{}", owner.kind, owner.name).unwrap();
        }
        if !spec["parallelism"].is_null() {
            writeln!(out, "Parallelism:\t{}", integer(&spec["parallelism"])).unwrap();
        }
        writeln!(
            out,
            "Completions:\t{}",
            if spec["completions"].is_null() {
                "<unset>".into()
            } else {
                scalar(&spec["completions"])
            }
        )
        .unwrap();
        for (field, label) in [
            ("completionMode", "Completion Mode"),
            ("suspend", "Suspend"),
            ("backoffLimit", "Backoff Limit"),
            ("ttlSecondsAfterFinished", "TTL Seconds After Finished"),
        ] {
            if !spec[field].is_null() {
                writeln!(out, "{label}:\t{}", scalar(&spec[field])).unwrap();
            }
        }
        for (field, label) in [
            ("startTime", "Start Time"),
            ("completionTime", "Completed At"),
        ] {
            if !status[field].is_null() {
                writeln!(
                    out,
                    "{label}:\t{}",
                    containers::timestamp(&status[field], zone)
                )
                .unwrap();
            }
        }
        if let (Ok(start), Ok(end)) = (
            text(&status["startTime"]).parse::<Timestamp>(),
            text(&status["completionTime"]).parse::<Timestamp>(),
        ) {
            writeln!(
                out,
                "Duration:\t{}",
                human_duration((end.as_nanosecond() - start.as_nanosecond()) / 1_000_000_000)
            )
            .unwrap();
        }
        if !spec["activeDeadlineSeconds"].is_null() {
            writeln!(
                out,
                "Active Deadline Seconds:\t{}s",
                integer(&spec["activeDeadlineSeconds"])
            )
            .unwrap();
        }
        writeln!(
            out,
            "Pods Statuses:\t{} Active{} / {} Succeeded / {} Failed",
            integer(&status["active"]),
            if status["ready"].is_null() {
                String::new()
            } else {
                format!(" ({} Ready)", integer(&status["ready"]))
            },
            integer(&status["succeeded"]),
            integer(&status["failed"])
        )
        .unwrap();
        if text(&spec["completionMode"]) == "Indexed" {
            let indexes = text(&status["completedIndexes"]);
            let indexes = if indexes.is_empty() {
                "<none>".into()
            } else if let Some(end) = indexes
                .bytes()
                .enumerate()
                .find(|(i, b)| *i >= 50 && *b == b',')
                .map(|(i, _)| i)
            {
                format!("{}...", &indexes[..=end])
            } else {
                indexes.into()
            };
            writeln!(out, "Completed Indexes:\t{indexes}").unwrap();
        }
        template(&mut out, &spec["template"], zone);
    } else {
        writeln!(
            out,
            "Schedule:\t{}\nConcurrency Policy:\t{}\nSuspend:\t{}",
            text(&spec["schedule"]),
            text(&spec["concurrencyPolicy"]),
            spec["suspend"]
                .as_bool()
                .map(|b| if b { "True" } else { "False" })
                .unwrap_or("<unset>")
        )
        .unwrap();
        writeln!(
            out,
            "Time Zone:\t{}",
            spec["timeZone"].as_str().unwrap_or("<unset>")
        )
        .unwrap();
        for (field, label) in [
            ("successfulJobsHistoryLimit", "Successful Job History Limit"),
            ("failedJobsHistoryLimit", "Failed Job History Limit"),
        ] {
            writeln!(
                out,
                "{label}:\t{}",
                spec[field]
                    .as_i64()
                    .map(|i| i.to_string())
                    .unwrap_or("<unset>".into())
            )
            .unwrap();
        }
        writeln!(
            out,
            "Starting Deadline Seconds:\t{}",
            spec["startingDeadlineSeconds"]
                .as_i64()
                .map(|i| format!("{i}s"))
                .unwrap_or("<unset>".into())
        )
        .unwrap();
        let job = &spec["jobTemplate"]["spec"];
        let selector = policy::selector(&job["selector"]);
        writeln!(
            out,
            "Selector:\t{}",
            if job["selector"].is_null() {
                "<unset>"
            } else if selector == "<none>" {
                ""
            } else {
                &selector
            }
        )
        .unwrap();
        for (field, label) in [
            ("parallelism", "Parallelism"),
            ("completions", "Completions"),
        ] {
            writeln!(
                out,
                "{label}:\t{}",
                job[field]
                    .as_i64()
                    .map(|i| i.to_string())
                    .unwrap_or("<unset>".into())
            )
            .unwrap();
        }
        if !job["activeDeadlineSeconds"].is_null() {
            writeln!(
                out,
                "Active Deadline Seconds:\t{}s",
                integer(&job["activeDeadlineSeconds"])
            )
            .unwrap();
        }
        template(&mut out, &job["template"], zone);
        writeln!(
            out,
            "Last Schedule Time:\t{}",
            if status["lastScheduleTime"].is_null() {
                "<unset>".into()
            } else {
                containers::timestamp(&status["lastScheduleTime"], zone)
            }
        )
        .unwrap();
        let active = items(&status["active"])
            .iter()
            .map(|j| text(&j["name"]))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(
            out,
            "Active Jobs:\t{}",
            if active.is_empty() { "<none>" } else { &active }
        )
        .unwrap();
    }
    with_events(out, events, now)
}
