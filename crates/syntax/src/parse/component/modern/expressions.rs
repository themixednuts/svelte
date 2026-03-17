use std::sync::Arc;

use oxc_span::GetSpan;
use tree_sitter::Node as TsNode;

use crate::ast::common::{ParseError, ParseErrorKind};
use crate::ast::legacy::Expression as LegacyExpression;
use crate::ast::modern::*;
use crate::{SourceId, SourceText, LineColumn};

pub fn find_matching_brace_close(source: &str, open_index: usize, limit: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut index = open_index;
    let mut depth = 0usize;
    let mut in_single = false;
    let mut in_double = false;
    let mut in_template = false;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let mut escaped = false;

    while index < limit {
        let byte = *bytes.get(index)?;
        let ch = byte as char;
        let next = bytes.get(index + 1).copied().unwrap_or_default() as char;

        if in_line_comment {
            if ch == '\n' || ch == '\r' {
                in_line_comment = false;
            }
            index += 1;
            continue;
        }

        if in_block_comment {
            if ch == '*' && next == '/' {
                in_block_comment = false;
                index += 2;
                continue;
            }
            index += 1;
            continue;
        }

        if escaped {
            escaped = false;
            index += 1;
            continue;
        }

        if in_single {
            if ch == '\\' {
                escaped = true;
            } else if ch == '\'' {
                in_single = false;
            }
            index += 1;
            continue;
        }

        if in_double {
            if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_double = false;
            }
            index += 1;
            continue;
        }

        if in_template {
            if ch == '\\' {
                escaped = true;
            } else if ch == '`' {
                in_template = false;
            }
            index += 1;
            continue;
        }

        if ch == '/' && next == '/' {
            in_line_comment = true;
            index += 2;
            continue;
        }

        if ch == '/' && next == '*' {
            in_block_comment = true;
            index += 2;
            continue;
        }

        match ch {
            '\'' => in_single = true,
            '"' => in_double = true,
            '`' => in_template = true,
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }

        index += 1;
    }

    None
}

pub(crate) fn parse_modern_expression_field(source: &str, node: TsNode<'_>) -> Option<Expression> {
    let raw = node.utf8_text(source.as_bytes()).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let leading = raw.find(trimmed).unwrap_or(0);
    let abs = node.start_byte() + leading;
    let (line, column) = line_column_at_offset(source, abs);
    parse_modern_expression_from_text(trimmed, abs, line, column)
}

pub(super) fn parse_modern_expression_field_or_empty(source: &str, node: TsNode<'_>) -> Expression {
    parse_modern_expression_field(source, node)
        .unwrap_or_else(|| modern_empty_identifier_expression_for_field(source, node))
}

fn modern_empty_identifier_expression_for_field(source: &str, node: TsNode<'_>) -> Expression {
    let raw = node.utf8_text(source.as_bytes()).ok().unwrap_or_default();
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        // For zero-width nodes, use start_byte directly (equals end_byte).
        // For non-zero-width nodes with only whitespace, end_byte - 1 is the last char.
        let pos = if node.start_byte() == node.end_byte() {
            node.start_byte()
        } else {
            node.end_byte().saturating_sub(1)
        };
        return modern_empty_identifier_expression_span(pos, 0);
    }

    let leading = raw.find(trimmed).unwrap_or(0);
    let start = node.start_byte() + leading;
    modern_empty_identifier_expression_span(start, trimmed.len())
}

pub(super) fn modern_empty_identifier_at_block_tag_end(node: TsNode<'_>) -> Expression {
    modern_empty_identifier_expression_span(node.end_byte().saturating_sub(1), 0)
}

pub(super) fn parse_modern_binding_field(
    source: &str,
    node: TsNode<'_>,
    with_character: bool,
) -> Option<Expression> {
    parse_modern_binding_field_with_error(source, node, with_character).0
}

