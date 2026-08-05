//! Experimental `codex debug session` viewer over `codex-session-inspector`.
//!
//! Fork-only console surface for inspecting persisted tool rollouts. Wire format
//! and flags are intentionally unstable until filters/export land in a later PR.

use std::fmt;
use std::io::IsTerminal;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use clap::Parser;
use codex_core::config::find_codex_home;
use codex_rollout::ARCHIVED_SESSIONS_SUBDIR;
use codex_rollout::ThreadSortKey;
use codex_rollout::find_archived_thread_path_by_id_str;
use codex_rollout::find_thread_path_by_id_str;
use codex_rollout::get_threads;
use codex_rollout::get_threads_in_root;
use codex_rollout::read_thread_item_from_rollout;
use codex_session_inspector::Completeness;
use codex_session_inspector::SessionInspectorError;
use codex_session_inspector::SessionToolRecords;
use codex_session_inspector::ToolCallRecord;
use codex_session_inspector::ToolKind;
use codex_session_inspector::ToolResultBody;
use codex_session_inspector::read_tool_records;
use owo_colors::OwoColorize;
use serde_json::json;

const EXIT_MISSING_SESSION: u8 = 2;
const EXIT_MISSING_CALL: u8 = 3;
const EXIT_PARSE: u8 = 4;

const DEFAULT_LIST_LIMIT: usize = 50;
/// Hard cap so `--limit` cannot preallocate an enormous `Vec` in listing helpers.
const MAX_LIST_LIMIT: usize = 1_000;

/// Inspect session rollouts using codex-session-inspector.
#[derive(Debug, Parser)]
pub struct DebugSessionCommand {
    #[command(subcommand)]
    pub subcommand: DebugSessionSubcommand,
}

#[derive(Debug, clap::Subcommand)]
pub enum DebugSessionSubcommand {
    /// List recent session rollouts under $CODEX_HOME.
    List(ListArgs),
    /// Show session metadata and tool-call summary counts.
    Show(ShowArgs),
    /// List tool calls in a session without dumping full outputs.
    Tools(ToolsArgs),
    /// Show one tool call's arguments and persisted result.
    Tool(ToolArgs),
}

#[derive(Debug, Parser)]
pub struct ListArgs {
    /// Maximum number of sessions to print (clamped to 1000).
    #[arg(long, default_value_t = DEFAULT_LIST_LIMIT)]
    pub limit: usize,

    /// Include archived sessions instead of active ones.
    #[arg(long, default_value_t = false)]
    pub archived: bool,

    /// Emit JSON instead of a human table.
    #[arg(long, default_value_t = false)]
    pub json: bool,
}

#[derive(Debug, Parser)]
pub struct ShowArgs {
    /// Thread id (UUID). Omit with `--last` or `--file`.
    #[arg(value_name = "THREAD_ID")]
    pub thread_id: Option<String>,

    /// Use the most recent recorded session.
    #[arg(long, default_value_t = false)]
    pub last: bool,

    /// Read a rollout file path directly (bypasses session discovery).
    #[arg(long = "file", value_name = "SESSION_FILE")]
    pub file: Option<PathBuf>,

    /// Search archived sessions when resolving a thread id / `--last`.
    #[arg(long, default_value_t = false)]
    pub archived: bool,

    /// Emit JSON instead of human text.
    #[arg(long, default_value_t = false)]
    pub json: bool,
}

#[derive(Debug, Parser)]
pub struct ToolsArgs {
    /// Thread id (UUID). Omit with `--last` or `--file`.
    #[arg(value_name = "THREAD_ID")]
    pub thread_id: Option<String>,

    /// Use the most recent recorded session.
    #[arg(long, default_value_t = false)]
    pub last: bool,

    /// Read a rollout file path directly (bypasses session discovery).
    #[arg(long = "file", value_name = "SESSION_FILE")]
    pub file: Option<PathBuf>,

    /// Search archived sessions when resolving a thread id / `--last`.
    #[arg(long, default_value_t = false)]
    pub archived: bool,

    /// Emit JSON instead of a human table.
    #[arg(long, default_value_t = false)]
    pub json: bool,
}

