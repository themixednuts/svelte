use std::sync::Arc;

use oxc_allocator::Allocator;

use oxc_ast::ast::{
    BindingPattern, Expression, FormalParameter, FormalParameterRest, FormalParameters, Program,
    Statement, VariableDeclaration,
};
use oxc_ast::CommentKind;
use oxc_diagnostics::OxcDiagnostic;
use oxc_parser::{ParseOptions, Parser, ParserReturn};
use oxc_span::{GetSpan, SourceType};

use self_cell::self_cell;

struct ProgramOwner {
    source: Box<str>,
    allocator: Allocator,
    source_type: SourceType,
    options: ParseOptions,
}

self_cell! {
    struct ParsedProgramCell {
        owner: ProgramOwner,

        #[covariant]
        dependent: ParserReturn,
    }
}

struct ExpressionOwner {
    source: Box<str>,
    allocator: Allocator,
    source_type: SourceType,
}

struct ParsedExpressionData<'a> {
    expression: Expression<'a>,
}

self_cell! {
    struct ParsedExpressionCell {
        owner: ExpressionOwner,

        #[covariant]
        dependent: ParsedExpressionData,
    }
}

/// Reusable OXC-backed JavaScript/TypeScript program handle.
///
/// This owns the source text and arena allocator required by the parsed OXC
/// AST so downstream tools can inspect the AST without reparsing or converting
/// through ESTree JSON.
pub struct JsProgram {
    cell: ParsedProgramCell,
}

impl std::fmt::Debug for JsProgram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsProgram")
            .field("source", &self.source())
            .field("source_type", &self.source_type())
            .field("panicked", &self.panicked())
            .field("error_count", &self.errors().len())
            .finish()
    }
}

impl PartialEq for JsProgram {
    fn eq(&self, other: &Self) -> bool {
        self.source() == other.source() && self.source_type() == other.source_type()
    }
}

impl Eq for JsProgram {}

// SAFETY: JsProgram owns all its data (Box<str>, Allocator, parsed AST).
// The !Send/!Sync comes from self_cell's self-referential borrow, but the
// underlying data is fully owned and not shared across threads unsafely.
unsafe impl Send for JsProgram {}
unsafe impl Sync for JsProgram {}

impl JsProgram {
    /// Parse JavaScript or TypeScript source into a program AST.
    #[must_use]
    pub fn parse(source: impl Into<Box<str>>, source_type: SourceType) -> Self {
        Self::parse_with_options(source, source_type, ParseOptions::default())
    }

    /// Parse with explicit OXC parser options.
    #[must_use]
    pub fn parse_with_options(
        source: impl Into<Box<str>>,
        source_type: SourceType,
        options: ParseOptions,
    ) -> Self {
        let owner = ProgramOwner {
            source: source.into(),
            allocator: Allocator::default(),
            source_type,
            options,
        };
        let cell = ParsedProgramCell::new(owner, |owner| {
            Parser::new(&owner.allocator, owner.source.as_ref(), owner.source_type)
                .with_options(owner.options)
                .parse()
        });
        Self { cell }
    }

    /// Return the original source text.
    #[must_use]
    pub fn source(&self) -> &str {
        self.cell.borrow_owner().source.as_ref()
    }

    /// Return the OXC source type used for parsing.
    #[must_use]
    pub fn source_type(&self) -> SourceType {
        self.cell.borrow_owner().source_type
    }

