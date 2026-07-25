//! Detect whether a persisted tool payload already lost content upstream.
//!
//! Codex writers inject distinctive markers when they discard tool or exec
//! output before the payload reaches a session rollout. This module recognizes
//! those markers so viewers can show an honest completeness signal.
//!
//! Absence of a known marker means the *stored* text does not advertise
//! truncation. It does not prove the original tool output was smaller than
//! every possible limit, and it can false-positive when a payload merely quotes
//! one of these marker strings.

use codex_protocol::models::FunctionCallOutputContentItem;

use crate::ToolResult;
use crate::ToolResultBody;

/// Whether a persisted tool result still contains everything Codex wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Completeness {
    /// No known Codex truncation marker was found in scannable text.
    Complete,
    /// At least one known Codex truncation marker was found.
    Truncated { markers: Vec<TruncationMarker> },
    /// Completeness cannot be assessed (no persisted result).
    Unknown,
}

impl Completeness {
    pub fn is_truncated(&self) -> bool {
        matches!(self, Self::Truncated { .. })
    }
}

/// One match of a known Codex truncation marker inside persisted text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TruncationMarker {
    pub kind: TruncationMarkerKind,
    pub matched_text: String,
    pub byte_offset: usize,
    pub count: Option<u64>,
}

/// Families of markers Codex writers inject when discarding content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TruncationMarkerKind {
    /// `Warning: truncated output (original token count: N)`
    WarningTruncatedOutput,
    /// `…N tokens truncated…`
    TokensTruncated,
    /// `…N chars truncated…`
    CharsTruncated,
    /// Legacy `…N bytes truncated…` spelling that older sessions may contain.
    BytesTruncated,
    /// `... N bytes omitted ...` from the unified-exec head/tail buffer.
    BytesOmitted,
    /// `[omitted N text items ...]`
    OmittedTextItems,
    /// `[omitted N audio items ...]`
    OmittedAudioItems,
    /// Compaction replacement for oversized function outputs.
    ContextWindowTruncated,
    /// Realtime handoff marker `…output truncated…`.
    OutputTruncatedEllipsis,
}

/// Returns `true` when `text` contains at least one known truncation marker.
pub fn text_contains_truncation_marker(text: &str) -> bool {
    !detect_truncation_markers(text).is_empty()
}

/// Scan `text` for known Codex truncation markers, ordered by byte offset.
pub fn detect_truncation_markers(text: &str) -> Vec<TruncationMarker> {
    let mut markers = Vec::new();

    scan_counted(
        text,
        "Warning: truncated output (original token count: ",
        ")",
        TruncationMarkerKind::WarningTruncatedOutput,
        &mut markers,
    );
    scan_counted(
        text,
        "…",
        " tokens truncated…",
        TruncationMarkerKind::TokensTruncated,
        &mut markers,
    );
    scan_counted(
        text,
        "…",
        " chars truncated…",
        TruncationMarkerKind::CharsTruncated,
        &mut markers,
    );
    scan_counted(
        text,
        "…",
        " bytes truncated…",
        TruncationMarkerKind::BytesTruncated,
        &mut markers,
    );
    scan_counted(
        text,
        "... ",
        " bytes omitted ...",
        TruncationMarkerKind::BytesOmitted,
        &mut markers,
    );
    scan_counted(
        text,
        "[omitted ",
        " text items ...]",
        TruncationMarkerKind::OmittedTextItems,
        &mut markers,
    );
    scan_counted(
        text,
        "[omitted ",
        " audio items ...]",
        TruncationMarkerKind::OmittedAudioItems,
        &mut markers,
    );
    scan_fixed(
        text,
        "Output exceeded the available model context and was truncated",
        TruncationMarkerKind::ContextWindowTruncated,
        &mut markers,
    );
    scan_fixed(
        text,
        "…output truncated…",
        TruncationMarkerKind::OutputTruncatedEllipsis,
        &mut markers,
    );

    markers.sort_by_key(|marker| marker.byte_offset);
    markers
}

/// Assess completeness for a normalized tool result body.
pub fn assess_tool_result(result: &ToolResult) -> Completeness {
    match &result.body {
        ToolResultBody::Text(text) => assess_text(text),
        ToolResultBody::ContentItems(items) => assess_content_items(items),
    }
}

fn assess_text(text: &str) -> Completeness {
    let markers = detect_truncation_markers(text);
    if markers.is_empty() {
        Completeness::Complete
    } else {
        Completeness::Truncated { markers }
    }
}

fn assess_content_items(items: &[FunctionCallOutputContentItem]) -> Completeness {
    let mut markers = Vec::new();
    for item in items {
        if let FunctionCallOutputContentItem::InputText { text } = item {
            markers.extend(detect_truncation_markers(text));
        }
    }
    if markers.is_empty() {
        Completeness::Complete
    } else {
        Completeness::Truncated { markers }
    }
}

fn scan_counted(
    text: &str,
    prefix: &str,
    suffix: &str,
    kind: TruncationMarkerKind,
    out: &mut Vec<TruncationMarker>,
) {
    let mut search_from = 0;
    while let Some(relative) = text[search_from..].find(prefix) {
        let start = search_from + relative;
        let after_prefix = start + prefix.len();
        let rest = &text[after_prefix..];
        let digit_len = rest
            .as_bytes()
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        if digit_len == 0 {
            search_from = after_prefix;
            continue;
        }
        let count_str = &rest[..digit_len];
        let Ok(count) = count_str.parse::<u64>() else {
            search_from = after_prefix;
            continue;
        };
        let after_digits = after_prefix + digit_len;
        if text[after_digits..].starts_with(suffix) {
            let end = after_digits + suffix.len();
            out.push(TruncationMarker {
                kind,
                matched_text: text[start..end].to_string(),
                byte_offset: start,
                count: Some(count),
            });
            search_from = end;
        } else {
            search_from = after_prefix;
        }
    }
}

fn scan_fixed(
    text: &str,
    needle: &str,
    kind: TruncationMarkerKind,
    out: &mut Vec<TruncationMarker>,
) {
    let mut search_from = 0;
    while let Some(relative) = text[search_from..].find(needle) {
        let start = search_from + relative;
        let end = start + needle.len();
        out.push(TruncationMarker {
            kind,
            matched_text: needle.to_string(),
            byte_offset: start,
            count: None,
        });
        search_from = end;
    }
}

#[cfg(test)]
#[path = "completeness_tests.rs"]
mod tests;