#[derive(Debug, Parser)]
pub struct ToolArgs {
    /// Thread id (UUID). Omit with `--last` or `--file`.
    ///
    /// Kept as an optional positional so `--last`/`--file` remain valid; the
    /// call id is a required flag because clap forbids an optional positional
    /// before a required one.
    #[arg(value_name = "THREAD_ID")]
    pub thread_id: Option<String>,

    /// Tool call id within the session.
    #[arg(long = "call", value_name = "CALL_ID")]
    pub call_id: String,

    /// Use the most recent recorded session.
    #[arg(long, default_value_t = false)]
    pub last: bool,

    /// Read a rollout file path directly (bypasses session discovery).
    #[arg(long = "file", value_name = "SESSION_FILE")]
    pub file: Option<PathBuf>,

    /// Search archived sessions when resolving a thread id / `--last`.
    #[arg(long, default_value_t = false)]
    pub archived: bool,

    /// Prefer pretty-printed JSON for arguments/result when parseable.
    #[arg(long, default_value_t = false)]
    pub pretty: bool,

    /// Emit a JSON object for the matched call.
    #[arg(long, default_value_t = false)]
    pub json: bool,
}

#[derive(Debug)]
enum DebugSessionError {
    MissingSession(String),
    MissingCall(String),
    Parse(String),
    Other(anyhow::Error),
}

impl fmt::Display for DebugSessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingSession(message) | Self::MissingCall(message) | Self::Parse(message) => {
                f.write_str(message)
            }
            Self::Other(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for DebugSessionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Other(err) => Some(err.as_ref()),
            Self::MissingSession(_) | Self::MissingCall(_) | Self::Parse(_) => None,
        }
    }
}

impl DebugSessionError {
    fn exit_code(&self) -> i32 {
        match self {
            Self::MissingSession(_) => i32::from(EXIT_MISSING_SESSION),
            Self::MissingCall(_) => i32::from(EXIT_MISSING_CALL),
            Self::Parse(_) => i32::from(EXIT_PARSE),
            Self::Other(_) => 1,
        }
    }
}

impl From<anyhow::Error> for DebugSessionError {
    fn from(err: anyhow::Error) -> Self {
        Self::Other(err)
    }
}

impl From<SessionInspectorError> for DebugSessionError {
    fn from(err: SessionInspectorError) -> Self {
        Self::Parse(err.to_string())
    }
}

impl From<serde_json::Error> for DebugSessionError {
    fn from(err: serde_json::Error) -> Self {
        Self::Other(err.into())
    }
}

/// Run `codex debug session …`. Maps domain errors to stable process exit codes.
pub async fn run_debug_session_command(cmd: DebugSessionCommand) -> anyhow::Result<()> {
    match run(cmd).await {
        Ok(()) => Ok(()),
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(err.exit_code());
        }
    }
}

async fn run(cmd: DebugSessionCommand) -> Result<(), DebugSessionError> {
    match cmd.subcommand {
        DebugSessionSubcommand::List(args) => run_list(args).await,
        DebugSessionSubcommand::Show(args) => run_show(args).await,
        DebugSessionSubcommand::Tools(args) => run_tools(args).await,
        DebugSessionSubcommand::Tool(args) => run_tool(args).await,
    }
}