    /// Return the parsed OXC program AST.
    #[must_use]
    pub fn program(&self) -> &Program<'_> {
        &self.cell.borrow_dependent().program
    }

    /// Return any parse errors (the program may still be partially valid).
    pub fn errors(&self) -> &[OxcDiagnostic] {
        &self.cell.borrow_dependent().errors
    }

    /// Return `true` if the parser panicked during parsing.
    #[must_use]
    pub fn panicked(&self) -> bool {
        self.cell.borrow_dependent().panicked
    }

    /// Return `true` if the source uses Flow type annotations.
    #[must_use]
    pub fn is_flow_language(&self) -> bool {
        self.cell.borrow_dependent().is_flow_language
    }

    /// Access the full parser return for consumers that need module records,
    /// irregular whitespaces, or other parser metadata.
    #[must_use]
    pub fn parser_return(&self) -> &ParserReturn<'_> {
        self.cell.borrow_dependent()
    }

    /// Serialize the full Program AST to ESTree JSON, with span offsets adjusted
    /// by `offset` (typically `content_start` from the Script node), and `loc`
    /// fields injected.
    ///
    /// `full_source` is the full .svelte source.
    /// `offset` is `content_start` (byte position where script content begins).
    /// `script_tag_end` is the byte position after the closing `</script>` tag.
    #[must_use]
    pub fn to_estree_json(&self, full_source: &str, offset: usize, script_tag_end: usize) -> String {
        use oxc_estree::ESTree;

        let program = self.program();
        let raw_json = if self.source_type().is_typescript() {
            let mut ser = oxc_estree::CompactTSSerializer::new(false);
            program.serialize(&mut ser);
            ser.into_string()
        } else {
            let mut ser = oxc_estree::CompactJSSerializer::new(false);
            program.serialize(&mut ser);
            ser.into_string()
        };

        let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&raw_json) else {
            return raw_json;
        };

        // Fix TS-serializer-specific issues
        if self.source_type().is_typescript() {
            fix_template_element_spans(&mut value);
            // TS serializer's Program span starts at the first token, not at 0.
            // Fix to match JS serializer behavior (start=0, covering all content).
            if let serde_json::Value::Object(ref mut map) = value {
                if crate::estree::node_type(map) == "Program" {
                    map.insert("start".to_string(), serde_json::json!(0));
                }
            }
        }

        // Compute the starting line number for the script content by finding
        // what line `offset` falls on in the full source. Acorn uses this as
        // the starting line for loc computation within the script content.
        let start_line = line_at_offset(full_source, offset);

        let content = self.source();
        adjust_program_json(&mut value, content, offset, start_line);

        // Fix Program's loc.end to point at the </script> tag end (upstream behavior)
        // and add leadingComments/trailingComments from OXC's comment data.
        if let serde_json::Value::Object(map) = &mut value
            && crate::estree::node_type(map) == "Program"
        {
            if let Some(serde_json::Value::Object(loc)) = map.get_mut("loc") {
                let (end_line, end_col) = line_column_at_offset(full_source, script_tag_end, 1);
                loc.insert("end".to_string(), serde_json::json!({
                    "line": end_line,
                    "column": end_col
                }));
            }

            // Build comment entries from OXC program comments
            let comment_entries: Vec<(u32, u32, serde_json::Value)> = program
                .comments
                .iter()
                .map(|comment| {
                    let kind_str = match comment.kind {
                        CommentKind::Line => "Line",
                        CommentKind::SingleLineBlock | CommentKind::MultiLineBlock => "Block",
                    };
                    let content_span = comment.content_span();
                    let value_text =
                        &content[content_span.start as usize..content_span.end as usize];
                    let abs_start = comment.span.start as usize + offset;
                    let abs_end = comment.span.end as usize + offset;
                    (
                        comment.span.start + offset as u32,
                        comment.span.end + offset as u32,
                        serde_json::json!({
                            "type": kind_str,
                            "value": value_text,
                            "start": abs_start,
                            "end": abs_end
                        }),
                    )
                })
                .collect();

            // Attach comments to AST nodes using acorn-style algorithm
            attach_comments_to_json_tree(&mut value, &comment_entries, full_source);
        }

        serde_json::to_string(&value).unwrap_or(raw_json)
    }
}

