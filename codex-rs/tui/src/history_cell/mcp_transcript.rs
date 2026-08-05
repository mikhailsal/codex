//! Fork-only full transcript rendering for persisted MCP tool calls.

use super::lazy_transcript::LazyTranscript;
use super::*;

pub(super) fn build(cell: &McpToolCallCell, width: u16) -> LazyTranscript {
    let width = usize::from(width).max(1);
    let status = match cell.success() {
        Some(true) => "succeeded",
        Some(false) => "failed",
        None => "in progress",
    };
    let mut doc = LazyTranscript::new();
    doc.push_wrapped_line("• MCP tool call".bold().into(), width, "", "");
    doc.push_wrapped_line(
        format!("Tool: {}.{}", cell.invocation.server, cell.invocation.tool).into(),
        width,
        "  ",
        "  ",
    );
    doc.push_wrapped_line(
        format!("Call ID: {}", cell.call_id).into(),
        width,
        "  ",
        "  ",
    );
    doc.push_wrapped_line(format!("Status: {status}").into(), width, "  ", "  ");
    if let Some(duration) = cell.duration {
        doc.push_wrapped_line(
            format!("Duration: {duration:.2?}").into(),
            width,
            "  ",
            "  ",
        );
    }

    let arguments = cell
        .invocation
        .arguments
        .as_ref()
        .map(|arguments| {
            serde_json::to_string_pretty(arguments).unwrap_or_else(|_| arguments.to_string())
        })
        .unwrap_or_else(|| "<none>".to_string());
    doc.push_text_block("Arguments:", &arguments, width);

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
                    doc.push_wrapped_line(
                        "⚠ Saved output is already truncated upstream; missing content cannot be recovered."
                            .red()
                            .bold()
                            .into(),
                        width,
                        "",
                        "",
                    );
                }
                // Keep the joined result as one lazy text body so Ctrl+T can window it.
                doc.push_text_block("Result:", &rendered_content.join("\n"), width);
                if let Some(structured_content) = structured_content {
                    let content = serde_json::to_string_pretty(structured_content)
                        .unwrap_or_else(|_| structured_content.to_string());
                    doc.push_text_block("Structured content:", &content, width);
                }
            }
            Err(err) => {
                if saved_output_is_truncated(err) {
                    doc.push_wrapped_line(
                        "⚠ Saved output is already truncated upstream; missing content cannot be recovered."
                            .red()
                            .bold()
                            .into(),
                        width,
                        "",
                        "",
                    );
                }
                doc.push_text_block("Result:", &format!("Error: {err}"), width);
            }
        }
    }

    doc
}

pub(super) fn render(cell: &McpToolCallCell, width: u16) -> Vec<Line<'static>> {
    build(cell, width).materialize(width)
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
    codex_session_inspector::text_contains_truncation_marker(text)
}