async fn run_list(args: ListArgs) -> Result<(), DebugSessionError> {
    let codex_home = find_codex_home().context("failed to resolve CODEX_HOME")?;
    let page_size = args.limit.clamp(1, MAX_LIST_LIMIT);
    if args.limit > MAX_LIST_LIMIT {
        eprintln!(
            "note: --limit {} exceeds max {MAX_LIST_LIMIT}; clamping.",
            args.limit
        );
    }
    let page = if args.archived {
        get_threads_in_root(
            codex_home.join(ARCHIVED_SESSIONS_SUBDIR).to_path_buf(),
            page_size,
            /*cursor*/ None,
            ThreadSortKey::UpdatedAt,
            codex_rollout::ThreadListConfig {
                allowed_sources: &[],
                model_providers: None,
                cwd_filters: None,
                default_provider: "openai",
                layout: codex_rollout::ThreadListLayout::Flat,
            },
        )
        .await
        .context("failed to list archived sessions")?
    } else {
        get_threads(
            codex_home.as_path(),
            page_size,
            /*cursor*/ None,
            ThreadSortKey::UpdatedAt,
            &[],
            /*model_providers*/ None,
            /*cwd_filters*/ None,
            "openai",
        )
        .await
        .context("failed to list sessions")?
    };

    if args.json {
        let items: Vec<_> = page
            .items
            .iter()
            .map(|item| {
                json!({
                    "threadId": item.thread_id.map(|id| id.to_string()),
                    "createdAt": item.created_at,
                    "updatedAt": item.updated_at,
                    "cwd": item.cwd.as_ref().map(|p| p.display().to_string()),
                    "gitBranch": item.git_branch,
                    "modelProvider": item.model_provider,
                    "source": item.source.as_ref().map(ToString::to_string),
                    "preview": item.preview,
                    "path": item.path.display().to_string(),
                    "sizeBytes": std::fs::metadata(&item.path).map(|m| m.len()).ok(),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&items)?);
        return Ok(());
    }

    if page.items.is_empty() {
        println!("No sessions found under {}.", codex_home.display());
        return Ok(());
    }

    let color = stdout_wants_color();
    println!(
        "{:<36}  {:<20}  {:<24}  {:>10}  {}",
        header("THREAD", color),
        header("UPDATED", color),
        header("CWD", color),
        header("SIZE", color),
        header("PATH", color),
    );
    for item in &page.items {
        let thread = item
            .thread_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "-".to_string());
        let updated = item
            .updated_at
            .clone()
            .or_else(|| item.created_at.clone())
            .unwrap_or_else(|| "-".to_string());
        let cwd = item
            .cwd
            .as_ref()
            .map(|p| truncate_middle(&p.display().to_string(), 24))
            .unwrap_or_else(|| "-".to_string());
        let size = std::fs::metadata(&item.path)
            .map(|m| format_bytes(m.len()))
            .unwrap_or_else(|_| "-".to_string());
        println!(
            "{thread:<36}  {updated:<20}  {cwd:<24}  {size:>10}  {}",
            item.path.display()
        );
    }
    if page.next_cursor.is_some() || page.reached_scan_cap {
        eprintln!(
            "note: listing truncated to {page_size} entries; raise --limit to see more (pagination tokens come in a later PR)."
        );
    }
    Ok(())
}

async fn run_show(args: ShowArgs) -> Result<(), DebugSessionError> {
    let path = resolve_session_path(
        args.thread_id.as_deref(),
        args.last,
        args.file.as_deref(),
        args.archived,
    )
    .await?;
    let meta = read_thread_item_from_rollout(path.clone()).await;
    let records = read_tool_records(&path).await?;
    let summary = summarize_records(&records);

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "path": path.display().to_string(),
                "threadId": meta.as_ref().and_then(|m| m.thread_id.map(|id| id.to_string())),
                "createdAt": meta.as_ref().and_then(|m| m.created_at.clone()),
                "updatedAt": meta.as_ref().and_then(|m| m.updated_at.clone()),
                "cwd": meta.as_ref().and_then(|m| m.cwd.as_ref().map(|p| p.display().to_string())),
                "gitBranch": meta.as_ref().and_then(|m| m.git_branch.clone()),
                "modelProvider": meta.as_ref().and_then(|m| m.model_provider.clone()),
                "source": meta.as_ref().and_then(|m| m.source.as_ref().map(ToString::to_string)),
                "preview": meta.as_ref().and_then(|m| m.preview.clone()),
                "toolCalls": summary.calls,
                "truncatedToolCalls": summary.truncated,
                "openToolCalls": summary.open,
                "orphanOutputs": summary.orphans,
                "unknownRecords": summary.unknown,
            }))?
        );
        return Ok(());
    }

    let color = stdout_wants_color();
    println!("{} {}", label("path", color), path.display());
    if let Some(meta) = meta {
        if let Some(id) = meta.thread_id {
            println!("{} {id}", label("thread", color));
        }
        if let Some(created) = meta.created_at {
            println!("{} {created}", label("created", color));
        }
        if let Some(updated) = meta.updated_at {
            println!("{} {updated}", label("updated", color));
        }
        if let Some(cwd) = meta.cwd {
            println!("{} {}", label("cwd", color), cwd.display());
        }
        if let Some(branch) = meta.git_branch {
            println!("{} {branch}", label("branch", color));
        }
        if let Some(provider) = meta.model_provider {
            println!("{} {provider}", label("provider", color));
        }
        if let Some(source) = meta.source {
            println!("{} {source}", label("source", color));
        }
        if let Some(preview) = meta.preview {
            println!("{} {preview}", label("preview", color));
        }
    }
    println!("{} {}", label("tool_calls", color), summary.calls);
    println!("{} {}", label("truncated", color), summary.truncated);
    println!("{} {}", label("open", color), summary.open);
    println!("{} {}", label("orphan_outputs", color), summary.orphans);
    println!("{} {}", label("unknown_records", color), summary.unknown);
    Ok(())
}

