use super::*;

pub(super) fn compile_instance_script_client(
    source: &str,
    script: &Script,
    runes_mode: bool,
    root: &Root,
) -> Option<InstanceScriptResult> {
    let program = script.oxc_program();
    // OXC spans are 0-based within the script content snippet
    let snippet = &source[script.content_start..script.content_end];

    // Check for top-level await → use async run pattern
    if script_has_top_level_await(program, snippet) {
        let props_binding = detect_props_binding(script);
        return compile_instance_script_client_async_run(snippet, program, &props_binding);
    }

    let state_bindings = collect_instance_state_bindings(script);
    let mutated_bindings = collect_mutated_state_bindings(script, &state_bindings, Some(root));
    let proxy_set = collect_proxy_bindings(script, &mutated_bindings);
    // Only non-proxy state bindings get $.get() wrapping — proxy objects are already reactive
    let non_proxy_bindings: BTreeSet<String> = mutated_bindings.difference(&proxy_set).cloned().collect();
    let props_binding = detect_props_binding(script);

    let mut statements = Vec::new();
    let mut prev_end: usize = 0;

    for statement in &program.body {
        // Extract leading comments from the gap between previous statement and this one
        let stmt_start = statement.span().start as usize;
        let leading_comments = extract_leading_comments(snippet, prev_end, stmt_start);
        prev_end = statement.span().end as usize;

        match statement {
            OxcStatement::ImportDeclaration(_) => {
                // Imports are hoisted to module level
                continue;
            }
            OxcStatement::ExportNamedDeclaration(export) => {
                // Strip export keyword, keep declaration
                if let Some(decl) = export.declaration.as_ref() {
                    let rendered =
                        render_instance_declaration_client(snippet, decl, &state_bindings)?;
                    statements.push(prepend_comments(&leading_comments, &rendered));
                }
            }
            _ => {
                // Check for $props() declaration
                if let Some(ref props_name) = props_binding
                    && let Some(rendered) = render_props_declaration_client(statement, props_name, snippet, runes_mode)
                {
                    statements.push(prepend_comments(&leading_comments, &rendered));
                    continue;
                }
                // Check for state declarations
                if let Some(rendered) = render_state_declaration_with_mutation_analysis(
                    snippet, statement, &mutated_bindings,
                ) {
                    statements.push(prepend_comments(&leading_comments, &rendered));
                    continue;
                }
                // Check for $derived() declarations
                if let Some(rendered) = render_derived_declaration(snippet, statement) {
                    let mut rendered = rendered;
                    if !non_proxy_bindings.is_empty() {
                        rendered = rewrite_state_accesses(&rendered, &non_proxy_bindings);
                    }
                    statements.push(prepend_comments(&leading_comments, &rendered));
                    continue;
                }
                // Check for class declarations with rune fields
                if let Some(rendered) =
                    render_class_with_rune_fields(snippet, statement, GenerateTarget::Client)
                {
                    statements.push(prepend_comments(&leading_comments, &rendered));
                    continue;
                }
                // Use OXC codegen for function declarations (normalizes
                // indentation, blank lines, and spacing) but source
                // extraction for everything else (preserves array
                // formatting, template literals, etc.)
                let mut rendered = if matches!(statement, OxcStatement::FunctionDeclaration(_)) {
                    render_statement_via_codegen(snippet, statement)
                } else {
                    reindent_block(snippet[stmt_start..prev_end].trim())
                };
                // Rewrite $effect/$effect.pre calls
                rendered = rewrite_effect_calls(&rendered);
                // Rewrite props member access: props.x → $$props.x
                if let Some(ref props_name) = props_binding {
                    rendered = rewrite_props_member_access(&rendered, props_name);
                }
                // Rewrite state accesses in function bodies etc.
                if !non_proxy_bindings.is_empty() {
                    rendered = rewrite_state_accesses(&rendered, &non_proxy_bindings);
                }
                statements.push(prepend_comments(&leading_comments, &rendered));
            }
        }
    }

    Some(InstanceScriptResult {
        body: join_statements_with_blank_lines(&statements),
        async_run_info: None,
    })
}

