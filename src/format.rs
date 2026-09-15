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

// Go text/tabwriter's contiguous-column alignment (minwidth=0, padding=2).
// Column widths count Unicode code points, not terminal display width.

pub(crate) fn tabbed(text: &str) -> String {
    let escaped = text.replace('\x1b', "^[").replace('\r', "\\r");
    let mut output = String::new();
    let mut sections = escaped.split('\x0c').peekable();
    while let Some(section) = sections.next() {
        let mut rows: Vec<Vec<String>> = section
            .split_terminator('\n')
            .map(|line| line.split(['\t', '\x0b']).map(String::from).collect())
            .collect();
        align(&mut rows, 0);
        for row in rows {
            output.push_str(&row.concat());
            output.push('\n');
        }
        if sections.peek().is_some() && (section.is_empty() || section.ends_with('\n')) {
            output.push('\n');
        }
    }
    output
}

fn align(rows: &mut [Vec<String>], col: usize) {
    let mut start = 0;
    while start < rows.len() {
        if rows[start].len() <= col + 1 {
            start += 1;
            continue;
        }
        let end = (start..rows.len())
            .find(|&i| rows[i].len() <= col + 1)
            .unwrap_or(rows.len());
        let width = rows[start..end]
            .iter()
            .map(|r| r[col].chars().count() + 2)
            .max()
            .unwrap();
        for row in &mut rows[start..end] {
            let padding = width - row[col].chars().count();
            row[col].push_str(&" ".repeat(padding));
        }
        align(&mut rows[start..end], col + 1);
        start = end;
    }
}
