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
use std::fmt;

#[derive(Clone, Copy)]
pub(crate) enum HumanDuration {
    Unknown,
    Seconds(i128),
}

impl fmt::Display for HumanDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self::Seconds(seconds) = *self else {
            return f.write_str("<unknown>");
        };
        let minutes = seconds / 60;
        let hours = minutes / 60;
        let days = hours / 24;
        let years = days / 365;
        let pair = |f: &mut fmt::Formatter<'_>, n, suffix, rest, tail| {
            write!(f, "{n}{suffix}")?;
            if rest != 0 {
                write!(f, "{rest}{tail}")?;
            }
            Ok(())
        };
        match seconds {
            ..=-2 => f.write_str("<invalid>"),
            -1 => f.write_str("0s"),
            0..120 => write!(f, "{seconds}s"),
            _ if minutes < 10 => pair(f, minutes, "m", seconds % 60, "s"),
            _ if minutes < 180 => write!(f, "{minutes}m"),
            _ if hours < 8 => pair(f, hours, "h", minutes % 60, "m"),
            _ if hours < 48 => write!(f, "{hours}h"),
            _ if days < 8 => pair(f, days, "d", hours % 24, "h"),
            _ if days < 730 => write!(f, "{days}d"),
            _ if days < 2920 => pair(f, years, "y", days % 365, "d"),
            _ => write!(f, "{years}y"),
        }
    }
}

pub(crate) fn age(time: Option<Timestamp>, now: Timestamp) -> HumanDuration {
    match time.filter(|t| t.as_second() != -62135596800) {
        Some(time) => {
            HumanDuration::Seconds((now.as_nanosecond() - time.as_nanosecond()) / 1_000_000_000)
        }
        None => HumanDuration::Unknown,
    }
}

pub(crate) fn human_duration(seconds: i128) -> HumanDuration {
    HumanDuration::Seconds(seconds)
}

pub(crate) fn timestamp(value: &str, zone: &k8s_openapi::jiff::tz::TimeZone) -> String {
    value.parse::<Timestamp>().ok().map_or_else(
        || invalid_timestamp().into(),
        |time| format_timestamp(time, zone),
    )
}

pub(crate) fn optional_timestamp(value: Option<Timestamp>, zone: &TimeZone) -> String {
    value.map_or_else(
        || invalid_timestamp().into(),
        |time| format_timestamp(time, zone),
    )
}

fn format_timestamp(time: Timestamp, zone: &TimeZone) -> String {
    time.to_zoned(zone.clone())
        .strftime("%a, %d %b %Y %H:%M:%S %z")
        .to_string()
}

fn invalid_timestamp() -> &'static str {
    "Mon, 01 Jan 0001 00:00:00 +0000"
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
            assert_eq!(human_duration(seconds).to_string(), expected);
        }
    }
}