async fn run_tools(args: ToolsArgs) -> Result<(), DebugSessionError> {
    let path = resolve_session_path(
        args.thread_id.as_deref(),
        args.last,
        args.file.as_deref(),
        args.archived,
    )
    .await?;
    let records = read_tool_records(&path).await?;

    if args.json {
        let calls: Vec<_> = records
            .calls
            .iter()
            .map(|call| {
                json!({
                    "turnId": call.turn_id,
                    "callId": call.call_id,
                    "kind": tool_kind_str(call.tool.kind),
                    "namespace": call.tool.namespace,
                    "name": call.tool.name,
                    "completeness": completeness_str(&call.completeness),
                    "hasResult": call.result.is_some(),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "path": path.display().to_string(),
                "calls": calls,
                "orphanOutputs": records.orphan_outputs.len(),
                "unknownRecords": records.unknown_records.len(),
            }))?
        );
        return Ok(());
    }

    if records.calls.is_empty() {
        println!("No tool calls in {}.", path.display());
        return Ok(());
    }

    let color = stdout_wants_color();
    println!(
        "{:<36}  {:<10}  {:<8}  {:<12}  {}",
        header("CALL_ID", color),
        header("KIND", color),
        header("STATUS", color),
        header("TURN", color),
        header("NAME", color),
    );
    for call in &records.calls {
        let status = completeness_str(&call.completeness);
        let turn = call.turn_id.as_deref().unwrap_or("-");
        let name = format_tool_name(call);
        println!(
            "{:<36}  {:<10}  {:<8}  {:<12}  {name}",
            call.call_id,
            tool_kind_str(call.tool.kind),
            status,
            truncate_middle(turn, 12),
        );
    }
    if !records.orphan_outputs.is_empty() {
        eprintln!(
            "note: {} orphan tool output(s) without a matching call.",
            records.orphan_outputs.len()
        );
    }
    Ok(())
}

async fn run_tool(args: ToolArgs) -> Result<(), DebugSessionError> {
    let path = resolve_session_path(
        args.thread_id.as_deref(),
        args.last,
        args.file.as_deref(),
        args.archived,
    )
    .await?;
    let records = read_tool_records(&path).await?;
    let matches: Vec<&ToolCallRecord> = records
        .calls
        .iter()
        .filter(|call| call.call_id == args.call_id)
        .collect();
    match matches.as_slice() {
        [] => {
            return Err(DebugSessionError::MissingCall(format!(
                "tool call `{}` not found in {}",
                args.call_id,
                path.display()
            )));
        }
        [call] => print_tool_call(call, args.pretty, args.json)?,
        many => {
            let turns: Vec<String> = many
                .iter()
                .map(|call| call.turn_id.clone().unwrap_or_else(|| "<none>".to_string()))
                .collect();
            return Err(DebugSessionError::MissingCall(format!(
                "tool call `{}` matches {} turns ({}); disambiguation by --turn is not in this PR — pick a unique call_id or inspect `tools` output",
                args.call_id,
                many.len(),
                turns.join(", ")
            )));
        }
    }
    Ok(())
}