/// Acorn-style comment attachment: recursively walk the JSON AST and attach
/// leading/trailing comments to the nearest sibling nodes in container arrays
/// (Program.body, BlockStatement.body, ArrayExpression.elements, ObjectExpression.properties).
pub(crate) fn attach_comments_to_json_tree(
    root: &mut serde_json::Value,
    comments: &[(u32, u32, serde_json::Value)],
    content: &str,
) {
    if comments.is_empty() {
        return;
    }

    // Recursively find containers (arrays of AST nodes) and attach comments to their children.
    fn walk(value: &mut serde_json::Value, comments: &[(u32, u32, serde_json::Value)], content: &str) {
        let serde_json::Value::Object(map) = value else {
            return;
        };

        // Determine which child arrays to process for comment attachment
        let node_type = map
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let container_keys: &[&str] = match node_type {
            "Program" | "BlockStatement" | "StaticBlock" | "ClassBody" => &["body"],
            "SwitchCase" => &["consequent"],
            "ArrayExpression" => &["elements"],
            "ObjectExpression" => &["properties"],
            _ => &[],
        };

        // For Program with empty body, attach all comments as trailingComments
        if node_type == "Program" {
            let body_empty = map
                .get("body")
                .and_then(|v| v.as_array())
                .is_none_or(|a| a.is_empty());
            if body_empty {
                let node_start = map.get("start").and_then(|v| v.as_u64()).unwrap_or(0);
                let node_end = map.get("end").and_then(|v| v.as_u64()).unwrap_or(u64::MAX);
                let trailing: Vec<serde_json::Value> = comments
                    .iter()
                    .filter(|(cs, ce, _)| *cs as u64 >= node_start && (*ce as u64) <= node_end)
                    .map(|(_, _, e)| e.clone())
                    .collect();
                if !trailing.is_empty() {
                    map.insert(
                        "trailingComments".to_string(),
                        serde_json::Value::Array(trailing),
                    );
                }
                return;
            }
        }

        // Get this node's span to scope comments
        let node_start = map.get("start").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let node_end = map.get("end").and_then(|v| v.as_u64()).unwrap_or(u32::MAX as u64) as u32;

        for key in container_keys {
            if let Some(serde_json::Value::Array(children)) = map.get_mut(*key) {
                attach_comments_to_siblings(children, comments, content, node_start, node_end);
            }
        }

        // Recurse into all child values
        for v in map.values_mut() {
            match v {
                serde_json::Value::Object(_) => walk(v, comments, content),
                serde_json::Value::Array(arr) => {
                    for item in arr.iter_mut() {
                        walk(item, comments, content);
                    }
                }
                _ => {}
            }
        }
    }

    walk(root, comments, content);
}

/// Attach comments to sibling nodes in a container array.
/// Only considers comments within `container_start..container_end`.
fn attach_comments_to_siblings(
    children: &mut [serde_json::Value],
    comments: &[(u32, u32, serde_json::Value)],
    content: &str,
    container_start: u32,
    container_end: u32,
) {
    if children.is_empty() || comments.is_empty() {
        return;
    }

    // Collect (start, end) for each child
    let positions: Vec<(u64, u64)> = children
        .iter()
        .map(|c| {
            let s = c.get("start").and_then(|v| v.as_u64()).unwrap_or(0);
            let e = c.get("end").and_then(|v| v.as_u64()).unwrap_or(0);
            (s, e)
        })
        .collect();

    // For each child, collect leading and trailing comments
    let n = children.len();
    let mut child_leading: Vec<Vec<serde_json::Value>> = vec![Vec::new(); n];
    let mut child_trailing: Vec<Vec<serde_json::Value>> = vec![Vec::new(); n];

    for &(c_start, c_end, ref entry) in comments {
        // Only consider comments within the container's span
        if c_start < container_start || c_end > container_end {
            continue;
        }

        let c_start_u64 = c_start as u64;
        let c_end_u64 = c_end as u64;

        // Find which gap this comment falls in
        // Before first child
        if c_end_u64 <= positions[0].0 {
            child_leading[0].push(entry.clone());
            continue;
        }
        // After last child
        if c_start_u64 >= positions[n - 1].1 {
            child_trailing[n - 1].push(entry.clone());
            continue;
        }
        // Between children
        for i in 0..n - 1 {
            if c_start_u64 >= positions[i].1 && c_end_u64 <= positions[i + 1].0 {
                // Comment is between child[i] and child[i+1].
                // Heuristic: if comment is on the same line as child[i]'s end, it's trailing.
                // Otherwise, it's leading of child[i+1].
                let child_end_pos = positions[i].1 as usize;
                let comment_start_pos = c_start as usize;
                let same_line = child_end_pos <= content.len()
                    && comment_start_pos <= content.len()
                    && !content[child_end_pos..comment_start_pos].contains('\n');
                if same_line {
                    child_trailing[i].push(entry.clone());
                } else {
                    child_leading[i + 1].push(entry.clone());
                }
                break;
            }
        }
        // If comment is inside a child (not in a gap), skip — it will be handled recursively
    }

    // Attach to JSON nodes
    for (i, child) in children.iter_mut().enumerate() {
        if let serde_json::Value::Object(map) = child {
            if !child_leading[i].is_empty() {
                map.insert(
                    "leadingComments".to_string(),
                    serde_json::Value::Array(child_leading[i].clone()),
                );
            }
            if !child_trailing[i].is_empty() {
                map.insert(
                    "trailingComments".to_string(),
                    serde_json::Value::Array(child_trailing[i].clone()),
                );
            }
        }
    }
}

