//! The agent surface over the Model Context Protocol, on stdio.
//!
//! `assert`, `resolve`, `find`, `wait`, and `diff` exist because the real
//! consumer of a coordinate is a machine. Reaching them still required a
//! human to wire a subprocess call and parse stdout, which made the tool
//! invisible to the agent it was built for. This is the transport.
//!
//! **Read-only, by necessity.** An agent cannot mark regions — the
//! overlay is interactive — so every tool here answers questions about a
//! session a human already saved. The workflow is mark once, run many,
//! and the tool descriptions say so: a model that thinks it can create a
//! session will burn turns discovering it cannot.
//!
//! # Protocol
//!
//! Revision `2026-07-28`, which removed the `initialize` handshake and
//! protocol-level sessions. Every request carries its own protocol
//! version in `_meta`, and a conformant stdio server is three methods
//! and a read loop — which is what makes hand-rolling this reasonable
//! rather than pulling in an SDK and an async runtime.
//!
//! `initialize` is answered anyway. The revision is new, clients pinned
//! to `2025-11-25` still open with it, and a server that ignores them
//! fails in a way the user will blame on this tool rather than on their
//! client.

use std::io::{BufRead, Write};

use anyhow::Result;
use serde_json::{Map, Value, json};

use crate::capture::CaptureProvider;

/// Protocol revisions this server speaks, newest first.
///
/// `2025-11-25` is listed because it is answered through the legacy
/// `initialize` path, not because the transport differs — on stdio the
/// framing is identical and the handshake is the only difference.
const PROTOCOL_VERSIONS: [&str; 2] = ["2026-07-28", "2025-11-25"];

/// `_meta` key carrying the protocol version of an individual request.
const META_PROTOCOL_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
/// `_meta` key servers identify themselves under, in every result.
const META_SERVER_INFO: &str = "io.modelcontextprotocol/serverInfo";

/// `UnsupportedProtocolVersionError`, renumbered from -32004 into the
/// band this revision reserves for the specification.
const ERR_UNSUPPORTED_PROTOCOL: i64 = -32022;
const ERR_PARSE: i64 = -32700;
const ERR_INVALID_REQUEST: i64 = -32600;
const ERR_METHOD_NOT_FOUND: i64 = -32601;
const ERR_INVALID_PARAMS: i64 = -32602;

/// How long a client may cache `tools/list`. The list is a compile-time
/// constant, so this could be days; an hour keeps a stale client from
/// missing a new tool for a whole session.
const TOOLS_TTL_MS: u64 = 3_600_000;

/// The longest `wait` an MCP caller may ask for, in seconds.
///
/// Stdio is one client and one thread, so a blocking `wait` is fine —
/// but a model that asks for ten minutes freezes the server for ten
/// minutes, and nothing in the protocol lets it change its mind. The CLI
/// has no such ceiling because a human can press Ctrl-C.
const MAX_WAIT_SECS: u64 = 120;

/// Serve on stdio until stdin closes.
///
/// Newline-delimited JSON-RPC in, the same out, stderr left for logging —
/// which is both the stdio rule and what `env_logger` already does.
pub fn serve<P: CaptureProvider>(provider: &P) -> Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Some(response) = handle_line(provider, &line) else {
            // A notification: no id, so no reply. The protocol removed
            // every notification this server would care about, but a
            // client may still send one and must not be answered.
            continue;
        };
        writeln!(stdout, "{response}")?;
        stdout.flush()?;
    }
    Ok(())
}

/// One line in, at most one line out.
///
/// Split from [`serve`] because it is pure: the whole dispatch surface is
/// JSON in and JSON out, so it tests without a window system or a pipe.
fn handle_line<P: CaptureProvider>(provider: &P, line: &str) -> Option<String> {
    let request: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(e) => return Some(error_response(&Value::Null, ERR_PARSE, &format!("{e}"))),
    };
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    if id.is_null() {
        return None;
    }
    if request.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Some(error_response(
            &id,
            ERR_INVALID_REQUEST,
            "every request needs \"jsonrpc\": \"2.0\"",
        ));
    }
    let Some(method) = request.get("method").and_then(Value::as_str) else {
        return Some(error_response(&id, ERR_INVALID_REQUEST, "no method"));
    };
    let params = request.get("params");

    // The version travels per-request now. Absent means a client that
    // predates the field, which the legacy handshake already covers.
    if let Some(version) = params
        .and_then(|p| p.get("_meta"))
        .and_then(|m| m.get(META_PROTOCOL_VERSION))
        .and_then(Value::as_str)
        && !PROTOCOL_VERSIONS.contains(&version)
    {
        return Some(error_response(
            &id,
            ERR_UNSUPPORTED_PROTOCOL,
            &format!(
                "protocol version {version} is not supported; this server speaks {}",
                PROTOCOL_VERSIONS.join(", ")
            ),
        ));
    }

    let outcome = match method {
        "server/discover" => Ok(discover()),
        // Older clients open with this. Answering costs a static
        // document and buys every client shipping today.
        "initialize" => Ok(initialize()),
        "tools/list" => Ok(tools_list()),
        "tools/call" => call_tool(provider, params),
        other => {
            return Some(error_response(
                &id,
                ERR_METHOD_NOT_FOUND,
                &format!("unknown method {other}"),
            ));
        }
    };
    Some(match outcome {
        Ok(result) => success_response(&id, result),
        Err(message) => error_response(&id, ERR_INVALID_PARAMS, &message),
    })
}

