//! Fork-only full transcript rendering for dynamic (function/custom) tool calls.

use super::dynamic_tool::DynamicToolCallCell;
use super::*;
use crate::line_truncation::truncate_line_to_width;
use crate::wrapping::adaptive_wrap_line;
use crate::wrapping::word_wrap_line;
use codex_app_server_protocol::DynamicToolCallOutputContentItem;

const TRANSCRIPT_MAX_ROWS: usize = u16::MAX as usize;
const TRANSCRIPT_CONTENT_MAX_ROWS: usize = TRANSCRIPT_MAX_ROWS - 128;
const TRANSCRIPT_ROW_LIMIT_MARKER: &str = "⚠ Transcript row limit reached; more output is hidden.";

struct TranscriptBuilder {
    lines: Vec<Line<'static>>,
    width: usize,
    truncated: bool,
}

impl TranscriptBuilder {
    fn new(width: u16) -> Self {
        Self {
            lines: Vec::new(),
            width: usize::from(width).max(1),
            truncated: false,
        }
    }

    fn remaining_content_rows(&self) -> usize {
        TRANSCRIPT_CONTENT_MAX_ROWS.saturating_sub(self.lines.len())
    }

    fn push(&mut self, line: Line<'static>) {
        if self.lines.len() < TRANSCRIPT_CONTENT_MAX_ROWS {
            self.lines.push(line);
        } else {
            self.truncated = true;
        }
    }

    fn push_wrapped_line(
        &mut self,
        line: Line<'static>,
        initial_indent: &'static str,
        subsequent_indent: &'static str,
    ) {
        let remaining = self.remaining_content_rows();
        if remaining == 0 {
            self.truncated = true;
            return;
        }

        let initial_indent = if self.width >= 3 { initial_indent } else { "" };
        let subsequent_indent = if self.width >= 3 {
            subsequent_indent
        } else {
            ""
        };

        // Bound source width before wrapping so a huge single-line payload cannot
        // allocate more wrapped rows than the remaining transcript budget.
        let max_source_cols = remaining
            .saturating_add(1)
            .saturating_mul(self.width.max(1));
        let line = if line.width() > max_source_cols {
            self.truncated = true;
            truncate_line_to_width(line, max_source_cols)
        } else {
            line
        };

        let wrapped = adaptive_wrap_line(
            &line,
            RtOptions::new(self.width)
                .initial_indent(initial_indent.into())
                .subsequent_indent(subsequent_indent.into()),
        );
        for line in wrapped {
            if self.remaining_content_rows() == 0 {
                self.truncated = true;
                break;
            }
            let line = line_to_static(&line);
            if line.width() > self.width {
                let nested_remaining = self.remaining_content_rows();
                let nested_max_cols = nested_remaining
                    .saturating_add(1)
                    .saturating_mul(self.width.max(1));
                let line = if line.width() > nested_max_cols {
                    self.truncated = true;
                    truncate_line_to_width(line, nested_max_cols)
                } else {
                    line
                };
                for line in word_wrap_line(&line, RtOptions::new(self.width)) {
                    if self.remaining_content_rows() == 0 {
                        self.truncated = true;
                        break;
                    }
                    self.push(line_to_static(&line));
                }
            } else {
                self.push(line);
            }
        }
    }

    /// Push one logical source line under the result/arguments indent, truncating
    /// the borrowed `&str` before allocating a `Line` when past the row budget.
    fn push_indented_source_line(&mut self, source_line: &str) {
        if self.truncated || self.remaining_content_rows() == 0 {
            self.truncated = true;
            return;
        }
        if source_line.is_empty() {
            self.push(Line::from(""));
            return;
        }

        let max_cols = self
            .remaining_content_rows()
            .saturating_add(1)
            .saturating_mul(self.width.max(1));
        let (prefix, rest, _) = take_prefix_by_width(source_line, max_cols);
        if !rest.is_empty() {
            self.truncated = true;
        }
        self.push_wrapped_line(Line::from(prefix), "  ", "  ");
    }