use crate::estree::{SpanAdjustConfig, adjust_spans_and_loc, line_at_offset, line_column_at_offset};

/// Recursively adjust span offsets by `offset` and inject `loc` fields.
/// `content_source` is the script content text, `start_line` is the 1-based
/// line number in the full source where the content begins.
fn adjust_program_json(
    value: &mut serde_json::Value,
    content_source: &str,
    offset: usize,
    start_line: usize,
) {
    adjust_spans_and_loc(value, &SpanAdjustConfig {
        offset: offset as i64,
        source: content_source,
        base_line: start_line,
        column_offset: 0,
        with_character: false,
        program_mode: true,
    });
}

/// Recursively adjust span offsets by `offset` and inject `loc` fields for expressions.
/// `column_offset` is added to loc columns for nodes not on the first line
/// (matching upstream's behavior for destructured patterns).
/// Like `adjust_expression_json_inner` but with explicit column offset.
pub(crate) fn adjust_expression_json_with_column_offset(
    value: &mut serde_json::Value,
    full_source: &str,
    offset: i64,
    column_offset: usize,
) {
    adjust_expression_json_inner(value, full_source, offset, column_offset, false);
}

/// Like `adjust_expression_json_with_column_offset` but also adds `character` to loc.
pub(crate) fn adjust_expression_json_with_character(
    value: &mut serde_json::Value,
    full_source: &str,
    offset: i64,
) {
    adjust_expression_json_inner(value, full_source, offset, 0, true);
}

fn adjust_expression_json_inner(
    value: &mut serde_json::Value,
    full_source: &str,
    offset: i64,
    column_offset: usize,
    with_character: bool,
) {
    adjust_spans_and_loc(value, &SpanAdjustConfig {
        offset,
        source: full_source,
        base_line: 1,
        column_offset,
        with_character,
        program_mode: false,
    });
}

/// Re-export for callers that reference `crate::js::fix_template_element_spans`.
pub(crate) use crate::estree::fix_template_element_spans;

/// Re-export for callers that reference `crate::js::unwrap_parenthesized_expression`.
pub(crate) use crate::estree::unwrap_parenthesized_expression;

/// Reusable OXC-backed JavaScript/TypeScript expression handle.
///
/// This owns the source text and arena allocator required by the parsed OXC
/// AST so downstream tools can inspect a template/script expression without
/// reparsing.
pub struct JsExpression {
    cell: ParsedExpressionCell,
}

impl std::fmt::Debug for JsExpression {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsExpression")
            .field("source", &self.source())
            .field("source_type", &self.source_type())
            .finish()
    }
}

impl PartialEq for JsExpression {
    fn eq(&self, other: &Self) -> bool {
        self.source() == other.source() && self.source_type() == other.source_type()
    }
}

impl Eq for JsExpression {}

