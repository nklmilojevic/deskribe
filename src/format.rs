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
//
// Every describer funnels its whole document through here, which made this the
// hottest function in a describe. Cells borrow from the input instead of being
// copied into a `Vec<Vec<String>>`, widths are measured once into a side table
// rather than recomputed per alignment pass, and padding is written straight
// into an output buffer reserved to its exact final size.

use std::borrow::Cow;

/// Padding source. Runs longer than this are written in several pushes, which
/// no real describe column needs.
const SPACES: &str = "                                                                ";

pub(crate) fn tabbed(text: &str) -> String {
    let escaped = escape(text);
    let text: &str = &escaped;
    let mut out = String::with_capacity(text.len() + text.len() / 4);
    let mut layout = Layout::default();
    let mut sections = text.split('\x0c').peekable();
    while let Some(section) = sections.next() {
        layout.write(section, &mut out);
        if sections.peek().is_some() && (section.is_empty() || section.ends_with('\n')) {
            out.push('\n');
        }
    }
    out
}

/// Replace the two control characters the tabwriter renders literally.
///
/// Describe output is plain text, so the usual answer is "neither is present":
/// a vectorized search for both at once settles that in one pass and lets the
/// whole document be borrowed rather than copied twice.
fn escape(text: &str) -> Cow<'_, str> {
    let bytes = text.as_bytes();
    let Some(first) = memchr::memchr2(0x1b, b'\r', bytes) else {
        return Cow::Borrowed(text);
    };
    let mut out = String::with_capacity(text.len() + 16);
    let mut rest = first;
    let mut copied = 0;
    loop {
        out.push_str(&text[copied..rest]);
        // Both needles are ASCII, so `rest` is always a char boundary.
        out.push_str(if bytes[rest] == 0x1b { "^[" } else { "\\r" });
        copied = rest + 1;
        match memchr::memchr2(0x1b, b'\r', &bytes[copied..]) {
            Some(next) => rest = copied + next,
            None => break,
        }
    }
    out.push_str(&text[copied..]);
    Cow::Owned(out)
}

/// Code points in a cell.
///
/// Column widths count code points, and describe cells are overwhelmingly
/// ASCII — names, quantities, timestamps — where that equals the byte length.
/// `is_ascii` settles the common case with one vectorized pass.
#[inline]
fn cell_width(cell: &str) -> u32 {
    if cell.is_ascii() {
        cell.len() as u32
    } else {
        cell.chars().count() as u32
    }
}

/// Reusable scratch for one section's cell table. Buffers are kept across
/// sections so a multi-section document allocates once.
#[derive(Default)]
struct Layout<'a> {
    /// Every cell of every row, borrowed from the escaped input.
    cells: Vec<&'a str>,
    /// `(start, end)` into `cells` for each row.
    rows: Vec<(u32, u32)>,
    /// Code-point width of each cell, measured once.
    widths: Vec<u32>,
    /// Spaces to append after each cell.
    pad: Vec<u32>,
}

impl<'a> Layout<'a> {
    fn write(&mut self, section: &'a str, out: &mut String) {
        self.cells.clear();
        self.rows.clear();
        self.widths.clear();
        for line in section.split_terminator('\n') {
            let start = self.cells.len() as u32;
            for cell in line.split(['\t', '\x0b']) {
                self.cells.push(cell);
                self.widths.push(cell_width(cell));
            }
            self.rows.push((start, self.cells.len() as u32));
        }
        self.pad.clear();
        self.pad.resize(self.cells.len(), 0);
        align(
            &self.rows,
            &self.widths,
            &mut self.pad,
            0,
            0,
            self.rows.len(),
        );

        let body: usize = self.cells.iter().map(|c| c.len()).sum::<usize>()
            + self.pad.iter().map(|&p| p as usize).sum::<usize>()
            + self.rows.len();
        out.reserve(body);
        for &(start, end) in &self.rows {
            for i in start as usize..end as usize {
                out.push_str(self.cells[i]);
                push_spaces(out, self.pad[i] as usize);
            }
            out.push('\n');
        }
    }
}