pub(super) fn parse_modern_binding_field_with_error(
    source: &str,
    node: TsNode<'_>,
    with_character: bool,
) -> (Option<Expression>, Option<ParseError>) {
    let Ok(raw) = node.utf8_text(source.as_bytes()) else {
        return (None, None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return (None, None);
    }

    let leading = raw.find(trimmed).unwrap_or(0);
    let abs = node.start_byte() + leading;
    let (line, column) = line_column_at_offset(source, abs);

    if let Some(word) = reserved_binding_word(trimmed) {
        return (
            None,
            Some(ParseError {
                kind: ParseErrorKind::UnexpectedReservedWord {
                    word: Arc::from(word),
                },
                start: abs,
                end: abs,
            }),
        );
    }

    // Check for comma after rest element in the pattern text before parsing.
    if let Some(comma_pos) = find_rest_comma_in_text(trimmed) {
        return (
            None,
            Some(ParseError {
                kind: ParseErrorKind::JsParseError {
                    message: Arc::from("Comma is not permitted after the rest element"),
                },
                start: abs + comma_pos,
                end: abs + comma_pos,
            }),
        );
    }

    if let Some(mut expression) = parse_pattern_with_oxc(trimmed, abs, line, column) {
        if with_character {
            set_expression_character(source, &mut expression);
        }
        return (Some(expression), None);
    }

    if let Some((start, message)) = reserved_binding_pattern_error(trimmed, abs) {
        return (
            None,
            Some(ParseError {
                kind: ParseErrorKind::JsParseError { message },
                start,
                end: start,
            }),
        );
    }

    if let Some(expression) = parse_modern_expression_from_text(trimmed, abs, line, column)
        && let Some((start, message)) = invalid_binding_expression_error(&expression)
    {
        return (
            None,
            Some(ParseError {
                kind: ParseErrorKind::JsParseError { message },
                start,
                end: start,
            }),
        );
    }

    let error =
        parse_pattern_error_from_text(trimmed, abs, line, column).map(|(start, message)| {
            ParseError {
                kind: ParseErrorKind::JsParseError { message },
                start,
                end: start,
            }
        });
    (None, error)
}

fn is_js_reserved_word(text: &str) -> bool {
    matches!(
        text,
        "await"
            | "break"
            | "case"
            | "catch"
            | "class"
            | "const"
            | "continue"
            | "debugger"
            | "default"
            | "delete"
            | "do"
            | "else"
            | "enum"
            | "export"
            | "extends"
            | "false"
            | "finally"
            | "for"
            | "function"
            | "if"
            | "import"
            | "in"
            | "instanceof"
            | "new"
            | "null"
            | "return"
            | "super"
            | "switch"
            | "this"
            | "throw"
            | "true"
            | "try"
            | "typeof"
            | "var"
            | "void"
            | "while"
            | "with"
            | "yield"
    )
}

fn reserved_binding_word(text: &str) -> Option<&str> {
    let word = leading_identifier_word(text)?;
    let tail = &text[word.len()..];
    let tail = tail.trim_matches(|ch: char| ch.is_whitespace() || ch == '}');
    (is_js_reserved_word(word) && tail.is_empty()).then_some(word)
}

fn reserved_binding_pattern_error(text: &str, start: usize) -> Option<(usize, Arc<str>)> {
    let trimmed = text.trim();
    if trimmed.starts_with('[') {
        return reserved_array_binding_error(trimmed, start);
    }
    if trimmed.starts_with('{') {
        return reserved_object_binding_error(trimmed, start);
    }
    None
}

fn reserved_array_binding_error(text: &str, start: usize) -> Option<(usize, Arc<str>)> {
    let close = text.rfind(']')?;
    let inner = &text[1..close];
    let leading = inner.find(|ch: char| !ch.is_whitespace())?;
    let word = leading_identifier_word(&inner[leading..])?;
    is_js_reserved_word(word).then_some((start + 1 + leading, Arc::from("Unexpected token")))
}

fn reserved_object_binding_error(text: &str, start: usize) -> Option<(usize, Arc<str>)> {
    let close = text.rfind('}')?;
    let inner = &text[1..close];
    let leading = inner.find(|ch: char| !ch.is_whitespace())?;
    let rest = &inner[leading..];
    let word = leading_identifier_word(rest)?;
    if !is_js_reserved_word(word) {
        return None;
    }
    let tail = rest[word.len()..].trim_start();
    (tail.is_empty() || matches!(tail.chars().next(), Some(','))).then_some((
        start + 1 + leading,
        Arc::from(format!("Unexpected keyword '{word}'")),
    ))
}

fn leading_identifier_word(text: &str) -> Option<&str> {
    let mut end = 0usize;
    for (idx, ch) in text.char_indices() {
        let ok = if idx == 0 {
            ch == '_' || ch == '$' || ch.is_ascii_alphabetic()
        } else {
            ch == '_' || ch == '$' || ch.is_ascii_alphanumeric()
        };
        if !ok {
            break;
        }
        end = idx + ch.len_utf8();
    }
    (end > 0).then_some(&text[..end])
}

fn invalid_binding_expression_error(expression: &Expression) -> Option<(usize, Arc<str>)> {
    crate::parse::oxc_query::invalid_binding_expression_error(expression)
}

fn parse_pattern_error_from_text(
    text: &str,
    start_byte: usize,
    line: usize,
    column: usize,
) -> Option<(usize, Arc<str>)> {
    let wrapped = format!("({text})=>{{}}");
    let base_column = column.saturating_sub(1);
    crate::parse::parse_modern_expression_error_detail_with_oxc(
        &wrapped,
        start_byte.saturating_sub(1),
        line,
        base_column,
    )
}

pub(crate) fn split_top_level_commas(text: &str) -> Vec<(&str, usize)> {
    let mut segments = Vec::new();
    let mut start = 0usize;
    let mut depth_paren = 0usize;
    let mut depth_brace = 0usize;
    let mut depth_bracket = 0usize;
    let bytes = text.as_bytes();

    for (idx, byte) in bytes.iter().enumerate() {
        match *byte {
            b'(' => depth_paren += 1,
            b')' => depth_paren = depth_paren.saturating_sub(1),
            b'{' => depth_brace += 1,
            b'}' => depth_brace = depth_brace.saturating_sub(1),
            b'[' => depth_bracket += 1,
            b']' => depth_bracket = depth_bracket.saturating_sub(1),
            b',' if depth_paren == 0 && depth_brace == 0 && depth_bracket == 0 => {
                segments.push((&text[start..idx], start));
                start = idx + 1;
            }
            _ => {}
        }
    }

    if start <= text.len() {
        segments.push((&text[start..], start));
    }

    segments
}

pub(crate) fn parse_pattern_with_oxc(
    text: &str,
    abs_start: usize,
    line: usize,
    column: usize,
) -> Option<Expression> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }

    let leading_ws = text.find(trimmed).unwrap_or(0);
    let start = abs_start + leading_ws;
    let parsed = Arc::new(crate::js::JsPattern::parse(trimmed).ok()?);
    let end = start + trimmed.len();
    let mut expression = Expression::from_pattern(parsed, start, end);
    expression.syntax.parens = leading_parens(trimmed, start, expression.start);
    let _ = (line, column);
    Some(expression)
}

