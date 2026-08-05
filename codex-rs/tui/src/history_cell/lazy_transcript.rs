//! Lazy transcript documents for large tool outputs in `Ctrl+T`.
//!
//! Fork-only helper: keep header metadata as ordinary already-wrapped lines, store large
//! bodies as plain text, and materialize wrapped `Line`s only for the requested row window.
//! The pager can measure height and paint a viewport without retaining a full `Vec<Line>`
//! for every draw.

use super::*;

pub(crate) const TRANSCRIPT_MAX_ROWS: usize = u16::MAX as usize;
pub(crate) const TRANSCRIPT_CONTENT_MAX_ROWS: usize = TRANSCRIPT_MAX_ROWS - 128;
pub(crate) const TRANSCRIPT_ROW_LIMIT_MARKER: &str =
    "⚠ Transcript row limit reached; more output is hidden.";

enum LazyPart {
    /// Small already-wrapped fragments (titles, banners, short metadata). Each entry is one row.
    Lines(Vec<Line<'static>>),
    /// Large body retained as text and wrapped on demand.
    Text { text: String, indent: &'static str },
}

/// Width-aware transcript that can answer row counts and windows without keeping all lines.
pub(crate) struct LazyTranscript {
    parts: Vec<LazyPart>,
}

impl LazyTranscript {
    pub(crate) fn new() -> Self {
        Self { parts: Vec::new() }
    }

    pub(crate) fn push_row(&mut self, line: Line<'static>) {
        match self.parts.last_mut() {
            Some(LazyPart::Lines(lines)) => lines.push(line),
            _ => self.parts.push(LazyPart::Lines(vec![line])),
        }
    }

    pub(crate) fn push_wrapped_line(
        &mut self,
        line: Line<'static>,
        width: usize,
        initial_indent: &'static str,
        subsequent_indent: &'static str,
    ) {
        for wrapped in wrap_line_to_width(line, width, initial_indent, subsequent_indent) {
            self.push_row(wrapped);
        }
    }

    pub(crate) fn push_text_block(&mut self, label: &str, content: &str, width: usize) {
        self.push_wrapped_line(label.to_string().bold().into(), width, "", "");
        self.push_lazy_body(content, "  ");
    }

    /// Store a large body for on-demand wrapping (no `Line` allocation until windowed).
    pub(crate) fn push_lazy_body(&mut self, content: &str, indent: &'static str) {
        let content = content.strip_suffix('\n').unwrap_or(content);
        if content.is_empty() {
            return;
        }
        self.parts.push(LazyPart::Text {
            text: content.to_string(),
            indent,
        });
    }

    pub(crate) fn row_count(&self, width: u16) -> usize {
        let width = usize::from(width).max(1);
        let (content_rows, truncated) = self.content_rows(width);
        let mut rows = content_rows.min(TRANSCRIPT_CONTENT_MAX_ROWS);
        if truncated {
            rows = rows.saturating_add(marker_row_count(width));
        }
        rows.min(TRANSCRIPT_MAX_ROWS)
    }

    pub(crate) fn lines_window(
        &self,
        width: u16,
        start_row: usize,
        max_rows: usize,
    ) -> Vec<Line<'static>> {
        if max_rows == 0 {
            return Vec::new();
        }
        let width = usize::from(width).max(1);
        let (content_rows, truncated) = self.content_rows(width);
        let content_rows = content_rows.min(TRANSCRIPT_CONTENT_MAX_ROWS);

        let mut out = Vec::with_capacity(max_rows.min(256));
        let end_row = start_row.saturating_add(max_rows);

        if start_row < content_rows {
            self.emit_content_window(width, start_row, end_row.min(content_rows), &mut out);
        }

        if truncated && out.len() < max_rows {
            let marker = marker_lines(width);
            let marker_start = content_rows;
            let marker_end = marker_start.saturating_add(marker.len());
            let from = start_row.max(marker_start);
            let to = end_row.min(marker_end);
            if from < to {
                out.extend(marker.into_iter().skip(from - marker_start).take(to - from));
            }
        }

        out
    }

    pub(crate) fn materialize(&self, width: u16) -> Vec<Line<'static>> {
        self.lines_window(width, 0, TRANSCRIPT_MAX_ROWS)
    }

    fn content_rows(&self, width: usize) -> (usize, bool) {
        let mut rows = 0usize;
        for part in &self.parts {
            let remaining = TRANSCRIPT_CONTENT_MAX_ROWS.saturating_sub(rows);
            if remaining == 0 {
                return (TRANSCRIPT_CONTENT_MAX_ROWS, true);
            }
            match part {
                LazyPart::Lines(lines) => {
                    if lines.len() > remaining {
                        return (TRANSCRIPT_CONTENT_MAX_ROWS, true);
                    }
                    rows += lines.len();
                }
                LazyPart::Text { text, indent } => {
                    let (count, overflow) = text_row_count(text, width, indent, remaining);
                    rows += count;
                    if overflow {
                        return (TRANSCRIPT_CONTENT_MAX_ROWS, true);
                    }
                }
            }
        }
        (rows, false)
    }

