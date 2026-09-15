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

use k8s_openapi::jiff::Timestamp;
use k8s_openapi::jiff::tz::TimeZone;

pub(crate) fn age(time: Option<Timestamp>, now: Timestamp) -> String {
    let Some(time) = time.filter(|t| t.as_second() != -62135596800) else {
        return "<unknown>".into();
    };
    human_duration((now.as_nanosecond() - time.as_nanosecond()) / 1_000_000_000)
}

pub(crate) fn human_duration(seconds: i128) -> String {
    let minutes = seconds / 60;
    let hours = minutes / 60;
    let days = hours / 24;
    let years = days / 365;
    let pair = |n, suffix, rest, tail| {
        if rest == 0 {
            format!("{n}{suffix}")
        } else {
            format!("{n}{suffix}{rest}{tail}")
        }
    };
    match seconds {
        ..=-2 => "<invalid>".into(),
        -1 => "0s".into(),
        0..120 => format!("{seconds}s"),
        _ if minutes < 10 => pair(minutes, "m", seconds % 60, "s"),
        _ if minutes < 180 => format!("{minutes}m"),
        _ if hours < 8 => pair(hours, "h", minutes % 60, "m"),
        _ if hours < 48 => format!("{hours}h"),
        _ if days < 8 => pair(days, "d", hours % 24, "h"),
        _ if days < 730 => format!("{days}d"),
        _ if days < 2920 => pair(years, "y", days % 365, "d"),
        _ => format!("{years}y"),
    }
}

pub(crate) fn timestamp(value: &str, zone: &k8s_openapi::jiff::tz::TimeZone) -> String {
    value
        .parse::<Timestamp>()
        .ok()
        .map(|t| {
            t.to_zoned(zone.clone())
                .strftime("%a, %d %b %Y %H:%M:%S %z")
                .to_string()
        })
        .unwrap_or_else(|| "Mon, 01 Jan 0001 00:00:00 +0000".into())
}

pub(crate) fn value_timestamp(value: &serde_json::Value, zone: &TimeZone) -> String {
    timestamp(crate::json::text(value), zone)
}

#[cfg(test)]
mod tests {
    use super::human_duration;
    #[test]
    fn event_age_matches_upstream_duration_boundaries() {
        for (seconds, expected) in [
            (-2, "<invalid>"),
            (-1, "0s"),
            (0, "0s"),
            (119, "119s"),
            (120, "2m"),
            (121, "2m1s"),
            (600, "10m"),
            (10800, "3h"),
            (10860, "3h1m"),
            (28800, "8h"),
            (172800, "2d"),
            (176400, "2d1h"),
            (691200, "8d"),
            (63072000, "2y"),
            (63158400, "2y1d"),
            (252288000, "8y"),
        ] {
            assert_eq!(human_duration(seconds), expected);
        }
    }
}
