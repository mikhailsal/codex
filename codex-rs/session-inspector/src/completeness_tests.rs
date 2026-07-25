use pretty_assertions::assert_eq;

use super::*;
use crate::ToolResult;
use crate::ToolResultBody;
use codex_protocol::models::FunctionCallOutputContentItem;

#[test]
fn detects_warning_header_with_token_count() {
    let text = "Warning: truncated output (original token count: 42)\nTotal output lines: 9\n\nhead…3 tokens truncated…tail";
    let markers = detect_truncation_markers(text);

    assert_eq!(
        markers
            .iter()
            .map(|marker| (marker.kind, marker.count, marker.byte_offset))
            .collect::<Vec<_>>(),
        vec![
            (TruncationMarkerKind::WarningTruncatedOutput, Some(42), 0),
            (
                TruncationMarkerKind::TokensTruncated,
                Some(3),
                text.find('…').unwrap()
            ),
        ]
    );
    assert!(Completeness::Truncated { markers }.is_truncated());
}

#[test]
fn detects_chars_and_legacy_bytes_ellipsis_markers() {
    let chars = "prefix…12 chars truncated…suffix";
    let bytes = "prefix…7 bytes truncated…suffix";

    assert_eq!(
        detect_truncation_markers(chars)[0].kind,
        TruncationMarkerKind::CharsTruncated
    );
    assert_eq!(detect_truncation_markers(chars)[0].count, Some(12));
    assert_eq!(
        detect_truncation_markers(bytes)[0].kind,
        TruncationMarkerKind::BytesTruncated
    );
    assert_eq!(detect_truncation_markers(bytes)[0].count, Some(7));
}

#[test]
fn detects_unified_exec_bytes_omitted_marker() {
    let text = "HEAD\n... 123456 bytes omitted ...\nTAIL";
    let markers = detect_truncation_markers(text);

    assert_eq!(markers.len(), 1);
    assert_eq!(markers[0].kind, TruncationMarkerKind::BytesOmitted);
    assert_eq!(markers[0].count, Some(123_456));
    assert_eq!(markers[0].matched_text, "... 123456 bytes omitted ...");
}

#[test]
fn detects_omitted_content_item_markers() {
    let text_items = "[omitted 3 text items ...]";
    let audio_items = "[omitted 2 audio items ...]";

    assert_eq!(
        detect_truncation_markers(text_items)[0].kind,
        TruncationMarkerKind::OmittedTextItems
    );
    assert_eq!(detect_truncation_markers(text_items)[0].count, Some(3));
    assert_eq!(
        detect_truncation_markers(audio_items)[0].kind,
        TruncationMarkerKind::OmittedAudioItems
    );
    assert_eq!(detect_truncation_markers(audio_items)[0].count, Some(2));
}

#[test]
fn detects_context_window_and_handoff_markers() {
    let context = "Output exceeded the available model context and was truncated";
    let handoff = "before\n…output truncated…\nafter";

    assert_eq!(
        detect_truncation_markers(context)[0].kind,
        TruncationMarkerKind::ContextWindowTruncated
    );
    assert_eq!(detect_truncation_markers(context)[0].count, None);
    assert_eq!(
        detect_truncation_markers(handoff)[0].kind,
        TruncationMarkerKind::OutputTruncatedEllipsis
    );
}

#[test]
fn complete_text_has_no_markers() {
    let text = "hello λ world\nno truncation here";
    assert_eq!(detect_truncation_markers(text), Vec::new());
    assert_eq!(
        assess_tool_result(&ToolResult {
            raw: serde_json::json!(text),
            body: ToolResultBody::Text(text.to_string()),
        }),
        Completeness::Complete
    );
    assert!(!text_contains_truncation_marker(text));
}

#[test]
fn does_not_treat_telemetry_preview_notice_as_rollout_truncation() {
    let text = "preview\n[... telemetry preview truncated ...]";
    assert_eq!(detect_truncation_markers(text), Vec::new());
}

#[test]
fn assesses_content_items_per_text_block() {
    let result = ToolResult {
        raw: serde_json::json!([]),
        body: ToolResultBody::ContentItems(vec![
            FunctionCallOutputContentItem::InputText {
                text: "ok".to_string(),
            },
            FunctionCallOutputContentItem::InputText {
                text: "[omitted 1 text items ...]".to_string(),
            },
            FunctionCallOutputContentItem::InputImage {
                image_url: "data:image/png;base64,abc".to_string(),
                detail: None,
            },
        ]),
    };

    let completeness = assess_tool_result(&result);
    assert!(completeness.is_truncated());
    let Completeness::Truncated { markers } = completeness else {
        panic!("expected truncated");
    };
    assert_eq!(markers[0].kind, TruncationMarkerKind::OmittedTextItems);
}

#[test]
fn image_only_content_items_are_complete() {
    let result = ToolResult {
        raw: serde_json::json!([]),
        body: ToolResultBody::ContentItems(vec![FunctionCallOutputContentItem::InputImage {
            image_url: "data:image/png;base64,abc".to_string(),
            detail: None,
        }]),
    };

    assert_eq!(assess_tool_result(&result), Completeness::Complete);
}

#[test]
fn partial_prefix_without_digits_is_not_a_marker() {
    assert_eq!(
        detect_truncation_markers("Warning: truncated output (original token count: )"),
        Vec::new()
    );
    assert_eq!(detect_truncation_markers("… tokens truncated…"), Vec::new());
    assert_eq!(
        detect_truncation_markers("... bytes omitted ..."),
        Vec::new()
    );
}

#[test]
fn count_without_leading_ellipsis_is_not_a_chars_marker() {
    // The old TUI heuristic matched the suffix alone and false-positived on this.
    assert_eq!(
        detect_truncation_markers("backend failed: 100 chars truncated…"),
        Vec::new()
    );
}
