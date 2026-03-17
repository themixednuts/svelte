//! Typed helpers for ESTree JSON AST manipulation.
//!
//! Consolidates common patterns for cleaning OXC-emitted fields,
//! adjusting spans, injecting loc objects, and converting comments
//! to JSON — used by both `js.rs` (program-level) and `ast/modern.rs`
//! (expression-level) codepaths.

use serde::Serialize;

use crate::ast::modern::{JsComment, JsCommentKind};

// ---------------------------------------------------------------------------
// OXC / TS field constants
// ---------------------------------------------------------------------------

/// Fields that OXC emits but upstream acorn/Svelte does not.
pub(crate) const OXC_EXTRA_NULL_FIELDS: &[&str] = &["hashbang", "phase"];

/// TypeScript-specific fields that CompactTSSerializer emits as null
/// when no TS annotation is present. Upstream acorn doesn't emit these.
pub(crate) const TS_NULL_FIELDS: &[&str] = &[
    "typeAnnotation",
    "typeArguments",
    "typeParameters",
    "returnType",
    "superTypeArguments",
    "accessibility",
    "directive",
];

/// Boolean TS fields that OXC emits as false but upstream omits.
pub(crate) const TS_FALSE_FIELDS: &[&str] = &["definite", "declare", "abstract", "override"];

// ---------------------------------------------------------------------------
// EstreeComment
// ---------------------------------------------------------------------------

/// Comment kind in ESTree format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum EstreeCommentKind {
    Line,
    Block,
}

impl std::fmt::Display for EstreeCommentKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Line => f.write_str("Line"),
            Self::Block => f.write_str("Block"),
        }
    }
}

impl From<JsCommentKind> for EstreeCommentKind {
    fn from(kind: JsCommentKind) -> Self {
        match kind {
            JsCommentKind::Line => Self::Line,
            JsCommentKind::Block => Self::Block,
        }
    }
}

/// Typed representation of an ESTree comment node, replacing ad-hoc
/// `serde_json::json!` construction.
#[derive(Debug, Clone, Serialize)]
pub struct EstreeComment {
    #[serde(rename = "type")]
    pub kind: EstreeCommentKind,
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<usize>,
}

impl From<&JsComment> for EstreeComment {
    fn from(c: &JsComment) -> Self {
        Self {
            kind: c.kind.into(),
            value: c.value.to_string(),
            start: c.start,
            end: c.end,
        }
    }
}

impl EstreeComment {
    /// Convert to a `serde_json::Value` (object).
    pub fn to_json_value(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("EstreeComment always serializes")
    }
}

/// Convert a slice of `JsComment` to a `Vec<serde_json::Value>`.
pub fn make_comment_json(comments: &[JsComment]) -> Vec<serde_json::Value> {
    comments
        .iter()
        .map(|c| EstreeComment::from(c).to_json_value())
        .collect()
}

// ---------------------------------------------------------------------------
// Generic AST walker
// ---------------------------------------------------------------------------

