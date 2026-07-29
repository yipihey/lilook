//! MCP server over stdio (newline-delimited JSON-RPC 2.0).
//!
//! Tool descriptions are generated from the same schema the inspector consumes,
//! so an agent and the GUI see the same vocabulary. Every call opens and commits
//! one transaction, matching the CLI's atomicity.

use lilook_core::{Document, Intent, Schema};
use serde_json::{json, Value};
use std::io::{BufRead, Write};

const SCHEMA: &str = include_str!("../../../../assets/lilaq-0.6.0.schema.json");

fn tools(schema: &Schema) -> Value {
    let functions: Vec<&String> = schema.functions.keys().collect();
    json!([
        {
            "name": "lilook_inspect",
            "description": "List the lilaq call sites in a Typst file with their \
                            arguments, current values and editability.",
            "inputSchema": {
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }
        },
        {
            "name": "lilook_schema",
            "description": format!(
                "Describe the editable parameters of a lilaq function \
                 (lilaq {}). Known functions: {}.",
                schema.lilaq_version,
                functions.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
            ),
            "inputSchema": {
                "type": "object",
                "properties": { "callee": { "type": "string" } },
                "required": ["callee"]
            }
        },
        {
            "name": "lilook_set_arg",
            "description": "Set a named argument on one lilaq call site. The value \
                            is Typst source, e.g. `8cm`, `red`, `\"o\"`. Atomic: one \
                            undo step.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path":  { "type": "string" },
                    "node":  { "type": "integer", "description": "call-site id from lilook_inspect" },
                    "param": { "type": "string" },
                    "value": { "type": "string" }
                },
                "required": ["path", "node", "param", "value"]
            }
        },
        {
            "name": "lilook_add_arg",
            "description": "Insert a named argument not currently present on a call site.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path":  { "type": "string" },
                    "node":  { "type": "integer" },
                    "param": { "type": "string" },
                    "value": { "type": "string" }
                },
                "required": ["path", "node", "param", "value"]
            }
        }
    ])
}

fn call_tool(schema: &Schema, name: &str, args: &Value) -> Result<Value, String> {
    let path = args.get("path").and_then(Value::as_str);

    match name {
        "lilook_schema" => {
            let callee = args
                .get("callee")
                .and_then(Value::as_str)
                .ok_or("callee required")?;
            let f = schema
                .function_for_callee(callee)
                .ok_or_else(|| format!("unknown function {callee}"))?;
            let params: Vec<Value> = f
                .params
                .iter()
                .map(|p| {
                    json!({
                        "name": p.name, "widget": p.widget, "types": p.types,
                        "default": p.default, "choices": p.choices,
                        "accepts": p.sentinels, "doc": p.doc,
                    })
                })
                .collect();
            Ok(json!({ "function": callee, "params": params }))
        }
        "lilook_inspect" => {
            let path = path.ok_or("path required")?;
            let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
            let doc = Document::new(text);
            let calls: Vec<Value> = doc
                .calls()
                .iter()
                .map(|c| {
                    json!({
                        "node": c.id,
                        "callee": c.callee,
                        "generated": c.generated,
                        "positional": c.positional.len(),
                        "named": c.named.iter().map(|a| json!({
                            "param": a.name, "value": a.text,
                            "editability": format!("{:?}", a.editability).to_lowercase(),
                        })).collect::<Vec<_>>(),
                    })
                })
                .collect();
            Ok(json!({ "calls": calls }))
        }
        "lilook_set_arg" | "lilook_add_arg" => {
            let path = path.ok_or("path required")?;
            let node = args
                .get("node")
                .and_then(Value::as_u64)
                .ok_or("node required")? as usize;
            let param = args
                .get("param")
                .and_then(Value::as_str)
                .ok_or("param required")?;
            let value = args
                .get("value")
                .and_then(Value::as_str)
                .ok_or("value required")?;
            let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
            let mut doc = Document::new(text);
            let intent = if name == "lilook_set_arg" {
                Intent::SetNamedArg {
                    node,
                    param: param.into(),
                    value: value.into(),
                }
            } else {
                Intent::InsertNamedArg {
                    node,
                    param: param.into(),
                    value: value.into(),
                }
            };
            doc.begin(name);
            doc.apply(intent)?;
            doc.commit();
            std::fs::write(path, doc.text()).map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true, "undo_steps": doc.history_depth().0 }))
        }
        other => Err(format!("unknown tool {other}")),
    }
}

fn main() {
    let schema = Schema::from_json(SCHEMA).expect("bundled schema");
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let req: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let _ = writeln!(
                    stdout,
                    "{}",
                    json!({
                        "jsonrpc": "2.0", "id": Value::Null,
                        "error": { "code": -32700, "message": e.to_string() }
                    })
                );
                continue;
            }
        };
        let id = req.get("id").cloned().unwrap_or(Value::Null);
        let method = req.get("method").and_then(Value::as_str).unwrap_or("");

        let result = match method {
            "initialize" => Ok(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "lilook", "version": env!("CARGO_PKG_VERSION") }
            })),
            "tools/list" => Ok(json!({ "tools": tools(&schema) })),
            "tools/call" => {
                let p = req.get("params").cloned().unwrap_or(json!({}));
                let name = p.get("name").and_then(Value::as_str).unwrap_or("");
                let args = p.get("arguments").cloned().unwrap_or(json!({}));
                call_tool(&schema, name, &args)
                    .map(|v| json!({ "content": [{ "type": "text", "text": v.to_string() }] }))
            }
            "notifications/initialized" => continue,
            other => Err(format!("unknown method {other}")),
        };

        let msg = match result {
            Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
            Err(e) => json!({
                "jsonrpc": "2.0", "id": id,
                "error": { "code": -32000, "message": e }
            }),
        };
        let _ = writeln!(stdout, "{msg}");
        let _ = stdout.flush();
    }
}
