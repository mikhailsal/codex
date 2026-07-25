use std::fs::File;
use std::io::Write;
use std::path::Path;

use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;

use super::*;

#[tokio::test]
async fn normalizes_calls_without_losing_raw_or_structured_payloads() {
    let fixture = Fixture::plain(&[
        turn_started("turn-1"),
        response(json!({
            "type": "function_call",
            "name": "exec",
            "namespace": "functions",
            "arguments": "{\"cmd\":\"printf 'λ'\"}",
            "call_id": "call-1"
        })),
        response(json!({
            "type": "function_call_output",
            "call_id": "call-1",
            "output": "first\nsecond"
        })),
        response(json!({
            "type": "custom_tool_call",
            "status": "completed",
            "name": "apply_patch",
            "input": "*** Begin Patch\nλ\n*** End Patch",
            "call_id": "call-2"
        })),
        response(json!({
            "type": "custom_tool_call_output",
            "call_id": "call-2",
            "output": [{"type": "input_text", "text": "Done!"}]
        })),
    ]);

    let records = read_tool_records(&fixture.path).await.unwrap();

    assert_eq!(records.calls.len(), 2);
    assert_eq!(records.orphan_outputs, Vec::new());
    assert_eq!(records.unknown_records, Vec::new());
    assert_eq!(records.calls[0].turn_id.as_deref(), Some("turn-1"));
    assert_eq!(records.calls[0].tool.kind, ToolKind::Function);
    assert_eq!(
        records.calls[0].arguments.parsed_json,
        Some(json!({"cmd": "printf 'λ'"}))
    );
    assert_eq!(
        records.calls[0].result.as_ref().map(|result| &result.body),
        Some(&ToolResultBody::Text("first\nsecond".to_string()))
    );
    assert_eq!(
        records.calls[0].result.as_ref().map(|result| &result.raw),
        Some(&json!("first\nsecond"))
    );
    assert_eq!(records.calls[0].call_source.line, 2);
    assert_eq!(
        records.calls[0]
            .result_source
            .as_ref()
            .map(|source| source.line),
        Some(3)
    );
    assert_eq!(records.calls[0].completeness, Completeness::Complete);
    assert_eq!(records.calls[1].tool.kind, ToolKind::Custom);
    assert_eq!(records.calls[1].completeness, Completeness::Complete);
    assert_eq!(records.calls[1].arguments.parsed_json, None);
    let ToolResultBody::ContentItems(items) = &records.calls[1].result.as_ref().unwrap().body
    else {
        panic!("expected structured content items");
    };
    assert_eq!(items.len(), 1);
}

#[tokio::test]
async fn scopes_repeated_call_ids_to_their_turn() {
    let fixture = Fixture::plain(&[
        turn_started("turn-1"),
        function_call("same", "first"),
        function_output("same", "one"),
        turn_complete("turn-1"),
        turn_started("turn-2"),
        function_call("same", "second"),
        function_output("same", "two"),
    ]);

    let records = read_tool_records(&fixture.path).await.unwrap();

    assert_eq!(
        records
            .calls
            .iter()
            .map(|call| {
                (
                    &call.turn_id,
                    call.result.as_ref().map(|result| &result.body),
                )
            })
            .collect::<Vec<_>>(),
        vec![
            (
                &Some("turn-1".to_string()),
                Some(&ToolResultBody::Text("one".to_string()))
            ),
            (
                &Some("turn-2".to_string()),
                Some(&ToolResultBody::Text("two".to_string()))
            ),
        ]
    );
}