/// Recursively walk a JSON value, calling `f` on every Object node.
/// Objects are visited before their children (pre-order).
pub fn walk_json_ast_mut(
    value: &mut serde_json::Value,
    f: &mut impl FnMut(&mut serde_json::Map<String, serde_json::Value>),
) {
    match value {
        serde_json::Value::Object(map) => {
            f(map);
            for v in map.values_mut() {
                walk_json_ast_mut(v, f);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr.iter_mut() {
                walk_json_ast_mut(v, f);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Node helpers
// ---------------------------------------------------------------------------

/// Extract the `"type"` field from a JSON object map, or `""`.
pub fn node_type(map: &serde_json::Map<String, serde_json::Value>) -> &str {
    map.get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("")
}


// ---------------------------------------------------------------------------
// OXC field cleanup
// ---------------------------------------------------------------------------

/// Remove OXC/TS-specific fields that upstream acorn/Svelte does not emit.
/// This consolidates the duplicated cleanup from `adjust_program_json` and
/// `adjust_expression_json_inner`.
pub fn clean_oxc_fields(map: &mut serde_json::Map<String, serde_json::Value>) {
    // Remove OXC-specific null fields
    for field in OXC_EXTRA_NULL_FIELDS {
        if map.get(*field) == Some(&serde_json::Value::Null) {
            map.remove(*field);
        }
    }
    // Remove null TS-specific fields
    for field in TS_NULL_FIELDS {
        if map.get(*field) == Some(&serde_json::Value::Null) {
            map.remove(*field);
        }
    }
    // Remove false TS-specific fields
    for field in TS_FALSE_FIELDS {
        if map.get(*field) == Some(&serde_json::Value::Bool(false)) {
            map.remove(*field);
        }
    }
    // Remove empty decorators array
    if map.get("decorators").is_some_and(|v| v.as_array().is_some_and(|a| a.is_empty())) {
        map.remove("decorators");
    }
    // Remove `optional: false` except on Call/MemberExpression variants
    let nt = node_type(map);
    let keep_optional = matches!(
        nt,
        "CallExpression"
            | "MemberExpression"
            | "OptionalCallExpression"
            | "OptionalMemberExpression"
    );
    if !keep_optional && map.get("optional") == Some(&serde_json::Value::Bool(false)) {
        map.remove("optional");
    }
}

// ---------------------------------------------------------------------------
// Loc injection
// ---------------------------------------------------------------------------

/// Inject a `loc` field into a JSON object map, computing line/column from
/// source text.
///
/// - `source`: the text to compute line/column against
/// - `start`/`end`: byte offsets into `source`
/// - `base_line`: 1-based starting line number
/// - `column_offset`: extra column offset for lines > 1 (destructured patterns)
/// - `with_character`: if true, include `character` (byte offset) in loc
pub fn inject_loc(
    map: &mut serde_json::Map<String, serde_json::Value>,
    source: &str,
    start: usize,
    end: usize,
    base_line: usize,
    column_offset: usize,
    with_character: bool,
) {
    if map.contains_key("loc") {
        return;
    }
    if start > source.len() || end > source.len() {
        return;
    }
    let (sl, sc) = line_column_at_offset(source, start, base_line);
    let (el, ec) = line_column_at_offset(source, end, base_line);
    let sc = if column_offset > 0 && sl > 1 {
        sc + column_offset
    } else {
        sc
    };
    let ec = if column_offset > 0 && el > 1 {
        ec + column_offset
    } else {
        ec
    };
    if with_character {
        map.insert(
            "loc".to_string(),
            serde_json::json!({
                "start": { "line": sl, "column": sc, "character": start },
                "end": { "line": el, "column": ec, "character": end }
            }),
        );
    } else {
        map.insert(
            "loc".to_string(),
            serde_json::json!({
                "start": { "line": sl, "column": sc },
                "end": { "line": el, "column": ec }
            }),
        );
    }
}

// ---------------------------------------------------------------------------
// Unified span + loc adjustment
// ---------------------------------------------------------------------------

/// Configuration for `adjust_spans_and_loc`.
#[derive(Debug)]
pub struct SpanAdjustConfig<'a> {
    /// Byte offset to add to all span values.
    /// For programs this is `content_start` (positive).
    /// For expressions this is `oxc_span_offset` (can be negative).
    pub offset: i64,
    /// Source text for loc computation.
    /// For programs: the script content text (loc computed before offset).
    /// For expressions: the full .svelte source (loc computed after offset).
    pub source: &'a str,
    /// 1-based line number to start counting from.
    pub base_line: usize,
    /// Extra column offset for lines > 1 (destructured patterns).
    pub column_offset: usize,
    /// Include `character` (byte offset) field in loc positions.
    pub with_character: bool,
    /// If true, compute loc BEFORE adjusting spans (program mode).
    /// If false, compute loc AFTER adjusting spans (expression mode).
    pub program_mode: bool,
}

/// Adjust all `start`/`end` spans and inject `loc` fields throughout a JSON
/// AST tree. Also cleans OXC/TS-specific fields.
///
/// In program mode: loc is computed from content-relative positions, then
/// spans are shifted to absolute positions.
///
/// In expression mode: spans are shifted first, then loc is computed from
/// absolute positions. Also unwraps `ParenthesizedExpression` nodes.
pub fn adjust_spans_and_loc(value: &mut serde_json::Value, config: &SpanAdjustConfig<'_>) {
    match value {
        serde_json::Value::Object(map) => {
            clean_oxc_fields(map);

            if config.program_mode {
                // Program mode: compute loc BEFORE adjusting spans
                if let (Some(start), Some(end)) = (
                    map.get("start").and_then(|v| v.as_u64()),
                    map.get("end").and_then(|v| v.as_u64()),
                ) {
                    inject_loc(
                        map,
                        config.source,
                        start as usize,
                        end as usize,
                        config.base_line,
                        config.column_offset,
                        config.with_character,
                    );
                }
                // Then shift spans by offset
                if let Some(v) = map.get("start").and_then(|v| v.as_u64()) {
                    map.insert(
                        "start".to_string(),
                        serde_json::json!(v as usize + config.offset as usize),
                    );
                }
                if let Some(v) = map.get("end").and_then(|v| v.as_u64()) {
                    map.insert(
                        "end".to_string(),
                        serde_json::json!(v as usize + config.offset as usize),
                    );
                }
            } else {
                // Expression mode: shift spans first, then compute loc
                let mut abs_start = None;
                let mut abs_end = None;
                if let Some(v) = map.get("start").and_then(|v| v.as_u64()) {
                    let abs = (v as i64 + config.offset).max(0) as usize;
                    abs_start = Some(abs);
                    map.insert("start".to_string(), serde_json::json!(abs));
                }
                if let Some(v) = map.get("end").and_then(|v| v.as_u64()) {
                    let abs = (v as i64 + config.offset).max(0) as usize;
                    abs_end = Some(abs);
                    map.insert("end".to_string(), serde_json::json!(abs));
                }
                if let (Some(start), Some(end)) = (abs_start, abs_end) {
                    inject_loc(
                        map,
                        config.source,
                        start,
                        end,
                        config.base_line,
                        config.column_offset,
                        config.with_character,
                    );
                }
            }

            // Recurse into children
            for v in map.values_mut() {
                adjust_spans_and_loc(v, config);
            }

            // Expression mode: unwrap ParenthesizedExpression in children
            if !config.program_mode {
                for v in map.values_mut() {
                    unwrap_parenthesized_expression(v);
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr.iter_mut() {
                adjust_spans_and_loc(v, config);
            }
            if !config.program_mode {
                for v in arr.iter_mut() {
                    unwrap_parenthesized_expression(v);
                }
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// TemplateElement span fix
// ---------------------------------------------------------------------------

/// Fix TemplateElement spans produced by TS serializer.
/// TS serializer includes backtick/braces in spans, but upstream (acorn)
/// excludes them.
pub fn fix_template_element_spans(value: &mut serde_json::Value) {
    walk_json_ast_mut(value, &mut |map| {
        if node_type(map) != "TemplateElement" {
            return;
        }
        let is_tail = map
            .get("tail")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if let Some(v) = map.get("start").and_then(|v| v.as_u64()) {
            map.insert("start".to_string(), serde_json::json!(v + 1));
        }
        if let Some(v) = map.get("end").and_then(|v| v.as_u64()) {
            let adj = if is_tail { 1 } else { 2 };
            map.insert(
                "end".to_string(),
                serde_json::json!(v.saturating_sub(adj)),
            );
        }
    });
}

// ---------------------------------------------------------------------------
// ParenthesizedExpression unwrap
// ---------------------------------------------------------------------------

/// If `value` is a `{type: "ParenthesizedExpression", expression: ...}` object,
/// replace it in-place with the inner `expression` value.
pub fn unwrap_parenthesized_expression(value: &mut serde_json::Value) {
    let dominated = match value {
        serde_json::Value::Object(m) => {
            m.get("type").and_then(|v| v.as_str()) == Some("ParenthesizedExpression")
                && m.contains_key("expression")
        }
        _ => false,
    };
    if dominated {
        if let serde_json::Value::Object(map) = std::mem::take(value) {
            if let Some(inner) = map
                .into_iter()
                .find(|(k, _)| k == "expression")
                .map(|(_, v)| v)
            {
                *value = inner;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Line/column utilities (re-exported from js.rs originals)
// ---------------------------------------------------------------------------

/// Compute the 1-based line number at `offset` in `source`.
pub fn line_at_offset(source: &str, offset: usize) -> usize {
    let mut line = 1;
    for (i, ch) in source.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
        }
    }
    line
}

/// Compute 1-based line and 0-based column for a byte offset in source.
/// `base_line` is the 1-based line number to start counting from.
pub fn line_column_at_offset(source: &str, offset: usize, base_line: usize) -> (usize, usize) {
    let mut line = base_line;
    let mut col = 0;
    for (i, ch) in source.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    (line, col)
}
