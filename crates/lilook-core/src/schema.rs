//! The generated lilaq schema, as consumed by the inspector, the CLI help and
//! the MCP tool descriptions. One schema, three consumers.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParamSchema {
    pub name: String,
    /// `positional` or `named`. Element fields carry no kind -- they are always
    /// named -- so this defaults rather than failing to deserialise them.
    #[serde(default = "named")]
    pub kind: String,
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub types: Vec<String>,
    #[serde(default)]
    pub doc: String,
    /// Control to render: "number", "stroke", "enum", "variant", "opaque", ...
    pub widget: String,
    #[serde(default)]
    pub sentinels: Vec<String>,
    #[serde(default)]
    pub choices: Vec<String>,
    /// True when the widget came from the hand-curated union table rather than
    /// mechanical type mapping.
    #[serde(default)]
    pub curated: bool,
}

fn named() -> String {
    "named".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionSchema {
    pub file: String,
    #[serde(default)]
    pub doc: String,
    pub params: Vec<ParamSchema>,
}

/// An elembic element: `tick`, `legend`, `spine` and the rest. Their fields are
/// configured through `#show: lq.set-tick(..)` rather than per call, which is
/// why they live in their own map.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElementSchema {
    #[serde(default)]
    pub doc: String,
    pub fields: Vec<ParamSchema>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schema {
    pub lilaq_version: String,
    pub functions: BTreeMap<String, FunctionSchema>,
    #[serde(default)]
    pub elements: BTreeMap<String, ElementSchema>,
}

impl Schema {
    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }

    /// Look up by the callee text found in the source, e.g. `lq.plot`.
    pub fn function_for_callee(&self, callee: &str) -> Option<&FunctionSchema> {
        self.functions.get(short(callee))
    }

    /// The element a `lq.set-*` show rule configures, e.g. `lq.set-tick`.
    pub fn element_for_callee(&self, callee: &str) -> Option<&ElementSchema> {
        self.elements.get(short(callee).strip_prefix("set-")?)
    }

    /// An element rendered as a function, so the inspector -- which already
    /// knows how to lay named arguments out against a parameter list -- renders
    /// a set rule with no special case at all.
    pub fn element_as_function(&self, callee: &str) -> Option<FunctionSchema> {
        let e = self.element_for_callee(callee)?;
        Some(FunctionSchema {
            file: String::new(),
            doc: e.doc.clone(),
            params: e
                .fields
                .iter()
                .filter(|f| !f.name.starts_with('_'))
                .cloned()
                .collect(),
        })
    }
}

/// `lq.set-tick` -> `set-tick`. The alias is whatever the user imported lilaq
/// as, so the prefix cannot be assumed to be `lq`.
fn short(callee: &str) -> &str {
    callee.rsplit_once('.').map(|(_, n)| n).unwrap_or(callee)
}
