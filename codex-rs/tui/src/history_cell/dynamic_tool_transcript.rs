//! Fork-only full transcript rendering for dynamic (function/custom) tool calls.

use super::lazy_transcript::LazyTranscript;
use super::*;
use codex_app_server_protocol::DynamicToolCallOutputContentItem;

pub(super) fn build(cell: &DynamicToolCallCell, width: u16) -> LazyTranscript {
    let width = usize::from(width).max(1);
    let status = match cell.success() {
        Some(true) => "succeeded",
        Some(false) => "failed",
        None => "in progress",
    };
    let mut doc = LazyTranscript::new();
    doc.push_wrapped_line("• Tool call".bold().into(), width, "", "");
    doc.push_wrapped_line(
        format!("Tool: {}", cell.qualified_name()).into(),
        width,
        "  ",
        "  ",
    );
    doc.push_wrapped_line(
        format!("Call ID: {}", cell.call_id()).into(),
        width,
        "  ",
        "  ",
    );
    doc.push_wrapped_line(format!("Status: {status}").into(), width, "  ", "  ");
    if let Some(duration) = cell.duration() {
        doc.push_wrapped_line(
            format!("Duration: {duration:.2?}").into(),
            width,
            "  ",
            "  ",
        );
    }

    let arguments = serde_json::to_string_pretty(&cell.arguments())
        .unwrap_or_else(|_| cell.arguments().to_string());
    doc.push_text_block("Arguments:", &arguments, width);

    if let Some(content_items) = cell.content_items() {
        if content_items.iter().any(content_item_is_upstream_truncated) {
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
        if content_items.is_empty() {
            doc.push_text_block("Result:", "<empty>", width);
        } else {
            doc.push_wrapped_line("Result:".to_string().bold().into(), width, "", "");
            push_content_items(&mut doc, content_items, width);
        }
    } else if cell.success().is_some() {
        doc.push_text_block("Result:", "<none>", width);
    }

    doc
}

pub(super) fn render(cell: &DynamicToolCallCell, width: u16) -> Vec<Line<'static>> {
    build(cell, width).materialize(width)
}

fn push_content_items(
    doc: &mut LazyTranscript,
    items: &[DynamicToolCallOutputContentItem],
    width: usize,
) {
    let mut text_buf = String::new();
    for item in items {
        match item {
            DynamicToolCallOutputContentItem::InputText { text } => {
                if !text_buf.is_empty() {
                    text_buf.push('\n');
                }
                text_buf.push_str(text.strip_suffix('\n').unwrap_or(text));
            }
            DynamicToolCallOutputContentItem::InputImage { image_url } => {
                flush_lazy_text(doc, &mut text_buf);
                doc.push_wrapped_line(
                    format!(
                        "<image content: {} encoded bytes>",
                        encoded_payload_len(image_url)
                    )
                    .into(),
                    width,
                    "  ",
                    "  ",
                );
            }
            DynamicToolCallOutputContentItem::InputAudio { audio_url } => {
                flush_lazy_text(doc, &mut text_buf);
                doc.push_wrapped_line(
                    format!(
                        "<audio content: {} encoded bytes>",
                        encoded_payload_len(audio_url)
                    )
                    .into(),
                    width,
                    "  ",
                    "  ",
                );
            }
        }
    }
    flush_lazy_text(doc, &mut text_buf);
}

fn flush_lazy_text(doc: &mut LazyTranscript, text_buf: &mut String) {
    if text_buf.is_empty() {
        return;
    }
    let content = std::mem::take(text_buf);
    doc.push_lazy_body(&content, "  ");
}

fn content_item_is_upstream_truncated(item: &DynamicToolCallOutputContentItem) -> bool {
    match item {
        DynamicToolCallOutputContentItem::InputText { text } => saved_output_is_truncated(text),
        DynamicToolCallOutputContentItem::InputImage { .. }
        | DynamicToolCallOutputContentItem::InputAudio { .. } => false,
    }
}

fn encoded_payload_len(value: &str) -> usize {
    value.split(',').next_back().unwrap_or(value).trim().len()
}

fn saved_output_is_truncated(text: &str) -> bool {
    codex_session_inspector::text_contains_truncation_marker(text)
}
