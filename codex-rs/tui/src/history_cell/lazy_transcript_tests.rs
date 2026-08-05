use super::*;
use pretty_assertions::assert_eq;
use ratatui::text::Line;

fn plain(lines: &[Line<'static>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect()
}

#[test]
fn window_matches_materialized_slice() {
    let mut doc = LazyTranscript::new();
    doc.push_wrapped_line(Line::from("Header"), 40, "", "");
    doc.push_text_block("Result:", "alpha\nbeta\ngamma\ndelta\nepsilon", 40);

    let full = plain(&doc.materialize(/*width*/ 40));
    let window = plain(&doc.lines_window(/*width*/ 40, /*start_row*/ 2, /*max_rows*/ 3));
    assert_eq!(window, full[2..5]);
    assert_eq!(doc.row_count(/*width*/ 40), full.len());
}

#[test]
fn large_body_window_stays_bounded() {
    let body = (0..5_000)
        .map(|i| format!("line-{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut doc = LazyTranscript::new();
    doc.push_wrapped_line("• Tool call".bold().into(), 80, "", "");
    doc.push_text_block("Result:", &body, 80);

    let window = doc.lines_window(
        /*width*/ 80, /*start_row*/ 1_000, /*max_rows*/ 40,
    );
    assert_eq!(window.len(), 40);
    let rendered = plain(&window);
    assert!(rendered[0].contains("line-"));
    // Header is one row; result label one row; then body starts.
    // start_row 1000 should land well inside body.
    assert!(!rendered.iter().any(|line| line.contains("Tool call")));
}

#[test]
fn row_limit_marker_appears_for_huge_output() {
    // Stay well under materializing tens of thousands of rows in the assertion path:
    // only probe the tail window where the marker must appear.
    let body = std::iter::repeat_n("x", TRANSCRIPT_CONTENT_MAX_ROWS + 50)
        .collect::<Vec<_>>()
        .join("\n");
    let mut doc = LazyTranscript::new();
    doc.push_text_block("Result:", &body, 20);

    let count = doc.row_count(/*width*/ 20);
    assert!(count <= TRANSCRIPT_MAX_ROWS);
    assert!(
        count > TRANSCRIPT_CONTENT_MAX_ROWS,
        "expected marker rows beyond the content budget, got {count}"
    );

    let tail = plain(&doc.lines_window(
        /*width*/ 20,
        count.saturating_sub(8),
        /*max_rows*/ 8,
    ));
    let joined = tail.join(" ");
    assert!(
        joined.contains("Transcript row") && joined.contains("limit reached"),
        "missing row-limit marker in tail {tail:?}"
    );
}

#[test]
fn single_huge_source_line_window_stays_bounded() {
    // Minified JSON / log lines with no newlines are the failure mode called out in review:
    // wrapping the entire source line before skip/take would allocate hundreds of thousands
    // of Lines per draw. The window path must stay O(viewport).
    let body = "A".repeat(200_000);
    let mut doc = LazyTranscript::new();
    doc.push_text_block("Result:", &body, 40);

    let mid = plain(&doc.lines_window(
        /*width*/ 40, /*start_row*/ 500, /*max_rows*/ 20,
    ));
    assert_eq!(mid.len(), 20);
    assert!(
        mid.iter().all(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty() && trimmed.chars().all(|ch| ch == 'A')
        }),
        "scrolled window into a newline-free body must stay on wrapped body rows, got {mid:?}"
    );

    let head = plain(&doc.lines_window(/*width*/ 40, /*start_row*/ 0, /*max_rows*/ 5));
    assert!(
        head.iter().any(|line| line.contains("Result")),
        "head window should still include the label, got {head:?}"
    );
}