fn server_info() -> Value {
    json!({
        "name": "pixelcoords",
        "title": "pixelcoords",
        "version": env!("CARGO_PKG_VERSION"),
    })
}

/// Every result carries `resultType` and identifies the server, both
/// required by this revision.
fn success_response(id: &Value, mut result: Value) -> String {
    if let Some(object) = result.as_object_mut() {
        object.insert("resultType".into(), json!("complete"));
        let meta = object
            .entry("_meta")
            .or_insert_with(|| Value::Object(Map::new()));
        if let Some(meta) = meta.as_object_mut() {
            meta.insert(META_SERVER_INFO.into(), server_info());
        }
    }
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

fn error_response(id: &Value, code: i64, message: &str) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
    .to_string()
}

fn capabilities() -> Value {
    // Tools only. Resources and prompts would have nothing to serve that
    // a tool call does not already answer.
    json!({ "tools": {} })
}

fn discover() -> Value {
    json!({
        "protocolVersions": PROTOCOL_VERSIONS,
        "capabilities": capabilities(),
        "serverInfo": server_info(),
    })
}

fn initialize() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSIONS[0],
        "capabilities": capabilities(),
        "serverInfo": server_info(),
    })
}

/// The tools, in a fixed order.
///
/// Deterministic because the spec asks for it: a stable list is what lets
/// a client cache and a model's prompt cache hit. Alphabetical would put
/// `assert` first; this order is the one an agent actually walks —
/// find a session, ask where to click, check it landed, then the
/// capture-backed three.
const TOOLS: [&str; 6] = [
    "pixelcoords_sessions",
    "pixelcoords_resolve",
    "pixelcoords_assert",
    "pixelcoords_wait",
    "pixelcoords_find",
    "pixelcoords_diff",
];

fn session_arg() -> Value {
    json!({
        "type": "string",
        "description":
            "Path to a session directory or its session.json, as returned by \
             pixelcoords_sessions.",
    })
}

fn label_arg(what: &str) -> Value {
    json!({
        "type": "string",
        "description": format!(
            "Restrict to regions with this label, case-insensitive. Omit for {what}. \
             Labels come from pixelcoords_sessions.",
        ),
    })
}

fn tool_schema(name: &str) -> Value {
    match name {
        "pixelcoords_sessions" => schema_sessions(),
        "pixelcoords_resolve" => schema_resolve(),
        "pixelcoords_assert" => schema_assert(),
        "pixelcoords_wait" => schema_wait(),
        "pixelcoords_find" => schema_find(),
        "pixelcoords_diff" => schema_diff(),
        _ => Value::Null,
    }
}

fn schema_sessions() -> Value {
    json!({
            "name": "pixelcoords_sessions",
            "title": "List saved sessions",
            "description":
                "List the sessions on this machine, newest first, with the labels each one \
                 holds. Start here: every other tool needs a session, and a session is a set \
                 of screen regions a human marked in the pixelcoords overlay. This tool \
                 cannot create one — if nothing is listed, ask the user to run `pixelcoords` \
                 and mark the regions they care about. Does not capture the screen.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "root": {
                        "type": "string",
                        "description":
                            "Directory to search. Defaults to the captures folder \
                             pixelcoords saves to.",
                    },
                },
                "additionalProperties": false,
            },
    })
}

fn schema_resolve() -> Value {
    json!({
            "name": "pixelcoords_resolve",
            "title": "Where to click now",
            "description":
                "Return the click point for each marked region, in the units this platform's \
                 input APIs expect. This is the tool to reach for by default: it reads a file \
                 and answers in microseconds, sends no image, and needs no screen-recording \
                 permission. Set relocate only when the UI may have moved since the session \
                 was saved — that captures the screen and costs far more.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session": session_arg(),
                    "label": label_arg("every region"),
                    "space": {
                        "type": "string",
                        "enum": ["global", "monitor", "window"],
                        "description":
                            "Which origin the answer is measured from. Default global, the \
                             whole-desktop grid most input APIs use.",
                    },
                    "units": {
                        "type": "string",
                        "enum": ["auto", "physical", "logical"],
                        "description":
                            "Default auto: logical points on macOS, physical pixels on \
                             Windows and X11 — what the platform's input APIs take.",
                    },
                    "relocate": {
                        "type": "boolean",
                        "description":
                            "Search the live screen for each region first, and answer where it \
                             is now. Captures the screen. Default false.",
                    },
                },
                "required": ["session"],
                "additionalProperties": false,
            },
    })
}

fn schema_assert() -> Value {
    json!({
            "name": "pixelcoords_assert",
            "title": "Did this point land in the right region",
            "description":
                "Score a point against the marked regions: which one it landed in, and \
                 whether that was the expected one. Use it to confirm a click went where you \
                 meant before acting on the result. A miss is an answer, not a failure — the \
                 result says ok false and names the region the point actually hit. Does not \
                 capture the screen.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session": session_arg(),
                    "point": {
                        "type": "string",
                        "description": "The point to score, as \"x,y\".",
                    },
                    "points": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description":
                            "Score a whole trajectory in one call, one \"x,y\" per entry. \
                             Reads the session once instead of once per point.",
                    },
                    "expect": {
                        "type": "string",
                        "description":
                            "The label the point should land in. Omit to ask only which \
                             region it hit.",
                    },
                    "space": {
                        "type": "string",
                        "enum": ["global", "monitor", "window"],
                        "description": "Which origin the point is measured from. Default global.",
                    },
                    "monitor": {
                        "type": "integer",
                        "description": "Which monitor, when space is monitor.",
                    },
                },
                "required": ["session"],
                "additionalProperties": false,
            },
    })
}