    fn emit_content_window(
        &self,
        width: usize,
        start_row: usize,
        end_row: usize,
        out: &mut Vec<Line<'static>>,
    ) {
        let mut row = 0usize;
        for part in &self.parts {
            if row >= end_row {
                break;
            }
            match part {
                LazyPart::Lines(lines) => {
                    let part_end = row + lines.len();
                    if part_end <= start_row {
                        row = part_end;
                        continue;
                    }
                    let skip = start_row.saturating_sub(row);
                    let take = end_row.saturating_sub(row.max(start_row));
                    out.extend(lines.iter().skip(skip).take(take).cloned());
                    row = part_end;
                }
                LazyPart::Text { text, indent } => {
                    let skip = start_row.saturating_sub(row);
                    let take = end_row.saturating_sub(row.max(start_row));
                    let consumed = emit_text_rows(text, width, indent, skip, take, out);
                    row = row.saturating_add(consumed);
                }
            }
        }
    }
}

/// Returns `(rows_taken, overflowed_budget)`.
fn text_row_count(text: &str, width: usize, indent: &'static str, limit: usize) -> (usize, bool) {
    if limit == 0 {
        return (0, true);
    }
    let mut count = 0usize;
    let mut iter = text.split('\n').peekable();
    while let Some(source_line) = iter.next() {
        let line_rows = source_line_row_count(source_line, width, indent, limit - count);
        if count + line_rows > limit {
            return (limit, true);
        }
        count += line_rows;
        if count == limit {
            // Either this source line was capped, or more source remains.
            let capped = source_line_was_capped(source_line, width, indent, line_rows);
            return (limit, capped || iter.peek().is_some());
        }
    }
    (count, false)
}

fn source_line_was_capped(
    source_line: &str,
    width: usize,
    indent: &'static str,
    taken: usize,
) -> bool {
    if source_line.is_empty() {
        return false;
    }
    source_line_row_count(source_line, width, indent, taken.saturating_add(1)) > taken
}

fn source_line_row_count(
    source_line: &str,
    width: usize,
    indent: &'static str,
    limit: usize,
) -> usize {
    if limit == 0 {
        return 0;
    }
    wrap_source_line(source_line, width, indent, limit).len()
}

fn emit_text_rows(
    text: &str,
    width: usize,
    indent: &'static str,
    skip: usize,
    take: usize,
    out: &mut Vec<Line<'static>>,
) -> usize {
    let need = skip.saturating_add(take);
    if need == 0 {
        return 0;
    }
    let mut index = 0usize;
    let mut emitted = 0usize;
    for source_line in text.split('\n') {
        let wrapped = wrap_source_line(source_line, width, indent, usize::MAX);
        for line in wrapped {
            if index >= skip && emitted < take {
                out.push(line);
                emitted += 1;
            }
            index += 1;
            if emitted >= take && index >= need {
                return index;
            }
        }
        if emitted >= take && index >= skip {
            return index;
        }
    }
    index
}

fn wrap_source_line(
    source_line: &str,
    width: usize,
    indent: &'static str,
    limit: usize,
) -> Vec<Line<'static>> {
    if limit == 0 {
        return Vec::new();
    }
    if source_line.is_empty() {
        return vec![Line::from("")];
    }
    let indent = if width >= 3 { indent } else { "" };
    let max_source_cols = limit.saturating_add(1).saturating_mul(width.max(1));
    let (prefix, _rest, _) = take_prefix_by_width(source_line, max_source_cols);
    let mut lines = wrap_line_to_width(Line::from(prefix), width, indent, indent);
    if lines.len() > limit {
        lines.truncate(limit);
    }
    if lines.is_empty() {
        lines.push(Line::from(""));
    }
    lines
}

fn wrap_line_to_width(
    line: Line<'static>,
    width: usize,
    initial_indent: &'static str,
    subsequent_indent: &'static str,
) -> Vec<Line<'static>> {
    let initial_indent = if width >= 3 { initial_indent } else { "" };
    let subsequent_indent = if width >= 3 { subsequent_indent } else { "" };
    let mut out = Vec::new();
    let wrapped = adaptive_wrap_line(
        &line,
        RtOptions::new(width)
            .initial_indent(initial_indent.into())
            .subsequent_indent(subsequent_indent.into()),
    );
    for line in wrapped {
        let line = line_to_static(&line);
        if line.width() > width {
            for nested in crate::wrapping::word_wrap_line(&line, RtOptions::new(width)) {
                out.push(line_to_static(&nested));
            }
        } else {
            out.push(line);
        }
    }
    out
}

fn marker_lines(width: usize) -> Vec<Line<'static>> {
    let marker: Line<'static> = TRANSCRIPT_ROW_LIMIT_MARKER.red().bold().into();
    adaptive_wrap_line(&marker, RtOptions::new(width.max(1)))
        .into_iter()
        .map(|line| line_to_static(&line))
        .collect()
}

fn marker_row_count(width: usize) -> usize {
    marker_lines(width).len().max(1)
}

#[cfg(test)]
#[path = "lazy_transcript_tests.rs"]
mod tests;
