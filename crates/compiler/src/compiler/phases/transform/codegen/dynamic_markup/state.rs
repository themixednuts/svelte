use super::*;

/// Check if an expression is a `$state()` or `$derived()` call.
pub(super) fn is_state_or_derived_call(expr: &OxcExpression<'_>) -> bool {
    if let OxcExpression::CallExpression(call) = expr.get_inner_expression()
        && let OxcExpression::Identifier(id) = call.callee.get_inner_expression()
    {
        return matches!(id.name.as_str(), "$state" | "$derived" | "$derived.by");
    }
    false
}

pub(super) fn extract_rune_call_info(snippet: &str, expr: &OxcExpression<'_>) -> Option<RuneCallInfo> {
    let OxcExpression::CallExpression(call) = expr.get_inner_expression() else {
        return None;
    };
    let rune = match call.callee.get_inner_expression() {
        OxcExpression::Identifier(id) => match id.name.as_str() {
            "$state" => "$state",
            "$derived" => "$derived",
            _ => return None,
        },
        _ => return None,
    };

    let argument = if let Some(arg) = call.arguments.first() {
        if let Some(expr) = arg.as_expression() {
            let mut codegen = oxc_codegen_for(snippet);
            codegen.print_expression(expr);
            let text = codegen.into_source_text();
            super::super::strip_outer_parens(&text)
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    Some(RuneCallInfo { rune, argument })
}

pub(super) fn collect_instance_state_bindings(script: &Script) -> BTreeSet<String> {
    let mut bindings = BTreeSet::new();
    for statement in &script.oxc_program().body {
        let declaration = match statement {
            OxcStatement::VariableDeclaration(d) => Some(&**d),
            OxcStatement::ExportNamedDeclaration(e) => match e.declaration.as_ref() {
                Some(Declaration::VariableDeclaration(d)) => Some(&**d),
                _ => None,
            },
            _ => None,
        };
        let Some(declaration) = declaration else {
            continue;
        };
        for declarator in &declaration.declarations {
            let Some(id) = declarator.id.get_binding_identifier() else {
                continue;
            };
            let Some(init) = declarator.init.as_ref() else {
                continue;
            };
            if oxc_state_call_argument(init).is_some() {
                bindings.insert(id.name.to_string());
            }
        }
    }
    bindings
}

/// Collect which mutated state bindings use $.proxy() (object/array argument).
/// Proxy bindings do NOT need $.get() wrapping — they're already reactive via the proxy.
pub(super) fn collect_proxy_bindings(script: &Script, mutated: &BTreeSet<String>) -> BTreeSet<String> {
    let mut proxies = BTreeSet::new();
    for statement in &script.oxc_program().body {
        let declaration = match statement {
            OxcStatement::VariableDeclaration(d) => Some(&**d),
            OxcStatement::ExportNamedDeclaration(e) => match e.declaration.as_ref() {
                Some(Declaration::VariableDeclaration(d)) => Some(&**d),
                _ => None,
            },
            _ => None,
        };
        let Some(declaration) = declaration else { continue };
        for declarator in &declaration.declarations {
            let Some(id) = declarator.id.get_binding_identifier() else { continue };
            let name = id.name.to_string();
            if !mutated.contains(&name) { continue; }
            let Some(init) = declarator.init.as_ref() else { continue };
            if let Some(arg) = oxc_state_call_argument(init)
                && arg.is_proxy_like()
            {
                proxies.insert(name);
            }
        }
    }
    proxies
}

/// Collect $derived() binding names — these are signals that need $.get() wrapping.
pub(super) fn collect_derived_bindings(script: &Script) -> BTreeSet<String> {
    let mut derived = BTreeSet::new();
    for statement in &script.oxc_program().body {
        let declaration = match statement {
            OxcStatement::VariableDeclaration(d) => Some(&**d),
            OxcStatement::ExportNamedDeclaration(e) => match e.declaration.as_ref() {
                Some(Declaration::VariableDeclaration(d)) => Some(&**d),
                _ => None,
            },
            _ => None,
        };
        let Some(declaration) = declaration else { continue };
        for declarator in &declaration.declarations {
            let Some(id) = declarator.id.get_binding_identifier() else { continue };
            let Some(init) = declarator.init.as_ref() else { continue };
            if is_derived_call(init) {
                derived.insert(id.name.to_string());
            }
        }
    }
    derived
}

/// Check if an expression is a $derived() or $derived.by() call.
pub(super) fn is_derived_call(expr: &OxcExpression<'_>) -> bool {
    match expr.get_inner_expression() {
        OxcExpression::CallExpression(call) => {
            match &call.callee {
                OxcExpression::Identifier(id) => id.name == "$derived",
                OxcExpression::StaticMemberExpression(member) => {
                    if let OxcExpression::Identifier(obj) = &member.object {
                        obj.name == "$derived" && member.property.name == "by"
                    } else {
                        false
                    }
                }
                _ => false,
            }
        }
        _ => false,
    }
}

/// Collect state bindings that are actually mutated (assigned to) in the script.
/// Only mutated state variables need `$.state()` — unmutated ones become plain values.
pub(super) fn collect_mutated_state_bindings(
    script: &Script,
    state_bindings: &BTreeSet<String>,
    root: Option<&Root>,
) -> BTreeSet<String> {
    let mut mutated = BTreeSet::new();
    for statement in &script.oxc_program().body {
        scan_statement_for_mutations(statement, state_bindings, &mut mutated);
    }
    // Also scan template expressions/handlers for mutations
    if let Some(root) = root {
        scan_fragment_for_mutations(&root.fragment, state_bindings, &mut mutated);
    }
    mutated
}

pub(super) fn scan_fragment_for_mutations(
    fragment: &Fragment,
    state_bindings: &BTreeSet<String>,
    mutated: &mut BTreeSet<String>,
) {
    for node in &fragment.nodes {
        match node {
            Node::RegularElement(el) => {
                // Scan attribute expressions for mutations
                for attr in el.attributes.iter() {
                    match attr {
                        Attribute::Attribute(a) => {
                            scan_attribute_value_for_mutations(&a.value, state_bindings, mutated);
                        }
                        Attribute::BindDirective(bind) => {
                            if let Some(oxc_expr) = bind.expression.oxc_expression() {
                                scan_expression_for_mutations(oxc_expr, state_bindings, mutated);
                            }
                        }
                        _ => {}
                    }
                }
                scan_fragment_for_mutations(&el.fragment, state_bindings, mutated);
            }
            Node::ExpressionTag(tag) => {
                if let Some(oxc_expr) = tag.expression.oxc_expression() {
                    scan_expression_for_mutations(oxc_expr, state_bindings, mutated);
                }
            }
            Node::IfBlock(if_block) => {
                scan_fragment_for_mutations(&if_block.consequent, state_bindings, mutated);
                if let Some(alt) = &if_block.alternate {
                    match alt.as_ref() {
                        crate::ast::modern::Alternate::Fragment(frag) => {
                            scan_fragment_for_mutations(frag, state_bindings, mutated);
                        }
                        crate::ast::modern::Alternate::IfBlock(inner_if) => {
                            // Recurse into else-if chain
                            scan_fragment_for_mutations(&inner_if.consequent, state_bindings, mutated);
                            if let Some(inner_alt) = &inner_if.alternate
                                && let crate::ast::modern::Alternate::Fragment(f) = inner_alt.as_ref()
                            {
                                scan_fragment_for_mutations(f, state_bindings, mutated);
                            }
                        }
                    }
                }
            }
            Node::EachBlock(each) => {
                scan_fragment_for_mutations(&each.body, state_bindings, mutated);
                if let Some(fallback) = &each.fallback {
                    scan_fragment_for_mutations(fallback, state_bindings, mutated);
                }
            }
            Node::Component(comp) => {
                for attr in comp.attributes.iter() {
                    match attr {
                        Attribute::Attribute(a) => {
                            scan_attribute_value_for_mutations(&a.value, state_bindings, mutated);
                        }
                        Attribute::BindDirective(bind) => {
                            // bind:prop={expr} mutates expr
                            if let Some(expr_str) = bind.expression.render()
                                && state_bindings.contains(expr_str.as_str())
                            {
                                mutated.insert(expr_str);
                            }
                        }
                        _ => {}
                    }
                }
                scan_fragment_for_mutations(&comp.fragment, state_bindings, mutated);
            }
            _ => {}
        }
    }
}

pub(super) fn scan_attribute_value_for_mutations(
    value: &AttributeValueKind,
    state_bindings: &BTreeSet<String>,
    mutated: &mut BTreeSet<String>,
) {
    match value {
        AttributeValueKind::Values(parts) => {
            for part in parts.iter() {
                if let AttributeValue::ExpressionTag(tag) = part
                    && let Some(oxc_expr) = tag.expression.oxc_expression()
                {
                    scan_expression_for_mutations(oxc_expr, state_bindings, mutated);
                }
            }
        }
        AttributeValueKind::ExpressionTag(tag) => {
            if let Some(oxc_expr) = tag.expression.oxc_expression() {
                scan_expression_for_mutations(oxc_expr, state_bindings, mutated);
            }
        }
        _ => {}
    }
}

pub(super) fn scan_statement_for_mutations(
    statement: &OxcStatement<'_>,
    state_bindings: &BTreeSet<String>,
    mutated: &mut BTreeSet<String>,
) {
    match statement {
        OxcStatement::ExpressionStatement(expr_stmt) => {
            scan_expression_for_mutations(&expr_stmt.expression, state_bindings, mutated);
        }
        OxcStatement::IfStatement(if_stmt) => {
            scan_statement_for_mutations(&if_stmt.consequent, state_bindings, mutated);
            if let Some(alt) = &if_stmt.alternate {
                scan_statement_for_mutations(alt, state_bindings, mutated);
            }
        }
        OxcStatement::BlockStatement(block) => {
            for stmt in &block.body {
                scan_statement_for_mutations(stmt, state_bindings, mutated);
            }
        }
        OxcStatement::ForStatement(for_stmt) => {
            if let Some(update) = &for_stmt.update {
                scan_expression_for_mutations(update, state_bindings, mutated);
            }
            scan_statement_for_mutations(&for_stmt.body, state_bindings, mutated);
        }
        OxcStatement::WhileStatement(while_stmt) => {
            scan_statement_for_mutations(&while_stmt.body, state_bindings, mutated);
        }
        OxcStatement::ReturnStatement(ret) => {
            if let Some(arg) = &ret.argument {
                scan_expression_for_mutations(arg, state_bindings, mutated);
            }
        }
        // Function declarations may contain mutations
        OxcStatement::FunctionDeclaration(func) => {
            if let Some(body) = &func.body {
                for stmt in &body.statements {
                    scan_statement_for_mutations(stmt, state_bindings, mutated);
                }
            }
        }
        _ => {}
    }
}

pub(super) fn scan_expression_for_mutations(
    expr: &OxcExpression<'_>,
    state_bindings: &BTreeSet<String>,
    mutated: &mut BTreeSet<String>,
) {
    match expr {
        OxcExpression::AssignmentExpression(assign) => {
            // Check if left side is a state binding
            if let Some(name) = assignment_target_name(&assign.left)
                && state_bindings.contains(&name)
            {
                mutated.insert(name);
            }
            scan_expression_for_mutations(&assign.right, state_bindings, mutated);
        }
        OxcExpression::UpdateExpression(update) => {
            if let Some(name) = simple_assignment_target_name(&update.argument)
                && state_bindings.contains(&name)
            {
                mutated.insert(name);
            }
        }
        OxcExpression::CallExpression(call) => {
            for arg in &call.arguments {
                if let Some(e) = arg.as_expression() {
                    scan_expression_for_mutations(e, state_bindings, mutated);
                }
            }
            scan_expression_for_mutations(&call.callee, state_bindings, mutated);
        }
        OxcExpression::ArrowFunctionExpression(arrow) => {
            for stmt in &arrow.body.statements {
                scan_statement_for_mutations(stmt, state_bindings, mutated);
            }
        }
        OxcExpression::SequenceExpression(seq) => {
            for e in &seq.expressions {
                scan_expression_for_mutations(e, state_bindings, mutated);
            }
        }
        OxcExpression::ConditionalExpression(cond) => {
            scan_expression_for_mutations(&cond.consequent, state_bindings, mutated);
            scan_expression_for_mutations(&cond.alternate, state_bindings, mutated);
        }
        _ => {}
    }
}

pub(super) fn assignment_target_name(target: &oxc_ast::ast::AssignmentTarget<'_>) -> Option<String> {
    match target {
        oxc_ast::ast::AssignmentTarget::AssignmentTargetIdentifier(id) => {
            Some(id.name.to_string())
        }
        oxc_ast::ast::AssignmentTarget::StaticMemberExpression(mem) => {
            // obj.x = ... → check if obj is a state binding
            if let OxcExpression::Identifier(id) = &mem.object {
                Some(id.name.to_string())
            } else {
                None
            }
        }
        _ => None,
    }
}

pub(super) fn simple_assignment_target_name(target: &oxc_ast::ast::SimpleAssignmentTarget<'_>) -> Option<String> {
    match target {
        oxc_ast::ast::SimpleAssignmentTarget::AssignmentTargetIdentifier(id) => {
            Some(id.name.to_string())
        }
        _ => None,
    }
}

pub(super) fn collect_local_function_names(script: &Script) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for statement in &script.oxc_program().body {
        match statement {
            OxcStatement::FunctionDeclaration(func) => {
                if let Some(id) = &func.id {
                    names.insert(id.name.to_string());
                }
            }
            OxcStatement::ExportNamedDeclaration(export) => {
                if let Some(Declaration::FunctionDeclaration(func)) = &export.declaration
                    && let Some(id) = &func.id
                {
                    names.insert(id.name.to_string());
                }
            }
            _ => {}
        }
    }
    names
}

pub(super) fn collect_instance_imports(source: &str, script: &Script) -> Vec<String> {
    let snippet = &source[script.content_start..script.content_end];
    let mut imports = Vec::new();
    for statement in &script.oxc_program().body {
        if let OxcStatement::ImportDeclaration(_) = statement {
            let span = statement.span();
            if let Some(text) = snippet.get(span.start as usize..span.end as usize) {
                imports.push(text.trim().to_string());
            }
        }
    }
    imports
}

pub(super) fn collect_module_statements(source: &str, script: &Script) -> Vec<String> {
    let snippet = &source[script.content_start..script.content_end];
    let mut stmts = Vec::new();
    for statement in &script.oxc_program().body {
        if matches!(statement, OxcStatement::ImportDeclaration(_)) {
            continue;
        }
        let span = statement.span();
        if let Some(text) = snippet.get(span.start as usize..span.end as usize) {
            stmts.push(reindent_block(text.trim()));
        }
    }
    stmts
}

/// Collect variable bindings that are compile-time constants (never reassigned, no $state/$derived).
pub(super) fn collect_constant_bindings(script: &Script) -> HashMap<String, String> {
    use oxc_ast::ast::BindingPattern;
    let mut candidates = HashMap::new();

    // First pass: collect candidates from variable declarations
    for statement in &script.oxc_program().body {
        let decl = match statement {
            OxcStatement::VariableDeclaration(d) => &**d,
            _ => continue,
        };
        for declarator in &decl.declarations {
            let Some(init) = declarator.init.as_ref() else { continue };
            // Skip $state/$derived/$props calls
            if let OxcExpression::CallExpression(call) = init.get_inner_expression()
                && let OxcExpression::Identifier(id) = call.callee.get_inner_expression()
                && matches!(id.name.as_str(), "$state" | "$derived" | "$props" | "$effect")
            {
                continue;
            }
            // Only handle simple identifier bindings with const-evaluable initializers
            if let BindingPattern::BindingIdentifier(id) = &declarator.id
                && let Some(value) = try_eval_constant(init)
            {
                candidates.insert(id.name.to_string(), value);
            }
        }
    }

    // Second pass: remove candidates that are mutated anywhere in the script
    let all_names: BTreeSet<String> = candidates.keys().cloned().collect();
    let mutated = collect_mutated_state_bindings(script, &all_names, None);
    for name in &mutated {
        candidates.remove(name);
    }

    candidates
}

/// Try to resolve an expression as a constant binding (simple identifier lookup).
pub(super) fn try_resolve_constant_binding(
    expr: &crate::ast::modern::Expression,
    constant_bindings: &HashMap<String, String>,
) -> Option<String> {
    if constant_bindings.is_empty() {
        return None;
    }
    let oxc_expr = expr.oxc_expression()?;
    try_resolve_oxc_constant(oxc_expr, constant_bindings)
}

pub(super) fn try_resolve_oxc_constant(
    expr: &OxcExpression<'_>,
    constant_bindings: &HashMap<String, String>,
) -> Option<String> {
    match expr.get_inner_expression() {
        // Simple identifier
        OxcExpression::Identifier(id) => {
            constant_bindings.get(id.name.as_str()).cloned()
        }
        // LogicalExpression: name ?? 'fallback', or (name ?? 'a') ?? null
        OxcExpression::LogicalExpression(logical) => {
            use oxc_ast::ast::LogicalOperator;
            // Try resolving left side first
            if let Some(left_val) = try_resolve_oxc_constant(&logical.left, constant_bindings) {
                match logical.operator {
                    LogicalOperator::Coalesce => {
                        if left_val.is_empty() {
                            // null/undefined → use right
                            try_eval_constant(&logical.right)
                                .or_else(|| try_resolve_oxc_constant(&logical.right, constant_bindings))
                        } else {
                            Some(left_val)
                        }
                    }
                    LogicalOperator::Or => {
                        if left_val.is_empty() || left_val == "0" || left_val == "false" {
                            try_eval_constant(&logical.right)
                                .or_else(|| try_resolve_oxc_constant(&logical.right, constant_bindings))
                        } else {
                            Some(left_val)
                        }
                    }
                    LogicalOperator::And => {
                        if !left_val.is_empty() && left_val != "0" && left_val != "false" {
                            try_eval_constant(&logical.right)
                                .or_else(|| try_resolve_oxc_constant(&logical.right, constant_bindings))
                        } else {
                            Some(left_val)
                        }
                    }
                }
            } else {
                None
            }
        }
        _ => None,
    }
}

pub(super) fn has_props_rune(root: &Root) -> bool {
    if let Some(instance) = root.instance.as_ref() {
        for statement in &instance.oxc_program().body {
            if contains_props_call(statement) {
                return true;
            }
        }
    }
    false
}

pub(super) fn contains_props_call(statement: &OxcStatement<'_>) -> bool {
    match statement {
        OxcStatement::VariableDeclaration(decl) => decl.declarations.iter().any(|d| {
            d.init.as_ref().is_some_and(|init| {
                if let OxcExpression::CallExpression(call) = init.get_inner_expression()
                    && let OxcExpression::Identifier(id) = call.callee.get_inner_expression()
                {
                    return id.name.as_str() == "$props";
                }
                false
            })
        }),
        OxcStatement::ExportNamedDeclaration(export) => {
            if let Some(Declaration::VariableDeclaration(decl)) = export.declaration.as_ref() {
                decl.declarations.iter().any(|d| {
                    d.init.as_ref().is_some_and(|init| {
                        if let OxcExpression::CallExpression(call) = init.get_inner_expression()
                            && let OxcExpression::Identifier(id) =
                                call.callee.get_inner_expression()
                        {
                            return id.name.as_str() == "$props";
                        }
                        false
                    })
                })
            } else {
                false
            }
        }
        _ => false,
    }
}

pub(super) fn has_class_rune_fields(root: &Root) -> bool {
    if let Some(instance) = root.instance.as_ref() {
        use oxc_ast::ast::ClassElement;
        for statement in &instance.oxc_program().body {
            let class = match statement {
                OxcStatement::ClassDeclaration(cls) => Some(cls.as_ref()),
                OxcStatement::ExportNamedDeclaration(export) => match export.declaration.as_ref() {
                    Some(Declaration::ClassDeclaration(cls)) => Some(cls.as_ref()),
                    _ => None,
                },
                _ => None,
            };
            if let Some(class) = class {
                for element in &class.body.body {
                    if let ClassElement::PropertyDefinition(prop) = element
                        && let Some(init) = &prop.value
                        && is_state_or_derived_call(init)
                    {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Collect destructured prop names from a $props() destructuring pattern.
pub(super) fn collect_destructured_prop_names(script: &Script) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let program = script.oxc_program();
    for stmt in &program.body {
        if let OxcStatement::VariableDeclaration(decl) = stmt {
            for declarator in &decl.declarations {
                if let Some(init) = declarator.init.as_ref()
                    && let OxcExpression::CallExpression(call) = init.get_inner_expression()
                    && let OxcExpression::Identifier(id) = call.callee.get_inner_expression()
                    && id.name.as_str() == "$props"
                    && let oxc_ast::ast::BindingPattern::ObjectPattern(obj) = &declarator.id
                {
                    for prop in &obj.properties {
                        // Skip props with default values — they need $.prop()
                        if matches!(&prop.value, oxc_ast::ast::BindingPattern::AssignmentPattern(_)) {
                            return BTreeSet::new();
                        }
                        if let Some(name) = prop.key.static_name() {
                            names.insert(name.to_string());
                        }
                    }
                    // Also skip if there's a rest element
                    if obj.rest.is_some() {
                        return BTreeSet::new();
                    }
                }
            }
        }
    }
    names
}

/// Detect `let <name> = $props()` in the instance script, returning the binding name.
pub(super) fn detect_props_binding(script: &Script) -> Option<String> {
    use oxc_ast::ast::BindingPattern;
    for statement in &script.oxc_program().body {
        let decl = match statement {
            OxcStatement::VariableDeclaration(d) => &**d,
            OxcStatement::ExportNamedDeclaration(e) => match e.declaration.as_ref() {
                Some(Declaration::VariableDeclaration(d)) => &**d,
                _ => continue,
            },
            _ => continue,
        };
        for declarator in &decl.declarations {
            let Some(init) = declarator.init.as_ref() else { continue };
            if let OxcExpression::CallExpression(call) = init.get_inner_expression()
                && let OxcExpression::Identifier(id) = call.callee.get_inner_expression()
                && id.name.as_str() == "$props"
            {
                match &declarator.id {
                    BindingPattern::BindingIdentifier(binding_id) => {
                        return Some(binding_id.name.to_string());
                    }
                    BindingPattern::ObjectPattern(_) => {
                        // Destructured $props — use a sentinel name
                        return Some("$$destructured_props".to_string());
                    }
                    _ => {}
                }
            }
        }
    }
    None
}

/// Rewrite state variable accesses in rendered expressions:
/// - `name` (standalone identifier in expression context) → `$.get(name)`
/// - `name++` or `++name` → `$.update(name)`
/// - `name--` or `--name` → `$.update(name, -1)`
pub(super) fn rewrite_state_accesses(text: &str, state_bindings: &BTreeSet<String>) -> String {
    if state_bindings.is_empty() {
        return text.to_string();
    }
    let mut result = text.to_string();
    for name in state_bindings {
        // Replace name++ and ++name with $.update(name)
        let post_inc = format!("{name}++");
        let pre_inc = format!("++{name}");
        let post_dec = format!("{name}--");
        let pre_dec = format!("--{name}");
        result = result.replace(&post_inc, &format!("$.update({name})"));
        result = result.replace(&pre_inc, &format!("$.update({name})"));
        result = result.replace(&post_dec, &format!("$.update({name}, -1)"));
        result = result.replace(&pre_dec, &format!("$.update({name}, -1)"));

        // Handle assignments: name = expr, name += expr, etc.
        result = rewrite_state_assignments(&result, name);

        // Replace standalone identifier references with $.get(name)
        // Must not replace inside $.state(name), $.get(name), $.set(name, ...), $.update(name)
        // Use word boundary matching
        let mut new_result = String::with_capacity(result.len());
        let mut i = 0;
        let bytes = result.as_bytes();
        let name_bytes = name.as_bytes();
        let name_len = name_bytes.len();
        while i < bytes.len() {
            if i + name_len <= bytes.len() && &bytes[i..i + name_len] == name_bytes {
                // Check if this is a standalone identifier (not part of a larger word)
                let prev_is_ident = i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_' || bytes[i - 1] == b'$' || bytes[i - 1] == b'.');
                let next_is_ident = i + name_len < bytes.len() && (bytes[i + name_len].is_ascii_alphanumeric() || bytes[i + name_len] == b'_' || bytes[i + name_len] == b'$');
                if !prev_is_ident && !next_is_ident {
                    // Check if this is already inside $.get(), $.set(), $.state(), $.update()
                    let prefix_4 = if i >= 6 { std::str::from_utf8(&bytes[i.saturating_sub(6)..i]).unwrap_or("") } else { std::str::from_utf8(&bytes[..i]).unwrap_or("") };
                    let already_wrapped = prefix_4.ends_with("$.get(")
                        || prefix_4.ends_with("$.set(")
                        || prefix_4.ends_with("state(")
                        || prefix_4.ends_with("pdate(");
                    if already_wrapped {
                        new_result.push_str(name);
                    } else {
                        new_result.push_str(&format!("$.get({name})"));
                    }
                    i += name_len;
                    continue;
                }
            }
            new_result.push(bytes[i] as char);
            i += 1;
        }
        result = new_result;
    }
    result
}

/// Rewrite assignments to state variables: `name = expr` → `$.set(name, expr)`
/// Handles simple (`=`) and compound (`+=`, `-=`, `*=`, `/=`, `%=`) assignments.
pub(super) fn rewrite_state_assignments(text: &str, name: &str) -> String {
    let mut result = String::new();
    let name_bytes = name.as_bytes();
    let name_len = name_bytes.len();

    for line in text.lines() {
        if !result.is_empty() {
            result.push('\n');
        }
        // Try to match assignment pattern in this line
        if let Some(rewritten) = try_rewrite_assignment_line(line, name, name_bytes, name_len) {
            result.push_str(&rewritten);
        } else {
            result.push_str(line);
        }
    }
    // Preserve trailing newline if present
    if text.ends_with('\n') {
        result.push('\n');
    }
    result
}

/// Try to rewrite a single line containing `name <op>= expr;` → `$.set(name, ...);`
/// Preserves any prefix before the assignment (e.g. `() => ` in arrow functions).
pub(super) fn try_rewrite_assignment_line(line: &str, name: &str, name_bytes: &[u8], name_len: usize) -> Option<String> {
    let bytes = line.as_bytes();

    // Find all positions where `name` appears as a standalone identifier
    let mut i = 0;
    while i + name_len <= bytes.len() {
        if &bytes[i..i + name_len] == name_bytes {
            let prev_ok = i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_' || bytes[i - 1] == b'$' || bytes[i - 1] == b'.');
            let next_pos = i + name_len;
            let next_ok = next_pos >= bytes.len() || !(bytes[next_pos].is_ascii_alphanumeric() || bytes[next_pos] == b'_' || bytes[next_pos] == b'$');

            if prev_ok && next_ok {
                // Check if already wrapped
                let prefix = &line[..i];
                if prefix.ends_with("$.set(") || prefix.ends_with("$.get(") || prefix.ends_with("state(") || prefix.ends_with("pdate(") {
                    i += name_len;
                    continue;
                }

                // Look at what follows the name
                let after_name = &line[next_pos..];
                let after_trimmed = after_name.trim_start();

                // Check for compound assignment operators: +=, -=, *=, /=, %=
                let compound_ops = [("+=", "+"), ("-=", "-"), ("*=", "*"), ("/=", "/"), ("%=", "%")];
                for (op, arith) in &compound_ops {
                    if let Some(rest) = after_trimmed.strip_prefix(op) {
                        let rhs = rest.trim_start();
                        // Find the end: strip trailing `;` or `,` or `)`
                        let (rhs, suffix) = strip_stmt_terminator(rhs);
                        return Some(format!("{prefix}$.set({name}, $.get({name}) {arith} {rhs}){suffix}"));
                    }
                }

                // Check for simple assignment: = (but not == or ===)
                if after_trimmed.starts_with('=') && !after_trimmed.starts_with("==") {
                    let rhs = after_trimmed[1..].trim_start();
                    let (rhs, suffix) = strip_stmt_terminator(rhs);
                    // Add `, true` for non-literal RHS (function calls, expressions)
                    // to force update even if value reference is the same
                    let force = if is_simple_literal(rhs) { "" } else { ", true" };
                    return Some(format!("{prefix}$.set({name}, {rhs}{force}){suffix}"));
                }
            }
        }
        i += 1;
    }
    None
}

/// Rewrite direct member access on the props binding to use $$props.
/// `props.a` → `$$props.a`, `props.a.b` → `$$props.a.b`
/// But NOT computed access: `props[a]` stays as `props[a]`.
/// And NOT direct assignment targets: `props.a = true` stays as `props.a = true`.
pub(super) fn rewrite_props_member_access(text: &str, props_name: &str) -> String {
    let mut result = String::new();
    let mut chars = text.char_indices().peekable();
    let name_len = props_name.len();

    while let Some((i, c)) = chars.next() {
        if text[i..].starts_with(props_name) {
            let before_ok = i == 0 || {
                let prev = text[..i].chars().last().unwrap();
                !prev.is_alphanumeric() && prev != '_' && prev != '$'
            };
            let after_pos = i + name_len;
            if before_ok && after_pos < text.len() && text.as_bytes()[after_pos] == b'.' {
                let after_dot = after_pos + 1;
                if after_dot < text.len() {
                    let next_ch = text.as_bytes()[after_dot];
                    if next_ch.is_ascii_alphabetic() || next_ch == b'_' || next_ch == b'$' {
                        // Check if this is `props.X = ...` (direct assignment to a prop).
                        // Find where `props.X` ends and check if next non-space is `=` (not `==`).
                        let is_direct_assignment = is_props_direct_assignment(text, after_dot);
                        if !is_direct_assignment {
                            result.push_str("$$props");
                            for _ in 0..name_len - 1 {
                                chars.next();
                            }
                            continue;
                        }
                    }
                }
            }
        }
        result.push(c);
    }
    result
}

/// Check if `props.X` at position `prop_start` (the start of X after the dot)
/// is a direct assignment target: `props.X = value` but NOT `props.X.Y` or `props.X == ...`
pub(super) fn is_props_direct_assignment(text: &str, prop_start: usize) -> bool {
    // Scan past the identifier X
    let mut pos = prop_start;
    let bytes = text.as_bytes();
    while pos < bytes.len() && (bytes[pos].is_ascii_alphanumeric() || bytes[pos] == b'_' || bytes[pos] == b'$') {
        pos += 1;
    }
    // Skip whitespace
    while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
        pos += 1;
    }
    // Check if next char is `=` (but not `==`)
    if pos < bytes.len() && bytes[pos] == b'='
        && (pos + 1 >= bytes.len() || bytes[pos + 1] != b'=')
    {
        return true;
    }
    false
}

/// Extract the value from a $state() call for server: $state(val) → val
pub(super) fn extract_state_call_value<'a>(
    init: &'a OxcExpression<'a>,
    snippet: &str,
) -> Option<String> {
    let call = match init.get_inner_expression() {
        OxcExpression::CallExpression(c) => c,
        _ => return None,
    };
    match call.callee.get_inner_expression() {
        OxcExpression::Identifier(id) if id.name.as_str() == "$state" => {}
        _ => return None,
    }
    let arg = call.arguments.first()?.as_expression()?;
    let span = arg.span();
    Some(snippet[span.start as usize..span.end as usize].trim().to_string())
}

/// Describes the kind of derived run closure
pub(super) enum DerivedRunKind {
    /// `$derived(await expr)` → `async () => name = await $.async_derived(() => expr)`
    AsyncDerived(String),
    /// `$derived.by(fn)` → `() => name = $.derived(fn)` or `$derived(fn)` → `() => name = $.derived(() => fn)`
    SyncDerived(String),
}

/// Extract `$derived(...)` / `$derived.by(...)` patterns and generate run closures
pub(super) fn extract_derived_run(
    init: &OxcExpression<'_>,
    snippet: &str,
    binding_name: &str,
) -> Option<DerivedRunKind> {
    let call = match init.get_inner_expression() {
        OxcExpression::CallExpression(c) => c,
        _ => return None,
    };

    let callee = call.callee.get_inner_expression();

    // Check for $derived.by(fn)
    if let OxcExpression::StaticMemberExpression(member) = callee {
        if let OxcExpression::Identifier(id) = member.object.get_inner_expression()
            && id.name.as_str() == "$derived" && member.property.name.as_str() == "by"
        {
            let arg = call.arguments.first()?.as_expression()?;
            let arg_text = &snippet[arg.span().start as usize..arg.span().end as usize];
            let arg_reindented = reindent_block(arg_text.trim());
            return Some(DerivedRunKind::SyncDerived(
                format!("() => {binding_name} = $.derived({arg_reindented})")
            ));
        }
        return None;
    }

    // Check for $derived(...)
    if let OxcExpression::Identifier(id) = callee {
        if id.name.as_str() != "$derived" {
            return None;
        }
    } else {
        return None;
    }

    let arg = call.arguments.first()?.as_expression()?;
    let arg_text = &snippet[arg.span().start as usize..arg.span().end as usize];

    // Check if arg is a function/arrow expression (sync or async)
    let arg_is_function = matches!(
        arg.get_inner_expression(),
        OxcExpression::ArrowFunctionExpression(_) | OxcExpression::FunctionExpression(_)
    );

    // $derived(async () => {...}) or $derived(() => {...}) where arg is function → SyncDerived
    if arg_is_function {
        let arg_reindented = reindent_block(arg_text.trim());
        return Some(DerivedRunKind::SyncDerived(
            format!("() => {binding_name} = $.derived(() => {arg_reindented})")
        ));
    }

    // $derived(await expr) → async () => name = await $.async_derived(() => expr)
    if arg_text.trim().starts_with("await ") {
        let inner = arg_text.trim().strip_prefix("await ").unwrap_or(arg_text.trim());
        return Some(DerivedRunKind::AsyncDerived(
            format!("async () => {binding_name} = await $.async_derived(() => {inner})")
        ));
    }

    // $derived(fn(await expr)) → async () => name = await $.async_derived(async () => fn(await expr))
    if arg_text.contains("await ") {
        return Some(DerivedRunKind::AsyncDerived(
            format!("async () => {binding_name} = await $.async_derived(async () => {arg_text})")
        ));
    }

    None
}
