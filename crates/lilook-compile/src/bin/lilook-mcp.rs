//! lilook as an MCP server: an agent making and tweaking publication-ready
//! figures, in pure Rust, over stdio.
//!
//! # Why five tools and not forty-eight
//!
//! The obvious shape -- one tool per lilaq function -- was measured before it was
//! built. lilaq has 48 functions and 398 parameters; emitting a tool per function
//! with its parameters as a JSON-Schema costs **~18,000 tokens on every single
//! request**, because tool definitions are re-sent each turn. The whole schema as
//! one blob is 44,000.
//!
//! So discovery is hierarchical instead, and the standing cost is five small tool
//! definitions:
//!
//! | surface | tokens | paid |
//! | --- | --- | --- |
//! | one tool per function | 17,980 | every request |
//! | `capabilities` index | 810 | once, on demand |
//! | `describe` one function | 47 | only when used |
//!
//! An agent asks what exists (810), expands the two or three functions it
//! actually needs (~150), and edits. That is roughly 1,000 tokens against 18,000
//! per turn, and it gets *more* detail on what it is using rather than less.
//!
//! # Mechanically derived
//!
//! Nothing here enumerates lilaq. `capabilities` and `describe` are projections
//! of the generated schema, and `edit` is a projection of [`Session`]'s
//! vocabulary -- the same operations the GUI performs. When lilaq adds a
//! function, this file does not change.
//!
//! # Publication-ready means checking your work
//!
//! `render` compiles in process and reports diagnostics *and a scene readback*:
//! what was drawn, how many points, where the axes ended up. An agent can verify
//! a figure without pixels, which is the same thing lilook's own test suite does
//! and for the same reason.

use lilook_compile::{backend::Hints, Backend};
use lilook_core::{schema::Schema, CanvasEvent, Document, Intent, Session};
use serde_json::{json, Value};
use std::io::{BufRead, Write};

const SCHEMA: &str = include_str!("../../../../assets/lilaq-0.6.0.schema.json");

/// Which bucket a lilaq function falls in, for the capability index.
///
/// Derived from what lilook already knows about a name -- the series tables and
/// the schema's own element list -- rather than from a hand-written list, so a
/// new lilaq function is categorised without touching this file.
fn category(name: &str, schema: &Schema) -> &'static str {
    use lilook_core::{SeriesShape, XY_SERIES};
    if schema.elements.contains_key(name) {
        return "style";
    }
    if let Some(stripped) = name.strip_prefix("set-") {
        let _ = stripped;
        return "style";
    }
    if XY_SERIES.contains(&name) {
        // A shape that draws data, or one that annotates.
        return match lilook_core::series_shape_of(name) {
            SeriesShape::Anchor | SeriesShape::Vertices => "annotation",
            _ => "series",
        };
    }
    match name {
        "diagram" | "colorbar" | "layout" | "legend" | "title" | "label" | "axis" => "figure",
        _ => "helper",
    }
}

fn first_line(doc: &str) -> String {
    let line = doc
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let line = line.split("```").next().unwrap_or(line);
    if line.len() > 90 {
        format!("{}…", &line[..line.floor_char_boundary(89)])
    } else {
        line.to_string()
    }
}