fn schema_wait() -> Value {
    json!({
            "name": "pixelcoords_wait",
            "title": "Block until the screen settles",
            "description":
                "Poll until a marked region matches its saved appearance again, or stops \
                 matching. Use it instead of taking screenshots in a loop to find out whether \
                 a dialog has appeared or a spinner has finished. Captures the screen on every \
                 poll and blocks until it resolves or the timeout runs out; a timeout is an \
                 answer, reported as ok false.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session": session_arg(),
                    "label": label_arg("every region"),
                    "condition": {
                        "type": "string",
                        "enum": ["match", "change"],
                        "description":
                            "match waits for the region to look like its saved crop again; \
                             change waits for it to stop. Default match.",
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": MAX_WAIT_SECS,
                        "description": format!(
                            "How long to wait, in seconds. Default 30, maximum {MAX_WAIT_SECS}. \
                             The server answers nothing else while this runs.",
                        ),
                    },
                    "interval_ms": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Milliseconds between polls. Default 500.",
                    },
                    "min_score": {
                        "type": "number",
                        "description":
                            "Match threshold, 0 to 1. Default 0.9. Lower it when something \
                             small moves inside the region on its own, such as a blinking \
                             text cursor, which otherwise reads as a change.",
                    },
                },
                "required": ["session"],
                "additionalProperties": false,
            },
    })
}

fn schema_find() -> Value {
    json!({
            "name": "pixelcoords_find",
            "title": "Where did this region move to",
            "description":
                "Search the live screen for each marked region and report where it is now, \
                 with the offset from where it was saved. Use it when the UI has drifted — a \
                 resized window, a scrolled page — and the saved coordinates no longer land. \
                 Captures the screen and is the most expensive tool here; prefer \
                 pixelcoords_resolve when nothing has moved.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session": session_arg(),
                    "label": label_arg("every region"),
                },
                "required": ["session"],
                "additionalProperties": false,
            },
    })
}

fn schema_diff() -> Value {
    json!({
            "name": "pixelcoords_diff",
            "title": "Do these regions still look right",
            "description":
                "Compare each marked region against the screen now, and report how much of it \
                 changed. Scoped to the regions a human marked rather than whole screenshots, \
                 so unrelated movement elsewhere is not a difference. Captures the screen \
                 unless against names stored images. Over-tolerance is an answer, reported as \
                 ok false.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session": session_arg(),
                    "label": label_arg("every region"),
                    "against": {
                        "type": "string",
                        "description":
                            "Directory of stored screenshots to compare against instead of the \
                             live screen. Avoids capturing.",
                    },
                    "tolerance": {
                        "type": "number",
                        "description":
                            "Percent of a region's pixels allowed to differ. Default 0, exact.",
                    },
                },
                "required": ["session"],
                "additionalProperties": false,
            },
    })
}

fn tools_list() -> Value {
    json!({
        "tools": TOOLS.iter().map(|name| tool_schema(name)).collect::<Vec<_>>(),
        // Required by this revision. `public` because the list is a
        // compile-time constant with nothing caller-specific in it.
        "ttlMs": TOOLS_TTL_MS,
        "cacheScope": "public",
    })
}

/// Read a required string argument, or say which one was missing.
fn arg_str<'a>(args: &'a Map<String, Value>, key: &str) -> Result<&'a str, String> {
    match args.get(key) {
        Some(Value::String(s)) => Ok(s),
        Some(other) => Err(format!("{key} must be a string, got {other}")),
        None => Err(format!("{key} is required")),
    }
}

fn opt_str<'a>(args: &'a Map<String, Value>, key: &str) -> Result<Option<&'a str>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(other) => Err(format!("{key} must be a string, got {other}")),
    }
}

fn opt_bool(args: &Map<String, Value>, key: &str) -> Result<Option<bool>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(other) => Err(format!("{key} must be a boolean, got {other}")),
    }
}

fn opt_u64(args: &Map<String, Value>, key: &str) -> Result<Option<u64>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n
            .as_u64()
            .map(Some)
            .ok_or_else(|| format!("{key} must be a positive whole number, got {n}")),
        Some(other) => Err(format!("{key} must be a number, got {other}")),
    }
}

fn opt_f64(args: &Map<String, Value>, key: &str) -> Result<Option<f64>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => Ok(n.as_f64()),
        Some(other) => Err(format!("{key} must be a number, got {other}")),
    }
}

/// Reject arguments the schema does not name.
///
/// Strict on params, deliberately lenient on `_meta`: the spec keeps
/// extending that namespace, and a server that rejects a key it has not
/// heard of breaks the next time the protocol grows.
fn reject_unknown(args: &Map<String, Value>, allowed: &[&str]) -> Result<(), String> {
    for key in args.keys() {
        if key != "_meta" && !allowed.contains(&key.as_str()) {
            return Err(format!(
                "unknown argument {key}; this tool takes {}",
                allowed.join(", ")
            ));
        }
    }
    Ok(())
}

