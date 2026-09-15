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

use crate::format::tabbed;
use crate::time::age;
use k8s_openapi::api::core::v1::Event;
use k8s_openapi::jiff::Timestamp;
use std::fmt::Write;

pub(crate) fn with_events(mut out: String, events: Option<&[Event]>, now: Timestamp) -> String {
    let Some(events) = events else {
        return tabbed(&out);
    };
    if events.is_empty() {
        out.push_str("Events:\t<none>\n");
        return tabbed(&out);
    }
    let mut out = tabbed(&out); // Upstream flushes before the Events table.
    let mut rows = String::from(
        "Events:\n  Type\tReason\tAge\tFrom\tMessage\n  ----\t------\t----\t----\t-------\n",
    );
    let mut events: Vec<_> = events.iter().collect();
    events.sort_by_key(|e| e.last_timestamp.as_ref().map(|t| t.0));
    for e in events {
        let first = age(
            e.event_time
                .as_ref()
                .map(|t| t.0)
                .or_else(|| e.first_timestamp.as_ref().map(|t| t.0)),
            now,
        );
        let interval = if let Some(series) = &e.series {
            format!(
                "{} (x{} over {first})",
                age(series.last_observed_time.as_ref().map(|t| t.0), now),
                series.count.unwrap_or_default()
            )
        } else if e.count.unwrap_or_default() > 1 {
            format!(
                "{} (x{} over {first})",
                age(e.last_timestamp.as_ref().map(|t| t.0), now),
                e.count.unwrap()
            )
        } else {
            first
        };
        let source = e
            .source
            .as_ref()
            .and_then(|s| s.component.as_deref())
            .filter(|s| !s.is_empty())
            .or(e.reporting_component.as_deref())
            .unwrap_or_default();
        let message = e.message.as_deref().unwrap_or_default().trim();
        let message = match e
            .involved_object
            .field_path
            .as_deref()
            .filter(|s| !s.is_empty())
        {
            Some(field) => format!("{field}: {message}"),
            None => message.into(),
        };
        writeln!(
            rows,
            "  {}\t{}\t{interval}\t{source}\t{message}",
            e.type_.as_deref().unwrap_or_default(),
            e.reason.as_deref().unwrap_or_default()
        )
        .unwrap();
    }
    out.push_str(&tabbed(&rows));
    out
}

#[cfg(test)]
mod tests {
    use crate::render_config_map;
    use k8s_openapi::api::core::v1::ConfigMap;
    use k8s_openapi::api::core::v1::Event;
    #[test]
    fn events_sort_by_last_timestamp_and_render_intervals() {
        let cm = ConfigMap::default();
        let events: Vec<Event> = serde_json::from_value(serde_json::json!([
            {"metadata":{},"involvedObject":{},"reason":"Later","firstTimestamp":"2026-01-01T00:00:00Z","lastTimestamp":"2026-01-01T00:04:00Z","count":4},
            {"metadata":{},"involvedObject":{"fieldPath":"data.key"},"reason":"Earlier","eventTime":"2026-01-01T00:01:00.000000Z","lastTimestamp":"2026-01-01T00:02:00Z","series":{"count":2,"lastObservedTime":"2026-01-01T00:03:00.000000Z"},"reportingComponent":"controller","message":"  hello  "}
        ])).unwrap();
        let text = render_config_map(&cm, &events, "2026-01-01T00:05:00Z".parse().unwrap());
        assert!(text.find("Earlier").unwrap() < text.find("Later").unwrap());
        assert!(text.contains("2m (x2 over 4m)"));
        assert!(text.contains("60s (x4 over 5m)"));
        assert!(text.contains("controller"));
        assert!(text.contains("data.key: hello\n"));
    }
}