/// Scans pattern text for `...identifier,` (rest element followed by comma)
/// which is invalid in destructuring patterns. Returns the byte offset of
/// the comma within the text.
fn find_rest_comma_in_text(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut i = 0;
    let mut brace_depth: i32 = 0;
    let mut bracket_depth: i32 = 0;

    while i < bytes.len() {
        match bytes[i] {
            b'{' => brace_depth += 1,
            b'}' => brace_depth -= 1,
            b'[' => bracket_depth += 1,
            b']' => bracket_depth -= 1,
            b'.' if i + 2 < bytes.len() && bytes[i + 1] == b'.' && bytes[i + 2] == b'.' => {
                // Found `...` — skip past the identifier to see if a comma follows
                let rest_start = i;
                i += 3;
                // Skip whitespace
                while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                // Skip identifier
                let id_start = i;
                while i < bytes.len()
                    && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'$')
                {
                    i += 1;
                }
                if i > id_start {
                    // Skip whitespace after identifier
                    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                        i += 1;
                    }
                    if i < bytes.len() && bytes[i] == b',' {
                        // Check context: only inside `{` or `[` destructuring
                        if brace_depth > 0 || bracket_depth > 0 {
                            return Some(i);
                        }
                    }
                }
                let _ = rest_start;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

pub fn line_column_at_offset(source: &str, offset: usize) -> (usize, usize) {
    SourceText::new(SourceId::new(0), source, None).line_column_at_offset(offset)
}

pub(super) fn location_at_offset(source: &str, offset: usize) -> LineColumn {
    SourceText::new(SourceId::new(0), source, None).location_at_offset(offset)
}

pub(super) fn set_expression_character(_source: &str, _expression: &mut Expression) {}

pub(crate) fn parse_modern_expression(source: &str, node: TsNode<'_>) -> Option<Expression> {
    let (raw, start) = expression_node_text(source, node)?;
    let (line, column) = line_column_at_offset(source, start);
    parse_modern_expression_from_text(raw, start, line, column)
}

pub(super) fn parse_modern_expression_error(source: &str, node: TsNode<'_>) -> Option<(usize, Arc<str>)> {
    let raw = node.utf8_text(source.as_bytes()).ok()?;
    if raw.starts_with("{:") {
        return None;
    }

    let (raw, start) = expression_node_text(source, node)?;
    let (line, column) = line_column_at_offset(source, start);
    parse_modern_expression_error_from_text(raw, start, line, column)
}

fn expression_node_text<'a>(source: &'a str, node: TsNode<'_>) -> Option<(&'a str, usize)> {
    if node.kind() == "expression" {
        if let Some(content) = node.child_by_field_name("content") {
            let raw = content.utf8_text(source.as_bytes()).ok()?;
            return Some((raw, content.start_byte()));
        }
        let raw = node.utf8_text(source.as_bytes()).ok()?;
        if raw.len() >= 2 && raw.starts_with('{') && raw.ends_with('}') {
            return Some((&raw[1..raw.len().saturating_sub(1)], node.start_byte() + 1));
        }
    }

    Some((node.utf8_text(source.as_bytes()).ok()?, node.start_byte()))
}

pub fn modern_empty_identifier_expression(node: TsNode<'_>) -> Expression {
    let start = node.start_byte().saturating_add(1).min(node.end_byte());
    modern_empty_identifier_expression_span(start, 0)
}

pub(super) fn modern_empty_identifier_expression_span(start: usize, len: usize) -> Expression {
    let end = start.saturating_add(len);
    Expression::empty(start, end)
}

pub(super) fn modern_identifier_expression_with_loc(
    name: Arc<str>,
    start: usize,
    end: usize,
    line: usize,
    column: usize,
) -> Expression {
    let _ = (name, line, column);
    Expression::empty(start, end)
}

pub fn parse_modern_expression_from_text(
    text: &str,
    start_byte: usize,
    line: usize,
    column: usize,
) -> Option<Expression> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }

    let leading_ws = text.find(trimmed).unwrap_or(0);
    let start = start_byte + leading_ws;
    let (start_line, start_col) = offset_to_line_column(text, leading_ws, line, column);
    let mut raw =
        crate::parse::parse_modern_expression_with_oxc(trimmed, start, start_line, start_col)?;
    raw.syntax.parens = leading_parens(trimmed, start, raw.start);
    attach_leading_comments_to_expression(&mut raw, trimmed, start);
    attach_trailing_comments_to_expression(&mut raw, trimmed, start);
    Some(raw)
}