fn space_from(args: &Map<String, Value>) -> Result<crate::cli::SpaceArg, String> {
    match opt_str(args, "space")? {
        None | Some("global") => Ok(crate::cli::SpaceArg::Global),
        Some("monitor") => Ok(crate::cli::SpaceArg::Monitor),
        Some("window") => Ok(crate::cli::SpaceArg::Window),
        Some(other) => Err(format!(
            "space must be global, monitor, or window; got {other}"
        )),
    }
}

fn units_from(args: &Map<String, Value>) -> Result<crate::cli::UnitsArg, String> {
    match opt_str(args, "units")? {
        None | Some("auto") => Ok(crate::cli::UnitsArg::Auto),
        Some("physical") => Ok(crate::cli::UnitsArg::Physical),
        Some("logical") => Ok(crate::cli::UnitsArg::Logical),
        Some(other) => Err(format!(
            "units must be auto, physical, or logical; got {other}"
        )),
    }
}

/// Wrap a command's report as a tool result.
///
/// **`ok: false` does not set `isError`.** The whole agent surface rests
/// on the difference between a negative answer and a broken question: a
/// miss, a timeout, an over-tolerance diff are answers, and the exit
/// codes have said so since 0.4.0. Reported as errors, a calling model
/// sees a broken tool and retries instead of reacting — which is worse
/// than not serving the tool at all.
///
/// `isError` is reserved for what the CLI exits 2 for, and that path
/// comes back as `Err` from the functions below.
/// Takes the report already serialized rather than a `Serialize` bound,
/// because `serde` is not a direct dependency of this crate and adding
/// one to name a trait would be a manifest entry for nothing.
fn report_result(structured: &Value, summary: &str) -> Value {
    json!({
        "content": [{ "type": "text", "text": summary }],
        "structuredContent": structured,
        "isError": false,
    })
}

fn call_tool<P: CaptureProvider>(provider: &P, params: Option<&Value>) -> Result<Value, String> {
    let params = params.ok_or("tools/call needs params")?;
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or("tools/call needs a tool name")?;
    let empty = Map::new();
    let args = match params.get("arguments") {
        None | Some(Value::Null) => &empty,
        Some(Value::Object(map)) => map,
        Some(other) => return Err(format!("arguments must be an object, got {other}")),
    };
    match name {
        "pixelcoords_sessions" => tool_sessions(args),
        "pixelcoords_resolve" => tool_resolve(provider, args),
        "pixelcoords_assert" => tool_assert(args),
        "pixelcoords_wait" => tool_wait(provider, args),
        "pixelcoords_find" => tool_find(provider, args),
        "pixelcoords_diff" => tool_diff(provider, args),
        other => Err(format!(
            "unknown tool {other}; this server serves {}",
            TOOLS.join(", ")
        )),
    }
}

/// A session's identity and, crucially, its labels.
///
/// The CLI's `SessionEntry` carries a prose summary for a human picker.
/// A model needs the labels as data: without them its first move is a
/// `resolve` with no label purely to discover what the session holds.
fn tool_sessions(args: &Map<String, Value>) -> Result<Value, String> {
    reject_unknown(args, &["root"])?;
    let root = match opt_str(args, "root")? {
        Some(path) => std::path::PathBuf::from(path),
        None => crate::captures_root(dirs::download_dir()),
    };
    let entries = crate::sessions_under(&root);
    let listed: Vec<Value> = entries
        .iter()
        .map(|entry| {
            json!({
                "path": entry.path.to_string_lossy(),
                "name": entry.name,
                "created_utc": entry.created,
                "labels": entry.labels,
                "summary": entry.summary,
            })
        })
        .collect();
    let summary = if listed.is_empty() {
        format!(
            "No sessions under {}. A session is a set of screen regions a human marked in \
             the pixelcoords overlay — ask the user to run `pixelcoords`, mark what matters, \
             and press S.",
            root.display()
        )
    } else {
        format!("{} session(s) under {}", listed.len(), root.display())
    };
    Ok(json!({
        "content": [{ "type": "text", "text": summary }],
        "structuredContent": { "root": root.to_string_lossy(), "sessions": listed },
        "isError": false,
    }))
}

fn tool_resolve<P: CaptureProvider>(
    provider: &P,
    args: &Map<String, Value>,
) -> Result<Value, String> {
    reject_unknown(args, &["session", "label", "space", "units", "relocate"])?;
    let session = std::path::PathBuf::from(arg_str(args, "session")?);
    let label = opt_str(args, "label")?;
    let space = space_from(args)?;
    let units = units_from(args)?;
    let relocate = opt_bool(args, "relocate")?.unwrap_or(false);
    let report = crate::run_resolve(provider, &session, label, space, units, relocate)
        .map_err(|e| format!("{e:#}"))?;
    let summary = format!(
        "{} region(s) resolved{}",
        report.results.len(),
        if relocate { ", relocated" } else { "" }
    );
    Ok(report_result(
        &serde_json::to_value(&report).map_err(|e| format!("serializing the report: {e}"))?,
        &summary,
    ))
}