/// Go's tabwriter over row ranges: a column is aligned across the run of
/// consecutive rows that reach it, and each run is then aligned one column
/// deeper. Only padding is recorded — cells are never rewritten.
fn align(rows: &[(u32, u32)], widths: &[u32], pad: &mut [u32], col: usize, lo: usize, hi: usize) {
    let reaches = |r: usize| (rows[r].1 - rows[r].0) as usize > col + 1;
    let mut start = lo;
    while start < hi {
        if !reaches(start) {
            start += 1;
            continue;
        }
        let end = (start..hi).find(|&i| !reaches(i)).unwrap_or(hi);
        let cell = |r: usize| rows[r].0 as usize + col;
        let width = (start..end)
            .map(|r| widths[cell(r)] + 2)
            .max()
            .expect("run is non-empty");
        for r in start..end {
            let i = cell(r);
            pad[i] = width - widths[i];
        }
        align(rows, widths, pad, col + 1, start, end);
        start = end;
    }
}

#[inline]
fn push_spaces(out: &mut String, mut n: usize) {
    while n > SPACES.len() {
        out.push_str(SPACES);
        n -= SPACES.len();
    }
    out.push_str(&SPACES[..n]);
}

#[cfg(test)]
mod tests {
    use super::tabbed;

    /// The pre-optimization tabwriter, kept verbatim as the oracle: the fast
    /// path is only correct if it agrees with this on every input.
    fn reference(text: &str) -> String {
        let escaped = text.replace('\x1b', "^[").replace('\r', "\\r");
        let mut output = String::new();
        let mut sections = escaped.split('\x0c').peekable();
        while let Some(section) = sections.next() {
            let mut rows: Vec<Vec<String>> = section
                .split_terminator('\n')
                .map(|line| line.split(['\t', '\x0b']).map(String::from).collect())
                .collect();
            ref_align(&mut rows, 0);
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

    fn ref_align(rows: &mut [Vec<String>], col: usize) {
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
            ref_align(&mut rows[start..end], col + 1);
            start = end;
        }
    }

    fn agree(text: &str) {
        assert_eq!(tabbed(text), reference(text), "input: {text:?}");
    }

    #[test]
    fn matches_reference_on_describe_shapes() {
        for text in [
            "",
            "\n",
            "\n\n",
            "a",
            "Name:\tnginx\nNamespace:\tdefault\n",
            // Ragged rows: a short row ends a column run.
            "a\tb\tc\nlonger\tb\nx\ty\tz\n",
            // Trailing cell is never padded.
            "a\tbbbbbb\nccccc\td\n",
            // Empty cells and consecutive separators.
            "\t\t\na\t\tb\n",
            // Vertical tab is a cell separator too.
            "a\x0bb\tc\n",
            // Form feed starts a new alignment section.
            "a\tb\n\x0clonger\tb\n",
            "a\tb\n\n\x0cc\td\n",
            "\x0c\x0c",
            // Control characters the writer renders literally.
            "a\x1b[31mred\tb\n",
            "line\r\nnext\tcell\n",
            "\x1b\r\x1b\r",
            // Multi-byte cells: width counts code points, not bytes.
            "ünïcödé\tb\nasciicell\tc\n",
            "日本語\tb\nxx\tc\n",
            // No trailing newline.
            "a\tb\nc\td",
        ] {
            agree(text);
        }
    }

    #[test]
    fn matches_reference_on_generated_tables() {
        // Deterministic pseudo-random tables over the alphabet that matters:
        // separators, newlines, section breaks, control chars and non-ASCII.
        let alphabet = [
            'a', 'Z', '0', ' ', '\t', '\n', '\x0b', '\x0c', '\x1b', '\r', 'é', '日',
        ];
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..2_000 {
            let len = (next() % 80) as usize;
            let text: String = (0..len)
                .map(|_| alphabet[(next() % alphabet.len() as u64) as usize])
                .collect();
            agree(&text);
        }
    }

    #[test]
    fn pads_runs_longer_than_the_space_buffer() {
        let wide = "x".repeat(200);
        agree(&format!("{wide}\tb\nshort\tc\n"));
    }
}
