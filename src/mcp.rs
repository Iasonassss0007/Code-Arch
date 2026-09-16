//! `codearch mcp`: the two index lookups as MCP tools over stdio.
//!
//! Job    Serve `query.rs` over newline-delimited JSON-RPC 2.0 on
//!        stdin/stdout, reusing the CLI's parsing, walks and answer shapes
//!        unchanged. Nothing but protocol messages goes to stdout;
//!        diagnostics go to stderr.
//! In     One JSON-RPC message per stdin line; `--repo` / `--codearch-dir`.
//! Out    One JSON-RPC response per request line to stdout.
//! Fails  Never on a bad message (protocol error reply instead) and never
//!        by re-analyzing: the index files are loaded per call, so a fresh
//!        `codearch` run is picked up without restarting the server.
//!
//! Spec notes (checked against modelcontextprotocol.io, 2026-09-16):
//! - The newest revision (2026-07-28) is stateless: no `initialize`
//!   handshake, per-request versions in `_meta`, plus `server/discover`.
//!   Deployed clients still speak the legacy handshake, and this server's
//!   acceptance gate is a real-client check — so it implements legacy
//!   semantics (valid for 2025-11-25 and earlier) *plus* `server/discover`
//!   advertising those versions, which is the spec's own stdio
//!   backward-compatibility probe: a dual-era client falls back to the
//!   handshake from that answer.
//! - Tool descriptions reuse the eval prompts' wording
//!   (`eval/openrouter_agent.py` `IMPORTERS_TOOL` / `ROUTES_TOOL`), minus
//!   the harness's `{"tool":...}` action wrapper, which is eval syntax.

use crate::query;
use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;

/// Legacy protocol versions this server handshakes, oldest first.
const SUPPORTED_VERSIONS: &[&str] = &[
    "2024-11-05",
    "2025-03-26",
    "2025-06-18",
    "2025-11-25",
];

/// Latest legacy version: offered when the client asks for anything else.
const LATEST_VERSION: &str = "2025-11-25";

const IMPORTERS_DESCRIPTION: &str = "Returns every file that imports that path, directly (depth 1) or transitively (depth 2+), from a static import index. It may miss runtime or string-based coupling.";
const ROUTE_CALLERS_DESCRIPTION: &str = "Returns frontend files whose request URLs name a route served by that backend view, from a static route index. It may miss URLs built at runtime and may include files that merely mention the route's words.";

pub struct Server {
    pub repo: PathBuf,
    pub dir: PathBuf,
}

impl Server {
    pub fn new(repo: PathBuf, codearch_dir: Option<PathBuf>) -> Server {
        let dir = codearch_dir.unwrap_or_else(|| repo.join(".codearch"));
        Server { repo, dir }
    }

    /// Serve one JSON-RPC message per stdin line until EOF. Bad lines get a
    /// protocol error reply; nothing here exits the process early.
    pub fn serve<R: BufRead, W: Write>(&self, reader: R, mut writer: W) {
        for line in reader.lines() {
            let Ok(line) = line else {
                break;
            };
            if line.trim().is_empty() {
                continue;
            }
            if let Some(reply) = self.handle_line(&line) {
                let _ = writer.write_all(reply.as_bytes());
                let _ = writer.write_all(b"\n");
                let _ = writer.flush();
            }
        }
    }