fn tool_assert(args: &Map<String, Value>) -> Result<Value, String> {
    use pixelcoords_core::report::{Command, Report};
    reject_unknown(
        args,
        &["session", "point", "points", "expect", "space", "monitor"],
    )?;
    let session = std::path::PathBuf::from(arg_str(args, "session")?);
    let expect = opt_str(args, "expect")?;
    let space = space_from(args)?;
    let monitor = opt_u64(args, "monitor")?
        .map(usize::try_from)
        .transpose()
        .map_err(|_| "monitor is out of range".to_string())?;

    let verdicts = match (args.get("point"), args.get("points")) {
        (Some(_), Some(_)) => {
            return Err("give point or points, not both".into());
        }
        (None, None) => return Err("point or points is required".into()),
        (Some(_), None) => {
            let point = arg_str(args, "point")?;
            vec![
                crate::assess_session(&session, point, expect, space, monitor)
                    .map_err(|e| format!("{e:#}"))?,
            ]
        }
        (None, Some(Value::Array(items))) => {
            // One reader over the whole trajectory, so the session is
            // parsed once — the same saving `assert --stdin` exists for.
            let mut lines = String::new();
            for item in items {
                let Value::String(text) = item else {
                    return Err(format!(
                        "every entry in points must be a string, got {item}"
                    ));
                };
                lines.push_str(text);
                lines.push('\n');
            }
            crate::assess_stream(&session, lines.as_bytes(), expect, space, monitor)
                .map_err(|e| format!("{e:#}"))?
        }
        (None, Some(other)) => {
            return Err(format!("points must be an array of strings, got {other}"));
        }
    };

    // No `captured_utc`: scoring a point is pure session math, and
    // stamping a time on it would imply a capture that never happened.
    let ok = verdicts.iter().all(|v| v.hit);
    let hits = verdicts.iter().filter(|v| v.hit).count();
    let report = Report::offline(Command::Assert, ok, verdicts);
    Ok(report_result(
        &serde_json::to_value(&report).map_err(|e| format!("serializing the report: {e}"))?,
        &format!("{hits}/{} point(s) hit", report.results.len()),
    ))
}

fn tool_wait<P: CaptureProvider>(provider: &P, args: &Map<String, Value>) -> Result<Value, String> {
    use pixelcoords_core::wait::Condition;
    reject_unknown(
        args,
        &[
            "session",
            "label",
            "condition",
            "timeout_secs",
            "interval_ms",
            "min_score",
        ],
    )?;
    let session = std::path::PathBuf::from(arg_str(args, "session")?);
    let label = opt_str(args, "label")?;
    let condition = match opt_str(args, "condition")? {
        None | Some("match") => Condition::Match,
        Some("change") => Condition::Change,
        Some(other) => return Err(format!("condition must be match or change; got {other}")),
    };
    let timeout = opt_u64(args, "timeout_secs")?.unwrap_or(30);
    if timeout == 0 || timeout > MAX_WAIT_SECS {
        return Err(format!(
            "timeout_secs must be 1 to {MAX_WAIT_SECS}; got {timeout}. The server answers \
             nothing else while a wait runs, so the ceiling is deliberate."
        ));
    }
    let interval = opt_u64(args, "interval_ms")?.unwrap_or(500);
    if interval == 0 {
        return Err("interval_ms must be at least 1".into());
    }
    let min_score = opt_f64(args, "min_score")?.unwrap_or(pixelcoords_core::locate::SCORE_FLOOR);
    // Bounded here rather than left to `wait_setup`, which words its
    // refusal for a command line: it names `--min-score` and contrasts it
    // with diff's `--tolerance`, and a caller who passed `min_score` in
    // JSON has neither flag and cannot act on either name.
    if !(0.0..=1.0).contains(&min_score) {
        return Err(format!(
            "min_score must be between 0 and 1 — it is a correlation score, not a \
             percentage; got {min_score}"
        ));
    }
    let (budget, interval) =
        crate::wait_setup(&format!("{timeout}s"), &format!("{interval}ms"), min_score)
            .map_err(|e| format!("{e:#}"))?;
    let report = crate::run_wait(
        provider, &session, label, condition, budget, interval, min_score,
    )
    .map_err(|e| format!("{e:#}"))?;
    let summary = if report.ok {
        format!("condition met after {} poll(s)", report.polls.unwrap_or(0))
    } else {
        format!(
            "timed out after {} poll(s) — the condition never held",
            report.polls.unwrap_or(0)
        )
    };
    Ok(report_result(
        &serde_json::to_value(&report).map_err(|e| format!("serializing the report: {e}"))?,
        &summary,
    ))
}

fn tool_find<P: CaptureProvider>(provider: &P, args: &Map<String, Value>) -> Result<Value, String> {
    reject_unknown(args, &["session", "label"])?;
    let session = std::path::PathBuf::from(arg_str(args, "session")?);
    let label = opt_str(args, "label")?;
    let report = crate::run_find(provider, &session, label).map_err(|e| format!("{e:#}"))?;
    let found = report.results.iter().filter(|r| r.found).count();
    Ok(report_result(
        &serde_json::to_value(&report).map_err(|e| format!("serializing the report: {e}"))?,
        &format!("{found}/{} region(s) located", report.results.len()),
    ))
}

