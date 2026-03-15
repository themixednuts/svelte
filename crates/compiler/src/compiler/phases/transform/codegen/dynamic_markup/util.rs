use super::*;

/// Re-indent a multi-line block to have no base indentation.
pub(super) fn reindent_block(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= 1 {
        return text.to_string();
    }
    // Find minimum indentation of non-empty lines after the first
    let min_indent = lines
        .iter()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    if min_indent == 0 {
        return text.to_string();
    }
    let mut result = String::new();
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            result.push('\n');
        }
        if i == 0 || line.trim().is_empty() {
            result.push_str(line);
        } else if line.len() > min_indent {
            result.push_str(&line[min_indent..]);
        } else {
            result.push_str(line.trim_start());
        }
    }
    result
}

/// Re-indent a method body to use a single tab base indentation.
/// Strips common leading whitespace from non-first, non-empty lines,
/// then adds `base_indent` to all lines.
pub(super) fn reindent_method(text: &str, base_indent: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return String::new();
    }

    // Find the minimum indentation across non-empty lines after the first
    let min_indent = lines
        .iter()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);

    let mut result = String::new();
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            result.push('\n');
        }
        if line.trim().is_empty() {
            continue;
        }
        result.push_str(base_indent);
        if i == 0 {
            result.push_str(line.trim_start());
        } else {
            let stripped = if line.len() >= min_indent {
                &line[min_indent..]
            } else {
                line.trim_start()
            };
            result.push_str(stripped);
        }
    }
    result
}

pub(super) fn is_whitespace_text(node: &Node) -> bool {
    if let Node::Text(text) = node {
        text.data.trim().is_empty()
    } else {
        false
    }
}

/// Render an attribute value as a JS expression (for component props).
pub(super) fn render_attribute_value_js(value: &AttributeValueKind, _source: &str) -> String {
    match value {
        AttributeValueKind::Boolean(true) => "true".to_string(),
        AttributeValueKind::Boolean(false) => "false".to_string(),
        AttributeValueKind::ExpressionTag(tag) => {
            tag.expression.render().unwrap_or_default()
        }
        AttributeValueKind::Values(parts) => {
            // Template literal with mixed text/expressions
            let mut pieces = Vec::new();
            for part in parts.iter() {
                match part {
                    AttributeValue::Text(text) => pieces.push(text.data.to_string()),
                    AttributeValue::ExpressionTag(tag) => {
                        if let Some(rendered) = tag.expression.render() {
                            pieces.push(format!("${{{rendered}}}"));
                        }
                    }
                }
            }
            format!("`{}`", pieces.join(""))
        }
    }
}

/// Check if an attribute value contains any dynamic (expression) parts.
pub(super) fn is_dynamic_attribute_value(value: &AttributeValueKind) -> bool {
    match value {
        AttributeValueKind::Boolean(_) => false,
        AttributeValueKind::ExpressionTag(_) => true,
        AttributeValueKind::Values(parts) => parts.iter().any(|p| matches!(p, AttributeValue::ExpressionTag(_))),
    }
}

/// Render a dynamic attribute value as an expression (for server $.attr() calls).
pub(super) fn render_attribute_value_dynamic(value: &AttributeValueKind) -> Option<String> {
    match value {
        AttributeValueKind::ExpressionTag(tag) => tag.expression.render(),
        AttributeValueKind::Values(parts) => {
            // Mixed text + expression → template literal
            let mut pieces = Vec::new();
            for part in parts.iter() {
                match part {
                    AttributeValue::Text(text) => pieces.push(text.data.to_string()),
                    AttributeValue::ExpressionTag(tag) => {
                        if let Some(rendered) = tag.expression.render() {
                            pieces.push(format!("${{{rendered}}}"));
                        }
                    }
                }
            }
            Some(format!("`{}`", pieces.join("")))
        }
        _ => None,
    }
}

pub(super) fn render_attribute_value_static(value: &AttributeValueKind, _source: &str) -> String {
    let mut result = String::new();
    match value {
        AttributeValueKind::Boolean(_) => {}
        AttributeValueKind::ExpressionTag(tag) => {
            if let Some(rendered) = tag.expression.render() {
                result.push_str(&rendered);
            }
        }
        AttributeValueKind::Values(parts) => {
            for part in parts.iter() {
                match part {
                    AttributeValue::Text(text) => result.push_str(&text.data),
                    AttributeValue::ExpressionTag(tag) => {
                        if let Some(rendered) = tag.expression.render() {
                            result.push_str(&rendered);
                        }
                    }
                }
            }
        }
    }
    result
}

pub(super) fn collapse_template_whitespace(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut in_ws = false;
    for c in text.chars() {
        if c.is_ascii_whitespace() {
            if !in_ws {
                result.push(' ');
                in_ws = true;
            }
        } else {
            result.push(c);
            in_ws = false;
        }
    }
    result
}

/// Collapse runs of multiple spaces into a single space.
pub(super) fn collapse_spaces(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut prev_space = false;
    for c in s.chars() {
        if c == ' ' {
            if !prev_space {
                result.push(' ');
            }
            prev_space = true;
        } else {
            result.push(c);
            prev_space = false;
        }
    }
    result
}

/// Check if an expression is a simple literal (number, string, boolean, null, undefined, template literal)
pub(super) fn is_simple_literal(expr: &str) -> bool {
    let s = expr.trim();
    // String literals
    if (s.starts_with('\'') && s.ends_with('\'')) || (s.starts_with('"') && s.ends_with('"')) {
        return true;
    }
    // Template literal
    if s.starts_with('`') && s.ends_with('`') {
        return true;
    }
    // Numeric
    if s.parse::<f64>().is_ok() {
        return true;
    }
    // Boolean, null, undefined
    matches!(s, "true" | "false" | "null" | "undefined")
}