    fn handle_line(&self, line: &str) -> Option<String> {
        let msg: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => return Some(error_reply(None, -32700, "Parse error").to_string()),
        };
        self.handle(&msg).map(|v| v.to_string())
    }

    fn handle(&self, msg: &serde_json::Value) -> Option<serde_json::Value> {
        let obj = msg.as_object()?;
        let method = obj.get("method")?.as_str()?;
        // A notification (no id) never gets a reply; `notifications/*`
        // never gets one even with an id — no client sends those, and
        // silence is the safe answer either way.
        if method.starts_with("notifications/") || !obj.contains_key("id") {
            return None;
        }
        let id = obj.get("id").cloned().unwrap_or(serde_json::Value::Null);
        Some(match method {
            "initialize" => result_reply(&id, self.initialize(msg)),
            "tools/list" => result_reply(&id, self.tools_list()),
            "tools/call" => self.tools_call(&id, msg),
            "server/discover" => result_reply(&id, self.discover()),
            "ping" => result_reply(&id, serde_json::json!({})),
            _ => error_reply(Some(&id), -32601, &format!("Method not found: {method}")),
        })
    }

    fn initialize(&self, msg: &serde_json::Value) -> serde_json::Value {
        let requested = msg
            .get("params")
            .and_then(|p| p.get("protocolVersion"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        // Same version back when supported, else the latest we speak —
        // the lifecycle's version-negotiation rule verbatim.
        let version = if SUPPORTED_VERSIONS.contains(&requested) {
            requested
        } else {
            LATEST_VERSION
        };
        serde_json::json!({
            "protocolVersion": version,
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "codearch", "version": env!("CARGO_PKG_VERSION")},
        })
    }

    fn discover(&self) -> serde_json::Value {
        serde_json::json!({
            "resultType": "complete",
            "supportedVersions": SUPPORTED_VERSIONS,
            "capabilities": {"tools": {}},
            "_meta": {"io.modelcontextprotocol/serverInfo":
                {"name": "codearch", "version": env!("CARGO_PKG_VERSION")}},
        })
    }

    fn tools_list(&self) -> serde_json::Value {
        serde_json::json!({
            "tools": [
                {
                    "name": "importers",
                    "description": IMPORTERS_DESCRIPTION,
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "path": {"type": "string",
                                     "description": "Repo-relative path of the file to look up (e.g. src/a.ts)."},
                            "depth": {"type": "integer",
                                      "description": "Deepest import hop to report (default: no limit)."},
                        },
                        "required": ["path"],
                    },
                },
                {
                    "name": "route_callers",
                    "description": ROUTE_CALLERS_DESCRIPTION,
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "view": {"type": "string",
                                     "description": "Backend view name (e.g. TagViewSet)."},
                        },
                        "required": ["view"],
                    },
                },
            ],
        })
    }

    fn tools_call(&self, id: &serde_json::Value, msg: &serde_json::Value) -> serde_json::Value {
        let params = msg.get("params").cloned().unwrap_or(serde_json::Value::Null);
        let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
        let args = params.get("arguments").cloned().unwrap_or(serde_json::Value::Null);
        match name {
            "importers" => self.call_importers(id, &args),
            "route_callers" => self.call_route_callers(id, &args),
            _ => error_reply(Some(id), -32602, &format!("Unknown tool: {name}")),
        }
    }

    fn call_importers(&self, id: &serde_json::Value, args: &serde_json::Value) -> serde_json::Value {
        let Some(path) = args.get("path").and_then(|p| p.as_str()) else {
            return tool_error(id, "importers needs a 'path' string argument");
        };
        let depth = match args.get("depth") {
            None | Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::Number(n)) => {
                let Some(d) = n.as_u64() else {
                    return tool_error(id, "importers 'depth' must be a non-negative integer");
                };
                Some(d as usize)
            }
            _ => return tool_error(id, "importers 'depth' must be a non-negative integer"),
        };
        let text = match query::index_text(&self.dir, "imports.md", &self.repo.display().to_string()) {
            Ok(t) => t,
            Err(e) => return tool_error(id, &e),
        };
        let target = match query::normalize_target(&self.repo, path) {
            Ok(t) => t,
            Err(e) => return tool_error(id, &e),
        };
        let hits = query::importers(&query::parse_imports(&text), &target, depth);
        if hits.is_empty() {
            return tool_text(
                id,
                &format!("no importers for '{target}': nothing imports it, or it was not analyzed"),
                false,
            );
        }
        tool_text(id, &query::importers_json(&target, &hits), false)
    }

    fn call_route_callers(
        &self,
        id: &serde_json::Value,
        args: &serde_json::Value,
    ) -> serde_json::Value {
        let Some(view) = args.get("view").and_then(|v| v.as_str()) else {
            return tool_error(id, "route_callers needs a 'view' string argument");
        };
        let text = match query::index_text(&self.dir, "routes.md", &self.repo.display().to_string()) {
            Ok(t) => t,
            Err(e) => return tool_error(id, &e),
        };
        let index: BTreeMap<String, Vec<query::RouteEntry>> = query::parse_routes(&text);
        let matches = index.get(view).cloned().unwrap_or_default();
        if matches.is_empty() {
            let mut reason = format!("no route in the index names this view '{view}'");
            let suggestions = query::suggest_views(&index, view);
            if !suggestions.is_empty() {
                reason.push_str(&format!("; similar indexed views: {}", suggestions.join(", ")));
            }
            return tool_text(id, &reason, false);
        }
        tool_text(id, &query::callers_json(view, &matches), false)
    }
}