fn parse_modern_expression_error_from_text(
    text: &str,
    start_byte: usize,
    line: usize,
    column: usize,
) -> Option<(usize, Arc<str>)> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }

    let leading_ws = text.find(trimmed).unwrap_or(0);
    let start = start_byte + leading_ws;
    let (start_line, start_col) = offset_to_line_column(text, leading_ws, line, column);
    let message = crate::parse::parse_modern_expression_error_with_oxc(
        trimmed, start, start_line, start_col,
    )?;
    Some((start, message))
}

fn leading_parens(text: &str, start: usize, node_start: usize) -> u16 {
    let prefix_len = node_start.saturating_sub(start).min(text.len());
    let bytes = text.as_bytes();
    let mut i = 0usize;
    let mut parens = 0u16;

    while i < prefix_len {
        match bytes[i] {
            b'(' => {
                parens = parens.saturating_add(1);
                i += 1;
            }
            b'/' if i + 1 < prefix_len && bytes[i + 1] == b'/' => {
                i += 2;
                while i < prefix_len && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if i + 1 < prefix_len && bytes[i + 1] == b'*' => {
                i += 2;
                while i + 1 < prefix_len && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(prefix_len);
            }
            b' ' | b'\t' | b'\r' | b'\n' => {
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }

    parens
}

pub fn attach_leading_comments_to_expression(
    expression: &mut Expression,
    source: &str,
    global_start: usize,
) {
    use crate::ast::modern::{JsComment, JsCommentKind};

    // Check if OXC parsed expression starts after position 0 in source text.
    // If so, there may be leading comments in the skipped region.
    let oxc_expr = match &expression.node {
        Some(JsNodeHandle::Expression(parsed)) => Some(parsed.expression().span()),
        _ => return,
    };
    let Some(oxc_span) = oxc_expr else { return };
    let oxc_start = oxc_span.start as usize;
    if oxc_start == 0 || oxc_start > source.len() {
        return;
    }

    // Parse leading comments from the prefix text
    let prefix = &source[..oxc_start];
    let mut comments = Vec::new();
    let bytes = prefix.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        match bytes[i] {
            b' ' | b'\t' | b'\r' | b'\n' => {
                i += 1;
            }
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
                // Line comment
                let start = i;
                i += 2; // skip //
                let value_start = i;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                let value = &source[value_start..i];
                comments.push(JsComment {
                    kind: JsCommentKind::Line,
                    value: Arc::from(value),
                    start: Some(global_start + start),
                    end: Some(global_start + i),
                });
                if i < bytes.len() {
                    i += 1; // skip \n
                }
            }
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                // Block comment
                let start = i;
                i += 2; // skip /*
                let value_start = i;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                let value_end = i;
                if i + 1 < bytes.len() {
                    i += 2; // skip */
                } else {
                    i = bytes.len();
                }
                let value = &source[value_start..value_end];
                comments.push(JsComment {
                    kind: JsCommentKind::Block,
                    value: Arc::from(value),
                    start: Some(global_start + start),
                    end: Some(global_start + i),
                });
            }
            _ => break, // Non-comment, non-whitespace content — stop scanning
        }
    }

    if !comments.is_empty() {
        // Adjust expression start to the actual expression start (after comments)
        expression.start = global_start + oxc_start;
        expression.leading_comments = comments;
    }
}

