use std::fs;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Result;
use predicates::str::contains;
use tempfile::TempDir;

fn codex_command(codex_home: &Path) -> Result<assert_cmd::Command> {
    let mut cmd = assert_cmd::Command::new(codex_utils_cargo_bin::cargo_bin("codex")?);
    cmd.env("CODEX_HOME", codex_home);
    Ok(cmd)
}

fn write_session_rollout(
    codex_home: &Path,
    thread_id: &str,
    lines: &[serde_json::Value],
) -> Result<PathBuf> {
    write_rollout_at(
        &codex_home
            .join("sessions/2024/01/02")
            .join(format!("rollout-2024-01-02T12-00-00-{thread_id}.jsonl")),
        lines,
    )
}

fn write_archived_session_rollout(
    codex_home: &Path,
    thread_id: &str,
    lines: &[serde_json::Value],
) -> Result<PathBuf> {
    write_rollout_at(
        &codex_home
            .join("archived_sessions")
            .join(format!("rollout-2024-01-02T12-00-00-{thread_id}.jsonl")),
        lines,
    )
}

fn write_rollout_at(path: &Path, lines: &[serde_json::Value]) -> Result<PathBuf> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::File::create(path)?;
    for line in lines {
        writeln!(file, "{line}")?;
    }
    Ok(path.to_path_buf())
}

fn session_meta(thread_id: &str) -> serde_json::Value {
    serde_json::json!({
        "timestamp": "2024-01-02T12:00:00.000Z",
        "type": "session_meta",
        "payload": {
            "id": thread_id,
            "timestamp": "2024-01-02T12:00:00Z",
            "cwd": "/tmp/demo",
            "originator": "test",
            "cli_version": "test",
            "source": "cli",
            "model_provider": "test-provider",
        }
    })
}

/// Rollout listing requires session_meta plus a discoverable preview.
fn user_message(text: &str) -> serde_json::Value {
    serde_json::json!({
        "timestamp": "2024-01-02T12:00:00.500Z",
        "type": "event_msg",
        "payload": {
            "type": "user_message",
            "message": text,
            "kind": "plain"
        }
    })
}

fn turn_started(turn_id: &str) -> serde_json::Value {
    serde_json::json!({
        "timestamp": "2024-01-02T12:00:01.000Z",
        "type": "event_msg",
        "payload": {
            "type": "turn_started",
            "turn_id": turn_id,
            "trace_id": null,
            "started_at": 1,
            "model_context_window": 1000,
            "collaboration_mode_kind": "default"
        }
    })
}

fn function_call(call_id: &str, arguments: &str) -> serde_json::Value {
    serde_json::json!({
        "timestamp": "2024-01-02T12:00:02.000Z",
        "type": "response_item",
        "payload": {
            "type": "function_call",
            "name": "exec",
            "namespace": "functions",
            "arguments": arguments,
            "call_id": call_id,
        }
    })
}

fn function_output(call_id: &str, output: &str) -> serde_json::Value {
    serde_json::json!({
        "timestamp": "2024-01-02T12:00:03.000Z",
        "type": "response_item",
        "payload": {
            "type": "function_call_output",
            "call_id": call_id,
            "output": output,
        }
    })
}

fn custom_tool_call(call_id: &str, input: &str) -> serde_json::Value {
    serde_json::json!({
        "timestamp": "2024-01-02T12:00:04.000Z",
        "type": "response_item",
        "payload": {
            "type": "custom_tool_call",
            "status": "completed",
            "name": "apply_patch",
            "namespace": "custom",
            "input": input,
            "call_id": call_id,
        }
    })
}

fn custom_tool_output(call_id: &str) -> serde_json::Value {
    serde_json::json!({
        "timestamp": "2024-01-02T12:00:05.000Z",
        "type": "response_item",
        "payload": {
            "type": "custom_tool_call_output",
            "call_id": call_id,
            "output": [{"type": "input_text", "text": "patched"}],
        }
    })
}