// SAFETY: JsExpression owns all its data (Box<str>, Allocator, parsed AST).
// The !Send/!Sync comes from self_cell's self-referential borrow, but the
// underlying data is fully owned and not shared across threads unsafely.
unsafe impl Send for JsExpression {}
unsafe impl Sync for JsExpression {}

impl JsExpression {
    /// Parse a single JavaScript or TypeScript expression.
    pub fn parse(
        source: impl Into<Box<str>>,
        source_type: SourceType,
    ) -> Result<Self, Box<[OxcDiagnostic]>> {
        let owner = ExpressionOwner {
            source: source.into(),
            allocator: Allocator::default(),
            source_type,
        };
        let cell = ParsedExpressionCell::try_new(owner, |owner| {
            Parser::new(&owner.allocator, owner.source.as_ref(), owner.source_type)
                .parse_expression()
                .map(|expression| ParsedExpressionData { expression })
                .map_err(|errors| errors.into_boxed_slice())
        })?;
        Ok(Self { cell })
    }

    /// Return the original source text.
    #[must_use]
    pub fn source(&self) -> &str {
        self.cell.borrow_owner().source.as_ref()
    }

    /// Return the OXC source type used for parsing.
    #[must_use]
    pub fn source_type(&self) -> SourceType {
        self.cell.borrow_owner().source_type
    }

    /// Return the parsed OXC expression AST node.
    #[must_use]
    pub fn expression(&self) -> &Expression<'_> {
        &self.cell.borrow_dependent().expression
    }
}

/// Reusable OXC-backed binding pattern handle.
///
/// Svelte stores certain binding and parameter positions in the same logical
/// expression slot as ordinary expressions. This handle keeps those nodes in
/// OXC form without routing through ESTree compatibility trees.
pub struct JsPattern {
    source: Box<str>,
    wrapper: Arc<JsExpression>,
}

impl std::fmt::Debug for JsPattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsPattern")
            .field("source", &self.source())
            .finish()
    }
}

/// Reusable OXC-backed formal parameter list handle.
///
/// This preserves richer parameter information like rest/default/type metadata
/// while still exposing each parameter binding pattern without reparsing.
pub struct JsParameters {
    source: Box<str>,
    wrapper: Arc<JsExpression>,
}

impl std::fmt::Debug for JsParameters {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsParameters")
            .field("source", &self.source())
            .field("parameter_count", &self.parameters().items.len())
            .field("has_rest", &self.parameters().rest.is_some())
            .finish()
    }
}

impl PartialEq for JsParameters {
    fn eq(&self, other: &Self) -> bool {
        self.source() == other.source()
    }
}

impl Eq for JsParameters {}

impl JsParameters {
    pub fn parse(source: impl Into<Box<str>>) -> Result<Self, Box<[OxcDiagnostic]>> {
        let source = source.into();
        let wrapper_source = format!("({})=>{{}}", source);
        let wrapper = Arc::new(JsExpression::parse(
            wrapper_source,
            SourceType::ts().with_module(true),
        )?);
        let _ = Self::parameters_from_wrapper(&wrapper).ok_or_else(|| {
            vec![OxcDiagnostic::error(
                "failed to recover formal parameters from wrapper",
            )]
            .into_boxed_slice()
        })?;
        Ok(Self { source, wrapper })
    }

    #[must_use]
    pub fn source(&self) -> &str {
        self.source.as_ref()
    }

    #[must_use]
    pub fn parameters(&self) -> &FormalParameters<'_> {
        Self::parameters_from_wrapper(&self.wrapper).expect("validated parsed parameters")
    }

    #[must_use]
    pub fn parameter(&self, index: usize) -> Option<&FormalParameter<'_>> {
        self.parameters().items.get(index)
    }

    #[must_use]
    pub fn rest_parameter(&self) -> Option<&FormalParameterRest<'_>> {
        self.parameters().rest.as_deref()
    }

    fn parameters_from_wrapper(wrapper: &JsExpression) -> Option<&FormalParameters<'_>> {
        match wrapper.expression() {
            Expression::ArrowFunctionExpression(function) => Some(&function.params),
            _ => None,
        }
    }
}