/// The five tools. Small on purpose: this text is re-sent every request.
fn tools() -> Value {
    json!([
    {
        "name": "lilook_doc",
        "description": "The figure's structure: source, diagrams, the series in each, \
                        themes, and linked data files. Start here.",
        "inputSchema": {"type": "object", "properties": {
            "source": {"type": "boolean", "description": "include the full Typst source (default true)"}
        }}
    },
    {
        "name": "lilook_capabilities",
        "description": "Index of everything lilaq can draw and style: one line per \
                        function. Filter by category (series, annotation, figure, \
                        style, helper) or by a name substring. Cheap; call before \
                        describe.",
        "inputSchema": {"type": "object", "properties": {
            "category": {"type": "string"},
            "match": {"type": "string"}
        }}
    },
    {
        "name": "lilook_describe",
        "description": "Full parameter detail for named functions or style elements: \
                        types, defaults, allowed values. Ask only for what you will use.",
        "inputSchema": {"type": "object", "properties": {
            "names": {"type": "array", "items": {"type": "string"}}
        }, "required": ["names"]}
    },
    {
        "name": "lilook_edit",
        "description": "Apply a batch of edits as one undoable step. Each op is \
                        {op, ...}: set|add|remove {node,param,value}; \
                        set_slot {node,index,value}; delete {node}; duplicate {node}; \
                        move_point {node,index,to:[x,y]}; set_limits {figure,x:[lo,hi],y:[lo,hi]}; \
                        set_size {figure,width_pt,height_pt}; style {element,param,value} \
                        (adds a #show rule); theme {name} or {name:null} to clear; \
                        fork_theme {name}; rename_theme {name}; replace {range:[a,b],value}; \
                        source {value}; undo; redo. \
                        Node ids are positions in a document-order walk, so an op \
                        that inserts text above a node renumbers it: put ops that \
                        target existing nodes BEFORE theme/fork_theme/style ops in \
                        the same batch, or send them as two batches. The reply \
                        lists the ids as they are afterwards.",
        "inputSchema": {"type": "object", "properties": {
            "ops": {"type": "array", "items": {"type": "object"}},
            "label": {"type": "string", "description": "name for the undo step"}
        }, "required": ["ops"]}
    },
    {
        "name": "lilook_render",
        "description": "Compile and report what actually happened: errors, and a \
                        readback of every diagram drawn -- its series, their point \
                        counts and channels, and the axis ranges and scales. Use this \
                        to check your work. Optionally write the figure to a file.",
        "inputSchema": {"type": "object", "properties": {
            "write": {"type": "string", "description": "path to write a PNG to"},
            "ppi": {"type": "number", "description": "pixels per inch for `write` (default 300, publication quality)"}
        }}
    }])
}

struct Server {
    session: Session,
    backend: Backend<typst_kit::files::SystemFiles>,
    root: std::path::PathBuf,
}

