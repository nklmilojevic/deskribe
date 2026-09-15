// Copyright 2026 Nikola Milojević.
// SPDX-License-Identifier: Apache-2.0

use serde_json::Value;

pub(crate) fn text(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}
pub(crate) fn items(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or_default()
}
pub(crate) fn integer(v: &Value) -> i64 {
    v.as_i64().unwrap_or_default()
}