/// Rewrite `$effect(` → `$.user_effect(` and `$effect.pre(` → `$.user_pre_effect(`.
pub(super) fn rewrite_effect_calls(source: &str) -> String {
    source
        .replace("$effect.pre(", "$.user_pre_effect(")
        .replace("$effect(", "$.user_effect(")
}

/// Render a statement using OXC codegen for consistent formatting,
/// with fallback to source extraction.
pub(super) fn render_statement_via_codegen(snippet: &str, statement: &OxcStatement<'_>) -> String {
    let mut codegen = oxc_codegen_for(snippet);
    statement.print(&mut codegen, Context::default());
    let text = codegen.into_source_text();
    text.trim().to_string()
}

/// Check if a script program has any top-level await expressions (not inside async functions/arrows)
pub(super) fn script_has_top_level_await(
    program: &oxc_ast::ast::Program<'_>,
    snippet: &str,
) -> bool {
    for statement in &program.body {
        match statement {
            OxcStatement::ImportDeclaration(_) => continue,
            OxcStatement::VariableDeclaration(decl) => {
                for declarator in &decl.declarations {
                    if let Some(init) = &declarator.init {
                        let span = init.span();
                        let text = &snippet[span.start as usize..span.end as usize];
                        // Check for top-level await (not inside async arrow/function)
                        if text.starts_with("await ") || text.contains(" await ") {
                            return true;
                        }
                        // Check for $derived(await ...)
                        if let OxcExpression::CallExpression(call) = init.get_inner_expression()
                            && let OxcExpression::Identifier(id) = call.callee.get_inner_expression()
                            && id.name.as_str() == "$derived"
                            && let Some(arg) = call.arguments.first()
                            && let Some(expr) = arg.as_expression()
                        {
                            let arg_text = &snippet[expr.span().start as usize..expr.span().end as usize];
                            if arg_text.starts_with("await ") || arg_text.contains(" await ") {
                                return true;
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    false
}

/// Compile client script declarations with top-level await into $.run() pattern.
/// Same logic as server but uses `$.run()` instead of `$$renderer.run()`.
pub(super) fn compile_instance_script_client_async_run(
    snippet: &str,
    program: &oxc_ast::ast::Program<'_>,
    props_binding: &Option<String>,
) -> Option<InstanceScriptResult> {
    let mut hoisted_vars: Vec<String> = Vec::new();
    let mut run_closures: Vec<String> = Vec::new();
    let mut non_run_statements: Vec<String> = Vec::new();
    let mut async_vars: Vec<String> = Vec::new();
    let mut state_vars: Vec<String> = Vec::new();
    let mut has_sync_derived = false;

    for statement in &program.body {
        match statement {
            OxcStatement::ImportDeclaration(_) => continue,
            OxcStatement::VariableDeclaration(decl) => {
                for declarator in &decl.declarations {
                    let binding_name = declarator.id.get_binding_identifier()
                        .map(|id| id.name.to_string())?;

                    if let Some(init) = &declarator.init {
                        let init_span = init.span();
                        let init_text = snippet[init_span.start as usize..init_span.end as usize].trim();

                        // Check for $state() — inline as state value
                        if let Some(state_val) = extract_state_call_value(init, snippet) {
                            state_vars.push(binding_name.clone());
                            non_run_statements.push(format!("let {binding_name} = {state_val};"));
                            continue;
                        }

                        // Check for $derived(await ...) or $derived.by(...) patterns
                        if let Some(derived_kind) = extract_derived_run(init, snippet, &binding_name) {
                            hoisted_vars.push(binding_name.clone());
                            match derived_kind {
                                DerivedRunKind::AsyncDerived(closure) => {
                                    async_vars.push(binding_name);
                                    run_closures.push(closure);
                                }
                                DerivedRunKind::SyncDerived(closure) => {
                                    has_sync_derived = true;
                                    run_closures.push(closure);
                                }
                            }
                            continue;
                        }

                        // Check for top-level await
                        if init_text.starts_with("await ") || init_text.contains(" await ") {
                            hoisted_vars.push(binding_name.clone());
                            async_vars.push(binding_name.clone());
                            run_closures.push(format!("async () => {binding_name} = {init_text}"));
                            continue;
                        }

                        // Regular declaration
                        non_run_statements.push(format!("let {binding_name} = {init_text};"));
                    } else {
                        non_run_statements.push(format!("let {binding_name};"));
                    }
                }
            }
            _ => {
                if let Some(props_name) = props_binding
                    && let Some(rendered) = render_props_declaration_server(statement, props_name, snippet)
                {
                    non_run_statements.push(rendered);
                    continue;
                }
                let rendered = render_statement_via_codegen(snippet, statement);
                let trimmed = rendered.trim();
                // $inspect() calls are dropped on client — add empty run slot
                if trimmed.starts_with("$inspect(") || trimmed.starts_with("$inspect.") {
                    run_closures.push(String::new()); // empty slot
                    continue;
                }
                non_run_statements.push(rendered);
            }
        }
    }

    let run_slot_count = run_closures.len();

    // Build output
    let mut output = String::new();

    // Non-run statements first — separated by blank lines
    for (idx, stmt) in non_run_statements.iter().enumerate() {
        if idx > 0 {
            output.push('\n');
        }
        output.push_str(stmt);
        output.push('\n');
    }

    // Hoisted var declarations — combine on one line if multiple
    if !hoisted_vars.is_empty() {
        output.push_str(&format!("var {};\n", hoisted_vars.join(", ")));
    }

    // $.run() call — same as server but with $.run() instead of $$renderer.run()
    let any_multiline = run_closures.iter().any(|c| c.contains('\n'));
    let has_empty_slots = run_closures.iter().any(|c| c.is_empty());

    if any_multiline {
        if !hoisted_vars.is_empty() {
            output.push('\n');
        }
        output.push_str("var $$promises = $.run([\n");
        for (i, closure) in run_closures.iter().enumerate() {
            for line in closure.lines() {
                output.push('\t');
                output.push_str(line);
                output.push('\n');
            }
            let is_last = i == run_closures.len() - 1;
            if !is_last || has_empty_slots {
                if output.ends_with('\n') {
                    output.pop();
                }
                output.push_str(",\n");
                let next_is_multiline = run_closures.get(i + 1).is_some_and(|c| c.contains('\n'));
                if closure.contains('\n') && next_is_multiline {
                    output.push('\n');
                }
            }
        }
        output.push_str("]);");
    } else {
        output.push_str("var $$promises = $.run([");
        for (i, closure) in run_closures.iter().enumerate() {
            output.push_str(closure);
            if i < run_closures.len() - 1 || has_empty_slots {
                output.push(',');
            }
        }
        output.push_str("]);");
    }

    let info = ServerAsyncRunInfo {
        run_slot_count,
        async_vars,
        state_vars,
        has_sync_derived,
        promise_var: "$$promises".to_string(),
    };

    Some(InstanceScriptResult { body: output, async_run_info: Some(info) })
}

pub(super) fn render_instance_declaration_client(
    snippet: &str,
    decl: &Declaration<'_>,
    _state_bindings: &BTreeSet<String>,
) -> Option<String> {
    Some(render_declaration_from_snippet(snippet, decl))
}

pub(super) fn render_declaration_from_snippet(snippet: &str, decl: &Declaration<'_>) -> String {
    let span = match decl {
        Declaration::VariableDeclaration(d) => d.span,
        Declaration::FunctionDeclaration(d) => d.span,
        Declaration::ClassDeclaration(d) => d.span,
        _ => return String::new(),
    };
    snippet
        .get(span.start as usize..span.end as usize)
        .map(|t| reindent_block(t.trim()))
        .unwrap_or_default()
}

/// Render a $state() declaration with mutation analysis:
/// - If the binding IS mutated → keep as `$.state(x)` (or `$.proxy(x)`)
/// - If the binding is NOT mutated → strip to plain `let name = x`
pub(super) fn render_state_declaration_with_mutation_analysis(
    snippet: &str,
    statement: &OxcStatement<'_>,
    mutated_bindings: &BTreeSet<String>,
) -> Option<String> {
    let declaration = match statement {
        OxcStatement::VariableDeclaration(d) => &**d,
        _ => return None,
    };

    let mut has_state_call = false;
    // Track whether any binding is mutated vs unmutated
    let mut all_unmutated = true;

    for declarator in &declaration.declarations {
        let Some(init) = declarator.init.as_ref() else { continue };
        if oxc_state_call_argument(init).is_none() { continue; }
        has_state_call = true;

        let binding_name = declarator.id.get_binding_identifier()
            .map(|id| id.name.to_string());
        if let Some(ref name) = binding_name
            && mutated_bindings.contains(name)
        {
            all_unmutated = false;
        }
    }

    if !has_state_call {
        return None;
    }

    if all_unmutated {
        // All state bindings are unmutated — render each as plain `let name = value`
        let mut parts = Vec::new();
        for declarator in &declaration.declarations {
            let Some(init) = declarator.init.as_ref() else { continue };
            let Some(argument) = oxc_state_call_argument(init) else { continue };
            let binding_name = declarator.id.get_binding_identifier()
                .map(|id| id.name.to_string())?;
            let value = argument.render(snippet).ok()?;
            parts.push(format!("let {binding_name} = {value};"));
        }
        Some(parts.join("\n"))
    } else {
        // Some bindings are mutated — use $.state()/$.proxy()
        let mut replacements = Vec::new();
        for declarator in &declaration.declarations {
            let Some(init) = declarator.init.as_ref() else { continue };
            let Some(argument) = oxc_state_call_argument(init) else { continue };

            let binding_name = declarator.id.get_binding_identifier()
                .map(|id| id.name.to_string())?;

            let replacement = if mutated_bindings.contains(&binding_name) {
                let helper = if argument.is_proxy_like() {
                    "$.proxy"
                } else {
                    "$.state"
                };
                let rendered = argument.render(snippet).ok()?;
                format!("{helper}({rendered})")
            } else {
                argument.render(snippet).ok()?
            };

            replacements.push(SourceReplacement::from_span(init.span(), replacement));
        }

        let declaration_span = declaration.span();
        replace_source_ranges(snippet, declaration_span, replacements).ok()
    }
}

/// Render `$derived(expr)` → `$.derived(() => expr)` or `$derived.by(fn)` → `$.derived(fn)`
pub(super) fn render_derived_declaration(
    snippet: &str,
    statement: &OxcStatement<'_>,
) -> Option<String> {
    let declaration = match statement {
        OxcStatement::VariableDeclaration(d) => &**d,
        _ => return None,
    };

    let mut has_derived = false;
    let mut replacements = Vec::new();

    for declarator in &declaration.declarations {
        let Some(init) = declarator.init.as_ref() else { continue };
        let OxcExpression::CallExpression(call) = init.get_inner_expression() else { continue };

        match call.callee.get_inner_expression() {
            OxcExpression::Identifier(id) if id.name.as_str() == "$derived" => {
                has_derived = true;
                // $derived(expr) → $.derived(() => expr)
                if let Some(arg) = call.arguments.first()
                    && let Some(expr) = arg.as_expression()
                {
                    let mut codegen = oxc_codegen_for(snippet);
                    codegen.print_expression(expr);
                    let arg_text = codegen.into_source_text();
                    let arg_text = super::super::strip_outer_parens(&arg_text);
                    replacements.push(SourceReplacement::from_span(init.span(), format!("$.derived(() => {arg_text})")));
                }
            }
            OxcExpression::StaticMemberExpression(member) => {
                if let OxcExpression::Identifier(id) = &member.object
                    && id.name.as_str() == "$derived" && member.property.name.as_str() == "by"
                {
                    has_derived = true;
                    // $derived.by(fn) → $.derived(fn)
                    if let Some(arg) = call.arguments.first()
                        && let Some(expr) = arg.as_expression()
                    {
                        let mut codegen = oxc_codegen_for(snippet);
                        codegen.print_expression(expr);
                        let arg_text = codegen.into_source_text();
                        replacements.push(SourceReplacement::from_span(init.span(), format!("$.derived({arg_text})")));
                    }
                }
            }
            _ => {}
        }
    }

    if !has_derived {
        return None;
    }

    let declaration_span = declaration.span();
    let mut result = replace_source_ranges(snippet, declaration_span, replacements).ok()?;
    // Ensure trailing semicolon
    if !result.trim_end().ends_with(';') {
        result = format!("{};", result.trim_end());
    }
    Some(result)
}

pub(super) fn render_class_with_rune_fields(
    snippet: &str,
    statement: &OxcStatement<'_>,
    target: GenerateTarget,
) -> Option<String> {
    let class = match statement {
        OxcStatement::ClassDeclaration(cls) => cls.as_ref(),
        _ => return None,
    };

    // Check if any fields use $state or $derived
    use oxc_ast::ast::ClassElement;
    let has_rune_fields = class.body.body.iter().any(|element| {
        if let ClassElement::PropertyDefinition(prop) = element
            && let Some(init) = &prop.value
        {
            return is_state_or_derived_call(init);
        }
        false
    });

    if !has_rune_fields {
        return None;
    }

    let class_name = class.id.as_ref().map(|id| id.name.as_str()).unwrap_or("Anonymous");
    let mut output = format!("class {class_name} {{\n");

    // First pass: collect private state field names for constructor rewriting
    let mut private_state_fields = BTreeSet::new();
    for element in &class.body.body {
        if let ClassElement::PropertyDefinition(prop) = element
            && let Some(init) = &prop.value
            && is_state_or_derived_call(init)
        {
            // For client: public fields become private, private fields stay private
            if let oxc_ast::ast::PropertyKey::PrivateIdentifier(id) = &prop.key {
                private_state_fields.insert(id.name.to_string());
            } else if target == GenerateTarget::Client
                && let Some(name) = prop.key.static_name()
            {
                private_state_fields.insert(name.to_string());
            }
        }
    }

    // Second pass: emit class body
    for element in &class.body.body {
        match element {
            ClassElement::PropertyDefinition(prop) => {
                let is_static = prop.r#static;
                let (key_name, _is_private) = if let oxc_ast::ast::PropertyKey::PrivateIdentifier(id) = &prop.key {
                    (format!("#{}", id.name), true)
                } else if let Some(name) = prop.key.static_name() {
                    (name.to_string(), false)
                } else {
                    let span = prop.span;
                    if let Some(text) = snippet.get(span.start as usize..span.end as usize) {
                        output.push_str(&format!("\t{}\n", text.trim()));
                    }
                    continue;
                };

                let actual_name = key_name.clone();

                if let Some(init) = &prop.value
                    && let Some(info) = extract_rune_call_info(snippet, init)
                {
                    let RuneCallInfo { rune, argument: arg_text } = info;
                    match target {
                        GenerateTarget::Client => {
                            let private_name = if actual_name.starts_with('#') {
                                actual_name.clone()
                            } else {
                                format!("#{actual_name}")
                            };

                            let static_prefix = if is_static { "static " } else { "" };
                            match rune {
                                "$state" => {
                                    if arg_text.is_empty() {
                                        output.push_str(&format!(
                                            "\t{static_prefix}{private_name} = $.state();\n"
                                        ));
                                    } else {
                                        output.push_str(&format!(
                                            "\t{static_prefix}{private_name} = $.state({arg_text});\n"
                                        ));
                                    }
                                }
                                "$derived" => {
                                    output.push_str(&format!(
                                        "\t{static_prefix}{private_name} = $.derived(() => ({arg_text}));\n"
                                    ));
                                }
                                _ => {
                                    output.push_str(&format!(
                                        "\t{static_prefix}{private_name} = $.{rune}({arg_text});\n"
                                    ));
                                }
                            }

                            // Emit getter/setter if field was public
                            if !actual_name.starts_with('#') {
                                output.push_str(&format!(
                                    "\n\tget {actual_name}() {{\n\t\treturn $.get(this.{private_name});\n\t}}\n"
                                ));
                                let extra_arg = if rune == "$state" { ", true" } else { "" };
                                output.push_str(&format!(
                                    "\n\tset {actual_name}(value) {{\n\t\t$.set(this.{private_name}, value{extra_arg});\n\t}}\n\n"
                                ));
                            }
                        }
                        GenerateTarget::Server => {
                            let static_prefix = if is_static { "static " } else { "" };
                            if rune == "$state" {
                                // Server: $state fields become plain fields
                                if arg_text.is_empty() {
                                    output.push_str(&format!(
                                        "\t{static_prefix}{actual_name};\n"
                                    ));
                                } else {
                                    output.push_str(&format!(
                                        "\t{static_prefix}{actual_name} = {arg_text};\n"
                                    ));
                                }
                            } else if rune == "$derived" {
                                // Server: $derived fields keep the derived call with getter/setter
                                let private_name = if actual_name.starts_with('#') {
                                    actual_name.clone()
                                } else {
                                    format!("#{actual_name}")
                                };
                                output.push_str(&format!(
                                    "\t{static_prefix}{private_name} = $.derived(() => ({arg_text}));\n"
                                ));
                                if !actual_name.starts_with('#') {
                                    output.push_str(&format!(
                                        "\n\tget {actual_name}() {{\n\t\treturn this.{private_name}();\n\t}}\n"
                                    ));
                                    output.push_str(&format!(
                                        "\n\tset {actual_name}($$value) {{\n\t\treturn this.{private_name}($$value);\n\t}}\n\n"
                                    ));
                                }
                            } else if arg_text.is_empty() {
                                output.push_str(&format!(
                                    "\t{static_prefix}{actual_name};\n"
                                ));
                            } else {
                                output.push_str(&format!(
                                    "\t{static_prefix}{actual_name} = {arg_text};\n"
                                ));
                            }
                        }
                        GenerateTarget::None => {}
                    }
                    continue;
                }

                // Non-rune field: render from snippet
                let span = prop.span;
                if let Some(text) = snippet.get(span.start as usize..span.end as usize) {
                    output.push_str(&format!("\t{}\n", text.trim()));
                }
            }
            ClassElement::MethodDefinition(method) => {
                let span = method.span;
                if let Some(text) = snippet.get(span.start as usize..span.end as usize) {
                    let trimmed = text.trim();
                    // Trim trailing blank lines before adding method
                    while output.ends_with("\n\n") {
                        output.pop();
                    }
                    if target == GenerateTarget::Client && !private_state_fields.is_empty() && is_constructor_method(trimmed) {
                        let rewritten = rewrite_constructor_for_client(trimmed, &private_state_fields);
                        let reindented = reindent_method(&rewritten, "\t");
                        output.push_str(&format!("\n{reindented}\n"));
                    } else {
                        let reindented = reindent_method(trimmed, "\t");
                        output.push_str(&format!("\n{reindented}\n"));
                    }
                }
            }
            ClassElement::StaticBlock(block) => {
                let span = block.span;
                if let Some(text) = snippet.get(span.start as usize..span.end as usize) {
                    output.push_str(&format!("\t{}\n", text.trim()));
                }
            }
            _ => {}
        }
    }

    output.push('}');
    Some(output)
}

pub(super) fn is_constructor_method(text: &str) -> bool {
    text.starts_with("constructor(") || text.starts_with("constructor (")
}

/// Rewrite constructor body: `this.#field = value` → `$.set(this.#field, value)`
/// for private state fields only.
pub(super) fn rewrite_constructor_for_client(text: &str, private_state_fields: &BTreeSet<String>) -> String {
    let mut result = String::new();
    for line in text.lines() {
        let trimmed = line.trim();
        // Check for `this.#field = value;` pattern
        if let Some(rewritten) = try_rewrite_private_assignment(trimmed, private_state_fields) {
            // Preserve original indentation
            let indent = &line[..line.len() - line.trim_start().len()];
            result.push_str(indent);
            result.push_str(&rewritten);
            result.push('\n');
        } else {
            result.push_str(line);
            result.push('\n');
        }
    }
    // Remove trailing newline to match the convention
    if result.ends_with('\n') {
        result.pop();
    }
    result
}

pub(super) fn try_rewrite_private_assignment(line: &str, private_state_fields: &BTreeSet<String>) -> Option<String> {
    // Match: this.#fieldname = expression;
    let rest = line.strip_prefix("this.#")?;
    // Find the assignment operator (simple `=`, not `==`, `===`, `+=` etc.)
    let eq_pos = rest.find(" = ")?;
    let field_name = &rest[..eq_pos];
    // Make sure field_name is a valid identifier (no dots or brackets)
    if field_name.contains('.') || field_name.contains('[') {
        return None;
    }
    if !private_state_fields.contains(field_name) {
        return None;
    }
    let value_part = &rest[eq_pos + 3..]; // skip " = "
    let value = value_part.strip_suffix(';').unwrap_or(value_part);
    Some(format!("$.set(this.#{field_name}, {value});"))
}

/// Check if the component uses `await` expressions (in template or script).
pub(super) fn has_async_content_with_source(root: &Root, source: &str) -> bool {
    if has_async_content(root) {
        return true;
    }
    // Check instance script source for await expressions
    if let Some(instance) = root.instance.as_ref()
        && let Some(snippet) = source.get(instance.content_start..instance.content_end)
        && snippet.contains("await ")
    {
        return true;
    }
    false
}

pub(super) fn has_async_content(root: &Root) -> bool {
    fn check_fragment(fragment: &Fragment) -> bool {
        for node in &fragment.nodes {
            if check_node(node) { return true; }
        }
        false
    }

    fn check_node(node: &Node) -> bool {
        match node {
            // Note: AwaitBlock ({#await}) does NOT trigger flags/async —
            // only `await` keyword in expressions does
            Node::AwaitBlock(_) => false,
            Node::IfBlock(if_block) => {
                if let Some(src) = if_block.test.render()
                    && src.contains("await ") { return true; }
                if check_fragment(&if_block.consequent) { return true; }
                if let Some(alt) = &if_block.alternate {
                    match alt.as_ref() {
                        crate::ast::modern::Alternate::Fragment(f) => {
                            if check_fragment(f) { return true; }
                        }
                        crate::ast::modern::Alternate::IfBlock(inner) => {
                            if check_node(&Node::IfBlock(inner.clone())) { return true; }
                        }
                    }
                }
                false
            }
            Node::EachBlock(each) => {
                if let Some(src) = each.expression.render()
                    && src.contains("await ") { return true; }
                if check_fragment(&each.body) { return true; }
                if let Some(ref fallback) = each.fallback
                    && check_fragment(fallback) { return true; }
                false
            }
            Node::ConstTag(ct) => {
                ct.declaration.render().is_some_and(|s| s.contains("await "))
            }
            Node::ExpressionTag(tag) => {
                tag.expression.render().is_some_and(|s| s.contains("await "))
            }
            Node::RegularElement(el) => check_fragment(&el.fragment),
            Node::Component(comp) => check_fragment(&comp.fragment),
            _ => false,
        }
    }

    check_fragment(&root.fragment)
}

/// Render `$props()` declarations on client.
/// - `let props = $props()` → `let props = $.rest_props($$props, [...])`
/// - `let { tag = 'hr' } = $props()` → `let tag = $.prop($$props, 'tag', 3, 'hr')`
pub(super) fn render_props_declaration_client(
    statement: &OxcStatement<'_>,
    props_name: &str,
    _snippet: &str,
    runes_mode: bool,
) -> Option<String> {
    let decl = match statement {
        OxcStatement::VariableDeclaration(d) => &**d,
        _ => return None,
    };
    for declarator in &decl.declarations {
        let Some(init) = declarator.init.as_ref() else { continue };
        if let OxcExpression::CallExpression(call) = init.get_inner_expression()
            && let OxcExpression::Identifier(id) = call.callee.get_inner_expression()
        {
            if id.name.as_str() != "$props" {
                continue;
            }

            // Check if it's a destructured pattern
            use oxc_ast::ast::BindingPattern;
            match &declarator.id {
                BindingPattern::BindingIdentifier(_) => {
                    // Simple: let props = $props()
                    return Some(format!(
                        "let {props_name} = $.rest_props($$props, ['$$slots', '$$events', '$$legacy']);"
                    ));
                }
                BindingPattern::ObjectPattern(obj_pat) => {
                    // Destructured: let { tag = 'hr', name } = $props()
                    let mut prop_lines = Vec::new();
                    let mut rest_name = None;

                    for prop in &obj_pat.properties {
                        let prop_name = prop.key.static_name()?;
                        let prop_name_str = prop_name.to_string();

                        // Calculate flags
                        let mut flags: u32 = 0;
                        if runes_mode {
                            flags |= 1; // PROPS_IS_IMMUTABLE
                            flags |= 2; // PROPS_IS_RUNES
                        }

                        // Check for default value
                        if let BindingPattern::AssignmentPattern(assign) = &prop.value {
                            // Has default value
                            // Render with OXC codegen for consistent formatting
                            let mut codegen = Codegen::new().with_options(codegen_options());
                            codegen.print_expression(&assign.right);
                            let default_rendered = codegen.into_source_text();
                            let default_rendered = default_rendered.trim();

                            prop_lines.push(format!(
                                "let {prop_name_str} = $.prop($$props, '{prop_name_str}', {flags}, {default_rendered});"
                            ));
                        } else {
                            // No default value
                            if flags > 0 {
                                prop_lines.push(format!(
                                    "let {prop_name_str} = $.prop($$props, '{prop_name_str}', {flags});"
                                ));
                            } else {
                                prop_lines.push(format!(
                                    "let {prop_name_str} = $.prop($$props, '{prop_name_str}');"
                                ));
                            }
                        }
                    }

                    // Handle rest element: ...rest
                    if let Some(ref rest) = obj_pat.rest
                        && let BindingPattern::BindingIdentifier(id) = &rest.argument
                    {
                        rest_name = Some(id.name.to_string());
                    }

                    if let Some(rest) = rest_name {
                        let prop_names: Vec<String> = obj_pat.properties.iter()
                            .filter_map(|p| p.key.static_name().map(|n| format!("'{n}'")))
                            .collect();
                        prop_lines.push(format!(
                            "let {rest} = $.rest_props($$props, [{}]);",
                            prop_names.join(", ")
                        ));
                    }

                    return Some(prop_lines.join("\n"));
                }
                _ => {}
            }
        }
    }
    None
}