#[tokio::test]
async fn uses_persisted_turn_ids_without_turn_lifecycle_events() {
    let fixture = Fixture::plain(&[
        response_with_turn(
            json!({
                "type": "function_call",
                "name": "tool",
                "arguments": "first",
                "call_id": "same"
            }),
            "turn-1",
        ),
        response_with_turn(
            json!({
                "type": "function_call_output",
                "call_id": "same",
                "output": "one"
            }),
            "turn-1",
        ),
        response_with_turn(
            json!({
                "type": "function_call",
                "name": "tool",
                "arguments": "second",
                "call_id": "same"
            }),
            "turn-2",
        ),
        response_with_turn(
            json!({
                "type": "function_call_output",
                "call_id": "same",
                "output": "two"
            }),
            "turn-2",
        ),
    ]);

    let records = read_tool_records(&fixture.path).await.unwrap();

    assert_eq!(
        records
            .calls
            .iter()
            .map(|call| (
                call.turn_id.as_deref(),
                call.result.as_ref().map(|result| &result.body),
            ))
            .collect::<Vec<_>>(),
        vec![
            (
                Some("turn-1"),
                Some(&ToolResultBody::Text("one".to_string()))
            ),
            (
                Some("turn-2"),
                Some(&ToolResultBody::Text("two".to_string()))
            ),
        ]
    );
}

#[tokio::test]
async fn clears_lifecycle_turn_after_an_abort_without_a_turn_id() {
    let fixture = Fixture::plain(&[
        turn_started("turn-1"),
        turn_aborted(),
        function_call("call-1", "after abort"),
        function_output("call-1", "ok"),
    ]);

    let records = read_tool_records(&fixture.path).await.unwrap();

    assert_eq!(records.calls[0].turn_id, None);
    assert_eq!(
        records.calls[0].result.as_ref().map(|result| &result.body),
        Some(&ToolResultBody::Text("ok".to_string()))
    );
}

#[tokio::test]
async fn retains_unmatched_outputs() {
    let fixture = Fixture::plain(&[turn_started("turn-1"), function_output("missing", "orphan")]);

    let records = read_tool_records(&fixture.path).await.unwrap();

    assert_eq!(records.calls, Vec::new());
    assert_eq!(records.orphan_outputs.len(), 1);
    assert_eq!(records.orphan_outputs[0].call_id, "missing");
    assert_eq!(records.orphan_outputs[0].source.line, 2);
    assert_eq!(
        records.orphan_outputs[0].completeness,
        Completeness::Complete
    );
}

#[tokio::test]
async fn marks_truncated_results_and_leaves_open_calls_unknown() {
    let truncated =
        "Warning: truncated output (original token count: 99)\n\nhead…5 tokens truncated…tail";
    let fixture = Fixture::plain(&[
        turn_started("turn-1"),
        function_call("call-1", "args"),
        function_output("call-1", truncated),
        function_call("call-2", "still running"),
    ]);

    let records = read_tool_records(&fixture.path).await.unwrap();

    assert!(records.calls[0].completeness.is_truncated());
    let Completeness::Truncated { markers } = &records.calls[0].completeness else {
        panic!("expected truncated completeness");
    };
    assert_eq!(
        markers.iter().map(|marker| marker.kind).collect::<Vec<_>>(),
        vec![
            TruncationMarkerKind::WarningTruncatedOutput,
            TruncationMarkerKind::TokensTruncated,
        ]
    );
    assert_eq!(records.calls[1].result, None);
    assert_eq!(records.calls[1].completeness, Completeness::Unknown);
}

#[tokio::test]
async fn reads_zstd_rollouts() {
    let fixture = Fixture::compressed(&[
        turn_started("turn-1"),
        function_call("call-1", "compressed"),
        function_output("call-1", "ok"),
    ]);

    let records = read_tool_records(&fixture.path).await.unwrap();

    assert_eq!(records.calls.len(), 1);
    assert_eq!(
        records.calls[0].result.as_ref().map(|result| &result.body),
        Some(&ToolResultBody::Text("ok".to_string()))
    );
}

