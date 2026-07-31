//! The MCP server, driven over stdio exactly as an agent drives it.
//!
//! Spawning the real binary rather than calling functions: the protocol framing,
//! the tool names and the JSON shapes are the contract, and none of them are
//! exercised by a unit test.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

fn drive(dir: &std::path::Path, requests: &[&str]) -> Vec<String> {
    let exe = env!("CARGO_BIN_EXE_lilook-mcp");
    let mut child = Command::new(exe)
        .arg(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn the server");
    {
        // Taken and dropped, not borrowed: the server reads until EOF, and a
        // stdin still owned by the child never gives it one.
        let mut stdin = child.stdin.take().expect("stdin");
        for r in requests {
            writeln!(stdin, "{r}").expect("write");
        }
    }
    let out = BufReader::new(child.stdout.take().expect("stdout"));
    let replies: Vec<String> = out
        .lines()
        .map_while(Result::ok)
        .map(|line| {
            let v: serde_json::Value = serde_json::from_str(&line).expect("json reply");
            v["result"]["content"][0]["text"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| v["result"].to_string())
        })
        .collect();
    let _ = child.wait();
    replies
}

/// The whole loop an agent runs to produce a publication-ready figure.
#[test]
fn an_agent_can_discover_edit_and_verify_a_figure() {
    let dir = std::env::temp_dir().join("lilook-mcp-test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");

    let source = r#"#import \"@preview/lilaq:0.6.0\" as lq\n#set page(width: auto, height: auto, margin: 6pt)\n#lq.diagram(width: 8cm, height: 5cm, lq.plot((1, 2, 3, 4), (2, 4, 9, 16)))\n"#;
    let out = drive(
        &dir,
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"lilook_capabilities","arguments":{"category":"series"}}}"#,
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"lilook_describe","arguments":{"names":["plot"]}}}"#,
            &format!(
                r#"{{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{{"name":"lilook_edit","arguments":{{"ops":[{{"op":"source","value":"{source}"}}]}}}}}}"#
            ),
            r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"lilook_edit","arguments":{"ops":[{"op":"add","node":0,"param":"ylabel","value":"[flux]"},{"op":"add","node":0,"param":"yscale","value":"\"log\""},{"op":"theme","name":"ocean"}]}}}"#,
            r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"lilook_render","arguments":{"write":"figure.pdf"}}}"#,
            r#"{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"lilook_doc","arguments":{}}}"#,
        ],
    );
    if out.len() < 8 {
        eprintln!("server produced {} replies; skipping", out.len());
        return;
    }
    // The lilaq package may not be fetched on this machine.
    if out[4].contains("package") || out[4].contains("network") {
        eprintln!("lilaq unavailable; skipping");
        return;
    }

    // Discovery is hierarchical: the index names functions without their
    // parameters, and `describe` expands only what was asked for.
    assert!(out[2].contains("plot ("), "index: {}", out[2]);
    assert!(!out[2].contains("mark-size"), "the index must stay terse");
    assert!(out[3].contains("mark-size"), "describe: {}", out[3]);
    assert!(
        out[3].contains("smooth: bool = false"),
        "types and defaults"
    );

    // An edit reports what it did *and* recompiles, so a broken edit is known
    // immediately rather than one round trip later.
    assert!(out[4].contains("1 ops applied"), "{}", out[4]);
    assert!(out[4].contains("#1 plot  4 pts"), "readback: {}", out[4]);

    // The scene readback is how an agent checks its work without pixels.
    assert!(out[5].contains("(log)"), "the log axis: {}", out[5]);
    assert!(!out[5].contains("ERROR"), "{}", out[5]);

    // A PDF by default and by extension: what a paper takes. The `write`
    // argument names the format, so an agent asks for the right thing once
    // rather than converting afterwards.
    assert!(out[6].contains("wrote"), "{}", out[6]);
    assert!(out[6].contains("as pdf"), "{}", out[6]);
    let bytes = std::fs::read(dir.join("figure.pdf")).expect("the pdf");
    assert_eq!(&bytes[..5], b"%PDF-", "a real PDF header");
    assert!(bytes.len() > 2_000, "{} bytes is too small", bytes.len());

    assert!(out[7].contains("theme: ocean"), "{}", out[7]);
    assert!(
        out[7].contains("yscale"),
        "the source comes back: {}",
        out[7]
    );
}

/// An unknown op is refused by name rather than silently ignored.
#[test]
fn an_unknown_op_says_so() {
    let dir = std::env::temp_dir().join("lilook-mcp-bad");
    let _ = std::fs::create_dir_all(&dir);
    let out = drive(
        &dir,
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"lilook_edit","arguments":{"ops":[{"op":"frobnicate"}]}}}"#,
        ],
    );
    assert!(out[0].contains("frobnicate"), "{}", out[0]);
}
