//! Dynamic (function/custom) tool-call history cells.
//!
//! Fork-only: surfaces `ThreadItem::DynamicToolCall` in the main chat (compact)
//! and `Ctrl+T` transcript (full persisted payload).

use super::*;
use codex_app_server_protocol::DynamicToolCallOutputContentItem;
use codex_app_server_protocol::DynamicToolCallStatus;

#[path = "dynamic_tool_transcript.rs"]
mod transcript;

#[derive(Debug)]
pub(crate) struct DynamicToolCallCell {
    call_id: String,
    namespace: Option<String>,
    tool: String,
    arguments: serde_json::Value,
    start_time: Instant,
    duration: Option<Duration>,
    status: DynamicToolCallStatus,
    content_items: Option<Vec<DynamicToolCallOutputContentItem>>,
    success: Option<bool>,
    animations_enabled: bool,
}

impl DynamicToolCallCell {
    pub(crate) fn new(
        call_id: String,
        namespace: Option<String>,
        tool: String,
        arguments: serde_json::Value,
        animations_enabled: bool,
    ) -> Self {
        Self {
            call_id,
            namespace,
            tool,
            arguments,
            start_time: Instant::now(),
            duration: None,
            status: DynamicToolCallStatus::InProgress,
            content_items: None,
            success: None,
            animations_enabled,
        }
    }

    pub(crate) fn call_id(&self) -> &str {
        &self.call_id
    }

    pub(crate) fn qualified_name(&self) -> String {
        match &self.namespace {
            Some(namespace) => format!("{namespace}/{}", self.tool),
            None => self.tool.clone(),
        }
    }

    pub(crate) fn arguments(&self) -> &serde_json::Value {
        &self.arguments
    }

    pub(crate) fn duration(&self) -> Option<Duration> {
        self.duration
    }

    pub(crate) fn content_items(&self) -> Option<&[DynamicToolCallOutputContentItem]> {
        self.content_items.as_deref()
    }

    pub(crate) fn success(&self) -> Option<bool> {
        match self.status {
            DynamicToolCallStatus::InProgress => None,
            DynamicToolCallStatus::Completed | DynamicToolCallStatus::Failed => Some(
                self.success
                    .unwrap_or(matches!(self.status, DynamicToolCallStatus::Completed)),
            ),
        }
    }

    pub(crate) fn complete(
        &mut self,
        duration: impl Into<Option<Duration>>,
        status: DynamicToolCallStatus,
        content_items: Option<Vec<DynamicToolCallOutputContentItem>>,
        success: Option<bool>,
    ) {
        self.duration = duration.into();
        self.status = match status {
            DynamicToolCallStatus::InProgress => DynamicToolCallStatus::Completed,
            other => other,
        };
        self.content_items = content_items;
        self.success = success.or(Some(matches!(
            self.status,
            DynamicToolCallStatus::Completed
        )));
    }

    pub(crate) fn mark_failed(&mut self) {
        self.duration = Some(self.start_time.elapsed());
        self.status = DynamicToolCallStatus::Failed;
        self.success = Some(false);
        if self.content_items.is_none() {
            self.content_items = Some(vec![DynamicToolCallOutputContentItem::InputText {
                text: "interrupted".to_string(),
            }]);
        }
    }

    fn render_content_preview(
        item: &DynamicToolCallOutputContentItem,
        width: usize,
        max_lines: usize,
    ) -> String {
        match item {
            DynamicToolCallOutputContentItem::InputText { text } => {
                format_and_truncate_tool_result(text, max_lines, width)
            }
            DynamicToolCallOutputContentItem::InputImage { .. } => "<image content>".to_string(),
            DynamicToolCallOutputContentItem::InputAudio { .. } => "<audio content>".to_string(),
        }
    }
}