fn tool_diff<P: CaptureProvider>(provider: &P, args: &Map<String, Value>) -> Result<Value, String> {
    reject_unknown(args, &["session", "label", "against", "tolerance"])?;
    let session = std::path::PathBuf::from(arg_str(args, "session")?);
    let label = opt_str(args, "label")?;
    let against = opt_str(args, "against")?.map(std::path::PathBuf::from);
    let tolerance = opt_f64(args, "tolerance")?.unwrap_or(0.0);
    // As above: `run_diff` refuses this by naming `--tolerance`.
    if !(0.0..=100.0).contains(&tolerance) {
        return Err(format!(
            "tolerance must be between 0 and 100 — it is the percentage of a region's \
             pixels allowed to differ; got {tolerance}"
        ));
    }
    let report = crate::run_diff(provider, &session, against.as_deref(), label, tolerance)
        .map_err(|e| format!("{e:#}"))?;
    // `ok` is the aggregate the exit code mirrors; a row carries its own
    // `changed_pct` rather than a verdict, so the count comes from the
    // tolerance the caller asked for.
    let over = report
        .results
        .iter()
        .filter(|r| r.diff.changed_pct > tolerance)
        .count();
    let summary = if report.ok {
        format!("{} region(s) within tolerance", report.results.len())
    } else {
        format!(
            "{over}/{} region(s) changed beyond tolerance",
            report.results.len()
        )
    };
    Ok(report_result(
        &serde_json::to_value(&report).map_err(|e| format!("serializing the report: {e}"))?,
        &summary,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::FakeCapture;
    use serde_json::json;

    /// Drive one request the way stdin would, and parse the reply.
    fn ask(request: &Value) -> Value {
        let line = handle_line(&FakeCapture, &request.to_string()).expect("a reply");
        serde_json::from_str(&line).expect("valid JSON out")
    }

    fn result(request: &Value) -> Value {
        let mut reply = ask(request);
        reply["result"].take()
    }

    fn error_code(request: &Value) -> i64 {
        ask(request)["error"]["code"]
            .as_i64()
            .expect("an error code")
    }

    /// A session on disk with two labelled regions and no crops — enough
    /// for the headless tools, which never look at pixels.
    fn fixture(name: &str) -> std::path::PathBuf {
        use pixelcoords_core::geometry::{Point, Rect, Shape, Size};
        use pixelcoords_core::selection::Selection;
        use pixelcoords_core::session::{MonitorRecord, SessionFile};
        let mut email = Selection::new(Shape::Rect(Rect::new(10, 10, 40, 20)), 0);
        email.label = "email".into();
        let mut submit = Selection::new(Shape::Rect(Rect::new(10, 60, 40, 20)), 0);
        submit.label = "submit".into();
        let file = SessionFile::build(
            "test",
            "2026-08-03T00:00:00Z".into(),
            vec![MonitorRecord {
                index: 0,
                name: "Fake".into(),
                primary: true,
                origin_px: Point::new(0, 0),
                size_px: Size::new(160, 120),
                scale: 2.0,
            }],
            &[email, submit],
            &["crop-0-email.png".into(), "crop-1-submit.png".into()],
            None,
        );
        let dir = std::env::temp_dir().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("session.json"),
            serde_json::to_string(&file).unwrap(),
        )
        .unwrap();
        dir
    }

    fn call(tool: &str, arguments: &Value) -> Value {
        result(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": tool, "arguments": arguments },
        }))
    }

    // ---- protocol ----------------------------------------------------

    #[test]
    fn discover_advertises_versions_capabilities_and_identity() {
        let r = result(&json!({"jsonrpc": "2.0", "id": 1, "method": "server/discover"}));
        assert_eq!(r["protocolVersions"][0], "2026-07-28");
        assert!(r["capabilities"]["tools"].is_object());
        assert_eq!(r["serverInfo"]["name"], "pixelcoords");
        assert_eq!(r["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn every_result_carries_result_type_and_server_info() {
        // Both required by this revision, and both easy to forget on a
        // path that was added later.
        for method in ["server/discover", "initialize", "tools/list"] {
            let r = result(&json!({"jsonrpc": "2.0", "id": 1, "method": method}));
            assert_eq!(r["resultType"], "complete", "{method}");
            assert_eq!(
                r["_meta"][META_SERVER_INFO]["name"], "pixelcoords",
                "{method}"
            );
        }
    }

    #[test]
    fn older_clients_can_still_open_with_initialize() {
        // The revision removed it. Clients pinned to 2025-11-25 have not,
        // and a server that ignores them fails in a way the user blames
        // on this tool rather than on their client.
        let r = result(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"}));
        assert_eq!(r["protocolVersion"], "2026-07-28");
        assert!(r["capabilities"]["tools"].is_object());
    }

    #[test]
    fn tools_list_is_deterministic_and_cacheable() {
        let first = result(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}));
        let again = result(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}));
        let names: Vec<&str> = first["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, TOOLS, "order is what lets a client cache");
        assert_eq!(first["tools"], again["tools"]);
        assert!(first["ttlMs"].as_u64().is_some_and(|ms| ms > 0));
        assert_eq!(first["cacheScope"], "public");
    }

    #[test]
    fn every_tool_declares_a_schema_and_says_whether_it_captures() {
        // The capture note is not decoration: on macOS it is the
        // difference between an instant answer and a permission prompt,
        // and the model can only weigh that if the description says so.
        let r = result(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}));
        for tool in r["tools"].as_array().unwrap() {
            let name = tool["name"].as_str().unwrap();
            assert!(tool["description"].is_string(), "{name} has no description");
            assert!(
                tool["inputSchema"]["type"] == "object",
                "{name} has no object schema"
            );
            assert_eq!(
                tool["inputSchema"]["additionalProperties"], false,
                "{name} accepts unknown arguments"
            );
            let described = tool["description"].as_str().unwrap().to_lowercase();
            assert!(
                described.contains("captur"),
                "{name} does not say whether it captures: {described}"
            );
        }
    }

    #[test]
    fn an_unsupported_protocol_version_is_refused_with_the_reserved_code() {
        assert_eq!(
            error_code(&json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/list",
                "params": { "_meta": { META_PROTOCOL_VERSION: "1999-01-01" } },
            })),
            ERR_UNSUPPORTED_PROTOCOL
        );
    }

    #[test]
    fn a_supported_version_in_meta_passes_through() {
        for version in PROTOCOL_VERSIONS {
            let reply = ask(&json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/list",
                "params": { "_meta": { META_PROTOCOL_VERSION: version } },
            }));
            assert!(reply["error"].is_null(), "{version} was refused");
        }
    }

    #[test]
    fn unknown_meta_keys_are_tolerated() {
        // Strict on params, lenient on `_meta`: the spec keeps extending
        // that namespace, and rejecting a key we have not heard of breaks
        // the next time it grows.
        let reply = ask(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/list",
            "params": { "_meta": {
                META_PROTOCOL_VERSION: "2026-07-28",
                "io.modelcontextprotocol/clientInfo": { "name": "someone" },
                "io.modelcontextprotocol/clientCapabilities": {},
                "io.modelcontextprotocol/logLevel": "debug",
                "traceparent": "00-abc-def-01",
                "something.invented.later": true,
            } },
        }));
        assert!(reply["error"].is_null(), "a future `_meta` key was refused");
    }

    #[test]
    fn malformed_requests_are_refused_by_kind() {
        assert_eq!(
            serde_json::from_str::<Value>(
                &handle_line(&FakeCapture, "{not json").expect("a reply")
            )
            .unwrap()["error"]["code"],
            ERR_PARSE
        );
        assert_eq!(
            error_code(&json!({"id": 1, "method": "tools/list"})),
            ERR_INVALID_REQUEST,
            "missing jsonrpc"
        );
        assert_eq!(
            error_code(&json!({"jsonrpc": "2.0", "id": 1})),
            ERR_INVALID_REQUEST,
            "missing method"
        );
        assert_eq!(
            error_code(&json!({"jsonrpc": "2.0", "id": 1, "method": "sorcery"})),
            ERR_METHOD_NOT_FOUND
        );
        assert_eq!(
            error_code(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call"})),
            ERR_INVALID_PARAMS,
            "no params"
        );
    }

    #[test]
    fn a_notification_gets_no_reply() {
        // No id means no answer, and answering anyway corrupts the stream
        // for everything after it.
        assert!(handle_line(&FakeCapture, r#"{"jsonrpc":"2.0","method":"tools/list"}"#).is_none());
    }

    // ---- the rule that matters ---------------------------------------

    #[test]
    fn a_miss_is_an_answer_not_an_error() {
        // THE regression test. A miss, a timeout, an over-tolerance diff
        // are answers to a well-formed question — the exit codes have
        // said so since 0.4.0. Reported as `isError`, a calling model
        // sees a broken tool and retries instead of reacting, which is
        // worse than not serving the tool at all.
        let dir = fixture("mcp-miss");
        let hit = call(
            "pixelcoords_assert",
            &json!({ "session": dir.to_string_lossy(), "point": "30,20" }),
        );
        assert_eq!(hit["isError"], false);
        assert_eq!(hit["structuredContent"]["ok"], true);

        let miss = call(
            "pixelcoords_assert",
            &json!({ "session": dir.to_string_lossy(), "point": "150,110" }),
        );
        assert_eq!(
            miss["isError"], false,
            "a miss must not look like a broken tool"
        );
        assert_eq!(miss["structuredContent"]["ok"], false);
        assert_eq!(miss["structuredContent"]["results"][0]["hit"], false);
    }

    #[test]
    fn a_malformed_question_is_an_error() {
        // The other half of the same rule: exit 2's cases, and only
        // those, come back as JSON-RPC errors.
        assert_eq!(
            error_code(&json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": "pixelcoords_assert", "arguments": {
                    "session": "/nowhere/at/all", "point": "1,1" } },
            })),
            ERR_INVALID_PARAMS
        );
    }

    // ---- arguments ---------------------------------------------------

    #[test]
    fn unknown_tools_and_arguments_name_what_was_expected() {
        let reply = ask(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": "pixelcoords_teleport" },
        }));
        let message = reply["error"]["message"].as_str().unwrap();
        assert!(message.contains("pixelcoords_teleport"), "{message}");
        assert!(message.contains("pixelcoords_resolve"), "{message}");

        let reply = ask(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": "pixelcoords_resolve", "arguments": {
                "session": "x", "untis": "auto" } },
        }));
        let message = reply["error"]["message"].as_str().unwrap();
        assert!(
            message.contains("untis"),
            "the typo is not named: {message}"
        );
        assert!(
            message.contains("units"),
            "the real name is not offered: {message}"
        );
    }

    #[test]
    fn a_missing_required_argument_says_which() {
        let reply = ask(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": "pixelcoords_resolve", "arguments": {} },
        }));
        assert!(
            reply["error"]["message"]
                .as_str()
                .unwrap()
                .contains("session"),
            "{reply}"
        );
    }

    #[test]
    fn enum_arguments_are_checked_against_their_values() {
        for (key, bad) in [("space", "sideways"), ("units", "furlongs")] {
            let reply = ask(&json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": "pixelcoords_resolve", "arguments": {
                    "session": "x", key: bad } },
            }));
            let message = reply["error"]["message"].as_str().unwrap();
            assert!(message.contains(bad), "{key}: {message}");
        }
    }

    #[test]
    fn wait_refuses_a_timeout_that_would_hang_the_server() {
        // Stdio is one thread and one client, so a long wait blocks every
        // other call. The ceiling is the reason the description mentions
        // it, and the error explains rather than just refusing.
        let reply = ask(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": "pixelcoords_wait", "arguments": {
                "session": "x", "timeout_secs": MAX_WAIT_SECS + 1 } },
        }));
        let message = reply["error"]["message"].as_str().unwrap();
        assert!(message.contains(&MAX_WAIT_SECS.to_string()), "{message}");
    }

    // ---- headless tools ----------------------------------------------

    #[test]
    fn sessions_lists_labels_as_data_not_prose() {
        // An agent's first move needs the label set. The CLI's summary
        // shows four and truncates, which is right for a picker and
        // useless for planning.
        let dir = fixture("mcp-sessions/one");
        let root = dir.parent().unwrap();
        let r = call(
            "pixelcoords_sessions",
            &json!({ "root": root.to_string_lossy() }),
        );
        assert_eq!(r["isError"], false);
        let sessions = r["structuredContent"]["sessions"].as_array().unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0]["labels"], json!(["email", "submit"]));
    }

    #[test]
    fn an_empty_root_says_how_to_make_a_session() {
        // The agent cannot create one, so the text has to point at the
        // human. Silence here costs a turn and teaches nothing.
        let root = std::env::temp_dir().join("mcp-empty-root");
        std::fs::create_dir_all(&root).unwrap();
        let r = call(
            "pixelcoords_sessions",
            &json!({ "root": root.to_string_lossy() }),
        );
        let text = r["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("pixelcoords"), "{text}");
        assert!(text.to_lowercase().contains("mark"), "{text}");
    }

    #[test]
    fn resolve_answers_in_the_units_it_was_asked_for() {
        let dir = fixture("mcp-resolve");
        let session = dir.to_string_lossy().into_owned();
        let physical = call(
            "pixelcoords_resolve",
            &json!({ "session": session, "units": "physical", "label": "email" }),
        );
        let logical = call(
            "pixelcoords_resolve",
            &json!({ "session": session, "units": "logical", "label": "email" }),
        );
        let p = &physical["structuredContent"]["results"][0]["point"];
        let l = &logical["structuredContent"]["results"][0]["point"];
        assert_eq!(
            p["x"].as_i64().unwrap(),
            l["x"].as_i64().unwrap() * 2,
            "scale 2.0"
        );
        assert_eq!(
            physical["structuredContent"]["results"][0]["label"],
            "email"
        );
    }

    #[test]
    fn assert_scores_a_whole_trajectory_in_one_call() {
        // The point of taking an array: the session is parsed once, which
        // is the same saving `--stdin` exists for.
        let dir = fixture("mcp-trajectory");
        let r = call(
            "pixelcoords_assert",
            &json!({
                "session": dir.to_string_lossy(),
                "points": ["30,20", "30,70", "150,110"],
            }),
        );
        assert_eq!(r["isError"], false);
        let rows = r["structuredContent"]["results"].as_array().unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["hit"], true);
        assert_eq!(rows[1]["hit"], true);
        assert_eq!(rows[2]["hit"], false);
        assert_eq!(
            r["structuredContent"]["ok"], false,
            "one miss fails the set"
        );
    }

    #[test]
    fn assert_refuses_both_point_and_points() {
        let dir = fixture("mcp-both");
        let reply = ask(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": "pixelcoords_assert", "arguments": {
                "session": dir.to_string_lossy(), "point": "1,1", "points": ["1,1"] } },
        }));
        assert!(
            reply["error"]["message"]
                .as_str()
                .unwrap()
                .contains("not both")
        );
    }

    /// A refusal must name the argument the caller passed, never the flag
    /// the CLI spells it with. A model that reads "--tolerance" has no
    /// such flag and cannot act on the advice; the leak *is* the bug, so
    /// this asserts its absence directly.
    #[test]
    fn a_refusal_never_names_a_command_line_flag() {
        let cases = [
            json!({"session": "/tmp/s", "min_score": 5}),
            json!({"session": "/tmp/s", "min_score": -1}),
        ];
        for args in cases {
            let reply = ask(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
                                    "params":{"name":"pixelcoords_wait","arguments":args}}));
            let message = reply["error"]["message"].as_str().expect("a message");
            assert!(!message.contains("--"), "leaked a flag: {message}");
            assert!(
                message.contains("min_score"),
                "names the argument: {message}"
            );
        }

        let reply = ask(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
                                "params":{"name":"pixelcoords_diff",
                                          "arguments":{"session":"/tmp/s","tolerance":150}}}));
        let message = reply["error"]["message"].as_str().expect("a message");
        assert!(!message.contains("--"), "leaked a flag: {message}");
        assert!(
            message.contains("tolerance"),
            "names the argument: {message}"
        );
    }

    /// Bounds are checked before the session is read, so a caller learns
    /// what is wrong with their argument rather than what is wrong with
    /// their path.
    #[test]
    fn bounds_are_checked_before_the_session_is_touched() {
        let reply = ask(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
                                "params":{"name":"pixelcoords_diff",
                                          "arguments":{"session":"/definitely/not/here",
                                                       "tolerance":150}}}));
        let message = reply["error"]["message"].as_str().expect("a message");
        assert!(message.contains("tolerance"), "{message}");
        assert!(!message.contains("No such file"), "{message}");
    }
}
