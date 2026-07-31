//! Thin CLI over lilook-core. Every command opens and commits one transaction,
//! so a coarse consumer gets atomic behaviour without the core exposing a
//! coarse API. This is also the surface an agent drives.

use lilook_core::{Document, Editability, Intent, Schema};
use std::io::Write;

/// `println!` panics on a closed pipe, which breaks `lilook schema x | head`.
/// An agent-facing CLI must not panic when its reader goes away.
macro_rules! outln {
    ($($a:tt)*) => {{ let _ = writeln!(std::io::stdout(), $($a)*); }};
}
use std::process::ExitCode;

const SCHEMA: &str = lilook_core::schema::BUNDLED;

fn usage() -> ExitCode {
    eprintln!(
        "lilook <command>\n\
         \n\
           inspect <file>                      list lilaq call sites and arguments\n\
           set <file> <node> <param> <value>    set a named argument\n\
           add <file> <node> <param> <value>    insert a named argument\n\
           schema <callee>                      show inspector controls for a function\n"
    );
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first().map(String::as_str) else {
        return usage();
    };
    let schema = Schema::from_json(SCHEMA).expect("bundled schema is valid");

    match cmd {
        "inspect" if args.len() == 2 => {
            let text = std::fs::read_to_string(&args[1]).expect("read");
            let doc = Document::new(text);
            for c in doc.calls() {
                let flag = if c.generated { " [generated]" } else { "" };
                outln!("#{}  {}{}", c.id, c.callee, flag);
                if !c.positional.is_empty() {
                    outln!("      {} positional (data slots)", c.positional.len());
                }
                for a in &c.named {
                    let w = schema
                        .function_for_callee(&c.callee)
                        .and_then(|f| f.params.iter().find(|p| p.name == a.name))
                        .map(|p| p.widget.as_str())
                        .unwrap_or("?");
                    let e = match a.editability {
                        Editability::Literal => "literal",
                        Editability::Builtin => "builtin",
                        Editability::Binding => "binding",
                        Editability::Opaque => "opaque",
                    };
                    outln!("      {:<10} = {:<22} [{:<8} {}]", a.name, a.text, w, e);
                }
            }
            ExitCode::SUCCESS
        }
        "set" | "add" if args.len() == 5 => {
            let text = std::fs::read_to_string(&args[1]).expect("read");
            let mut doc = Document::new(text);
            let node: usize = args[2].parse().expect("node id");
            let intent = if cmd == "set" {
                Intent::SetNamedArg {
                    node,
                    param: args[3].clone(),
                    value: args[4].clone(),
                }
            } else {
                Intent::InsertNamedArg {
                    node,
                    param: args[3].clone(),
                    value: args[4].clone(),
                }
            };
            doc.begin(cmd);
            match doc.apply(intent) {
                Ok(()) => {
                    doc.commit();
                    std::fs::write(&args[1], doc.text()).expect("write");
                    outln!("ok ({} undo steps)", doc.history_depth().0);
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        "schema" if args.len() == 2 => {
            let Some(f) = schema.function_for_callee(&args[1]) else {
                eprintln!("no such function: {}", args[1]);
                return ExitCode::FAILURE;
            };
            outln!("{}  ({})", args[1], f.file);
            for p in &f.params {
                let mark = if p.curated { "*" } else { " " };
                let extra = if !p.choices.is_empty() {
                    format!(" {{{}}}", p.choices.join(", "))
                } else if !p.sentinels.is_empty() {
                    format!(" (+{})", p.sentinels.join(", "))
                } else {
                    String::new()
                };
                outln!(
                    "  {mark}{:<14} {:<16}{:<20} default {}",
                    p.name,
                    p.widget,
                    extra,
                    p.default.clone().unwrap_or_else(|| "<positional>".into())
                );
            }
            outln!("\n  * = widget from the curated union table");
            ExitCode::SUCCESS
        }
        _ => usage(),
    }
}