impl HistoryCell for DynamicToolCallCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        let status = self.success();
        let bullet = match status {
            Some(true) => "•".green().bold(),
            Some(false) => "•".red().bold(),
            None => activity_indicator(
                Some(self.start_time),
                MotionMode::from_animations_enabled(self.animations_enabled),
                ReducedMotionIndicator::StaticBullet,
            )
            .unwrap_or_else(|| "•".dim()),
        };
        let header_text = if status.is_some() {
            "Called"
        } else {
            "Calling"
        };

        let invocation_line = line_to_static(&format_dynamic_tool_invocation(
            self.namespace.as_deref(),
            &self.tool,
            &self.arguments,
        ));
        let mut compact_spans = vec![bullet.clone(), " ".into(), header_text.bold(), " ".into()];
        let mut compact_header = Line::from(compact_spans.clone());
        let reserved = compact_header.width();

        let inline_invocation =
            invocation_line.width() <= (width as usize).saturating_sub(reserved);

        if inline_invocation {
            compact_header.extend(invocation_line.spans.clone());
            lines.push(compact_header);
        } else {
            compact_spans.pop();
            lines.push(Line::from(compact_spans));

            let opts = RtOptions::new((width as usize).saturating_sub(4))
                .initial_indent("".into())
                .subsequent_indent("    ".into());
            let wrapped = adaptive_wrap_line(&invocation_line, opts);
            let body_lines: Vec<Line<'static>> = wrapped.iter().map(line_to_static).collect();
            lines.extend(prefix_lines(body_lines, "  └ ".dim(), "    ".into()));
        }

        let mut detail_lines: Vec<Line<'static>> = Vec::new();
        let detail_wrap_width = (width as usize).saturating_sub(4).max(1);

        if let Some(content_items) = &self.content_items {
            let mut remaining_lines = TOOL_CALL_MAX_LINES;
            for item in content_items {
                if remaining_lines == 0 {
                    break;
                }
                let text = Self::render_content_preview(item, detail_wrap_width, remaining_lines);
                for segment in text.split('\n') {
                    if remaining_lines == 0 {
                        break;
                    }
                    let line = Line::from(segment.to_string().dim());
                    // Bound source before wrapping so one overlong segment cannot
                    // allocate more wrapped rows than the remaining compact budget.
                    let max_source_cols = remaining_lines
                        .saturating_add(1)
                        .saturating_mul(detail_wrap_width.max(1));
                    let line = if line.width() > max_source_cols {
                        crate::line_truncation::truncate_line_to_width(line, max_source_cols)
                    } else {
                        line
                    };
                    let wrapped = adaptive_wrap_line(
                        &line,
                        RtOptions::new(detail_wrap_width)
                            .initial_indent("".into())
                            .subsequent_indent("    ".into()),
                    );
                    for wrapped_line in wrapped {
                        if remaining_lines == 0 {
                            break;
                        }
                        detail_lines.push(line_to_static(&wrapped_line));
                        remaining_lines = remaining_lines.saturating_sub(1);
                    }
                }
            }
        }

        if !detail_lines.is_empty() {
            let initial_prefix: Span<'static> = if inline_invocation {
                "  └ ".dim()
            } else {
                "    ".into()
            };
            lines.extend(prefix_lines(detail_lines, initial_prefix, "    ".into()));
        }

        lines
    }

    fn raw_lines(&self) -> Vec<Line<'static>> {
        let header_text = if self.success().is_some() {
            "Called"
        } else {
            "Calling"
        };
        let mut lines = vec![Line::from(format!(
            "{header_text} {}",
            format_dynamic_tool_invocation_text(
                self.namespace.as_deref(),
                &self.tool,
                &self.arguments,
            )
        ))];

        if let Some(content_items) = &self.content_items {
            let mut remaining_lines = TOOL_CALL_MAX_LINES;
            for item in content_items {
                if remaining_lines == 0 {
                    break;
                }
                let text =
                    Self::render_content_preview(item, RAW_TOOL_OUTPUT_WIDTH, remaining_lines);
                for line in raw_lines_from_source(&text) {
                    if remaining_lines == 0 {
                        break;
                    }
                    lines.push(line);
                    remaining_lines = remaining_lines.saturating_sub(1);
                }
            }
        }

        lines
    }

    fn transcript_lines(&self, width: u16) -> Vec<Line<'static>> {
        transcript::render(self, width)
    }

    fn transcript_row_count(&self, width: u16) -> usize {
        transcript::build(self, width).row_count(width)
    }

    fn transcript_lines_window(
        &self,
        width: u16,
        start_row: usize,
        max_rows: usize,
    ) -> Vec<Line<'static>> {
        transcript::build(self, width).lines_window(width, start_row, max_rows)
    }

    fn transcript_hyperlink_lines_window(
        &self,
        width: u16,
        start_row: usize,
        max_rows: usize,
    ) -> Vec<HyperlinkLine> {
        plain_hyperlink_lines(self.transcript_lines_window(width, start_row, max_rows))
    }

    fn desired_transcript_height(&self, width: u16) -> u16 {
        self.transcript_row_count(width)
            .try_into()
            .unwrap_or(u16::MAX)
    }

    fn transcript_animation_tick(&self) -> Option<u64> {
        None
    }
}

pub(crate) fn new_active_dynamic_tool_call(
    call_id: String,
    namespace: Option<String>,
    tool: String,
    arguments: serde_json::Value,
    animations_enabled: bool,
) -> DynamicToolCallCell {
    DynamicToolCallCell::new(call_id, namespace, tool, arguments, animations_enabled)
}

fn format_dynamic_tool_invocation<'a>(
    namespace: Option<&str>,
    tool: &str,
    arguments: &serde_json::Value,
) -> Line<'a> {
    let args_str = serde_json::to_string(arguments).unwrap_or_else(|_| arguments.to_string());
    let mut spans: Vec<Span<'a>> = Vec::new();
    if let Some(namespace) = namespace {
        spans.push(namespace.to_string().cyan());
        spans.push("/".into());
    }
    spans.push(tool.to_string().cyan());
    spans.push("(".into());
    spans.push(args_str.dim());
    spans.push(")".into());
    spans.into()
}

fn format_dynamic_tool_invocation_text(
    namespace: Option<&str>,
    tool: &str,
    arguments: &serde_json::Value,
) -> String {
    let args_str = serde_json::to_string(arguments).unwrap_or_else(|_| arguments.to_string());
    match namespace {
        Some(namespace) => format!("{namespace}/{tool}({args_str})"),
        None => format!("{tool}({args_str})"),
    }
}