#[test]
fn debug_session_list_show_tools_and_tool() -> Result<()> {
    let codex_home = TempDir::new()?;
    let thread_id = "00000000-0000-0000-0000-0000000000aa";
    let truncated =
        "Warning: truncated output (original token count: 9)\nhead…2 tokens truncated…tail";
    let path = write_session_rollout(
        codex_home.path(),
        thread_id,
        &[
            session_meta(thread_id),
            user_message("demo session"),
            turn_started("turn-1"),
            function_call("call-1", "{\"cmd\":\"echo hi\"}"),
            function_output("call-1", truncated),
            function_call("call-2", "{\"cmd\":\"pwd\"}"),
            function_output("call-2", "/tmp/demo"),
        ],
    )?;

    let list = codex_command(codex_home.path())?
        .args(["debug", "session", "list", "--json"])
        .output()?;
    assert!(
        list.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&list.stderr)
    );
    let list_json: serde_json::Value = serde_json::from_slice(&list.stdout)?;
    assert_eq!(list_json.as_array().map(Vec::len), Some(1));
    assert_eq!(list_json[0]["threadId"].as_str(), Some(thread_id));
    assert!(
        list_json[0]["path"]
            .as_str()
            .is_some_and(|p| p.ends_with(path.file_name().unwrap().to_str().unwrap()))
    );

    let show = codex_command(codex_home.path())?
        .args(["debug", "session", "show", "--last", "--json"])
        .output()?;
    assert!(
        show.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&show.stderr)
    );
    let show_json: serde_json::Value = serde_json::from_slice(&show.stdout)?;
    assert_eq!(show_json["toolCalls"], 2);
    assert_eq!(show_json["truncatedToolCalls"], 1);
    assert_eq!(show_json["openToolCalls"], 0);

    let tools = codex_command(codex_home.path())?
        .args(["debug", "session", "tools", thread_id, "--json"])
        .output()?;
    assert!(
        tools.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&tools.stderr)
    );
    let tools_json: serde_json::Value = serde_json::from_slice(&tools.stdout)?;
    assert_eq!(tools_json["calls"].as_array().map(Vec::len), Some(2));
    assert_eq!(tools_json["calls"][0]["completeness"], "truncated");
    assert_eq!(tools_json["calls"][1]["completeness"], "complete");

    let tool = codex_command(codex_home.path())?
        .args([
            "debug",
            "session",
            "tool",
            "--file",
            path.to_str().unwrap(),
            "--call",
            "call-2",
            "--json",
        ])
        .output()?;
    assert!(
        tool.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&tool.stderr)
    );
    let tool_json: serde_json::Value = serde_json::from_slice(&tool.stdout)?;
    assert_eq!(tool_json["callId"], "call-2");
    assert_eq!(tool_json["result"], "/tmp/demo");
    assert_eq!(tool_json["completeness"], "complete");

    let truncated_tool = codex_command(codex_home.path())?
        .args([
            "debug",
            "session",
            "tool",
            "--file",
            path.to_str().unwrap(),
            "--call",
            "call-1",
        ])
        .output()?;
    assert!(
        truncated_tool.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&truncated_tool.stderr)
    );
    let truncated_out = String::from_utf8(truncated_tool.stdout)?;
    assert!(truncated_out.contains("completeness: truncated"));
    assert!(
        truncated_out.contains("WARNING: persisted result contains upstream truncation marker")
    );

    Ok(())
}