/// Strip trailing statement terminator (`;`, `,`, or trailing `)`) from RHS and return (rhs, suffix)
pub(super) fn strip_stmt_terminator(rhs: &str) -> (&str, &str) {
    if let Some(r) = rhs.strip_suffix(';') {
        (r.trim_end(), ";")
    } else if let Some(r) = rhs.strip_suffix(',') {
        (r.trim_end(), ",")
    } else {
        (rhs, "")
    }
}

/// Check if `text` contains `word` as a whole word (not part of a larger identifier).
pub(super) fn contains_word(text: &str, word: &str) -> bool {
    let bytes = text.as_bytes();
    let word_bytes = word.as_bytes();
    let word_len = word_bytes.len();
    let mut i = 0;
    while i + word_len <= bytes.len() {
        if &bytes[i..i + word_len] == word_bytes {
            let prev_ok = i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_' || bytes[i - 1] == b'$');
            let next_ok = i + word_len >= bytes.len() || !(bytes[i + word_len].is_ascii_alphanumeric() || bytes[i + word_len] == b'_' || bytes[i + word_len] == b'$');
            if prev_ok && next_ok {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// Check if a string is a simple JS identifier (no dots, parens, spaces, etc.)
pub(super) fn is_simple_identifier(s: &str) -> bool {
    let s = s.trim();
    !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '$')
        && !s.chars().next().unwrap().is_ascii_digit()
}

pub(super) fn has_dynamic_content(fragment: &Fragment) -> bool {
    fragment.nodes.iter().any(|node| match node {
        Node::ExpressionTag(_)
        | Node::HtmlTag(_)
        | Node::ConstTag(_)
        | Node::DebugTag(_)
        | Node::RenderTag(_)
        | Node::IfBlock(_)
        | Node::EachBlock(_)
        | Node::AwaitBlock(_)
        | Node::KeyBlock(_)
        | Node::SnippetBlock(_)
        | Node::SvelteComponent(_)
        | Node::SvelteElement(_)
        | Node::SvelteSelf(_)
        | Node::Component(_)
        | Node::SvelteBoundary(_) => true,
        Node::RegularElement(el) => {
            has_dynamic_content(&el.fragment)
                || el.attributes.iter().any(|a| match a {
                    Attribute::Attribute(attr) => {
                        attr.name.starts_with("on") || is_dynamic_attribute_value(&attr.value)
                    }
                    _ => true,
                })
        }
        _ => false,
    })
}

/// Normalize blank lines in client output to match upstream Svelte conventions.
/// Rules:
/// 1. Add blank line before `$.reset(...)` when preceded by `});` or `}`
/// 2. Add blank line before `$.append(...)` when preceded by a template/fragment assignment
/// 3. Collapse triple+ blank lines to double
/// 4. Add blank line before `$.customizable_select` when preceded by a var assignment
pub(super) fn normalize_client_blank_lines(output: &str) -> String {
    let lines: Vec<&str> = output.lines().collect();
    let mut result = Vec::with_capacity(lines.len() + 20);

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        let prev_trimmed = if i > 0 { lines[i - 1].trim() } else { "" };
        let prev_is_blank = i > 0 && lines[i - 1].trim().is_empty();

        // Rule 1: blank line before $.reset() after }); or } or var assignment
        if trimmed.starts_with("$.reset(") && !prev_is_blank
            && (prev_trimmed.ends_with("});") || prev_trimmed.ends_with('}')
                || (prev_trimmed.starts_with("var ") && prev_trimmed.contains("= ")))
        {
            result.push("");
        }

        // Rule 2: blank line before $.append() when inside callback (indented)
        if trimmed.starts_with("$.append(") && !prev_is_blank && line.starts_with('\t') {
            // Add blank line if preceded by a var assignment, block closing, or callback closing
            if (prev_trimmed.starts_with("var ") && prev_trimmed.contains("= "))
                || prev_trimmed == "}"
                || prev_trimmed == "});"
            {
                result.push("");
            }
        }

        // Rule 3: blank line before $.next() when preceded by var assignment
        if trimmed.starts_with("$.next()") && !prev_is_blank
            && prev_trimmed.starts_with("var ") && prev_trimmed.contains("= ")
        {
            result.push("");
        }

        // Rule 4: blank line before $.html() when preceded by var assignment
        if trimmed.starts_with("$.html(") && !prev_is_blank
            && prev_trimmed.starts_with("var ") && prev_trimmed.contains("= ")
        {
            result.push("");
        }

        // Rule 5: blank line before $.customizable_select when preceded by var assignment
        if trimmed.starts_with("$.customizable_select(") && !prev_is_blank
            && prev_trimmed.starts_with("var ") && prev_trimmed.contains("= ")
        {
            result.push("");
        }

        // Collapse triple blank lines: if current is blank and prev two are also blank, skip
        if trimmed.is_empty() && i >= 2 && lines[i - 1].trim().is_empty() && lines[i - 2].trim().is_empty() {
            continue;
        }

        // Strip whitespace-only lines to truly empty lines
        if trimmed.is_empty() {
            result.push("");
        } else {
            result.push(line);
        }
    }

    result.join("\n")
}

/// Normalize blank lines in server select output.
/// Ensures blank lines between top-level (zero-indent) statements.
pub(super) fn normalize_server_select_blank_lines(code: &str) -> String {
    let lines: Vec<&str> = code.lines().collect();
    let mut result = Vec::new();
    for (i, &line) in lines.iter().enumerate() {
        if i > 0 && !result.is_empty() {
            let prev_line = result.last().copied().unwrap_or("");
            let prev_not_blank = !prev_line.is_empty();
            // Only add blank lines between top-level statements (not indented)
            let is_top_level = !line.starts_with('\t') && !line.is_empty();
            let trimmed = line.trim_start();
            let prev_trimmed = prev_line.trim();
            // A "block end" is a line that's just `}` or `);` (end of multi-line construct)
            let prev_is_block_end = prev_trimmed == "}" || prev_trimmed == ");" || prev_trimmed == "});";
            let needs_blank = prev_not_blank && is_top_level && (
                (trimmed.starts_with("$$renderer.push(") && prev_is_block_end)
                || trimmed.starts_with("const each_array")
                || (trimmed.starts_with("if (") && !prev_trimmed.starts_with("} else"))
                || trimmed.starts_with("for (let $$")
                || (trimmed == "{")
            );
            if needs_blank {
                result.push("");
            }
        }
        result.push(line);
    }
    let mut output = result.join("\n");
    if code.ends_with('\n') {
        output.push('\n');
    }
    output
}

pub(super) fn add_blank_lines_in_arrow_body(text: &str) -> String {
    // Only process multi-line arrow functions with block bodies
    if !text.contains("=> {") || !text.contains('\n') {
        return text.to_string();
    }
    // Find the opening `{` of the arrow body
    let Some(brace_pos) = text.find("=> {") else {
        return text.to_string();
    };
    let body_start = brace_pos + 4; // after "=> {"
    // Find the matching closing `}`
    let body_content = &text[body_start..];
    // Split into lines, add blank lines between statement lines
    let prefix = &text[..body_start];
    let lines: Vec<&str> = body_content.lines().collect();
    if lines.len() < 3 {
        return text.to_string();
    }
    let mut result = prefix.to_string();
    for (i, line) in lines.iter().enumerate() {
        if i == 0 {
            // First line (after `{`)
            result.push_str(line);
            result.push('\n');
        } else if line.trim() == "}" && i == lines.len() - 1 {
            // Closing brace
            result.push_str(line);
        } else {
            // Statement line — add blank line before it if previous wasn't blank
            let prev = lines[i - 1];
            if !prev.trim().is_empty() && i > 1 {
                result.push('\n');
            }
            result.push_str(line);
            result.push('\n');
        }
    }
    result
}

/// Replace whole-word occurrences of `word` with `replacement` in text.
pub(super) fn replace_word_with(text: &str, word: &str, replacement: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let word_bytes = word.as_bytes();
    let word_len = word_bytes.len();
    let mut i = 0;
    while i < bytes.len() {
        if i + word_len <= bytes.len() && &bytes[i..i + word_len] == word_bytes {
            let prev_is_ident = i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_' || bytes[i - 1] == b'$' || bytes[i - 1] == b'.');
            let next_is_ident = i + word_len < bytes.len() && (bytes[i + word_len].is_ascii_alphanumeric() || bytes[i + word_len] == b'_' || bytes[i + word_len] == b'$');
            if !prev_is_ident && !next_is_ident {
                result.push_str(replacement);
                i += word_len;
                continue;
            }
        }
        result.push(bytes[i] as char);
        i += 1;
    }
    result
}

/// Replace a word (at word boundaries) in a string
pub(super) fn replace_word_boundary(text: &str, word: &str, replacement: &str) -> String {
    let mut result = String::new();
    let mut remaining = text;
    while let Some(pos) = remaining.find(word) {
        // Check word boundaries
        let before_ok = pos == 0 || !remaining.as_bytes()[pos - 1].is_ascii_alphanumeric() && remaining.as_bytes()[pos - 1] != b'_' && remaining.as_bytes()[pos - 1] != b'$';
        let after_pos = pos + word.len();
        let after_ok = after_pos >= remaining.len() || {
            let c = remaining.as_bytes()[after_pos];
            !c.is_ascii_alphanumeric() && c != b'_' && c != b'$' && c != b'('
        };
        if before_ok && after_ok {
            result.push_str(&remaining[..pos]);
            result.push_str(replacement);
            remaining = &remaining[after_pos..];
        } else {
            result.push_str(&remaining[..after_pos]);
            remaining = &remaining[after_pos..];
        }
    }
    result.push_str(remaining);
    result
}

/// Transform `await expr` in an expression to `(await $.save(expr))()`
/// e.g. `await foo` → `(await $.save(foo))()`
/// e.g. `await foo > 10` → `(await $.save(foo))() > 10`
pub(super) fn transform_await_in_expr(expr: &str) -> String {
    if let Some(pos) = expr.find("await ") {
        let prefix = &expr[..pos];
        let after_await = &expr[pos + 6..]; // skip "await "
        // Find where the await expression ends (before comparison operators, etc.)
        let await_end = find_await_expr_end(after_await);
        let await_expr = &after_await[..await_end];
        let suffix = &after_await[await_end..];
        return format!("{prefix}(await $.save({await_expr}))(){suffix}");
    }
    expr.to_string()
}

/// Find where an await expression ends (before operators)
pub(super) fn find_await_expr_end(s: &str) -> usize {
    let mut depth = 0;
    let mut i = 0;
    let chars: Vec<char> = s.chars().collect();
    while i < chars.len() {
        match chars[i] {
            '(' => depth += 1,
            ')' => {
                if depth == 0 {
                    return i;
                }
                depth -= 1;
            }
            '>' | '<' | '=' | '!' | '+' | '-' | '*' | '/' | '%' | '&' | '|' | '^' if depth == 0 => {
                // Trim trailing space
                let end = s[..i].trim_end().len();
                return end;
            }
            _ => {}
        }
        i += 1;
    }
    s.len()
}

/// Transform `await expr OP rest` into `(await $.save(expr))() OP rest`.
/// E.g., `await foo > 10` → `(await $.save(foo))() > 10`
pub(super) fn transform_await_with_save(test: &str) -> String {
    // Find `await ` and extract the target
    if let Some(pos) = test.find("await ") {
        let before = &test[..pos];
        let after = &test[pos + 6..];
        // Find end of the awaited expression (before operator/space)
        let end = after.find([' ', '>', '<', '=', '!', '+', '-', '*', '/', '%', '&', '|']).unwrap_or(after.len());
        let target = &after[..end];
        let rest = &after[end..];
        format!("{before}(await $.save({target}))(){rest}")
    } else {
        test.to_string()
    }
}

pub(super) fn replace_var_with_get(text: &str, var_name: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut i = 0;
    let bytes = text.as_bytes();
    let name_bytes = var_name.as_bytes();
    let name_len = name_bytes.len();
    while i < bytes.len() {
        if i + name_len <= bytes.len() && &bytes[i..i + name_len] == name_bytes {
            let prev_is_ident = i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_' || bytes[i - 1] == b'$' || bytes[i - 1] == b'.');
            let next_is_ident = i + name_len < bytes.len() && (bytes[i + name_len].is_ascii_alphanumeric() || bytes[i + name_len] == b'_' || bytes[i + name_len] == b'$');
            if !prev_is_ident && !next_is_ident {
                result.push_str(&format!("$.get({var_name})"));
                i += name_len;
                continue;
            }
        }
        result.push(bytes[i] as char);
        i += 1;
    }
    result
}

/// Check if a test expression needs $.derived() extraction.
/// This applies when the expression contains a function call (has `()` pattern).
pub(super) fn test_needs_derived(test: &str) -> bool {
    // Look for function call pattern: identifier followed by ()
    // Skip simple comparisons like `simple2 > 10` or variable refs like `simple1`
    test.contains("()")
        || (test.contains('(') && test.contains(')') && {
            // Check it's a function call, not just parenthesized expression
            let trimmed = test.trim();
            // Has pattern like `name(` which indicates a function call
            trimmed.chars().zip(trimmed.chars().skip(1)).any(|(a, b)| {
                (a.is_alphanumeric() || a == '_' || a == '$') && b == '('
            })
        })
}

/// Check if an ExpressionTag is a pure/static expression that can be constant-folded.
/// Returns true for: string literals, number literals, null, boolean,
/// and pure function calls like Math.max(), encodeURIComponent(), etc.
pub(super) fn is_pure_expression(expr: &crate::ast::modern::Expression) -> bool {
    let Some(oxc_expr) = expr.oxc_expression() else {
        return false;
    };
    is_pure_oxc_expression(oxc_expr)
}

pub(super) fn is_pure_oxc_expression(expr: &OxcExpression<'_>) -> bool {
    match expr.get_inner_expression() {
        OxcExpression::StringLiteral(_)
        | OxcExpression::NumericLiteral(_)
        | OxcExpression::BooleanLiteral(_)
        | OxcExpression::NullLiteral(_) => true,
        OxcExpression::TemplateLiteral(t) => {
            t.expressions.iter().all(|e| is_pure_oxc_expression(e))
        }
        OxcExpression::BinaryExpression(b) => {
            is_pure_oxc_expression(&b.left) && is_pure_oxc_expression(&b.right)
        }
        OxcExpression::LogicalExpression(l) => {
            is_pure_oxc_expression(&l.left) && is_pure_oxc_expression(&l.right)
        }
        OxcExpression::CallExpression(call) => {
            // Pure global functions: Math.max, Math.min, encodeURIComponent, etc.
            is_pure_global_call(&call.callee)
                && call.arguments.iter().all(|a| {
                    a.as_expression().is_some_and(|e| is_pure_oxc_expression(e))
                })
        }
        OxcExpression::StaticMemberExpression(mem) => {
            // location.href, etc. — not pure in general but treated as static in Svelte
            is_global_member_access(mem)
        }
        _ => false,
    }
}

pub(super) fn is_pure_global_call(callee: &OxcExpression<'_>) -> bool {
    match callee.get_inner_expression() {
        OxcExpression::Identifier(id) => {
            matches!(id.name.as_str(), "encodeURIComponent" | "decodeURIComponent" | "encodeURI" | "decodeURI" | "parseInt" | "parseFloat" | "isNaN" | "isFinite" | "String" | "Number" | "Boolean" | "Array")
        }
        OxcExpression::StaticMemberExpression(mem) => {
            if let OxcExpression::Identifier(obj) = mem.object.get_inner_expression() {
                matches!(obj.name.as_str(), "Math" | "JSON" | "Object" | "Number" | "String")
            } else {
                false
            }
        }
        _ => false,
    }
}

pub(super) fn is_global_member_access(mem: &oxc_ast::ast::StaticMemberExpression<'_>) -> bool {
    if let OxcExpression::Identifier(obj) = mem.object.get_inner_expression() {
        matches!(obj.name.as_str(), "location" | "navigator" | "document" | "window" | "globalThis" | "Math" | "JSON" | "Number" | "String")
    } else {
        false
    }
}

/// Try to evaluate a pure expression to a constant JS value string.
pub(super) fn try_eval_constant(expr: &OxcExpression<'_>) -> Option<String> {
    match expr.get_inner_expression() {
        OxcExpression::StringLiteral(s) => Some(s.value.to_string()),
        OxcExpression::NumericLiteral(n) => {
            // Format without trailing .0 for integers
            if n.value == n.value.floor() && n.value.abs() < 1e15 {
                Some(format!("{}", n.value as i64))
            } else {
                Some(n.value.to_string())
            }
        }
        OxcExpression::NullLiteral(_) => Some(String::new()),
        OxcExpression::BooleanLiteral(b) => Some(if b.value { "true" } else { "false" }.to_string()),
        OxcExpression::LogicalExpression(logical) => {
            use oxc_ast::ast::LogicalOperator;
            let left = try_eval_constant(&logical.left)?;
            match logical.operator {
                LogicalOperator::Coalesce => {
                    // a ?? b: if a is null/undefined, return b
                    // For constant evaluation: null/empty → use right, otherwise use left
                    if left.is_empty() {
                        try_eval_constant(&logical.right)
                    } else {
                        Some(left)
                    }
                }
                LogicalOperator::Or => {
                    // a || b: if a is falsy, return b
                    if left.is_empty() || left == "0" || left == "false" {
                        try_eval_constant(&logical.right)
                    } else {
                        Some(left)
                    }
                }
                LogicalOperator::And => {
                    // a && b: if a is truthy, return b
                    if !left.is_empty() && left != "0" && left != "false" {
                        try_eval_constant(&logical.right)
                    } else {
                        Some(left)
                    }
                }
            }
        }
        OxcExpression::CallExpression(call) => {
            // Try to evaluate Math.max, Math.min with constant args
            if let OxcExpression::StaticMemberExpression(mem) = call.callee.get_inner_expression()
                && let OxcExpression::Identifier(obj) = mem.object.get_inner_expression()
                && obj.name.as_str() == "Math"
            {
                let args: Vec<f64> = call.arguments.iter()
                    .filter_map(|a| a.as_expression())
                    .filter_map(|e| try_eval_constant(e))
                    .filter_map(|s| s.parse::<f64>().ok())
                    .collect();
                if args.len() == call.arguments.len() {
                    match mem.property.name.as_str() {
                        "max" => {
                            let result = args.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                            return Some(format!("{}", result as i64));
                        }
                        "min" => {
                            let result = args.iter().copied().fold(f64::INFINITY, f64::min);
                            return Some(format!("{}", result as i64));
                        }
                        _ => {}
                    }
                }
            }
            None
        }
        _ => None,
    }
}

/// Try to evaluate an ExpressionTag as a constant string.
/// Returns Some(string_value) for string/template literals, None for dynamic expressions.
pub(super) fn try_fold_expression_to_string(expr: &crate::ast::modern::Expression) -> Option<String> {
    let oxc_expr = expr.oxc_expression()?;
    match oxc_expr.get_inner_expression() {
        OxcExpression::StringLiteral(s) => Some(s.value.to_string()),
        OxcExpression::TemplateLiteral(t) if t.expressions.is_empty() => {
            // Pure template literal with no interpolations
            Some(t.quasis.iter().map(|q| q.value.raw.as_str()).collect())
        }
        OxcExpression::NumericLiteral(n) => Some(n.value.to_string()),
        OxcExpression::NullLiteral(_) => Some(String::new()),
        _ => {
            // Try compile-time evaluation for pure calls like Math.max(0, Math.min(0, 100))
            if is_pure_oxc_expression(oxc_expr) {
                try_eval_constant(oxc_expr)
            } else {
                None
            }
        }
    }
}

/// Build a template string using `$0`, `$1` params for dynamic expressions.
pub(super) fn build_template_string_with_folding_params(children: &[&Node]) -> String {
    let mut parts = Vec::new();
    let mut param_idx = 0;
    for child in children {
        match child {
            Node::Text(t) => parts.push(t.data.to_string()),
            Node::ExpressionTag(tag) => {
                if let Some(folded) = try_fold_expression_to_string(&tag.expression) {
                    parts.push(folded);
                } else {
                    parts.push(format!("${{${param_idx} ?? ''}}"));
                    param_idx += 1;
                }
            }
            _ => {}
        }
    }
    parts.join("")
}

/// Build a template string from a list of text/expression nodes, folding
/// constant expressions (like `{' '}`) into literal text.
pub(super) fn build_template_string_with_folding(children: &[&Node]) -> String {
    build_template_string_impl(children, true)
}

pub(super) fn build_template_string_no_null_coalesce(children: &[&Node]) -> String {
    build_template_string_impl(children, false)
}

pub(super) fn build_template_string_impl(children: &[&Node], null_coalesce: bool) -> String {
    let mut parts: Vec<(bool, String)> = Vec::new(); // (is_text, content)
    for child in children {
        match child {
            Node::Text(t) => {
                // Collapse whitespace in text parts (normalize tabs/newlines to spaces)
                let collapsed = collapse_template_whitespace(&t.data);
                parts.push((true, collapsed));
            }
            Node::ExpressionTag(tag) => {
                // Try to fold constant expressions
                if let Some(folded) = try_fold_expression_to_string(&tag.expression) {
                    parts.push((false, folded));
                } else if let Some(expr) = tag.expression.render() {
                    if null_coalesce {
                        parts.push((false, format!("${{{expr} ?? ''}}")));
                    } else {
                        parts.push((false, format!("${{{expr}}}")));
                    }
                }
            }
            _ => {}
        }
    }

    // Trim leading whitespace from first text part, trailing from last text part
    if let Some((true, first_text)) = parts.first_mut() {
        *first_text = first_text.trim_start().to_string();
    }
    if let Some((true, last_text)) = parts.last_mut() {
        *last_text = last_text.trim_end().to_string();
    }

    parts.iter().map(|(_, s)| s.as_str()).collect::<Vec<_>>().join("")
}

/// Render an expression preferring source text over OXC codegen to preserve formatting.
pub(super) fn render_expression_from_source(expr: &crate::ast::modern::Expression) -> Option<String> {
    // Use source snippet (original text) when available
    if let Some(snippet) = expr.source_snippet() {
        let trimmed = snippet.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    // Fallback to OXC codegen
    expr.render()
}

pub(super) fn join_statements_with_blank_lines(statements: &[String]) -> String {
    if statements.is_empty() {
        return String::new();
    }
    let mut result = String::new();
    for (i, stmt) in statements.iter().enumerate() {
        if i > 0 {
            let prev = &statements[i - 1];
            // Add blank line between different statement types
            if should_add_blank_line_between(prev, stmt) {
                result.push('\n');
            }
            result.push('\n');
        }
        result.push_str(stmt);
    }
    result
}

/// Determine if a blank line should be added between two statements.
pub(super) fn should_add_blank_line_between(prev: &str, next: &str) -> bool {
    let prev_kind = statement_kind(prev);
    let next_kind = statement_kind(next);

    // Always blank line between or around multi-line statements (functions, classes)
    if prev.contains('\n') || next.contains('\n') {
        return true;
    }

    // Blank line when statement kind changes (e.g., declarations → expressions → functions)
    prev_kind != next_kind
}

#[derive(PartialEq)]
pub(super) enum StatementKind {
    Declaration,
    Function,
    Class,
    Expression,
}

pub(super) fn statement_kind(s: &str) -> StatementKind {
    if s.starts_with("let ") || s.starts_with("const ") || s.starts_with("var ") {
        StatementKind::Declaration
    } else if s.starts_with("function ") || s.starts_with("async function ") {
        StatementKind::Function
    } else if s.starts_with("class ") {
        StatementKind::Class
    } else {
        StatementKind::Expression
    }
}

/// Extract leading line comments (// ...) from the gap between two statement spans.
pub(super) fn extract_leading_comments(snippet: &str, start: usize, end: usize) -> Vec<String> {
    let gap = snippet.get(start..end).unwrap_or("");
    let mut comments = Vec::new();
    for line in gap.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("//") {
            comments.push(trimmed.to_string());
        }
    }
    comments
}

/// Prepend extracted comments to a rendered statement.
pub(super) fn prepend_comments(comments: &[String], rendered: &str) -> String {
    if comments.is_empty() {
        return rendered.to_string();
    }
    let mut result = comments.join("\n");
    result.push('\n');
    result.push_str(rendered);
    result
}

/// Check if an element needs JS-side traversal (not purely static in template).
/// Returns true if the element or any descendant has:
/// - Dynamic children (expressions, blocks, components)
/// - Dynamic/event/property attributes
/// - Is a custom element
/// - Has autofocus, muted attributes (handled as JS properties)
/// - Has value attribute on option (handled as JS property)
pub(super) fn element_needs_js_traversal(el: &RegularElement) -> bool {
    // Custom elements always need traversal
    if is_custom_element(&el.name) {
        return true;
    }

    // Select/optgroup with rich content (options with elements, Component/RenderTag/HtmlTag children)
    // need JS traversal for $.customizable_select()
    if &*el.name == "select" || &*el.name == "optgroup" {
        let sig: Vec<&Node> = el.fragment.nodes.iter()
            .filter(|n| !is_whitespace_text(n) && !matches!(n, Node::Comment(_)))
            .collect();
        if select_children_need_wrapper(&sig) {
            return true;
        }
        // Check for rich options
        for child in &el.fragment.nodes {
            match child {
                Node::RegularElement(opt) if &*opt.name == "option" && option_has_rich_content(opt) => return true,
                Node::RegularElement(og) if &*og.name == "optgroup" && element_needs_js_traversal(og) => return true,
                Node::EachBlock(_) | Node::IfBlock(_) | Node::KeyBlock(_) | Node::SvelteBoundary(_) => return true,
                _ => {}
            }
        }
    }

    // Check for attributes that need JS handling
    for attr in &el.attributes {
        match attr {
            Attribute::Attribute(a) => {
                if a.name.starts_with("on") {
                    return true;
                }
                if is_dynamic_attribute_value(&a.value) {
                    return true;
                }
                if &*a.name == "autofocus" || &*a.name == "muted" {
                    return true;
                }
                if &*a.name == "value" && &*el.name == "option" {
                    return true;
                }
            }
            _ => return true, // Spread, directive, etc.
        }
    }

    // Check children recursively
    for child in &el.fragment.nodes {
        match child {
            Node::ExpressionTag(_)
            | Node::HtmlTag(_)
            | Node::ConstTag(_)
            | Node::DebugTag(_)
            | Node::RenderTag(_)
            | Node::IfBlock(_)
            | Node::EachBlock(_)
            | Node::AwaitBlock(_)
            | Node::KeyBlock(_)
            | Node::SnippetBlock(_)
            | Node::SvelteComponent(_)
            | Node::SvelteElement(_)
            | Node::SvelteSelf(_)
            | Node::Component(_)
            | Node::SvelteBoundary(_) => return true,
            Node::RegularElement(child_el) => {
                if element_needs_js_traversal(child_el) {
                    return true;
                }
            }
            _ => {}
        }
    }

    false
}

/// Check if select/optgroup children need a full `$.customizable_select()` wrapper.
/// Returns true when direct children include Component/RenderTag/HtmlTag, or
/// when each/if blocks contain Component/RenderTag/HtmlTag in their body.
pub(super) fn select_children_need_wrapper(children: &[&Node]) -> bool {
    fn has_rich_types(nodes: &[Node]) -> bool {
        nodes.iter().any(|n| matches!(n, Node::Component(_) | Node::RenderTag(_) | Node::HtmlTag(_)))
    }
    children.iter().any(|child| match child {
        Node::Component(_) | Node::RenderTag(_) | Node::HtmlTag(_) => true,
        Node::EachBlock(each) => has_rich_types(&each.body.nodes),
        Node::IfBlock(if_block) => {
            if has_rich_types(&if_block.consequent.nodes) {
                return true;
            }
            if let Some(alternate) = &if_block.alternate {
                use crate::ast::modern::Alternate;
                match &**alternate {
                    Alternate::Fragment(frag) => has_rich_types(&frag.nodes),
                    Alternate::IfBlock(nested) => has_rich_types(&nested.consequent.nodes),
                }
            } else {
                false
            }
        }
        _ => false,
    })
}

/// Check if an `<option>` has "rich content" — i.e., its children include
/// non-text nodes like HTML elements, components, {@html}, {@render}, etc.
pub(super) fn option_has_rich_content(el: &RegularElement) -> bool {
    el.fragment.nodes.iter().any(|child| matches!(child,
        Node::RegularElement(_) | Node::Component(_) | Node::HtmlTag(_) | Node::RenderTag(_)
    ))
}

pub(super) fn each_body_has_rich_content(fragment: &Fragment) -> bool {
    for node in &fragment.nodes {
        match node {
            Node::Component(_) | Node::RenderTag(_) | Node::HtmlTag(_) => return true,
            Node::RegularElement(el) if &*el.name == "option" => {
                if option_has_rich_content(el) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// Check if a <select> element needs special server-side rendering.
/// All <select> elements with any content go through the special path.
pub(super) fn has_option_children(element: &RegularElement) -> bool {
    element.fragment.nodes.iter().any(|n| !is_whitespace_text(n) && !matches!(n, Node::Comment(_)))
}

/// Check if select children need a `<!>` fragment anchor (customizable_select pattern).
/// Only checks for components/snippets/html inside dynamic blocks (each/if),
/// since top-level ones already emit their own `<!>` markers.
pub(super) fn select_needs_fragment_anchor(children: &[Node]) -> bool {
    fn has_dynamic_content(nodes: &[Node]) -> bool {
        for node in nodes {
            match node {
                Node::Component(_) | Node::RenderTag(_) | Node::HtmlTag(_) => return true,
                Node::EachBlock(each) => {
                    if has_dynamic_content(&each.body.nodes) { return true; }
                }
                Node::IfBlock(if_block) => {
                    if has_dynamic_content(&if_block.consequent.nodes) { return true; }
                    if let Some(ref alt) = if_block.alternate {
                        match &**alt {
                            Alternate::IfBlock(nested) => {
                                if has_dynamic_content(&nested.consequent.nodes) { return true; }
                            }
                            Alternate::Fragment(frag) => {
                                if has_dynamic_content(&frag.nodes) { return true; }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        false
    }
    // Only check inside dynamic blocks, not top-level
    for child in children {
        match child {
            Node::EachBlock(each) => {
                if has_dynamic_content(&each.body.nodes) { return true; }
            }
            Node::IfBlock(if_block) => {
                if has_dynamic_content(&if_block.consequent.nodes) { return true; }
                if let Some(ref alt) = if_block.alternate {
                    match &**alt {
                        Alternate::IfBlock(nested) => {
                            if has_dynamic_content(&nested.consequent.nodes) { return true; }
                        }
                        Alternate::Fragment(frag) => {
                            if has_dynamic_content(&frag.nodes) { return true; }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    false
}

/// Detect indices of nodes that are part of text+expression runs at fragment level.
/// A text+expression run is a consecutive sequence of Text and ExpressionTag nodes
/// where at least one ExpressionTag is present — the entire run shares a single text anchor.
pub(super) fn detect_text_expr_runs(nodes: &[Node], first_sig: usize, last_sig: usize) -> HashSet<usize> {
    let mut run_indices = HashSet::new();

    // Find runs of consecutive Text/ExpressionTag nodes (skipping whitespace-only text and snippets)
    let mut i = first_sig;
    while i <= last_sig {
        // Check if this starts a text+expression run
        let run_start = i;
        let mut run_end = i;
        let mut has_expr = false;
        let mut has_text = false;

        loop {
            if run_end > last_sig {
                break;
            }
            match &nodes[run_end] {
                Node::Text(t) => {
                    if !t.data.trim().is_empty() {
                        has_text = true;
                    }
                    run_end += 1;
                }
                Node::ExpressionTag(_) => {
                    has_expr = true;
                    run_end += 1;
                }
                Node::SnippetBlock(_) => {
                    // Skip snippet blocks — they don't occupy DOM space
                    run_end += 1;
                }
                _ => break,
            }
        }

        // If we found a run with both text and expressions, mark all indices
        if has_expr && has_text && run_end > run_start + 1 {
            for (j, node) in nodes.iter().enumerate().skip(run_start).take(run_end - run_start) {
                match node {
                    Node::Text(_) | Node::ExpressionTag(_) => {
                        run_indices.insert(j);
                    }
                    _ => {}
                }
            }
        }

        i = if run_end > i { run_end } else { i + 1 };
    }

    run_indices
}

/// Flush impure attribute effects as batched template_effects.
/// Custom element effects get individual template_effects.
/// Regular element effects are batched into one template_effect with numbered params and deps array.
pub(super) fn flush_impure_attr_effects(effects: &[ImpureAttrEffect]) -> Vec<String> {
    let mut result = Vec::new();

    // Separate custom element effects from regular element effects
    let custom: Vec<&ImpureAttrEffect> = effects.iter().filter(|e| e.is_custom).collect();
    let regular: Vec<&ImpureAttrEffect> = effects.iter().filter(|e| !e.is_custom).collect();

    // Custom element effects → individual template_effects
    for effect in &custom {
        result.push(format!(
            "$.template_effect(() => $.set_custom_element_data({}, '{}', {}()));\n\n",
            effect.el_var, effect.attr_name, effect.dep
        ));
    }

    // Regular element effects → batched template_effect
    if regular.len() == 1 {
        let e = regular[0];
        result.push(format!(
            "$.template_effect(() => $.set_attribute({}, '{}', {}()));\n",
            e.el_var, e.attr_name, e.dep
        ));
    } else if regular.len() > 1 {
        // Batched: $.template_effect(($0, $1, ...) => { ... }, [dep0, dep1, ...])
        let params: Vec<String> = (0..regular.len()).map(|i| format!("${i}")).collect();
        let deps: Vec<String> = regular.iter().map(|e| e.dep.clone()).collect();
        let mut callback_body = String::new();
        for (i, e) in regular.iter().enumerate() {
            callback_body.push_str(&format!(
                "\t\t$.set_attribute({}, '{}', ${i});\n",
                e.el_var, e.attr_name
            ));
        }
        result.push(format!(
            "$.template_effect(\n\t({}) => {{\n{}\t}},\n\t[{}]\n);\n",
            params.join(", "),
            callback_body,
            deps.join(", ")
        ));
    }

    // Custom effects come first (based on expected output order)
    // Actually, let me re-check: in the expected output, custom comes before regular
    // The current order is: custom first, then regular. That matches.

    result
}

/// Check if any component in a fragment has bind: directives.
pub(super) fn fragment_has_component_bindings(fragment: &Fragment) -> bool {
    for node in &fragment.nodes {
        if let Node::Component(comp) = node {
            for attr in comp.attributes.iter() {
                if let Attribute::BindDirective(bind) = attr {
                    // bind:this is client-only, doesn't need $$settled on server
                    if bind.name.as_ref() != "this" {
                        return true;
                    }
                }
            }
        }
    }
    false
}

pub(super) fn is_void_element(name: &str) -> bool {
    matches!(
        name,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

/// Check if an element is an SVG element (attributes preserve case).
pub(super) fn is_svg_element(name: &str) -> bool {
    matches!(
        name,
        "svg"
            | "circle"
            | "ellipse"
            | "g"
            | "line"
            | "path"
            | "polygon"
            | "polyline"
            | "rect"
            | "text"
            | "tspan"
            | "textPath"
            | "defs"
            | "use"
            | "symbol"
            | "marker"
            | "clipPath"
            | "linearGradient"
            | "radialGradient"
            | "stop"
            | "filter"
            | "feBlend"
            | "feColorMatrix"
            | "feComponentTransfer"
            | "feComposite"
            | "feConvolveMatrix"
            | "feDiffuseLighting"
            | "feDisplacementMap"
            | "feDistantLight"
            | "feDropShadow"
            | "feFlood"
            | "feGaussianBlur"
            | "feImage"
            | "feMerge"
            | "feMergeNode"
            | "feMorphology"
            | "feOffset"
            | "fePointLight"
            | "feSpecularLighting"
            | "feSpotLight"
            | "feTile"
            | "feTurbulence"
            | "foreignObject"
            | "image"
            | "mask"
            | "pattern"
            | "animate"
            | "animateMotion"
            | "animateTransform"
            | "set"
    )
}

/// Check if an element is a custom element (contains a hyphen).
pub(super) fn is_custom_element(name: &str) -> bool {
    name.contains('-')
}

/// Sanitize an element name for use as a JS variable (replace hyphens with underscores).
pub(super) fn sanitize_var_name(name: &str) -> String {
    name.replace('-', "_")
}

/// Events that can use event delegation (bubbling DOM events).
pub(super) fn is_delegatable_event(name: &str) -> bool {
    matches!(
        name,
        "click"
            | "dblclick"
            | "mousedown"
            | "mouseup"
            | "mousemove"
            | "mouseenter"
            | "mouseleave"
            | "mouseover"
            | "mouseout"
            | "keydown"
            | "keypress"
            | "keyup"
            | "input"
            | "change"
            | "focus"
            | "blur"
            | "focusin"
            | "focusout"
            | "submit"
            | "reset"
            | "scroll"
            | "pointerdown"
            | "pointerup"
            | "pointermove"
            | "pointerenter"
            | "pointerleave"
            | "pointerover"
            | "pointerout"
            | "pointercancel"
            | "gotpointercapture"
            | "lostpointercapture"
            | "touchstart"
            | "touchend"
            | "touchmove"
            | "touchcancel"
            | "contextmenu"
            | "wheel"
            | "drag"
            | "dragstart"
            | "dragend"
            | "dragenter"
            | "dragleave"
            | "dragover"
            | "drop"
    )
}
