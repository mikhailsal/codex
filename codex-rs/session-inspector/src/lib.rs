//! Read-only normalization of tool calls persisted in Codex session rollouts.

mod completeness;

pub use completeness::Completeness;
pub use completeness::TruncationMarker;
pub use completeness::TruncationMarkerKind;
pub use completeness::assess_tool_result;
pub use completeness::detect_truncation_markers;
pub use completeness::text_contains_truncation_marker;

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::RolloutItem;
use codex_protocol::protocol::RolloutLine;
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq)]
pub struct ToolCallRecord {
    pub turn_id: Option<String>,
    pub call_id: String,
    pub tool: ToolIdentity,
    pub arguments: RawPayload,
    pub result: Option<ToolResult>,
    pub completeness: Completeness,
    pub call_source: RecordSource,
    pub result_source: Option<RecordSource>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolIdentity {
    pub kind: ToolKind,
    pub namespace: Option<String>,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    Function,
    Custom,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RawPayload {
    pub raw: String,
    pub parsed_json: Option<Value>,
}

impl RawPayload {
    fn new(raw: String) -> Self {
        let parsed_json = serde_json::from_str(&raw).ok();
        Self { raw, parsed_json }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolResult {
    pub raw: Value,
    pub body: ToolResultBody,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ToolResultBody {
    Text(String),
    ContentItems(Vec<codex_protocol::models::FunctionCallOutputContentItem>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordSource {
    pub path: PathBuf,
    pub line: u64,
    pub ordinal: Option<u64>,
    pub timestamp: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OrphanToolOutput {
    pub turn_id: Option<String>,
    pub call_id: String,
    pub kind: ToolKind,
    pub result: ToolResult,
    pub completeness: Completeness,
    pub source: RecordSource,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UnknownRolloutRecord {
    pub raw: Value,
    pub source: RecordSource,
}

#[derive(Debug, Default, PartialEq)]
pub struct SessionToolRecords {
    pub calls: Vec<ToolCallRecord>,
    pub orphan_outputs: Vec<OrphanToolOutput>,
    pub unknown_records: Vec<UnknownRolloutRecord>,
}

#[derive(Debug, Error)]
pub enum SessionInspectorError {
    #[error("failed to read rollout {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid JSON in rollout {path} at line {line}: {source}")]
    InvalidJson {
        path: PathBuf,
        line: u64,
        #[source]
        source: serde_json::Error,
    },
}

/// Reads function and custom tool records from a plain or zstd-compressed rollout.
pub async fn read_tool_records(
    path: impl AsRef<Path>,
) -> Result<SessionToolRecords, SessionInspectorError> {
    let path = path.as_ref().to_path_buf();
    let mut reader = codex_rollout::open_rollout_line_reader(&path)
        .await
        .map_err(|source| SessionInspectorError::Read {
            path: path.clone(),
            source,
        })?;
    let mut records = SessionToolRecords::default();
    let mut current_turn_id = None;
    let mut calls_by_key = HashMap::<(Option<String>, String), usize>::new();
    let mut line_number = 0_u64;

    while let Some(raw_line) =
        reader
            .next_line()
            .await
            .map_err(|source| SessionInspectorError::Read {
                path: path.clone(),
                source,
            })?
    {
        line_number += 1;
        let raw: Value = serde_json::from_str(&raw_line).map_err(|source| {
            SessionInspectorError::InvalidJson {
                path: path.clone(),
                line: line_number,
                source,
            }
        })?;
        let line = match serde_json::from_value::<RolloutLine>(raw.clone()) {
            Ok(line) => line,
            Err(_) => {
                records.unknown_records.push(UnknownRolloutRecord {
                    source: RecordSource {
                        path: path.clone(),
                        line: line_number,
                        ordinal: raw.get("ordinal").and_then(Value::as_u64),
                        timestamp: raw
                            .get("timestamp")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    },
                    raw,
                });
                continue;
            }
        };
        let source = RecordSource {
            path: path.clone(),
            line: line_number,
            ordinal: line.ordinal,
            timestamp: line.timestamp,
        };

        match line.item {
            RolloutItem::EventMsg(EventMsg::TurnStarted(event)) => {
                current_turn_id = Some(event.turn_id);
            }
            RolloutItem::EventMsg(EventMsg::TurnComplete(event)) => {
                if current_turn_id.as_ref() == Some(&event.turn_id) {
                    current_turn_id = None;
                }
            }
            RolloutItem::EventMsg(EventMsg::TurnAborted(event)) => {
                if event
                    .turn_id
                    .as_ref()
                    .is_none_or(|turn_id| current_turn_id.as_ref() == Some(turn_id))
                {
                    current_turn_id = None;
                }
            }
            RolloutItem::ResponseItem(ResponseItem::Other) => {
                records
                    .unknown_records
                    .push(UnknownRolloutRecord { raw, source });
            }
            RolloutItem::ResponseItem(item) => {
                let raw_output = raw.pointer("/payload/output").cloned();
                let turn_id = item
                    .turn_id()
                    .map(str::to_owned)
                    .or_else(|| current_turn_id.clone());
                normalize_response_item(
                    item,
                    raw_output,
                    turn_id,
                    source,
                    RecordCollector {
                        records: &mut records,
                        calls_by_key: &mut calls_by_key,
                    },
                );
            }
            RolloutItem::SessionMeta(_)
            | RolloutItem::InterAgentCommunication(_)
            | RolloutItem::InterAgentCommunicationMetadata { .. }
            | RolloutItem::Compacted(_)
            | RolloutItem::TurnContext(_)
            | RolloutItem::WorldState(_)
            | RolloutItem::EventMsg(_) => {}
        }
    }

    Ok(records)
}

fn normalize_response_item(
    item: ResponseItem,
    raw_output: Option<Value>,
    turn_id: Option<String>,
    source: RecordSource,
    collector: RecordCollector<'_>,
) {
    match item {
        ResponseItem::FunctionCall {
            name,
            namespace,
            arguments,
            call_id,
            ..
        } => push_call(
            ToolIdentity {
                kind: ToolKind::Function,
                namespace,
                name,
            },
            arguments,
            call_id,
            turn_id,
            source,
            collector,
        ),
        ResponseItem::CustomToolCall {
            name,
            namespace,
            input,
            call_id,
            ..
        } => push_call(
            ToolIdentity {
                kind: ToolKind::Custom,
                namespace,
                name,
            },
            input,
            call_id,
            turn_id,
            source,
            collector,
        ),
        ResponseItem::FunctionCallOutput {
            call_id, output, ..
        } => push_output(
            ToolKind::Function,
            call_id,
            output.body,
            raw_output.unwrap_or(Value::Null),
            turn_id,
            source,
            collector,
        ),
        ResponseItem::CustomToolCallOutput {
            call_id, output, ..
        } => push_output(
            ToolKind::Custom,
            call_id,
            output.body,
            raw_output.unwrap_or(Value::Null),
            turn_id,
            source,
            collector,
        ),
        _ => {}
    }
}

struct RecordCollector<'a> {
    records: &'a mut SessionToolRecords,
    calls_by_key: &'a mut HashMap<(Option<String>, String), usize>,
}

fn push_call(
    tool: ToolIdentity,
    arguments: String,
    call_id: String,
    turn_id: Option<String>,
    source: RecordSource,
    collector: RecordCollector<'_>,
) {
    let index = collector.records.calls.len();
    collector
        .calls_by_key
        .insert((turn_id.clone(), call_id.clone()), index);
    collector.records.calls.push(ToolCallRecord {
        turn_id,
        call_id,
        tool,
        arguments: RawPayload::new(arguments),
        result: None,
        completeness: Completeness::Unknown,
        call_source: source,
        result_source: None,
    });
}

fn push_output(
    kind: ToolKind,
    call_id: String,
    output: FunctionCallOutputBody,
    raw_output: Value,
    turn_id: Option<String>,
    source: RecordSource,
    collector: RecordCollector<'_>,
) {
    let body = match output {
        FunctionCallOutputBody::Text(text) => ToolResultBody::Text(text),
        FunctionCallOutputBody::ContentItems(items) => ToolResultBody::ContentItems(items),
    };
    let result = ToolResult {
        raw: raw_output,
        body,
    };
    let completeness = assess_tool_result(&result);
    let key = (turn_id.clone(), call_id.clone());
    if let Some(call) = collector
        .calls_by_key
        .get(&key)
        .and_then(|index| collector.records.calls.get_mut(*index))
        && call.tool.kind == kind
    {
        call.result = Some(result);
        call.completeness = completeness;
        call.result_source = Some(source);
    } else {
        collector.records.orphan_outputs.push(OrphanToolOutput {
            turn_id,
            call_id,
            kind,
            result,
            completeness,
            source,
        });
    }
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