#[test]
fn debug_session_archived_list_and_show_last() -> Result<()> {
    let codex_home = TempDir::new()?;
    let thread_id = "00000000-0000-0000-0000-0000000000cc";
    write_archived_session_rollout(
        codex_home.path(),
        thread_id,
        &[
            session_meta(thread_id),
            user_message("archived demo"),
            turn_started("turn-1"),
            function_call("call-1", "{}"),
            function_output("call-1", "archived-ok"),
        ],
    )?;

    let list = codex_command(codex_home.path())?
        .args(["debug", "session", "list", "--archived", "--json"])
        .output()?;
    assert!(
        list.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&list.stderr)
    );
    let list_json: serde_json::Value = serde_json::from_slice(&list.stdout)?;
    assert_eq!(list_json.as_array().map(Vec::len), Some(1));
    assert_eq!(list_json[0]["threadId"].as_str(), Some(thread_id));
    assert!(
        list_json[0]["path"]
            .as_str()
            .is_some_and(|p| p.contains("archived_sessions"))
    );

    let show = codex_command(codex_home.path())?
        .args(["debug", "session", "show", "--last", "--archived", "--json"])
        .output()?;
    assert!(
        show.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&show.stderr)
    );
    let show_json: serde_json::Value = serde_json::from_slice(&show.stdout)?;
    assert_eq!(show_json["threadId"].as_str(), Some(thread_id));
    assert_eq!(show_json["toolCalls"], 1);
    Ok(())
}

#[test]
fn debug_session_custom_tool_content_items() -> Result<()> {
    let codex_home = TempDir::new()?;
    let thread_id = "00000000-0000-0000-0000-0000000000dd";
    let path = write_session_rollout(
        codex_home.path(),
        thread_id,
        &[
            session_meta(thread_id),
            user_message("custom tool demo"),
            turn_started("turn-1"),
            custom_tool_call("call-custom", "*** Begin Patch\n*** End Patch"),
            custom_tool_output("call-custom"),
        ],
    )?;

    let tools = codex_command(codex_home.path())?
        .args([
            "debug",
            "session",
            "tools",
            "--file",
            path.to_str().unwrap(),
            "--json",
        ])
        .output()?;
    assert!(
        tools.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&tools.stderr)
    );
    let tools_json: serde_json::Value = serde_json::from_slice(&tools.stdout)?;
    assert_eq!(tools_json["calls"][0]["kind"], "custom");
    assert_eq!(tools_json["calls"][0]["callId"], "call-custom");

    let tool = codex_command(codex_home.path())?
        .args([
            "debug",
            "session",
            "tool",
            "--file",
            path.to_str().unwrap(),
            "--call",
            "call-custom",
            "--json",
        ])
        .output()?;
    assert!(
        tool.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&tool.stderr)
    );
    let tool_json: serde_json::Value = serde_json::from_slice(&tool.stdout)?;
    assert_eq!(tool_json["kind"], "custom");
    assert_eq!(tool_json["result"][0]["type"], "input_text");
    assert_eq!(tool_json["result"][0]["text"], "patched");
    Ok(())
}

#[test]
fn debug_session_missing_session_exits_2() -> Result<()> {
    let codex_home = TempDir::new()?;
    codex_command(codex_home.path())?
        .args([
            "debug",
            "session",
            "show",
            "00000000-0000-0000-0000-0000000000ff",
        ])
        .assert()
        .failure()
        .code(2)
        .stderr(contains("not found"));
    Ok(())
}

#[test]
fn debug_session_missing_call_exits_3() -> Result<()> {
    let codex_home = TempDir::new()?;
    let thread_id = "00000000-0000-0000-0000-0000000000bb";
    let path = write_session_rollout(
        codex_home.path(),
        thread_id,
        &[
            session_meta(thread_id),
            user_message("demo session"),
            turn_started("turn-1"),
            function_call("call-1", "{}"),
            function_output("call-1", "ok"),
        ],
    )?;

    codex_command(codex_home.path())?
        .args([
            "debug",
            "session",
            "tool",
            "--file",
            path.to_str().unwrap(),
            "--call",
            "missing-call",
        ])
        .assert()
        .failure()
        .code(3)
        .stderr(contains("not found"));
    Ok(())
}

#[test]
fn debug_session_parse_error_exits_4() -> Result<()> {
    let codex_home = TempDir::new()?;
    let dir = codex_home.path().join("sessions/2024/01/02");
    fs::create_dir_all(&dir)?;
    let path = dir.join("broken.jsonl");
    fs::write(&path, "{not-json\n")?;

    codex_command(codex_home.path())?
        .args([
            "debug",
            "session",
            "tools",
            "--file",
            path.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .code(4)
        .stderr(contains("invalid JSON"));
    Ok(())
}