    fn push_block(&mut self, label: &str, content: &str) {
        if self.truncated {
            return;
        }
        self.push_wrapped_line(label.to_string().bold().into(), "", "");

        if content.is_empty() {
            return;
        }

        let content_without_trailing_newline = content.strip_suffix('\n').unwrap_or(content);
        for source_line in content_without_trailing_newline.split('\n') {
            self.push_indented_source_line(source_line);
            if self.truncated {
                break;
            }
        }
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        if self.truncated {
            let marker: Line<'static> = TRANSCRIPT_ROW_LIMIT_MARKER.red().bold().into();
            let wrapped = adaptive_wrap_line(&marker, RtOptions::new(self.width));
            self.lines
                .extend(wrapped.into_iter().map(|line| line_to_static(&line)));
        }
        self.lines
    }
}

pub(super) fn render(cell: &DynamicToolCallCell, width: u16) -> Vec<Line<'static>> {
    let status = match cell.success() {
        Some(true) => "succeeded",
        Some(false) => "failed",
        None => "in progress",
    };
    let mut builder = TranscriptBuilder::new(width);
    builder.push_wrapped_line("• Tool call".bold().into(), "", "");
    builder.push_wrapped_line(
        format!("Tool: {}", cell.qualified_name()).into(),
        "  ",
        "  ",
    );
    builder.push_wrapped_line(format!("Call ID: {}", cell.call_id()).into(), "  ", "  ");
    builder.push_wrapped_line(format!("Status: {status}").into(), "  ", "  ");
    if let Some(duration) = cell.duration() {
        builder.push_wrapped_line(format!("Duration: {duration:.2?}").into(), "  ", "  ");
    }

    let arguments = serde_json::to_string_pretty(&cell.arguments())
        .unwrap_or_else(|_| cell.arguments().to_string());
    builder.push_block("Arguments:", &arguments);

    if let Some(content_items) = cell.content_items() {
        // Detect upstream truncation markers without cloning every payload first.
        if content_items.iter().any(content_item_is_upstream_truncated) {
            builder.push_wrapped_line(
                "⚠ Saved output is already truncated upstream; missing content cannot be recovered."
                    .red()
                    .bold()
                    .into(),
                "",
                "",
            );
        }
        if content_items.is_empty() {
            builder.push_block("Result:", "<empty>");
        } else {
            // Stream items into the builder so the row budget can stop before we
            // allocate owned copies of the entire saved result on every Ctrl+T draw.
            builder.push_wrapped_line("Result:".to_string().bold().into(), "", "");
            for item in content_items {
                if builder.truncated || builder.remaining_content_rows() == 0 {
                    builder.truncated = true;
                    break;
                }
                push_content_item(&mut builder, item);
            }
        }
    } else if cell.success().is_some() {
        builder.push_block("Result:", "<none>");
    }

    builder.finish()
}

fn push_content_item(builder: &mut TranscriptBuilder, item: &DynamicToolCallOutputContentItem) {
    match item {
        DynamicToolCallOutputContentItem::InputText { text } => {
            let content = text.strip_suffix('\n').unwrap_or(text);
            for source_line in content.split('\n') {
                if builder.truncated {
                    break;
                }
                builder.push_indented_source_line(source_line);
            }
        }
        DynamicToolCallOutputContentItem::InputImage { image_url } => {
            builder.push_indented_source_line(&format!(
                "<image content: {} encoded bytes>",
                encoded_payload_len(image_url)
            ));
        }
        DynamicToolCallOutputContentItem::InputAudio { audio_url } => {
            builder.push_indented_source_line(&format!(
                "<audio content: {} encoded bytes>",
                encoded_payload_len(audio_url)
            ));
        }
    }
}

fn content_item_is_upstream_truncated(item: &DynamicToolCallOutputContentItem) -> bool {
    match item {
        DynamicToolCallOutputContentItem::InputText { text } => saved_output_is_truncated(text),
        // Media metadata placeholders do not carry the original truncated body.
        DynamicToolCallOutputContentItem::InputImage { .. }
        | DynamicToolCallOutputContentItem::InputAudio { .. } => false,
    }
}

fn encoded_payload_len(url_or_data: &str) -> usize {
    url_or_data
        .rsplit_once(',')
        .map(|(_, payload)| payload.len())
        .unwrap_or(url_or_data.len())
}

fn saved_output_is_truncated(text: &str) -> bool {
    codex_session_inspector::text_contains_truncation_marker(text)
}
