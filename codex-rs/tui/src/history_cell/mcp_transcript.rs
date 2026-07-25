//! Fork-only full transcript rendering for persisted MCP tool calls.

use super::*;
use crate::wrapping::adaptive_wrap_line;
use crate::wrapping::word_wrap_line;

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

pub(super) fn render(cell: &McpToolCallCell, width: u16) -> Vec<Line<'static>> {
    let status = match cell.success() {
        Some(true) => "succeeded",
        Some(false) => "failed",
        None => "in progress",
    };
    let mut builder = TranscriptBuilder::new(width);
    builder.push_wrapped_line("• MCP tool call".bold().into(), "", "");
    builder.push_wrapped_line(
        format!("Tool: {}.{}", cell.invocation.server, cell.invocation.tool).into(),
        "  ",
        "  ",
    );
    builder.push_wrapped_line(format!("Call ID: {}", cell.call_id).into(), "  ", "  ");
    builder.push_wrapped_line(format!("Status: {status}").into(), "  ", "  ");
    if let Some(duration) = cell.duration {
        builder.push_wrapped_line(format!("Duration: {duration:.2?}").into(), "  ", "  ");
    }

    let arguments = cell
        .invocation
        .arguments
        .as_ref()
        .map(|arguments| {
            serde_json::to_string_pretty(arguments).unwrap_or_else(|_| arguments.to_string())
        })
        .unwrap_or_else(|| "<none>".to_string());
    builder.push_block("Arguments:", &arguments);

    if let Some(result) = &cell.result {
        match result {
            Ok(codex_protocol::mcp::CallToolResult {
                content,
                structured_content,
                ..
            }) => {
                let rendered_content = content.iter().map(render_content_block).collect::<Vec<_>>();
                if rendered_content
                    .iter()
                    .any(|text| saved_output_is_truncated(text))
                    || structured_content
                        .as_ref()
                        .is_some_and(|content| saved_output_is_truncated(&content.to_string()))
                {
                    builder.push_wrapped_line(
                        "⚠ Saved output is already truncated upstream; missing content cannot be recovered."
                            .red()
                            .bold()
                            .into(),
                        "",
                        "",
                    );
                }
                builder.push_block("Result:", &rendered_content.join("\n"));
                if let Some(structured_content) = structured_content {
                    let content = serde_json::to_string_pretty(structured_content)
                        .unwrap_or_else(|_| structured_content.to_string());
                    builder.push_block("Structured content:", &content);
                }
            }
            Err(err) => {
                if saved_output_is_truncated(err) {
                    builder.push_wrapped_line(
                        "⚠ Saved output is already truncated upstream; missing content cannot be recovered."
                            .red()
                            .bold()
                            .into(),
                        "",
                        "",
                    );
                }
                builder.push_block("Result:", &format!("Error: {err}"));
            }
        }
    }

    builder.finish()
}

fn render_content_block(block: &serde_json::Value) -> String {
    let Ok(content) = serde_json::from_value::<rmcp::model::Content>(block.clone()) else {
        return serde_json::to_string_pretty(block).unwrap_or_else(|_| block.to_string());
    };

    match content.raw {
        rmcp::model::RawContent::Text(text) => text.text,
        rmcp::model::RawContent::Image(image) => {
            format!(
                "<image content: {}, {} encoded bytes>",
                image.mime_type,
                image.data.len()
            )
        }
        rmcp::model::RawContent::Audio(audio) => {
            format!(
                "<audio content: {}, {} encoded bytes>",
                audio.mime_type,
                audio.data.len()
            )
        }
        rmcp::model::RawContent::Resource(resource) => match resource.resource {
            rmcp::model::ResourceContents::TextResourceContents { uri, text, .. } => {
                format!("embedded resource: {uri}\n{text}")
            }
            rmcp::model::ResourceContents::BlobResourceContents {
                uri,
                blob,
                mime_type,
                ..
            } => {
                let mime_type = mime_type.as_deref().unwrap_or("unknown MIME type");
                format!(
                    "<embedded binary resource: {uri}, {mime_type}, {} encoded bytes>",
                    blob.len()
                )
            }
        },
        rmcp::model::RawContent::ResourceLink(link) => format!("link: {}", link.uri),
    }
}

fn saved_output_is_truncated(text: &str) -> bool {
    text.contains("Warning: truncated output")
        || text.contains(" tokens truncated…")
        || text.contains(" chars truncated…")
        || text.contains(" bytes truncated…")
        || text.contains("…output truncated…")
}
