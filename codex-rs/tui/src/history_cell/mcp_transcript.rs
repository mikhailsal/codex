//! Fork-only full transcript rendering for persisted MCP tool calls.

use super::*;

pub(super) fn render(cell: &McpToolCallCell, _width: u16) -> Vec<Line<'static>> {
    let status = match cell.success() {
        Some(true) => "succeeded",
        Some(false) => "failed",
        None => "in progress",
    };
    let mut lines = vec![
        "• MCP tool call".bold().into(),
        format!(
            "  Tool: {}.{}",
            cell.invocation.server, cell.invocation.tool
        )
        .into(),
        format!("  Call ID: {}", cell.call_id).into(),
        format!("  Status: {status}").into(),
    ];
    if let Some(duration) = cell.duration {
        lines.push(format!("  Duration: {duration:.2?}").into());
    }

    let arguments = cell
        .invocation
        .arguments
        .as_ref()
        .map(|arguments| {
            serde_json::to_string_pretty(arguments).unwrap_or_else(|_| arguments.to_string())
        })
        .unwrap_or_else(|| "<none>".to_string());
    push_block(&mut lines, "Arguments:", &arguments);

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
                    lines.push(
                        "⚠ Saved output is already truncated upstream; missing content cannot be recovered."
                            .red()
                            .bold()
                            .into(),
                    );
                }
                push_block(&mut lines, "Result:", &rendered_content.join("\n"));
                if let Some(structured_content) = structured_content {
                    let content = serde_json::to_string_pretty(structured_content)
                        .unwrap_or_else(|_| structured_content.to_string());
                    push_block(&mut lines, "Structured content:", &content);
                }
            }
            Err(err) => push_block(&mut lines, "Result:", &format!("Error: {err}")),
        }
    }

    lines
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

fn push_block(lines: &mut Vec<Line<'static>>, label: &str, content: &str) {
    lines.push(label.to_string().bold().into());
    lines.extend(raw_lines_from_source(content).into_iter().map(|line| {
        if line.width() == 0 {
            return Line::from("");
        }
        let mut spans = vec!["  ".into()];
        spans.extend(line.spans);
        Line::from(spans)
    }));
}
