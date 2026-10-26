//! Model Context Protocol server over stdio (newline-delimited JSON-RPC), so any MCP-capable agent can ask
//! the same questions as the command line. Each call is forwarded to the repository's warm server.

use crate::daemon;
use crate::query::QueryRequest;
use serde_json::{json, Value};
use std::env;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

const PROTOCOL_VERSION: &str = "2025-06-18";

fn tools() -> Value {
    let repo = json!({ "type": "string", "description": "Absolute path of the repository (default: the directory the agent started this server in)." });
    json!([
        {
            "name": "callers",
            "description": "Everything that calls the target, walked back to entry points (routes, commands, events) with no depth limit. \
Use it before changing a function to see all code the change affects, and on a stack-trace line to see how it is reached. \
Each line: caller symbol [kind, confidence] path:line#hash. Confidence: exact (proven), likely (through a parent type or magic), \
unresolved (receiver type unknown: read it). Answers reflect the files on disk at the moment of the call.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "target": { "type": "string", "description": "Class::method, Class (its constructor), a function, or path:line." },
                    "repo": repo,
                    "include_tests": { "type": "boolean", "description": "Also list callers inside tests (default false: only counted)." },
                    "depth": { "type": "integer", "minimum": 1, "description": "Levels to walk up (default: all). 1 lists the direct callers only, including every unresolved line." },
                    "format": { "type": "string", "enum": ["compact", "full"], "description": "compact (default): direct callers with their code, then every other affected symbol grouped by file, then unresolved lines by file. full: the whole caller tree with the code of every call." }
                },
                "required": ["target"]
            }
        },
        {
            "name": "locate",
            "description": "Which function or class a source line belongs to (for example a stack-trace frame), as a symbol usable with callers.",
            "inputSchema": {
                "type": "object",
                "properties": { "position": { "type": "string", "description": "path:line" }, "repo": repo },
                "required": ["position"]
            }
        }
    ])
}

fn repo_root(arguments: &Value) -> Result<PathBuf, String> {
    let start = match arguments.get("repo").and_then(Value::as_str) {
        Some(repo) => PathBuf::from(repo),
        None => env::current_dir().map_err(|error| error.to_string())?,
    };
    let start = start.canonicalize().map_err(|error| format!("{}: {error}", start.display()))?;
    start.ancestors().find(|directory| directory.join(".git").exists()).map(Path::to_path_buf).ok_or_else(|| format!("{} is not inside a repository", start.display()))
}

fn call_tool(name: &str, arguments: &Value) -> Result<String, String> {
    let (command, query) = match name {
        "callers" => ("callers", arguments.get("target")),
        "locate" => ("locate", arguments.get("position")),
        other => return Err(format!("unknown tool {other}")),
    };
    let query = query.and_then(Value::as_str).ok_or("missing target/position")?.to_owned();
    let repo = repo_root(arguments)?;
    let request = QueryRequest {
        command: command.to_owned(),
        query,
        cwd: repo.clone(),
        max_depth: arguments.get("depth").and_then(Value::as_u64).map_or(usize::MAX, |depth| depth as usize),
        max_nodes: usize::MAX,
        include_tests: arguments.get("include_tests").and_then(Value::as_bool).unwrap_or(false),
        compact: arguments.get("format").and_then(Value::as_str) != Some("full"),
    };
    let response = daemon::query(&repo, &request)?;
    let mut text = response.stdout;
    if !response.stderr.is_empty() {
        text.push('\n');
        text.push_str(&response.stderr);
    }
    if response.exit_code == 2 {
        Err(text)
    } else {
        Ok(text)
    }
}

fn answer(message: &Value) -> Option<Value> {
    let id = message.get("id")?.clone();
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": message.pointer("/params/protocolVersion").and_then(Value::as_str).unwrap_or(PROTOCOL_VERSION),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "scout", "version": env!("CARGO_PKG_VERSION") }
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => {
            let name = message.pointer("/params/name").and_then(Value::as_str).unwrap_or("");
            let arguments = message.pointer("/params/arguments").cloned().unwrap_or(json!({}));
            Ok(match call_tool(name, &arguments) {
                Ok(text) => json!({ "content": [{ "type": "text", "text": text }], "isError": false }),
                Err(text) => json!({ "content": [{ "type": "text", "text": text }], "isError": true }),
            })
        }
        other => Err(json!({ "code": -32601, "message": format!("method not found: {other}") })),
    };
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(error) => json!({ "jsonrpc": "2.0", "id": id, "error": error }),
    })
}

pub fn serve() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines().map_while(Result::ok) {
        let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
        if let Some(response) = answer(&message) {
            let _ = writeln!(stdout, "{response}");
            let _ = stdout.flush();
        }
    }
}