fn print_tool_call(
    call: &ToolCallRecord,
    pretty: bool,
    as_json: bool,
) -> Result<(), DebugSessionError> {
    if as_json {
        let arguments = if pretty {
            call.arguments
                .parsed_json
                .clone()
                .unwrap_or_else(|| json!(call.arguments.raw))
        } else {
            json!(call.arguments.raw)
        };
        let result = call.result.as_ref().map(|result| match &result.body {
            ToolResultBody::Text(text) => {
                if pretty {
                    serde_json::from_str::<serde_json::Value>(text).unwrap_or_else(|_| json!(text))
                } else {
                    json!(text)
                }
            }
            ToolResultBody::ContentItems(items) => json!(items),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "turnId": call.turn_id,
                "callId": call.call_id,
                "kind": tool_kind_str(call.tool.kind),
                "namespace": call.tool.namespace,
                "name": call.tool.name,
                "completeness": completeness_str(&call.completeness),
                "arguments": arguments,
                "result": result,
                "callSourceLine": call.call_source.line,
                "resultSourceLine": call.result_source.as_ref().map(|s| s.line),
            }))?
        );
        return Ok(());
    }

    let color = stdout_wants_color();
    println!("{} {}", label("call_id", color), call.call_id);
    if let Some(turn) = &call.turn_id {
        println!("{} {turn}", label("turn_id", color));
    }
    println!("{} {}", label("tool", color), format_tool_name(call));
    println!("{} {}", label("kind", color), tool_kind_str(call.tool.kind));
    println!(
        "{} {}",
        label("completeness", color),
        completeness_str(&call.completeness)
    );
    if let Completeness::Truncated { markers } = &call.completeness {
        println!(
            "{}",
            emphasize(
                "WARNING: persisted result contains upstream truncation marker(s).",
                color
            )
        );
        for marker in markers {
            println!(
                "  - {:?} @ byte {} ({})",
                marker.kind, marker.byte_offset, marker.matched_text
            );
        }
    }

    println!("{}", label("arguments", color));
    println!("{}", format_arguments(call, pretty));
    println!("{}", label("result", color));
    match &call.result {
        Some(result) => println!("{}", format_result_body(&result.body, pretty)),
        None => println!("(no persisted result)"),
    }
    Ok(())
}

struct RecordSummary {
    calls: usize,
    truncated: usize,
    open: usize,
    orphans: usize,
    unknown: usize,
}

fn summarize_records(records: &SessionToolRecords) -> RecordSummary {
    let truncated = records
        .calls
        .iter()
        .filter(|call| call.completeness.is_truncated())
        .count();
    let open = records
        .calls
        .iter()
        .filter(|call| call.result.is_none())
        .count();
    RecordSummary {
        calls: records.calls.len(),
        truncated,
        open,
        orphans: records.orphan_outputs.len(),
        unknown: records.unknown_records.len(),
    }
}

async fn resolve_session_path(
    thread_id: Option<&str>,
    last: bool,
    file: Option<&Path>,
    archived: bool,
) -> Result<PathBuf, DebugSessionError> {
    match (file, last, thread_id) {
        (Some(path), false, None) => {
            if !path.exists() {
                return Err(DebugSessionError::MissingSession(format!(
                    "session file not found: {}",
                    path.display()
                )));
            }
            Ok(path.to_path_buf())
        }
        (Some(_), _, _) => Err(DebugSessionError::Other(anyhow::anyhow!(
            "--file cannot be combined with --last or THREAD_ID"
        ))),
        (None, true, None) => resolve_last_session(archived).await,
        (None, false, Some(id)) => resolve_thread_id(id, archived).await,
        (None, true, Some(_)) => Err(DebugSessionError::Other(anyhow::anyhow!(
            "pass either --last or THREAD_ID, not both"
        ))),
        (None, false, None) => Err(DebugSessionError::Other(anyhow::anyhow!(
            "provide THREAD_ID, --last, or --file"
        ))),
    }
}