impl Server {
    fn doc(&self, args: &Value) -> String {
        let mut out = String::new();
        let d = &self.session.doc;
        for f in d.figures() {
            out.push_str(&format!("diagram #{}\n", f.node));
            for id in &f.series {
                if let Some(c) = d.call(*id) {
                    let geom = self
                        .session
                        .scenes
                        .iter()
                        .flat_map(|s| &s.series)
                        .find(|g| g.node == *id);
                    out.push_str(&format!(
                        "  #{} {}  {}\n",
                        id,
                        c.short_name(),
                        geom.map(|g| g.summary()).unwrap_or_else(|| "—".into())
                    ));
                }
            }
        }
        if out.is_empty() {
            out.push_str("no diagrams yet\n");
        }
        let rules = d.set_rules();
        if !rules.is_empty() {
            out.push_str(&format!(
                "style rules: {}\n",
                rules
                    .iter()
                    .map(|r| format!("#{} {}", r.node, r.element))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if let Some(t) = self
            .session
            .doc
            .themes()
            .into_iter()
            .rfind(|t| t.document_level)
        {
            out.push_str(&format!(
                "theme: {}{}\n",
                t.name,
                if t.local { " (yours, editable)" } else { "" }
            ));
        }
        if !self.session.data_files.is_empty() {
            out.push_str(&format!(
                "data files: {}\n",
                self.session
                    .data_files
                    .iter()
                    .map(|f| f.path.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if args.get("source").and_then(Value::as_bool).unwrap_or(true) {
            out.push_str(&format!("\n--- source ---\n{}", d.text()));
        }
        out
    }

    fn capabilities(&self, args: &Value) -> String {
        let want = args.get("category").and_then(Value::as_str);
        let m = args.get("match").and_then(Value::as_str);
        let schema = &self.session.schema;
        let mut lines: Vec<(String, String)> = vec![];
        for (name, f) in &schema.functions {
            let cat = category(name, schema);
            if want.is_some_and(|w| w != cat) {
                continue;
            }
            if m.is_some_and(|m| !name.contains(m)) {
                continue;
            }
            lines.push((
                cat.to_string(),
                format!("  {name} ({}) {}", f.params.len(), first_line(&f.doc)),
            ));
        }
        for name in schema.elements.keys() {
            if want.is_some_and(|w| w != "style") || m.is_some_and(|m| !name.contains(m)) {
                continue;
            }
            lines.push(("style".into(), format!("  set-{name}")));
        }
        lines.sort();
        let mut out = String::new();
        let mut last = String::new();
        for (cat, line) in lines {
            if cat != last {
                out.push_str(&format!("{cat}:\n"));
                last = cat;
            }
            out.push_str(&line);
            out.push('\n');
        }
        if !schema.themes.is_empty() && want.is_none_or(|w| w == "style") {
            out.push_str(&format!("themes: {}\n", schema.themes.join(", ")));
        }
        out.push_str("\nlilook_describe(names) for parameters.\n");
        out
    }

    fn describe(&self, args: &Value) -> String {
        let schema = &self.session.schema;
        let mut out = String::new();
        for name in args
            .get("names")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .iter()
            .filter_map(Value::as_str)
        {
            let key = name.trim_start_matches("lq.").trim_start_matches("set-");
            let params = schema
                .functions
                .get(name)
                .or_else(|| schema.functions.get(key))
                .map(|f| {
                    out.push_str(&format!("{name} — {}\n", first_line(&f.doc)));
                    &f.params
                })
                .or_else(|| {
                    schema.elements.get(key).map(|e| {
                        out.push_str(&format!("set-{key} (style rule)\n"));
                        &e.fields
                    })
                });
            let Some(params) = params else {
                out.push_str(&format!("{name}: unknown\n"));
                continue;
            };
            for p in params {
                let choices = if p.choices.is_empty() {
                    String::new()
                } else {
                    format!(" one of: {}", p.choices.join("|"))
                };
                // What lilook's own inspector would put here, and what it would
                // start from. Same policy the GUI uses -- it lives beside the
                // schema now precisely so the two cannot drift.
                let control = lilook_core::widget_control(&p.widget);
                let seed = control
                    .and_then(|c| lilook_core::policy::seed(Some(p), c))
                    .map(|v| format!(" [safe value: {v}]"))
                    .unwrap_or_default();
                let unset = if p.sentinels.is_empty() {
                    String::new()
                } else {
                    format!(" [unset: {}]", p.sentinels.join("|"))
                };
                out.push_str(&format!(
                    "  {}: {}{}{}{seed}{unset}\n",
                    p.name,
                    p.types.join("|"),
                    p.default
                        .as_deref()
                        .map(|d| format!(" = {d}"))
                        .unwrap_or_default(),
                    choices
                ));
            }
        }
        out
    }

    fn edit(&mut self, args: &Value) -> Result<String, String> {
        let ops = args
            .get("ops")
            .and_then(Value::as_array)
            .ok_or("ops must be an array")?;
        let label = args
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or("agent edit");
        let node = |o: &Value| o.get("node").and_then(Value::as_u64).map(|n| n as usize);
        let sval = |o: &Value, k: &str| {
            o.get(k)
                .map(|v| match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .ok_or_else(|| format!("op needs `{k}`"))
        };
        let pair = |o: &Value, k: &str| -> Result<(f64, f64), String> {
            let a = o.get(k).and_then(Value::as_array).ok_or("need [lo, hi]")?;
            Ok((
                a.first().and_then(Value::as_f64).unwrap_or(0.0),
                a.get(1).and_then(Value::as_f64).unwrap_or(0.0),
            ))
        };
        self.session.doc.begin(label);
        let mut applied = 0usize;
        for o in ops {
            let op = o.get("op").and_then(Value::as_str).unwrap_or("");
            match op {
                "set" | "add" => {
                    let n = node(o).ok_or("set/add needs `node`")?;
                    let param = sval(o, "param")?;
                    let value = sval(o, "value")?;
                    let has = self
                        .session
                        .doc
                        .call(n)
                        .is_some_and(|c| c.named.iter().any(|a| a.name == param));
                    let intent = if has || op == "set" {
                        Intent::SetNamedArg {
                            node: n,
                            param,
                            value,
                        }
                    } else {
                        Intent::InsertNamedArg {
                            node: n,
                            param,
                            value,
                        }
                    };
                    self.session.apply(intent);
                }
                "remove" => self.session.apply(Intent::RemoveNamedArg {
                    node: node(o).ok_or("remove needs `node`")?,
                    param: sval(o, "param")?,
                }),
                "set_slot" => self.session.apply(Intent::SetPositionalArg {
                    node: node(o).ok_or("set_slot needs `node`")?,
                    index: o.get("index").and_then(Value::as_u64).unwrap_or(0) as usize,
                    value: sval(o, "value")?,
                }),
                "delete" => self.session.apply(Intent::RemoveNode {
                    node: node(o).ok_or("delete needs `node`")?,
                }),
                "duplicate" => {
                    self.session.selected = node(o).ok_or("duplicate needs `node`")?;
                    self.session.duplicate_selection();
                }
                "move_point" => {
                    let (x, y) = pair(o, "to")?;
                    self.session.handle_canvas(vec![CanvasEvent::MovePoint {
                        node: node(o).ok_or("move_point needs `node`")?,
                        index: o.get("index").and_then(Value::as_u64).unwrap_or(0) as usize,
                        to: (x, y),
                    }]);
                }
                "set_limits" => self.session.handle_canvas(vec![CanvasEvent::SetLimits {
                    figure: o
                        .get("figure")
                        .and_then(Value::as_u64)
                        .ok_or("need figure")? as usize,
                    x: pair(o, "x")?,
                    y: pair(o, "y")?,
                }]),
                "set_size" => self.session.handle_canvas(vec![CanvasEvent::SetSize {
                    figure: o
                        .get("figure")
                        .and_then(Value::as_u64)
                        .ok_or("need figure")? as usize,
                    width_pt: o.get("width_pt").and_then(Value::as_f64),
                    height_pt: o.get("height_pt").and_then(Value::as_f64),
                }]),
                "style" => {
                    let element = sval(o, "element")?;
                    let lq = self.session.doc.lilaq_alias();
                    let existing = self
                        .session
                        .doc
                        .set_rules()
                        .into_iter()
                        .find(|r| r.element == element);
                    match existing {
                        Some(r) => self.session.apply(Intent::SetNamedArg {
                            node: r.node,
                            param: sval(o, "param")?,
                            value: sval(o, "value")?,
                        }),
                        None => {
                            let at = self.session.import_end().ok_or("no lilaq import")?;
                            self.session.apply(Intent::ReplaceRange {
                                range: at..at,
                                value: format!(
                                    "\n#show: {lq}.set-{element}({}: {})",
                                    sval(o, "param")?,
                                    sval(o, "value")?
                                ),
                            });
                        }
                    }
                }
                "theme" => {
                    let name = o.get("name").and_then(Value::as_str);
                    // `set_theme` opens its own transaction, so this one is
                    // closed around it rather than nested.
                    self.session.doc.commit();
                    self.session.set_theme(name);
                    self.session.doc.begin(label);
                }
                "fork_theme" | "rename_theme" => {
                    let name = sval(o, "name")?;
                    self.session.doc.commit();
                    if op == "fork_theme" {
                        self.session.fork_theme(&name);
                    } else {
                        self.session.rename_theme(&name);
                    }
                    self.session.doc.begin(label);
                }
                "replace" => {
                    let a = o
                        .get("range")
                        .and_then(Value::as_array)
                        .ok_or("need range")?;
                    let (s, e) = (
                        a.first().and_then(Value::as_u64).unwrap_or(0) as usize,
                        a.get(1).and_then(Value::as_u64).unwrap_or(0) as usize,
                    );
                    self.session.apply(Intent::ReplaceRange {
                        range: s..e,
                        value: sval(o, "value")?,
                    });
                }
                "source" => {
                    let text = sval(o, "value")?;
                    let all = 0..self.session.doc.text().len();
                    self.session.apply(Intent::ReplaceRange {
                        range: all,
                        value: text,
                    });
                }
                "undo" => {
                    self.session.doc.commit();
                    self.session.doc.undo();
                    self.session.doc.begin(label);
                }
                "redo" => {
                    self.session.doc.commit();
                    self.session.doc.redo();
                    self.session.doc.begin(label);
                }
                other => {
                    self.session.doc.commit();
                    return Err(format!("unknown op `{other}`"));
                }
            }
            applied += 1;
        }
        self.session.doc.commit();
        // Compile straight away: an edit that does not compile is the thing an
        // agent most needs to hear about, and hearing it on the next call is one
        // wasted round trip.
        let report = self.render(&json!({}));
        Ok(format!("{applied} ops applied\n{report}"))
    }

    fn render(&mut self, args: &Value) -> String {
        let doc = Document::new(self.session.doc.text());
        let mut hints = Hints::new();
        let ppi = args.get("ppi").and_then(Value::as_f64).unwrap_or(300.0);
        let (render, scenes) = self
            .backend
            .render_scenes(&doc, (ppi / 72.0) as f32, &mut hints);
        self.session.scenes = scenes;
        let mut out = String::new();
        for d in render.errors() {
            out.push_str(&format!("ERROR: {}\n", d.message));
        }
        if render.failed() {
            return out;
        }
        for s in &self.session.scenes {
            let t = &s.transform;
            out.push_str(&format!(
                "diagram #{}: x {} … {}{}   y {} … {}{}\n",
                s.figure,
                // `gesture_num`: six significant figures. This is a readout for
                // an agent to reason about, not a value going into the document,
                // and `4.179999999999999` is noise it would have to parse past.
                lilook_core::gesture_num(t.x.min),
                lilook_core::gesture_num(t.x.max),
                if t.x.kind == lilook_core::AxisScale::Log {
                    " (log)"
                } else {
                    ""
                },
                lilook_core::gesture_num(t.y.min),
                lilook_core::gesture_num(t.y.max),
                if t.y.kind == lilook_core::AxisScale::Log {
                    " (log)"
                } else {
                    ""
                },
            ));
            for g in &s.series {
                let name = doc
                    .call(g.node)
                    .map(|c| c.short_name().to_string())
                    .unwrap_or_default();
                out.push_str(&format!("  #{} {name}  {}\n", g.node, g.summary()));
            }
        }
        if let Some(path) = args.get("write").and_then(Value::as_str) {
            let path = self.root.join(path);
            match render.pages.first() {
                Some(p) if p.image.width > 0 => match write_png(&path, &p.image) {
                    Ok(()) => out.push_str(&format!("wrote {} at {ppi} ppi\n", path.display())),
                    Err(e) => out.push_str(&format!("could not write {}: {e}\n", path.display())),
                },
                _ => out.push_str("nothing rendered to write\n"),
            }
        }
        out
    }
}

/// A minimal PNG writer, so the server has no image dependency.
///
/// One IDAT of stored (uncompressed) deflate blocks: larger on disk than a
/// compressed PNG and readable by everything, which is the right trade for a
/// file an agent writes once and a human opens once.
fn write_png(path: &std::path::Path, img: &lilook_core::render::Image) -> std::io::Result<()> {
    fn crc(bytes: &[u8]) -> u32 {
        let mut c: u32 = 0xffff_ffff;
        for &b in bytes {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xedb8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
        }
        !c
    }
    fn chunk(out: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = tag.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc(&body).to_be_bytes());
    }
    let (w, h) = (img.width, img.height);
    let mut raw = Vec::with_capacity((w * h * 4 + h) as usize);
    for y in 0..h {
        raw.push(0); // filter: none
        let row = (y * w * 4) as usize;
        raw.extend_from_slice(&img.rgba[row..row + (w * 4) as usize]);
    }
    // zlib with stored deflate blocks.
    let mut z = vec![0x78, 0x01];
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for &byte in &raw {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    for (i, part) in raw.chunks(65535).enumerate() {
        let last = u8::from((i + 1) * 65535 >= raw.len());
        z.push(last);
        z.extend_from_slice(&(part.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(part.len() as u16)).to_le_bytes());
        z.extend_from_slice(part);
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());

    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(&mut png, b"IDAT", &z);
    chunk(&mut png, b"IEND", &[]);
    std::fs::write(path, png)
}

fn main() {
    let root = std::env::args()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let schema = Schema::from_json(SCHEMA).expect("bundled schema");
    let start = std::fs::read_to_string(root.join("figure.typ")).unwrap_or_else(|_| {
        "#import \"@preview/lilaq:0.6.0\" as lq\n#set page(width: auto, height: auto, margin: 6pt)\n"
            .into()
    });
    let mut server = Server {
        session: Session::new(start, schema),
        backend: Backend::new(&root, ""),
        root,
    };

    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines().map_while(Result::ok) {
        let Ok(req): Result<Value, _> = serde_json::from_str(&line) else {
            continue;
        };
        let method = req.get("method").and_then(Value::as_str).unwrap_or("");
        let p = req.get("params").cloned().unwrap_or(json!({}));
        let result: Result<Value, String> = match method {
            "initialize" => Ok(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "lilook", "version": env!("CARGO_PKG_VERSION")}
            })),
            "tools/list" => Ok(json!({"tools": tools()})),
            "tools/call" => {
                let name = p.get("name").and_then(Value::as_str).unwrap_or("");
                let args = p.get("arguments").cloned().unwrap_or(json!({}));
                match name {
                    "lilook_doc" => Ok(server.doc(&args)),
                    "lilook_capabilities" => Ok(server.capabilities(&args)),
                    "lilook_describe" => Ok(server.describe(&args)),
                    "lilook_edit" => server.edit(&args),
                    "lilook_render" => Ok(server.render(&args)),
                    other => Err(format!("unknown tool {other}")),
                }
                .map(|text| json!({"content": [{"type": "text", "text": text}]}))
            }
            "notifications/initialized" => continue,
            other => Err(format!("unknown method {other}")),
        };
        let id = req.get("id").cloned().unwrap_or(Value::Null);
        let msg = match result {
            Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
            Err(e) => json!({"jsonrpc": "2.0", "id": id,
                             "result": {"content": [{"type": "text", "text": e}], "isError": true}}),
        };
        let _ = writeln!(out, "{msg}");
        let _ = out.flush();
    }
}