#[tokio::test]
async fn preserves_unknown_records_as_raw_json() {
    let unknown_outer = line(json!({
        "type": "future_record",
        "payload": {"text": "λ"}
    }));
    let unknown_nested = line(json!({
        "type": "event_msg",
        "payload": {
            "type": "future_event",
            "text": "still inspect later records"
        }
    }));
    let fixture = Fixture::plain(&[
        unknown_outer.clone(),
        unknown_nested.clone(),
        turn_started("turn-1"),
        function_call("call-1", "after unknown"),
        function_output("call-1", "ok"),
    ]);

    let records = read_tool_records(&fixture.path).await.unwrap();

    assert_eq!(
        records
            .unknown_records
            .iter()
            .map(|record| &record.raw)
            .collect::<Vec<_>>(),
        vec![&unknown_outer, &unknown_nested]
    );
    assert_eq!(records.unknown_records[0].source.line, 1);
    assert_eq!(records.unknown_records[1].source.line, 2);
    assert_eq!(records.calls.len(), 1);
    assert_eq!(
        records.calls[0].result.as_ref().map(|result| &result.body),
        Some(&ToolResultBody::Text("ok".to_string()))
    );
}

#[tokio::test]
async fn reports_the_line_containing_malformed_json() {
    let fixture = Fixture::plain_raw(&[
        serde_json::to_string(&turn_started("turn-1")).unwrap(),
        "{not json}".to_string(),
    ]);

    let error = read_tool_records(&fixture.path).await.unwrap_err();

    assert!(matches!(
        error,
        SessionInspectorError::InvalidJson { line: 2, .. }
    ));
    assert!(error.to_string().contains("at line 2"));
}

fn turn_started(turn_id: &str) -> Value {
    line(json!({
        "type": "event_msg",
        "payload": {
            "type": "turn_started",
            "turn_id": turn_id,
            "trace_id": null,
            "started_at": 1,
            "model_context_window": 1000,
            "collaboration_mode_kind": "default"
        }
    }))
}

fn turn_complete(turn_id: &str) -> Value {
    line(json!({
        "type": "event_msg",
        "payload": {
            "type": "turn_complete",
            "turn_id": turn_id,
            "last_agent_message": null
        }
    }))
}

fn turn_aborted() -> Value {
    line(json!({
        "type": "event_msg",
        "payload": {
            "type": "turn_aborted",
            "turn_id": null,
            "reason": "interrupted"
        }
    }))
}

fn function_call(call_id: &str, arguments: &str) -> Value {
    response(json!({
        "type": "function_call",
        "name": "tool",
        "arguments": arguments,
        "call_id": call_id
    }))
}

fn function_output(call_id: &str, output: &str) -> Value {
    response(json!({
        "type": "function_call_output",
        "call_id": call_id,
        "output": output
    }))
}

fn response(payload: Value) -> Value {
    line(json!({"type": "response_item", "payload": payload}))
}

fn response_with_turn(mut payload: Value, turn_id: &str) -> Value {
    payload.as_object_mut().unwrap().insert(
        "internal_chat_message_metadata_passthrough".to_string(),
        json!({"turn_id": turn_id}),
    );
    response(payload)
}

fn line(item: Value) -> Value {
    let mut object = item.as_object().unwrap().clone();
    object.insert("timestamp".to_string(), json!("2026-07-25T12:00:00Z"));
    object.insert("ordinal".to_string(), json!(7));
    Value::Object(object)
}

struct Fixture {
    _root: TempDir,
    path: PathBuf,
}

impl Fixture {
    fn plain(lines: &[Value]) -> Self {
        Self::plain_raw(
            &lines
                .iter()
                .map(|line| serde_json::to_string(line).unwrap())
                .collect::<Vec<_>>(),
        )
    }

    fn plain_raw(lines: &[String]) -> Self {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("rollout.jsonl");
        write_lines(&path, lines);
        Self { _root: root, path }
    }

    fn compressed(lines: &[Value]) -> Self {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("rollout.jsonl.zst");
        let file = File::create(&path).unwrap();
        let mut encoder = zstd::stream::write::Encoder::new(file, 1).unwrap();
        for line in lines {
            writeln!(encoder, "{}", serde_json::to_string(line).unwrap()).unwrap();
        }
        encoder.finish().unwrap();
        Self { _root: root, path }
    }
}

fn write_lines(path: &Path, lines: &[String]) {
    let mut file = File::create(path).unwrap();
    for line in lines {
        writeln!(file, "{line}").unwrap();
    }
}