async fn resolve_last_session(archived: bool) -> Result<PathBuf, DebugSessionError> {
    let codex_home = find_codex_home().context("failed to resolve CODEX_HOME")?;
    let page = if archived {
        get_threads_in_root(
            codex_home.join(ARCHIVED_SESSIONS_SUBDIR).to_path_buf(),
            1,
            /*cursor*/ None,
            ThreadSortKey::UpdatedAt,
            codex_rollout::ThreadListConfig {
                allowed_sources: &[],
                model_providers: None,
                cwd_filters: None,
                default_provider: "openai",
                layout: codex_rollout::ThreadListLayout::Flat,
            },
        )
        .await
        .context("failed to list archived sessions")?
    } else {
        get_threads(
            codex_home.as_path(),
            1,
            /*cursor*/ None,
            ThreadSortKey::UpdatedAt,
            &[],
            /*model_providers*/ None,
            /*cwd_filters*/ None,
            "openai",
        )
        .await
        .context("failed to list sessions")?
    };
    page.items
        .into_iter()
        .next()
        .map(|item| item.path)
        .ok_or_else(|| {
            DebugSessionError::MissingSession(format!(
                "no sessions found under {}",
                codex_home.display()
            ))
        })
}

async fn resolve_thread_id(id: &str, archived: bool) -> Result<PathBuf, DebugSessionError> {
    let codex_home = find_codex_home().context("failed to resolve CODEX_HOME")?;
    let found = if archived {
        find_archived_thread_path_by_id_str(&codex_home, id, /*state_db_ctx*/ None).await
    } else {
        find_thread_path_by_id_str(&codex_home, id, /*state_db_ctx*/ None).await
    }
    .context("failed while searching for session")?;

    if let Some(path) = found {
        return Ok(path);
    }

    if !archived {
        // Fall back to archived when the id is not in active sessions.
        if let Some(path) =
            find_archived_thread_path_by_id_str(&codex_home, id, /*state_db_ctx*/ None)
                .await
                .context("failed while searching archived sessions")?
        {
            return Ok(path);
        }
    }

    Err(DebugSessionError::MissingSession(format!(
        "session `{id}` not found under {}",
        codex_home.display()
    )))
}

fn format_arguments(call: &ToolCallRecord, pretty: bool) -> String {
    if pretty && let Some(value) = &call.arguments.parsed_json {
        return serde_json::to_string_pretty(value).unwrap_or_else(|_| call.arguments.raw.clone());
    }
    call.arguments.raw.clone()
}

fn format_result_body(body: &ToolResultBody, pretty: bool) -> String {
    match body {
        ToolResultBody::Text(text) => {
            if pretty && let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
                return serde_json::to_string_pretty(&value).unwrap_or_else(|_| text.clone());
            }
            text.clone()
        }
        ToolResultBody::ContentItems(items) => {
            serde_json::to_string_pretty(items).unwrap_or_else(|_| format!("{items:?}"))
        }
    }
}

fn format_tool_name(call: &ToolCallRecord) -> String {
    match &call.tool.namespace {
        Some(namespace) => format!("{namespace}/{}", call.tool.name),
        None => call.tool.name.clone(),
    }
}

fn tool_kind_str(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Function => "function",
        ToolKind::Custom => "custom",
    }
}

fn completeness_str(completeness: &Completeness) -> &'static str {
    match completeness {
        Completeness::Complete => "complete",
        Completeness::Truncated { .. } => "truncated",
        Completeness::Unknown => "unknown",
    }
}

fn stdout_wants_color() -> bool {
    std::io::stdout().is_terminal() && supports_color::on(supports_color::Stream::Stdout).is_some()
}

fn header(text: &str, color: bool) -> String {
    if color {
        text.bold().to_string()
    } else {
        text.to_string()
    }
}

fn label(text: &str, color: bool) -> String {
    if color {
        format!("{}:", text.dimmed())
    } else {
        format!("{text}:")
    }
}

fn emphasize(text: &str, color: bool) -> String {
    if color {
        text.yellow().to_string()
    } else {
        text.to_string()
    }
}

fn format_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    if bytes >= MIB {
        format!("{:.1}MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1}KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes}B")
    }
}

fn truncate_middle(input: &str, max_chars: usize) -> String {
    let chars: Vec<char> = input.chars().collect();
    if chars.len() <= max_chars {
        return input.to_string();
    }
    if max_chars <= 3 {
        return chars.into_iter().take(max_chars).collect();
    }
    let keep = max_chars - 1;
    let head = keep / 2;
    let tail = keep - head;
    let mut out: String = chars.iter().take(head).collect();
    out.push('…');
    out.extend(
        chars
            .iter()
            .rev()
            .take(tail)
            .collect::<Vec<_>>()
            .into_iter()
            .rev(),
    );
    out
}
