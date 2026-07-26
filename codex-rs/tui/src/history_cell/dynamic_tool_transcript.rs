//! Fork-only full transcript rendering for dynamic (function/custom) tool calls.

use super::dynamic_tool::DynamicToolCallCell;
use super::*;
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
        let initial_indent = if self.width >= 3 { initial_indent } else { "" };
        let subsequent_indent = if self.width >= 3 {
            subsequent_indent
        } else {
            ""
        };
        let wrapped = adaptive_wrap_line(
            &line,
            RtOptions::new(self.width)
                .initial_indent(initial_indent.into())
                .subsequent_indent(subsequent_indent.into()),
        );
        for line in wrapped {
            let line = line_to_static(&line);
            if line.width() > self.width {
                for line in word_wrap_line(&line, RtOptions::new(self.width)) {
                    self.push(line_to_static(&line));
                }
            } else {
                self.push(line);
            }
        }
    }

    fn push_block(&mut self, label: &str, content: &str) {
        self.push_wrapped_line(label.to_string().bold().into(), "", "");

        if content.is_empty() {
            return;
        }

        let content_without_trailing_newline = content.strip_suffix('\n').unwrap_or(content);
        for source_line in content_without_trailing_newline.split('\n') {
            if source_line.is_empty() {
                self.push(Line::from(""));
            } else {
                self.push_wrapped_line(Line::from(source_line.to_string()), "  ", "  ");
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
        let rendered = content_items
            .iter()
            .map(render_content_item)
            .collect::<Vec<_>>();
        if rendered.iter().any(|text| saved_output_is_truncated(text)) {
            builder.push_wrapped_line(
                "⚠ Saved output is already truncated upstream; missing content cannot be recovered."
                    .red()
                    .bold()
                    .into(),
                "",
                "",
            );
        }
        if rendered.is_empty() {
            builder.push_block("Result:", "<empty>");
        } else {
            builder.push_block("Result:", &rendered.join("\n"));
        }
    } else if cell.success().is_some() {
        builder.push_block("Result:", "<none>");
    }

    builder.finish()
}

fn render_content_item(item: &DynamicToolCallOutputContentItem) -> String {
    match item {
        DynamicToolCallOutputContentItem::InputText { text } => text.clone(),
        DynamicToolCallOutputContentItem::InputImage { image_url } => {
            format!(
                "<image content: {} encoded bytes>",
                encoded_payload_len(image_url)
            )
        }
        DynamicToolCallOutputContentItem::InputAudio { audio_url } => {
            format!(
                "<audio content: {} encoded bytes>",
                encoded_payload_len(audio_url)
            )
        }
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