fn result_reply(id: &serde_json::Value, result: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn error_reply(id: Option<&serde_json::Value>, code: i32, message: &str) -> serde_json::Value {
    serde_json::json!({"jsonrpc": "2.0",
        "id": id.cloned().unwrap_or(serde_json::Value::Null),
        "error": {"code": code, "message": message}})
}

fn tool_text(id: &serde_json::Value, text: &str, is_error: bool) -> serde_json::Value {
    serde_json::json!({"jsonrpc": "2.0", "id": id,
        "result": {"content": [{"type": "text", "text": text}], "isError": is_error}})
}

fn tool_error(id: &serde_json::Value, message: &str) -> serde_json::Value {
    tool_text(id, message, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn fixture_server() -> (Server, PathBuf) {
        // Unique per call: cargo runs tests in one process on threads, so
        // the pid alone collides between tests sharing this fixture.
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let tmp = std::env::temp_dir().join(format!("codearch-mcp-test-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let state = tmp.join("state");
        std::fs::create_dir_all(&state).unwrap();
        std::fs::write(
            state.join("imports.md"),
            "# Reverse Import Index — t\n\nsrc/a.ts ← src/b.ts, src/c.ts\nsrc/b.ts ← src/c.ts\nsrc/c.ts ← src/d.ts\n",
        )
        .unwrap();
        std::fs::write(
            state.join("routes.md"),
            "# Route callers\n\n## `TagViewSet` — `src/views.py`\n\nRoutes: `tags`\n\n- `ui/tag.service.ts`\n",
        )
        .unwrap();
        (Server::new(tmp.join("repo"), Some(state)), tmp)
    }

    fn session(server: &Server, lines: &[&str]) -> Vec<serde_json::Value> {
        let input = lines.join("\n") + "\n";
        let mut out: Vec<u8> = Vec::new();
        server.serve(Cursor::new(input.into_bytes()), &mut out);
        out.split(|b| *b == b'\n')
            .filter(|l| !l.is_empty())
            .map(|l| serde_json::from_slice(l).unwrap())
            .collect()
    }

    #[test]
    fn scripted_session_serves_both_tools() {
        let (server, tmp) = fixture_server();
        let replies = session(
            &server,
            &[
                r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#,
                r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
                r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"importers","arguments":{"path":"src/a.ts"}}}"#,
                r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"route_callers","arguments":{"view":"Unknown"}}}"#,
                r#"{"jsonrpc":"2.0","id":5,"method":"ping"}"#,
                r#"{"jsonrpc":"2.0","id":6,"method":"nope"}"#,
                r#"not json"#,
            ],
        );
        // initialize, tools/list, 2 calls, ping, unknown-method, parse error:
        // the notification gets no reply.
        assert_eq!(replies.len(), 7, "{replies:?}");
        assert_eq!(replies[0]["id"], 1);
        assert_eq!(replies[0]["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(replies[0]["result"]["capabilities"], serde_json::json!({"tools": {}}));
        assert_eq!(replies[0]["result"]["serverInfo"]["name"], "codearch");

        let tools = replies[1]["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0]["name"], "importers");
        assert_eq!(tools[1]["name"], "route_callers");
        assert!(tools[0]["inputSchema"]["properties"].get("depth").is_some());

        let text = replies[2]["result"]["content"][0]["text"].as_str().unwrap();
        let hit: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(
            hit,
            serde_json::json!({"target": "src/a.ts", "importers": [
                {"path": "src/b.ts", "depth": 1}, {"path": "src/c.ts", "depth": 1},
                {"path": "src/d.ts", "depth": 2}]}),
        );
        assert_eq!(replies[2]["result"]["isError"], false);

        assert!(replies[3]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("no route in the index names this view"));
        assert_eq!(replies[3]["result"]["isError"], false);

        assert_eq!(replies[4]["result"], serde_json::json!({}));
        assert_eq!(replies[5]["error"]["code"], -32601);
        assert_eq!(replies[6]["error"]["code"], -32700);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn missing_index_is_a_tool_error_not_a_protocol_error() {
        let tmp =
            std::env::temp_dir().join(format!("codearch-mcp-noindex-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let server = Server::new(tmp.join("repo"), Some(tmp.join("empty")));
        let replies = session(
            &server,
            &[r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"importers","arguments":{"path":"a.ts"}}}"#],
        );
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0]["result"]["isError"], true);
        assert!(replies[0]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("no .codearch/imports.md"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn initialize_negotiates_a_supported_version() {
        let (server, tmp) = fixture_server();
        let replies = session(
            &server,
            &[
                r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"1900-01-01","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#,
                r#"{"jsonrpc":"2.0","id":2,"method":"server/discover"}"#,
            ],
        );
        assert_eq!(replies[0]["result"]["protocolVersion"], "2025-11-25");
        assert!(replies[1]["result"]["supportedVersions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "2025-11-25"));
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