pub fn attach_trailing_comments_to_expression(
    expression: &mut Expression,
    source: &str,
    global_start: usize,
) {
    use crate::ast::modern::{JsComment, JsCommentKind};

    // Check if there's content after the OXC expression span
    let oxc_expr = match &expression.node {
        Some(JsNodeHandle::Expression(parsed)) => Some(parsed.expression().span()),
        _ => return,
    };
    let Some(oxc_span) = oxc_expr else { return };
    let oxc_end = oxc_span.end as usize;
    if oxc_end >= source.len() {
        return;
    }

    // Parse trailing comments from the suffix text
    let suffix = &source[oxc_end..];
    let mut comments = Vec::new();
    let bytes = suffix.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        match bytes[i] {
            b' ' | b'\t' | b'\r' | b'\n' | b';' => {
                i += 1;
            }
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
                // Line comment
                let start = i;
                i += 2; // skip //
                let value_start = i;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                let value = &source[oxc_end + value_start..oxc_end + i];
                comments.push(JsComment {
                    kind: JsCommentKind::Line,
                    value: Arc::from(value),
                    start: Some(global_start + oxc_end + start),
                    end: Some(global_start + oxc_end + i),
                });
                if i < bytes.len() {
                    i += 1; // skip \n
                }
            }
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                // Block comment
                let start = i;
                i += 2; // skip /*
                let value_start = i;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                let value_end = i;
                if i + 1 < bytes.len() {
                    i += 2; // skip */
                } else {
                    i = bytes.len();
                }
                let value = &source[oxc_end + value_start..oxc_end + value_end];
                comments.push(JsComment {
                    kind: JsCommentKind::Block,
                    value: Arc::from(value),
                    start: Some(global_start + oxc_end + start),
                    end: Some(global_start + oxc_end + i),
                });
            }
            _ => break, // Non-comment, non-whitespace content — stop scanning
        }
    }

    if !comments.is_empty() {
        // Adjust expression end to the actual expression end (before trailing comments)
        expression.end = global_start + oxc_end;
        expression.trailing_comments = comments;
    }
}

fn offset_to_line_column(
    text: &str,
    offset: usize,
    base_line: usize,
    base_column: usize,
) -> (usize, usize) {
    let mut line = base_line;
    let mut column = base_column;
    let bytes = text.as_bytes();
    let limit = offset.min(bytes.len());

    for byte in bytes.iter().take(limit) {
        if *byte == b'\n' {
            line += 1;
            column = 0;
        } else {
            column += 1;
        }
    }

    (line, column)
}

pub fn legacy_expression_from_modern_expression(
    source: &str,
    expression: Expression,
    include_character: bool,
) -> Option<LegacyExpression> {
    super::super::legacy::legacy_expression_from_modern(source, expression, include_character)
}

pub fn named_children_vec(node: TsNode<'_>) -> Vec<TsNode<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}

/// Returns the body nodes of an `else_clause`, filtering out grammar delimiter
/// nodes (`block_open` / `block_close`) that are not content. Without this
/// filter, `parse_modern_nodes_slice` would emit the gap text between the
/// delimiters (the literal "else" keyword) as a spurious `Text` node.
pub(crate) fn else_clause_body_nodes(clause: TsNode<'_>) -> Vec<TsNode<'_>> {
    named_children_vec(clause)
        .into_iter()
        .filter(|n| !matches!(n.kind(), "block_open" | "block_close"))
        .collect()
}