impl PartialEq for JsPattern {
    fn eq(&self, other: &Self) -> bool {
        self.source() == other.source()
    }
}

impl Eq for JsPattern {}

// SAFETY: JsPattern contains only owned data (Box<str>) and an Arc<JsExpression>
// which is itself Send+Sync.
unsafe impl Send for JsPattern {}
unsafe impl Sync for JsPattern {}

impl JsPattern {
    pub fn parse(source: impl Into<Box<str>>) -> Result<Self, Box<[OxcDiagnostic]>> {
        let source = source.into();
        let wrapper_source = format!("({})=>{{}}", source);
        let wrapper = Arc::new(JsExpression::parse(
            wrapper_source,
            SourceType::ts().with_module(true),
        )?);

        let _ = Self::pattern_from_wrapper(&wrapper).ok_or_else(|| {
            vec![OxcDiagnostic::error(
                "failed to recover binding pattern from wrapper",
            )]
            .into_boxed_slice()
        })?;

        Ok(Self { source, wrapper })
    }

    #[must_use]
    pub fn source(&self) -> &str {
        self.source.as_ref()
    }

    #[must_use]
    pub fn pattern(&self) -> &BindingPattern<'_> {
        Self::pattern_from_wrapper(&self.wrapper).expect("validated parsed pattern")
    }

    fn pattern_from_wrapper(wrapper: &JsExpression) -> Option<&BindingPattern<'_>> {
        match wrapper.expression() {
            Expression::ArrowFunctionExpression(function) => function
                .params
                .items
                .first()
                .map(|parameter| &parameter.pattern),
            _ => None,
        }
    }
}

impl JsProgram {
    #[must_use]
    pub fn statement(&self, index: usize) -> Option<&Statement<'_>> {
        self.program().body.get(index)
    }

    #[must_use]
    pub fn statement_source(&self, index: usize) -> Option<&str> {
        let statement = self.statement(index)?;
        let span = statement.span();
        self.source()
            .get(span.start as usize..span.end as usize)
    }

    #[must_use]
    pub fn variable_declaration(&self, index: usize) -> Option<&VariableDeclaration<'_>> {
        match self.statement(index)? {
            Statement::VariableDeclaration(declaration) => Some(declaration),
            Statement::ExportNamedDeclaration(declaration) => match declaration.declaration.as_ref() {
                Some(oxc_ast::ast::Declaration::VariableDeclaration(declaration)) => Some(declaration),
                _ => None,
            },
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use oxc_ast::ast::{BindingPattern, Expression, Statement};
    use oxc_span::SourceType;

    use super::{JsExpression, JsPattern, JsProgram};

    #[test]
    fn parsed_js_program_exposes_reusable_oxc_program() {
        let parsed = JsProgram::parse("export const answer = 42;", SourceType::mjs());

        assert_eq!(parsed.source(), "export const answer = 42;");
        assert!(parsed.errors().is_empty());
        assert!(!parsed.panicked());
        assert!(matches!(
            parsed.program().body.first(),
            Some(Statement::ExportNamedDeclaration(_))
        ));
    }

    #[test]
    fn parsed_js_expression_exposes_reusable_oxc_expression() {
        let parsed = JsExpression::parse("count + 1", SourceType::ts().with_module(true))
            .expect("expression should parse");

        assert_eq!(parsed.source(), "count + 1");
        assert!(matches!(
            parsed.expression(),
            Expression::BinaryExpression(_)
        ));
    }

    #[test]
    fn parsed_js_expression_returns_oxc_errors_on_invalid_input() {
        let errors = JsExpression::parse("foo(", SourceType::ts().with_module(true))
            .err()
            .expect("expression should fail");

        assert!(!errors.is_empty());
    }

    #[test]
    fn parsed_js_pattern_exposes_reusable_oxc_pattern() {
        let parsed =
            JsPattern::parse("{ count, items: [item] }").expect("pattern should parse");

        assert!(matches!(parsed.pattern(), BindingPattern::ObjectPattern(_)));
    }
}
