use super::*;

// ---------------------------------------------------------------------------
// Server codegen
// ---------------------------------------------------------------------------

/// Shared context for server-side template compilation.
/// Bundles source text and constant bindings to avoid parameter threading.
struct ServerContext<'a> {
    source: &'a str,
    constant_bindings: &'a HashMap<String, String>,
}

impl<'a> ServerContext<'a> {
    fn new(source: &'a str, constant_bindings: &'a HashMap<String, String>) -> Self {
        Self { source, constant_bindings }
    }

    /// Create a context with no constant bindings.
    fn source_only(source: &'a str) -> Self {
        Self { source, constant_bindings: EMPTY_BINDINGS.get_or_init(HashMap::new) }
    }
}

// Thread-local empty bindings map for contexts that don't need constant propagation
static EMPTY_BINDINGS: std::sync::OnceLock<HashMap<String, String>> = std::sync::OnceLock::new();

pub(super) fn compile_server(
    source: &str,
    root: &Root,
    runes_mode: bool,
    component_name: &str,
) -> Option<String> {
    let has_props = has_props_rune(root) || has_class_rune_fields(root);

    // Check if props come only from destructured $props() pattern
    let props_are_destructured_only = runes_mode
        && has_props
        && !has_class_rune_fields(root)
        && root.instance.as_ref().is_some_and(|inst| {
            detect_props_binding(inst) == Some("$$destructured_props".to_string())
        });

    let needs_component_wrapper = has_props && !props_are_destructured_only;

    // Detect if any component in the fragment has bind: directives → $$settled pattern
    let has_component_bindings = fragment_has_component_bindings(&root.fragment);

    // Build instance script body
    let InstanceScriptResult { body: script_body, async_run_info } = if let Some(instance) = root.instance.as_ref() {
        compile_instance_script_server(source, instance, runes_mode)?
    } else {
        InstanceScriptResult { body: String::new(), async_run_info: None }
    };

    // Sync derived ($derived.by, $derived with non-await fn) needs component wrapper + $$props
    let has_props = has_props || async_run_info.as_ref().is_some_and(|info| info.has_sync_derived);
    let needs_component_wrapper = needs_component_wrapper || async_run_info.as_ref().is_some_and(|info| info.has_sync_derived);

    // Collect constant bindings for constant propagation in templates
    let constant_bindings = if let Some(instance) = root.instance.as_ref() {
        collect_constant_bindings(instance)
    } else {
        HashMap::new()
    };

    // Build template output
    let sctx = ServerContext::new(source, &constant_bindings);
    let template_code = if let Some(ref run_info) = async_run_info {
        sctx.compile_fragment_with_script_run(&root.fragment, run_info)?
    } else {
        sctx.compile_fragment(&root.fragment)?
    };

    // Collect hoisted snippet functions
    let hoisted_snippets = collect_server_snippet_functions(&root.fragment, source);

    let uses_async = has_async_content_with_source(root, source);
    let mut output = String::new();
    if uses_async {
        output.push_str("import 'svelte/internal/flags/async';\n");
    }
    output.push_str("import * as $ from 'svelte/internal/server';\n");

    // Module-level imports from module and instance scripts
    for script in [root.module.as_ref(), root.instance.as_ref()].into_iter().flatten() {
        let imports = collect_instance_imports(source, script);
        for imp in &imports {
            output.push_str(imp);
            output.push('\n');
        }
    }

    // Module-level statements from <script module> (non-import)
    if let Some(module_script) = root.module.as_ref() {
        let stmts = collect_module_statements(source, module_script);
        if !stmts.is_empty() {
            output.push('\n');
            for stmt in &stmts {
                output.push_str(stmt);
                output.push('\n');
            }
        }
    }

    // Hoisted snippet functions (before export)
    for snippet_fn in &hoisted_snippets {
        output.push('\n');
        output.push_str(snippet_fn);
    }

    output.push('\n');
    let props_param = if has_props { ", $$props" } else { "" };
    output.push_str(&format!(
        "export default function {component_name}($$renderer{props_param}) {{\n"
    ));

    // Indentation: if wrapped in component(), use 2 tabs for body, else 1 tab
    let indent = if needs_component_wrapper { "\t\t" } else { "\t" };

    if needs_component_wrapper {
        output.push_str("\t$$renderer.component(($$renderer) => {\n");
    }

    if !script_body.is_empty() {
        for line in script_body.lines() {
            if line.is_empty() {
                output.push('\n');
            } else {
                output.push_str(indent);
                output.push_str(line);
                output.push('\n');
            }
        }
    }

    if has_component_bindings {
        // $$settled pattern: wrap template code in $$render_inner + do/while loop
        output.push_str(&format!("{indent}let $$settled = true;\n"));
        output.push_str(&format!("{indent}let $$inner_renderer;\n"));
        output.push('\n');
        output.push_str(&format!("{indent}function $$render_inner($$renderer) {{\n"));

        if !template_code.is_empty() {
            let inner_indent = format!("{indent}\t");
            for line in template_code.lines() {
                if line.is_empty() {
                    output.push('\n');
                } else {
                    output.push_str(&inner_indent);
                    output.push_str(line);
                    output.push('\n');
                }
            }
        }

        output.push_str(&format!("{indent}}}\n"));
        output.push('\n');
        output.push_str(&format!("{indent}do {{\n"));
        output.push_str(&format!("{indent}\t$$settled = true;\n"));
        output.push_str(&format!("{indent}\t$$inner_renderer = $$renderer.copy();\n"));
        output.push_str(&format!("{indent}\t$$render_inner($$inner_renderer);\n"));
        output.push_str(&format!("{indent}}} while (!$$settled);\n"));
        output.push('\n');
        output.push_str(&format!("{indent}$$renderer.subsume($$inner_renderer);\n"));
    } else {
        // Normal path: emit template code directly
        if !script_body.is_empty() && !template_code.is_empty() {
            output.push('\n');
        }

        if !template_code.is_empty() {
            for line in template_code.lines() {
                if line.is_empty() {
                    output.push('\n');
                } else {
                    output.push_str(indent);
                    output.push_str(line);
                    output.push('\n');
                }
            }
        }
    }

    if needs_component_wrapper {
        output.push_str("\t});\n");
    }

    output.push_str("}\n");
    Some(output)
}


pub(super) fn compile_instance_script_server(
    source: &str,
    script: &Script,
    _runes_mode: bool,
) -> Option<InstanceScriptResult> {
    let program = script.oxc_program();
    // OXC spans are 0-based within the script content snippet
    let snippet = &source[script.content_start..script.content_end];
    let props_binding = detect_props_binding(script);

    // Detect if any statement has top-level await
    let has_top_level_await = script_has_top_level_await(program, snippet);

    if has_top_level_await {
        return compile_instance_script_server_async_run(snippet, program, &props_binding);
    }

    let mut statements = Vec::new();
    let mut prev_end: usize = 0;

    for statement in &program.body {
        // Extract leading comments from the gap between previous statement and this one
        let stmt_start = statement.span().start as usize;
        let leading_comments = extract_leading_comments(snippet, prev_end, stmt_start);
        prev_end = statement.span().end as usize;

        match statement {
            OxcStatement::ImportDeclaration(_) => continue,
            OxcStatement::ExportNamedDeclaration(export) => {
                if let Some(decl) = export.declaration.as_ref() {
                    let rendered = render_instance_declaration_server(snippet, decl)?;
                    statements.push(prepend_comments(&leading_comments, &rendered));
                }
            }
            _ => {
                // Check for $props() declaration
                if let Some(ref props_name) = props_binding
                    && let Some(rendered) = render_props_declaration_server(statement, props_name, snippet) {
                        statements.push(prepend_comments(&leading_comments, &rendered));
                        continue;
                    }
                if let Some(rendered) =
                    render_state_declaration_server_formatted(snippet, statement)
                {
                    statements.push(prepend_comments(&leading_comments, &rendered));
                    continue;
                }
                // Check for $derived() declarations (server)
                if let Some(rendered) = render_derived_declaration(snippet, statement) {
                    statements.push(prepend_comments(&leading_comments, &rendered));
                    continue;
                }
                // Check for class declarations with rune fields (server)
                if let Some(rendered) =
                    render_class_with_rune_fields(snippet, statement, GenerateTarget::Server)
                {
                    statements.push(prepend_comments(&leading_comments, &rendered));
                    continue;
                }
                // Use OXC codegen for function declarations (normalizes
                // indentation and blank lines), source extraction for rest
                let stmt_end = statement.span().end as usize;
                let rendered = if matches!(statement, OxcStatement::FunctionDeclaration(_)) {
                    render_statement_via_codegen(snippet, statement)
                } else {
                    reindent_block(snippet[stmt_start..stmt_end].trim())
                };
                statements.push(prepend_comments(&leading_comments, &rendered));
            }
        }
    }

    Some(InstanceScriptResult {
        body: join_statements_with_blank_lines(&statements),
        async_run_info: None,
    })
}


pub(super) fn compile_instance_script_server_async_run(
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

                        // Check for $state() — strip to init value (not part of run, just inline)
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
                                    // Sync derived still goes in run array but doesn't make the var "async"
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

                        // Regular declaration — not part of run
                        non_run_statements.push(format!("let {binding_name} = {init_text};"));
                    } else {
                        non_run_statements.push(format!("let {binding_name};"));
                    }
                }
            }
            _ => {
                // Check for $inspect, $props, etc.
                if let Some(props_name) = props_binding
                    && let Some(rendered) = render_props_declaration_server(statement, props_name, snippet) {
                        non_run_statements.push(rendered);
                        continue;
                    }
                let rendered = render_statement_via_codegen(snippet, statement);
                let trimmed = rendered.trim();
                // $inspect() calls are dropped on server — add empty run slot
                if trimmed.starts_with("$inspect(") || trimmed.starts_with("$inspect.") {
                    run_closures.push(String::new()); // empty slot
                    continue;
                }
                // Function declarations go before run (same handling as other statements)
                non_run_statements.push(rendered);
            }
        }
    }

    let run_slot_count = run_closures.len();

    // Build output
    let mut output = String::new();

    // Non-run statements first (functions, regular vars) — separated by blank lines
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

    // $$renderer.run() call — multi-line if any closure is multi-line
    let any_multiline = run_closures.iter().any(|c| c.contains('\n'));
    let has_empty_slots = run_closures.iter().any(|c| c.is_empty());

    if any_multiline {
        if !hoisted_vars.is_empty() {
            output.push('\n'); // blank line between var declarations and run array
        }
        output.push_str("var $$promises = $$renderer.run([\n");
        for (i, closure) in run_closures.iter().enumerate() {
            // Indent each line of the closure by one tab
            for (j, line) in closure.lines().enumerate() {
                output.push('\t');
                output.push_str(line);
                output.push('\n');
                // Don't add extra newlines after last line
                let _ = j;
            }
            if closure.is_empty() {
                // empty slot
            }
            // Separator: comma, then blank line between multi-line closures
            let is_last = i == run_closures.len() - 1;
            if !is_last || has_empty_slots {
                // Trim trailing newline to append comma
                if output.ends_with('\n') {
                    output.pop();
                }
                output.push_str(",\n");
                // Add blank line between multi-line closures
                let next_is_multiline = run_closures.get(i + 1).is_some_and(|c| c.contains('\n'));
                if closure.contains('\n') && next_is_multiline {
                    output.push('\n');
                }
            }
        }
        output.push_str("]);");
    } else {
        output.push_str("var $$promises = $$renderer.run([");
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


pub(super) fn render_instance_declaration_server(
    snippet: &str,
    decl: &Declaration<'_>,
) -> Option<String> {
    Some(render_declaration_from_snippet(snippet, decl))
}


/// Render $state() declarations on server: always strip to plain value with proper formatting.
pub(super) fn render_state_declaration_server_formatted(
    snippet: &str,
    statement: &OxcStatement<'_>,
) -> Option<String> {
    let declaration = match statement {
        OxcStatement::VariableDeclaration(d) => &**d,
        _ => return None,
    };

    let mut has_state_call = false;
    let mut parts = Vec::new();

    for declarator in &declaration.declarations {
        let Some(init) = declarator.init.as_ref() else { continue };
        let Some(argument) = oxc_state_call_argument(init) else { continue };
        has_state_call = true;

        let binding_name = declarator.id.get_binding_identifier()
            .map(|id| id.name.to_string())?;
        let value = argument.render(snippet).ok()?;
        parts.push(format!("let {binding_name} = {value};"));
    }

    if !has_state_call {
        return None;
    }

    Some(parts.join("\n"))
}


// ---------------------------------------------------------------------------
// Server template compilation
// ---------------------------------------------------------------------------

impl ServerContext<'_> {

fn compile_fragment(&self,
    fragment: &Fragment,
) -> Option<String> {
    if fragment.nodes.is_empty() {
        return Some(String::new());
    }

    // ServerTemplateParts collects static text (to be escaped) and raw interpolation
    let mut parts = ServerTemplateParts::new();

    // For mixed static/dynamic content, collect output lines
    let mut output_lines: Vec<String> = Vec::new();

    // Skip leading/trailing whitespace text nodes and snippet blocks
    let is_sig_server = |n: &Node| !is_whitespace_text(n) && !matches!(n, Node::SnippetBlock(_));
    let first_sig = fragment.nodes.iter().position(&is_sig_server);
    let last_sig = fragment.nodes.iter().rposition(&is_sig_server);
    let (first_sig, last_sig) = match (first_sig, last_sig) {
        (Some(f), Some(l)) => (f, l),
        _ => return Some(String::new()),
    };

    let mut server_each_counter = 0usize;
    let mut last_was_comment = false;
    let mut last_was_component = false;
    for (i, node) in fragment.nodes.iter().enumerate() {
        if i < first_sig || i > last_sig {
            continue;
        }
        match node {
            Node::Text(text) => {
                if text.data.trim().is_empty() && i > first_sig && i < last_sig {
                    // Skip whitespace after a stripped comment (it was already accounted for)
                    if !last_was_comment {
                        parts.push_static(" ");
                    }
                } else if last_was_component {
                    // Text after a component: collapse leading whitespace to space
                    let collapsed = collapse_template_whitespace(&text.data);
                    parts.push_static(&collapsed);
                } else {
                    // Trim leading whitespace from first significant text, trailing from last
                    let mut data = text.data.as_ref();
                    if i == first_sig {
                        data = data.trim_start();
                    }
                    if i == last_sig {
                        data = data.trim_end();
                    }
                    if !data.is_empty() {
                        let collapsed = collapse_template_whitespace(data);
                        parts.push_static(&collapsed);
                    }
                }
                last_was_component = false;
                last_was_comment = false;
            }
            Node::RegularElement(element) => {
                if &*element.name == "select" && has_option_children(element) {
                    // Special handling for <select>
                    self.serialize_select_element(element, &mut parts, &mut output_lines, &mut server_each_counter)?;
                    // Add closing tag to parts buffer for consolidation
                    // Add <!> anchor if select needs customizable_select pattern
                    if select_needs_fragment_anchor(&element.fragment.nodes) {
                        parts.push_static("<!>");
                    }
                    parts.push_static("</select>");
                } else {
                    self.serialize_element(element, &mut parts)?;
                }
                last_was_comment = false;
                last_was_component = false;
            }
            Node::ExpressionTag(tag) => {
                // Try constant propagation first (known bindings)
                if let Some(value) = try_resolve_constant_binding(&tag.expression, self.constant_bindings) {
                    parts.push_static(&value);
                }
                // Then try pure expression constant folding
                else if let Some(folded) = try_fold_expression_to_string(&tag.expression) {
                    parts.push_static(&folded);
                } else if let Some(expr_text) = tag.expression.render() {
                    parts.push_interpolation(&format!("$.escape({expr_text})"));
                }
                last_was_comment = false;
                last_was_component = false;
            }
            Node::Comment(_comment) => {
                // HTML comments are stripped from server output
                last_was_comment = true;
            }
            Node::Component(comp) => {
                // Flush accumulated parts
                let flushed = parts.to_template_literal();
                if !flushed.is_empty() {
                    output_lines.push(format!("$$renderer.push(`{flushed}`);\n"));
                    parts = ServerTemplateParts::new();
                }
                // Emit component call
                let comp_code = self.compile_component(comp)?;
                let is_multiline_call = comp_code.trim_end().contains('\n');
                output_lines.push(comp_code);
                // Add component boundary marker when component has siblings
                let sig_count = fragment.nodes.iter()
                    .filter(|n| is_sig_server(n))
                    .count();
                if sig_count > 1 {
                    // Add blank line after multi-line component calls
                    if is_multiline_call {
                        output_lines.push("\n".to_string());
                    }
                    parts.push_static("<!---->");
                }
                last_was_comment = false;
                last_was_component = true;
            }
            Node::EachBlock(each) => {
                let flushed = parts.to_template_literal();
                if !flushed.is_empty() {
                    output_lines.push(format!("$$renderer.push(`{flushed}`);\n"));
                    parts = ServerTemplateParts::new();
                }
                let each_code = self.compile_each_block(each)?;
                output_lines.push(each_code);
            }
            Node::IfBlock(if_block) => {
                let flushed = parts.to_template_literal();
                if !flushed.is_empty() {
                    output_lines.push(format!("$$renderer.push(`{flushed}`);\n"));
                    parts = ServerTemplateParts::new();
                }
                let if_code = self.compile_if_block(if_block)?;
                output_lines.push(if_code);
            }
            Node::AwaitBlock(await_block) => {
                let flushed = parts.to_template_literal();
                if !flushed.is_empty() {
                    output_lines.push(format!("$$renderer.push(`{flushed}`);\n"));
                    parts = ServerTemplateParts::new();
                }
                let await_code = self.compile_await_block(await_block)?;
                output_lines.push(await_code);
                // Await block closing marker
                parts.push_static("<!--]-->");
                last_was_comment = false;
                last_was_component = false;
            }
            Node::HtmlTag(tag) => {
                if let Some(expr) = tag.expression.render() {
                    parts.push_interpolation(&format!("$.html({expr})"));
                }
            }
            Node::SvelteElement(el) => {
                let flushed = parts.to_template_literal();
                if !flushed.is_empty() {
                    output_lines.push(format!("$$renderer.push(`{flushed}`);\n"));
                    parts = ServerTemplateParts::new();
                }
                if let Some(ref expr) = el.expression
                    && let Some(tag_expr) = expr.render() {
                        output_lines.push(format!("$.element($$renderer, {tag_expr});\n"));
                    }
            }
            Node::ConstTag(const_tag) => {
                // {@const decl} — emit as a const declaration
                let flushed = parts.to_template_literal();
                if !flushed.is_empty() {
                    output_lines.push(format!("$$renderer.push(`{flushed}`);\n"));
                    parts = ServerTemplateParts::new();
                }
                if let Some(decl_text) = const_tag.declaration.render() {
                    output_lines.push(format!("{decl_text};\n"));
                }
                last_was_comment = false;
                last_was_component = false;
            }
            Node::SnippetBlock(_) => {
                // Snippet blocks are function definitions — handled at compile_server level
            }
            _ => return None,
        }
    }

    let template = parts.to_template_literal();
    if template.is_empty() && output_lines.is_empty() {
        return Some(String::new());
    }

    if !template.is_empty() {
        output_lines.push(format!("$$renderer.push(`{template}`);\n"));
    }

    let joined = output_lines.join("");
    Some(normalize_server_select_blank_lines(&joined))
}


/// Compile a server fragment when the script has async run pattern.
/// Template expressions that reference async-derived vars get wrapped in $$renderer.async().
fn compile_fragment_with_script_run(&self,
    fragment: &Fragment,
    run_info: &ServerAsyncRunInfo,
) -> Option<String> {
    let mut output_lines: Vec<String> = Vec::new();
    let mut parts = ServerTemplateParts::new();

    let is_sig_server = |n: &Node| !is_whitespace_text(n) && !matches!(n, Node::SnippetBlock(_) | Node::Comment(_));
    let first_sig = fragment.nodes.iter().position(&is_sig_server);
    let last_sig = fragment.nodes.iter().rposition(&is_sig_server);
    let (first_sig, last_sig) = match (first_sig, last_sig) {
        (Some(f), Some(l)) => (f, l),
        _ => return Some(String::new()),
    };

    // Last promise index for async wrapping
    let last_promise_idx = run_info.run_slot_count.saturating_sub(1);

    for (i, node) in fragment.nodes.iter().enumerate() {
        if i < first_sig || i > last_sig {
            continue;
        }

        match node {
            Node::Text(text) => {
                // Skip whitespace-only text that precedes or follows comments (collapse to single space)
                if text.data.trim().is_empty() {
                    let next_is_comment = matches!(fragment.nodes.get(i + 1), Some(Node::Comment(_)));
                    if next_is_comment {
                        continue; // skip text before comment
                    }
                    let prev_is_comment = i > 0 && matches!(fragment.nodes.get(i - 1), Some(Node::Comment(_)));
                    if prev_is_comment {
                        // After a comment, emit single space
                        parts.push_static(" ");
                        continue;
                    }
                    if i > first_sig && i < last_sig {
                        parts.push_static(" ");
                    }
                } else {
                    let mut data = text.data.as_ref();
                    if i == first_sig {
                        data = data.trim_start();
                    }
                    if i == last_sig {
                        data = data.trim_end();
                    }
                    if !data.is_empty() {
                        let collapsed = collapse_template_whitespace(data);
                        parts.push_static(&collapsed);
                    }
                }
            }
            Node::ExpressionTag(tag) => {
                if let Some(expr_text) = tag.expression.render() {
                    // Check if expr references any async var
                    let refs_async = run_info.async_vars.iter().any(|v| expr_text.contains(v.as_str()));
                    if refs_async {
                        // Flush before async
                        let flushed = parts.to_template_literal();
                        if !flushed.is_empty() {
                            output_lines.push(format!("$$renderer.push(`{flushed}`);\n"));
                            parts = ServerTemplateParts::new();
                        }
                        output_lines.push(format!(
                            "$$renderer.async([$$promises[{last_promise_idx}]], ($$renderer) => $$renderer.push(() => $.escape({expr_text})));\n"
                        ));
                    } else {
                        parts.push_interpolation(&format!("$.escape({expr_text})"));
                    }
                }
            }
            Node::RegularElement(element) => {
                // Check if element's content references async vars
                let refs_async = element_content_refs_async(element, run_info);
                if refs_async {
                    // Split element: static open/close tags, async for dynamic children
                    let tag_name = &*element.name;
                    // Open tag with attributes
                    parts.push_static(&format!("<{tag_name}>"));
                    let flushed = parts.to_template_literal();
                    if !flushed.is_empty() {
                        output_lines.push(format!("$$renderer.push(`{flushed}`);\n"));
                        parts = ServerTemplateParts::new();
                    }
                    // Process children
                    for child in &element.fragment.nodes {
                        match child {
                            Node::ExpressionTag(tag) => {
                                if let Some(expr_text) = tag.expression.render() {
                                    output_lines.push(format!(
                                        "$$renderer.async([$$promises[{last_promise_idx}]], ($$renderer) => $$renderer.push(() => $.escape({expr_text})));\n"
                                    ));
                                }
                            }
                            Node::Text(text) => {
                                let data = text.data.trim();
                                if !data.is_empty() {
                                    parts.push_static(data);
                                }
                            }
                            _ => {}
                        }
                    }
                    let flushed = parts.to_template_literal();
                    if !flushed.is_empty() {
                        output_lines.push(format!("$$renderer.push(`{flushed}`);\n"));
                        parts = ServerTemplateParts::new();
                    }
                    parts.push_static(&format!("</{tag_name}>"));
                } else {
                    self.serialize_element(element, &mut parts)?;
                }
            }
            Node::IfBlock(if_block) => {
                let flushed = parts.to_template_literal();
                if !flushed.is_empty() {
                    output_lines.push(format!("$$renderer.push(`{flushed}`);\n"));
                    parts = ServerTemplateParts::new();
                }
                // Check if the if-chain references any reactive/async vars
                let refs_reactive = if_chain_refs_reactive(if_block, run_info);
                if refs_reactive {
                    let if_code = self.compile_if_block_with_async_block(if_block, run_info)?;
                    output_lines.push(if_code);
                } else {
                    let mut if_code = self.compile_if_block(if_block)?;
                    // Strip the trailing $$renderer.push(`<!--]-->`); so we can merge it with next text
                    if let Some(stripped) = if_code.strip_suffix("$$renderer.push(`<!--]-->`);\n") {
                        if_code = stripped.to_string();
                    }
                    output_lines.push(if_code);
                }
                // Merge <!--]--> with subsequent text in the template literal
                parts.push_static("<!--]-->");
            }
            Node::EachBlock(each) => {
                let flushed = parts.to_template_literal();
                if !flushed.is_empty() {
                    output_lines.push(format!("$$renderer.push(`{flushed}`);\n"));
                    parts = ServerTemplateParts::new();
                }
                let each_code = self.compile_each_block(each)?;
                output_lines.push(each_code);
            }
            Node::Component(comp) => {
                let flushed = parts.to_template_literal();
                if !flushed.is_empty() {
                    output_lines.push(format!("$$renderer.push(`{flushed}`);\n"));
                    parts = ServerTemplateParts::new();
                }
                let comp_code = self.compile_component(comp)?;
                output_lines.push(comp_code);
            }
            Node::ConstTag(const_tag) => {
                let flushed = parts.to_template_literal();
                if !flushed.is_empty() {
                    output_lines.push(format!("$$renderer.push(`{flushed}`);\n"));
                    parts = ServerTemplateParts::new();
                }
                if let Some(decl_text) = const_tag.declaration.render() {
                    output_lines.push(format!("{decl_text};\n"));
                }
            }
            Node::HtmlTag(tag) => {
                let flushed = parts.to_template_literal();
                if !flushed.is_empty() {
                    output_lines.push(format!("$$renderer.push(`{flushed}`);\n"));
                    parts = ServerTemplateParts::new();
                }
                if let Some(expr) = tag.expression.render() {
                    output_lines.push(format!("$$renderer.push({expr});\n"));
                }
            }
            Node::SnippetBlock(_) => {}
            Node::Comment(_) => {}
            _ => {
                // Fall back to non-async fragment for unknown nodes
                return self.compile_fragment(fragment);
            }
        }
    }

    let template = parts.to_template_literal();
    if !template.is_empty() {
        output_lines.push(format!("$$renderer.push(`{template}`);\n"));
    }

    // Join output lines: add blank lines between multi-line blocks, but not between simple statements
    let mut result = String::new();
    for (idx, line) in output_lines.iter().enumerate() {
        if idx > 0 {
            let prev = &output_lines[idx - 1];
            let prev_is_block = prev.contains('\n') && (prev.contains("async_block") || prev.contains("child_block") || prev.contains("if ("));
            let cur_is_block = line.contains('\n') && (line.contains("async_block") || line.contains("child_block") || line.contains("if ("));
            if (prev_is_block || cur_is_block) && !result.ends_with("\n\n") {
                result.push('\n');
            }
        }
        result.push_str(line);
    }
    Some(result)
}

} // end impl ServerContext (part 1)

/// Check if a RegularElement's children reference any async vars
pub(super) fn element_content_refs_async(element: &RegularElement, run_info: &ServerAsyncRunInfo) -> bool {
    for node in &element.fragment.nodes {
        if let Node::ExpressionTag(tag) = node
            && let Some(expr_text) = tag.expression.render() {
                for var in &run_info.async_vars {
                    if expr_text.contains(var.as_str()) {
                        return true;
                    }
                }
            }
    }
    false
}

/// Collects parts of a server template literal, separating static text
/// (which needs escaping) from interpolation expressions (which must not be escaped).
pub(super) struct ServerTemplateParts {
    parts: Vec<ServerTemplatePart>,
}

pub(super) enum ServerTemplatePart {
    Static(String),
    Interpolation(String),
}

impl ServerTemplateParts {
    fn new() -> Self {
        Self { parts: Vec::new() }
    }

    fn push_static(&mut self, text: &str) {
        if let Some(ServerTemplatePart::Static(s)) = self.parts.last_mut() {
            s.push_str(text);
        } else {
            self.parts.push(ServerTemplatePart::Static(text.to_string()));
        }
    }

    fn push_interpolation(&mut self, expr: &str) {
        self.parts.push(ServerTemplatePart::Interpolation(expr.to_string()));
    }

    fn to_template_literal(&self) -> String {
        let mut result = String::new();
        for part in &self.parts {
            match part {
                ServerTemplatePart::Static(s) => {
                    result.push_str(&escape_js_template_literal(s));
                }
                ServerTemplatePart::Interpolation(expr) => {
                    result.push_str(&format!("${{{expr}}}"));
                }
            }
        }
        result
    }
}

impl ServerContext<'_> {

/// Compile a component call for server: `ComponentName($$renderer, { ...props })`
fn compile_component(&self, comp: &ComponentNode) -> Option<String> {
    let name = &comp.name;
    let mut props = Vec::new();

    for attr in comp.attributes.iter() {
        match attr {
            Attribute::Attribute(a) => {
                // Skip event handlers on server
                if a.name.starts_with("on") {
                    // Check for shorthand: {onmouseup} means name === value
                    if let AttributeValueKind::ExpressionTag(tag) = &a.value
                        && let Some(rendered) = tag.expression.render()
                        && rendered == a.name.as_ref() {
                                // Shorthand property
                                props.push(a.name.to_string());
                                continue;
                            }
                    let value = render_attribute_value_js(&a.value, self.source);
                    props.push(format!("{}: {value}", a.name));
                } else {
                    let value = render_attribute_value_js(&a.value, self.source);
                    props.push(format!("{}: {value}", a.name));
                }
            }
            Attribute::BindDirective(bind) => {
                if bind.name.as_ref() != "this"
                    && let Some(expr) = bind.expression.render() {
                        props.push(format!(
                            "get {}() {{\n\t\treturn {expr};\n\t}}", bind.name
                        ));
                        props.push(format!(
                            "set {}($$value) {{\n\t\t{expr} = $$value;\n\t\t$$settled = false;\n\t}}", bind.name
                        ));
                    }
            }
            _ => {}
        }
    }

    // Check for child content (default slot)
    let has_children = comp.fragment.nodes.iter().any(|n| !is_whitespace_text(n));
    if has_children {
        // Build children function body — server slot children get `<!---->` marker prefix
        let children_body = self.compile_fragment(&comp.fragment)?;
        if !children_body.is_empty() {
            // Insert `<!---->` marker at the start of the template content
            let trimmed = children_body.trim();
            // If it starts with $$renderer.push(`...`), inject <!---> at the start
            // and strip leading whitespace from the template content
            let body_with_marker = if let Some(rest) = trimmed.strip_prefix("$$renderer.push(`") {
                let rest_trimmed = rest.trim_start_matches(['\n', '\r', '\t']);
                format!("$$renderer.push(`<!---->{rest_trimmed}")
            } else {
                trimmed.to_string()
            };
            props.push(format!("children: ($$renderer) => {{\n\t\t{body_with_marker}\n\t}}"));
        }
        props.push("$$slots: { default: true }".to_string());
    }

    // Format props — multi-line when any prop is complex (contains newline)
    let has_complex_props = props.iter().any(|p| p.contains('\n'));
    let props_str = if props.is_empty() {
        "{}".to_string()
    } else if has_complex_props || props.len() > 3 {
        // Multi-line format
        let mut parts = String::from("{\n");
        for (i, prop) in props.iter().enumerate() {
            // Add blank line before setter that follows a getter
            if prop.starts_with("set ") && i > 0 && props[i - 1].starts_with("get ") {
                parts.push('\n');
            }
            parts.push_str(&format!("\t{prop}"));
            if i < props.len() - 1 {
                parts.push(',');
            }
            parts.push('\n');
        }
        parts.push('}');
        parts
    } else {
        format!("{{ {} }}", props.join(", "))
    };

    Some(format!("{name}($$renderer, {props_str});\n"))
}

/// Compile an each block for server
fn compile_each_block(&self, each: &EachBlock) -> Option<String> {
    let raw_expr = render_expression_from_source(&each.expression)
        .or_else(|| each.expression.render())?;

    // Extract collection expression and index name from SequenceExpression
    let (expr, inferred_index) = if !each.has_as_clause {
        if let Some(oxc_expr) = each.expression.oxc_expression() {
            if let OxcExpression::SequenceExpression(seq) = oxc_expr {
                if seq.expressions.len() == 2 {
                    if let OxcExpression::Identifier(id) = &seq.expressions[1] {
                        let idx_name = id.name.to_string();
                        let collection = if let Some((coll, _)) = raw_expr.rsplit_once(',') {
                            coll.trim().to_string()
                        } else {
                            raw_expr.clone()
                        };
                        (collection, Some(idx_name))
                    } else {
                        (raw_expr, None)
                    }
                } else {
                    (raw_expr, None)
                }
            } else {
                (raw_expr, None)
            }
        } else if let Some(idx) = each.index.as_deref() {
            if let Some((coll, idx_part)) = raw_expr.rsplit_once(',') {
                if idx_part.trim() == idx {
                    (coll.trim().to_string(), Some(idx.to_string()))
                } else {
                    (raw_expr, None)
                }
            } else {
                (raw_expr, None)
            }
        } else {
            (raw_expr, None)
        }
    } else {
        (raw_expr, None)
    };

    let context_name = each.context.as_ref()
        .and_then(|c| c.render())
        .unwrap_or_else(|| "$$item".to_string());
    let index_name: Option<String> = each.index.as_deref()
        .map(|s| s.to_string())
        .or(inferred_index);

    // Detect async: expression or body contains await
    let expr_has_await = expr.contains("await ");
    let body_has_await = fragment_has_await(&each.body);
    let has_fallback = each.fallback.is_some();
    let fallback_has_await = each.fallback.as_ref().is_some_and(fragment_has_await);
    let is_async = expr_has_await || body_has_await || fallback_has_await;

    let mut output = String::new();

    if is_async {
        // Async each: the <!--[--> marker goes OUTSIDE the child_block for no-fallback case
        if !has_fallback {
            output.push_str("$$renderer.push(`<!--[-->`);\n\n");
        }
        output.push_str("$$renderer.child_block(async ($$renderer) => {\n");

        let ensure_expr = if expr_has_await {
            // Strip leading "await " and wrap in (await $.save(...))()
            let inner = expr.strip_prefix("await ").unwrap_or(&expr);
            format!("(await $.save({inner}))()")
        } else {
            expr.clone()
        };
        output.push_str(&format!("\tconst each_array = $.ensure_array_like({ensure_expr});\n\n"));

        let idx_var = index_name.as_deref().unwrap_or("$$index");

        if has_fallback {
            // With fallback: if (length !== 0) { ... } else { ... }
            output.push_str("\tif (each_array.length !== 0) {\n");
            output.push_str("\t\t$$renderer.push('<!--[-->');\n\n");
            output.push_str(&format!(
                "\t\tfor (let {idx_var} = 0, $$length = each_array.length; {idx_var} < $$length; {idx_var}++) {{\n"
            ));
            if each.has_as_clause && context_name != "$$item" {
                output.push_str(&format!("\t\t\tlet {context_name} = each_array[{idx_var}];\n\n"));
            }
            // Body
            let body_code = self.compile_each_body_async(&each.body, body_has_await)?;
            for line in body_code.lines() {
                if line.is_empty() {
                    output.push('\n');
                } else {
                    output.push_str("\t\t\t");
                    output.push_str(line);
                    output.push('\n');
                }
            }
            output.push_str("\t\t}\n");
            output.push_str("\t} else {\n");
            output.push_str("\t\t$$renderer.push('<!--[!-->');\n");
            // Fallback body
            if let Some(ref fallback) = each.fallback {
                let fallback_code = self.compile_each_body_async(fallback, fallback_has_await)?;
                for line in fallback_code.lines() {
                    if line.is_empty() {
                        output.push('\n');
                    } else {
                        output.push_str("\t\t");
                        output.push_str(line);
                        output.push('\n');
                    }
                }
            }
            output.push_str("\t}\n");
        } else {
            // No fallback
            output.push_str(&format!(
                "\tfor (let {idx_var} = 0, $$length = each_array.length; {idx_var} < $$length; {idx_var}++) {{\n"
            ));
            if each.has_as_clause && context_name != "$$item" {
                output.push_str(&format!("\t\tlet {context_name} = each_array[{idx_var}];\n\n"));
            }
            let body_code = self.compile_each_body_async(&each.body, body_has_await)?;
            for line in body_code.lines() {
                if line.is_empty() {
                    output.push('\n');
                } else {
                    output.push_str("\t\t");
                    output.push_str(line);
                    output.push('\n');
                }
            }
            output.push_str("\t}\n");
        }

        output.push_str("});\n\n");
        output.push_str("$$renderer.push(`<!--]-->`);\n");
    } else {
        // Non-async path (existing logic)
        output.push_str("$$renderer.push(`<!--[-->`);\n\n");
        output.push_str(&format!("const each_array = $.ensure_array_like({expr});\n\n"));

        let idx_var = index_name.as_deref().unwrap_or("$$index");

        output.push_str(&format!(
            "for (let {idx_var} = 0, $$length = each_array.length; {idx_var} < $$length; {idx_var}++) {{\n"
        ));

        if each.has_as_clause && context_name != "$$item" {
            output.push_str(&format!("\tlet {context_name} = each_array[{idx_var}];\n\n"));
        }

        // Compile each body for server — inline with comment markers for expression tags
        let mut body_parts = ServerTemplateParts::new();
        for child in &each.body.nodes {
            if is_whitespace_text(child) {
                continue;
            }
            match child {
                Node::Text(text) => {
                    body_parts.push_static(&text.data);
                }
                Node::ExpressionTag(tag) => {
                    if let Some(folded) = try_fold_expression_to_string(&tag.expression) {
                        body_parts.push_static(&folded);
                    } else if let Some(expr_text) = tag.expression.render() {
                        body_parts.push_static("<!---->");
                        body_parts.push_interpolation(&format!("$.escape({expr_text})"));
                    }
                }
                Node::RegularElement(element) => {
                    self.serialize_element(element, &mut body_parts)?;
                }
                _ => {
                    // For other node types, fall back to server fragment
                    let body_server = self.compile_fragment(&each.body)?;
                    if !body_server.is_empty() {
                        for line in body_server.lines() {
                            if line.is_empty() {
                                output.push('\n');
                            } else {
                                output.push('\t');
                                output.push_str(line);
                                output.push('\n');
                            }
                        }
                    }
                    output.push_str("}\n\n");
                    output.push_str("$$renderer.push(`<!--]-->`);\n");
                    return Some(output);
                }
            }
        }
        let body_template = body_parts.to_template_literal();
        if !body_template.is_empty() {
            output.push_str(&format!("\t$$renderer.push(`{body_template}`);\n"));
        }

        output.push_str("}\n\n");
        output.push_str("$$renderer.push(`<!--]-->`);\n");
    }

    Some(output)
}

/// Compile a server each block body with async expression support
fn compile_each_body_async(&self, fragment: &Fragment, is_async: bool) -> Option<String> {
    let mut parts = ServerTemplateParts::new();
    let mut output_lines: Vec<String> = Vec::new();

    for child in &fragment.nodes {
        if is_whitespace_text(child) {
            continue;
        }
        match child {
            Node::Text(text) => {
                parts.push_static(&text.data);
            }
            Node::ExpressionTag(tag) => {
                if let Some(expr_text) = tag.expression.render() {
                    if is_async && expr_text.contains("await ") {
                        // Add comment marker, flush parts, emit async push
                        parts.push_static("<!---->");
                        let flushed = parts.to_template_literal();
                        if !flushed.is_empty() {
                            output_lines.push(format!("$$renderer.push(`{flushed}`);\n"));
                            parts = ServerTemplateParts::new();
                        }
                        output_lines.push(format!("$$renderer.push(async () => $.escape({expr_text}));\n"));
                    } else if let Some(folded) = try_fold_expression_to_string(&tag.expression) {
                        parts.push_static(&folded);
                    } else {
                        parts.push_static("<!---->");
                        parts.push_interpolation(&format!("$.escape({expr_text})"));
                    }
                }
            }
            Node::ConstTag(const_tag) => {
                let flushed = parts.to_template_literal();
                if !flushed.is_empty() {
                    output_lines.push(format!("$$renderer.push(`{flushed}`);\n"));
                    parts = ServerTemplateParts::new();
                }
                if let Some(decl_text) = const_tag.declaration.render() {
                    output_lines.push(format!("{decl_text};\n"));
                }
            }
            Node::RegularElement(element) => {
                self.serialize_element(element, &mut parts)?;
            }
            _ => {
                // Fall back to full fragment compilation
                let flushed = parts.to_template_literal();
                if !flushed.is_empty() {
                    output_lines.push(format!("$$renderer.push(`{flushed}`);\n"));
                }
                let frag_code = self.compile_fragment(fragment)?;
                output_lines.push(frag_code);
                return Some(output_lines.join(""));
            }
        }
    }

    let flushed = parts.to_template_literal();
    if !flushed.is_empty() {
        output_lines.push(format!("$$renderer.push(`{flushed}`);\n"));
    }

    Some(output_lines.join(""))
}

/// Compile an await block for server
fn compile_await_block(&self, await_block: &crate::ast::modern::AwaitBlock) -> Option<String> {
    let expr = await_block.expression.render()?;
    let mut output = String::new();

    // pending callback
    let pending_fn = if let Some(ref pending_frag) = await_block.pending {
        let has_content = pending_frag.nodes.iter().any(|n| !is_whitespace_text(n));
        if has_content {
            let body = self.compile_fragment(pending_frag)?;
            let indented: String = body.lines()
                .map(|l| if l.is_empty() { String::new() } else { format!("\t{l}") })
                .collect::<Vec<_>>()
                .join("\n");
            format!("() => {{\n{indented}\n}}")
        } else {
            "() => {}".to_string()
        }
    } else {
        "() => {}".to_string()
    };

    // then callback
    let then_fn = if let Some(ref then_frag) = await_block.then {
        let val_name = await_block.value.as_ref()
            .and_then(|v| v.render())
            .unwrap_or_default();
        let has_content = then_frag.nodes.iter().any(|n| !is_whitespace_text(n));
        if has_content {
            let body = self.compile_fragment(then_frag)?;
            let indented: String = body.lines()
                .map(|l| if l.is_empty() { String::new() } else { format!("\t{l}") })
                .collect::<Vec<_>>()
                .join("\n");
            if val_name.is_empty() {
                format!("() => {{\n{indented}\n}}")
            } else {
                format!("({val_name}) => {{\n{indented}\n}}")
            }
        } else {
            if val_name.is_empty() {
                "() => {}".to_string()
            } else {
                format!("({val_name}) => {{}}")
            }
        }
    } else {
        "() => {}".to_string()
    };

    output.push_str(&format!("$.await($$renderer, {expr}(), {pending_fn}, {then_fn});\n"));
    Some(output)
}

} // end impl ServerContext

/// Check if any test expression or body expression in an if chain contains `await`
pub(super) fn has_await_in_if_chain(if_block: &IfBlock) -> bool {
    let mut current = if_block;
    loop {
        // Check test expression
        if let Some(test) = current.test.render()
            && test.contains("await ") {
                return true;
            }
        // Check body expressions for await
        if fragment_has_await(&current.consequent) {
            return true;
        }
        // Check alternate
        match &current.alternate {
            Some(alt) => match alt.as_ref() {
                crate::ast::modern::Alternate::IfBlock(inner) => current = inner,
                crate::ast::modern::Alternate::Fragment(frag) => {
                    // Check if this Fragment wraps an {:else if} IfBlock
                    if let Some(elseif_block) = extract_elseif_from_fragment(frag) {
                        current = elseif_block;
                    } else {
                        return fragment_has_await(frag);
                    }
                }
            },
            None => return false,
        }
    }
}


/// Check if a fragment contains any expression tag with `await` (NOT const tags — those use $renderer.run())
pub(super) fn fragment_has_await(fragment: &Fragment) -> bool {
    for node in fragment.nodes.iter() {
        if let Node::ExpressionTag(tag) = node
            && let Some(expr) = tag.expression.render()
            && expr.contains("await ") {
                return true;
            }
    }
    false
}


/// Check if a fragment contains any @const tag with `await` in its init
pub(super) fn fragment_has_const_await(fragment: &Fragment) -> bool {
    fragment.nodes.iter().any(|n| {
        if let Node::ConstTag(ct) = n {
            ct.declaration.render().is_some_and(|s| s.contains("await "))
        } else {
            false
        }
    })
}

impl ServerContext<'_> {


/// Compile a server fragment with async expression handling.
/// When `is_async` is true, expression tags containing `await` are emitted as
/// `$$renderer.push(async () => $.escape(await expr))` instead of inline template interpolation.
fn compile_fragment_async(&self, 
    fragment: &Fragment,
    is_async: bool) -> Option<String> {
    // This takes priority over the is_async flag since const-await uses run(), not child_block
    if fragment_has_const_await(fragment) {
        return self.compile_fragment_with_const_run(fragment);
    }

    if !is_async {
        return self.compile_fragment(fragment);
    }

    // For async fragments, handle expression tags specially
    let mut output_lines: Vec<String> = Vec::new();
    let mut parts = ServerTemplateParts::new();

    let is_sig_server = |n: &Node| !is_whitespace_text(n) && !matches!(n, Node::SnippetBlock(_));
    let first_sig = fragment.nodes.iter().position(&is_sig_server);
    let last_sig = fragment.nodes.iter().rposition(&is_sig_server);
    let (first_sig, last_sig) = match (first_sig, last_sig) {
        (Some(f), Some(l)) => (f, l),
        _ => return Some(String::new()),
    };

    for (i, node) in fragment.nodes.iter().enumerate() {
        if i < first_sig || i > last_sig {
            continue;
        }
        match node {
            Node::Text(text) => {
                if text.data.trim().is_empty() && i > first_sig && i < last_sig {
                    parts.push_static(" ");
                } else {
                    // Trim leading whitespace from first significant text, trailing from last
                    let mut data = text.data.as_ref();
                    if i == first_sig {
                        data = data.trim_start();
                    }
                    if i == last_sig {
                        data = data.trim_end();
                    }
                    if !data.is_empty() {
                        let collapsed = collapse_template_whitespace(data);
                        parts.push_static(&collapsed);
                    }
                }
            }
            Node::ExpressionTag(tag) => {
                if let Some(expr_text) = tag.expression.render() {
                    if expr_text.contains("await ") {
                        // Flush accumulated parts
                        let flushed = parts.to_template_literal();
                        if !flushed.is_empty() {
                            output_lines.push(format!("$$renderer.push(`{flushed}`);\n"));
                            parts = ServerTemplateParts::new();
                        }
                        // Emit as async push
                        output_lines.push(format!("$$renderer.push(async () => $.escape({expr_text}));\n"));
                    } else {
                        parts.push_interpolation(&format!("$.escape({expr_text})"));
                    }
                }
            }
            Node::ConstTag(const_tag) => {
                let flushed = parts.to_template_literal();
                if !flushed.is_empty() {
                    output_lines.push(format!("$$renderer.push(`{flushed}`);\n"));
                    parts = ServerTemplateParts::new();
                }
                if let Some(decl_text) = const_tag.declaration.render() {
                    output_lines.push(format!("{decl_text};\n"));
                }
            }
            Node::IfBlock(if_block) => {
                let flushed = parts.to_template_literal();
                if !flushed.is_empty() {
                    output_lines.push(format!("$$renderer.push(`{flushed}`);\n"));
                    parts = ServerTemplateParts::new();
                }
                let if_code = self.compile_if_block(if_block)?;
                output_lines.push(if_code);
            }
            Node::RegularElement(element) => {
                self.serialize_element(element, &mut parts)?;
            }
            Node::Component(comp) => {
                let flushed = parts.to_template_literal();
                if !flushed.is_empty() {
                    output_lines.push(format!("$$renderer.push(`{flushed}`);\n"));
                    parts = ServerTemplateParts::new();
                }
                let comp_code = self.compile_component(comp)?;
                output_lines.push(comp_code);
            }
            _ => {}
        }
    }

    let flushed = parts.to_template_literal();
    if !flushed.is_empty() {
        output_lines.push(format!("$$renderer.push(`{flushed}`);\n"));
    }

    Some(output_lines.join(""))
}

} // end impl ServerContext

/// Parse a const tag declaration text like "const a = await 1" into (name, init_expr).
pub(super) fn parse_const_declaration(decl_text: &str) -> Option<(String, String)> {
    // Strip "const " prefix
    let rest = decl_text.strip_prefix("const ")?;
    // Find the "=" separator
    let eq_pos = rest.find('=')?;
    let name = rest[..eq_pos].trim().to_string();
    let init = rest[eq_pos + 1..].trim().to_string();
    Some((name, init))
}

impl ServerContext<'_> {


/// Compile a server fragment that has @const tags with await, using $renderer.run() pattern.
/// Pattern: hoisted let declarations + $renderer.run([closures]) + $renderer.async() for template content.
fn compile_fragment_with_const_run(&self,
    fragment: &Fragment,
) -> Option<String> {
    let mut output_lines: Vec<String> = Vec::new();

    // First pass: collect all @const declarations
    let mut const_entries: Vec<(String, String, bool)> = Vec::new(); // (name, init, has_await)
    for node in &fragment.nodes {
        if let Node::ConstTag(ct) = node
            && let Some(decl_text) = ct.declaration.render()
            && let Some((name, init)) = parse_const_declaration(&decl_text) {
                // Top-level await: `await expr` or `fn(await expr)`, but NOT `(async () => { ... await ... })()`
                let has_top_level_await = init.contains("await ") && !init.trim_start().starts_with("(async ");
                const_entries.push((name, init, has_top_level_await));
            }
    }

    if const_entries.is_empty() {
        return self.compile_fragment(fragment);
    }

    // Emit hoisted let declarations (with leading blank line for formatting)
    output_lines.push(String::from("\n"));
    for (name, _, _) in &const_entries {
        output_lines.push(format!("let {name};\n"));
    }
    output_lines.push(String::from("\n"));

    // Build $renderer.run() array
    output_lines.push(String::from("var promises = $$renderer.run([\n"));
    for (i, (name, init, has_await)) in const_entries.iter().enumerate() {
        let closure_kind = if *has_await { "async " } else { "" };
        let init_text = if *has_await {
            transform_await_in_expr(init)
        } else {
            init.clone()
        };
        // Re-indent multi-line init to be at 2-tab level inside the closure body
        let reindented_init = reindent_block(init_text.trim());
        let init_lines: Vec<&str> = reindented_init.lines().collect();
        if init_lines.len() > 1 {
            // Multi-line: first line at \t\t level, rest at \t\t level (same base)
            let mut closure = format!("\t{closure_kind}() => {{\n\t\t{name} = {}", init_lines[0]);
            for line in &init_lines[1..] {
                closure.push_str(&format!("\n\t\t{line}"));
            }
            closure.push_str(";\n\t}");
            output_lines.push(closure);
        } else {
            output_lines.push(format!("\t{closure_kind}() => {{\n\t\t{name} = {reindented_init};\n\t}}"));
        }
        if i < const_entries.len() - 1 {
            output_lines.push(String::from(",\n\n"));
        }
    }
    // Now emit template content (non-const nodes)
    let _is_sig_server = |n: &Node| !is_whitespace_text(n) && !matches!(n, Node::SnippetBlock(_) | Node::ConstTag(_));
    let non_const_nodes: Vec<&Node> = fragment.nodes.iter()
        .filter(|n| !matches!(n, Node::ConstTag(_)) && !is_whitespace_text(n))
        .collect();

    if non_const_nodes.is_empty() {
        output_lines.push(String::from("\n]);\n"));
    } else {
        output_lines.push(String::from("\n]);\n\n"));
    }

    // Last const index — template content depends on this
    let last_promise_idx = const_entries.len() - 1;

    if !non_const_nodes.is_empty() {
        // Build template content
        let mut parts = ServerTemplateParts::new();
        let mut template_lines: Vec<String> = Vec::new();

        for node in &non_const_nodes {
            match node {
                Node::Text(text) => {
                    let data = text.data.trim();
                    if !data.is_empty() {
                        let collapsed = collapse_template_whitespace(data);
                        parts.push_static(&collapsed);
                    }
                }
                Node::ExpressionTag(tag) => {
                    if let Some(expr_text) = tag.expression.render() {
                        // Flush before async expression
                        let flushed = parts.to_template_literal();
                        if !flushed.is_empty() {
                            template_lines.push(format!("$$renderer.push(`{flushed}`);\n"));
                            parts = ServerTemplateParts::new();
                        }
                        // Wrap in $renderer.async() since it may depend on const values
                        template_lines.push(format!(
                            "$$renderer.async([promises[{last_promise_idx}]], ($$renderer) => $$renderer.push(() => $.escape({expr_text})));\n"
                        ));
                    }
                }
                Node::RegularElement(element) => {
                    // Check if the element contains expressions that depend on const vars
                    let elem_has_const_deps = element_references_vars(element, &const_entries);
                    if elem_has_const_deps {
                        // Split element: push open tag statically, async for dynamic children, push close tag
                        // Open tag
                        parts.push_static(&format!("<{}>", &*element.name));
                        let flushed = parts.to_template_literal();
                        if !flushed.is_empty() {
                            template_lines.push(format!("$$renderer.push(`{flushed}`);\n"));
                            parts = ServerTemplateParts::new();
                        }
                        // Process children — wrap dynamic ones in async
                        for child in &element.fragment.nodes {
                            match child {
                                Node::ExpressionTag(tag) => {
                                    if let Some(expr_text) = tag.expression.render() {
                                        template_lines.push(format!(
                                            "$$renderer.async([promises[{last_promise_idx}]], ($$renderer) => $$renderer.push(() => $.escape({expr_text})));\n"
                                        ));
                                    }
                                }
                                Node::Text(text) => {
                                    let data = text.data.trim();
                                    if !data.is_empty() {
                                        parts.push_static(data);
                                    }
                                }
                                _ => {}
                            }
                        }
                        // Close tag
                        let flushed = parts.to_template_literal();
                        if !flushed.is_empty() {
                            template_lines.push(format!("$$renderer.push(`{flushed}`);\n"));
                            parts = ServerTemplateParts::new();
                        }
                        parts.push_static(&format!("</{}>", &*element.name));
                    } else {
                        self.serialize_element(element, &mut parts)?;
                    }
                }
                _ => {}
            }
        }

        let flushed = parts.to_template_literal();
        if !flushed.is_empty() {
            template_lines.push(format!("$$renderer.push(`{flushed}`);\n"));
        }

        output_lines.extend(template_lines);
    }

    Some(output_lines.join(""))
}

} // end impl ServerContext

/// Check if a RegularElement's content references any of the given variable names.
pub(super) fn element_references_vars(element: &RegularElement, const_entries: &[(String, String, bool)]) -> bool {
    for node in &element.fragment.nodes {
        if let Node::ExpressionTag(tag) = node
            && let Some(expr_text) = tag.expression.render() {
                for (name, _, _) in const_entries {
                    if expr_text.contains(name.as_str()) {
                        return true;
                    }
                }
            }
    }
    false
}


/// Check if an if-chain's test expressions reference any reactive ($state/$derived) variables
pub(super) fn if_chain_refs_reactive(if_block: &IfBlock, run_info: &ServerAsyncRunInfo) -> bool {
    let all_reactive_vars: Vec<&str> = run_info.state_vars.iter()
        .chain(run_info.async_vars.iter())
        .map(|s| s.as_str())
        .collect();

    let mut current = if_block;
    loop {
        if let Some(test) = current.test.render() {
            for var in &all_reactive_vars {
                if test.contains(var) {
                    return true;
                }
            }
        }
        match &current.alternate {
            Some(alt) => match alt.as_ref() {
                crate::ast::modern::Alternate::IfBlock(inner) => current = inner,
                crate::ast::modern::Alternate::Fragment(frag) => {
                    if let Some(elseif_block) = extract_elseif_from_fragment(frag) {
                        current = elseif_block;
                    } else {
                        return false;
                    }
                }
            },
            None => return false,
        }
    }
}

impl ServerContext<'_> {


/// Compile an if block wrapped in $$renderer.async_block() for script-level async run context.
fn compile_if_block_with_async_block(&self,
    if_block: &IfBlock,
    run_info: &ServerAsyncRunInfo,
) -> Option<String> {
    let last_promise_idx = run_info.run_slot_count.saturating_sub(1);

    // Check if any test expression in the chain has `await`
    let chain_has_await = has_await_in_if_chain(if_block);

    // Check if any test references an async-derived var (needs function call syntax)
    let async_callback = chain_has_await || if_chain_has_direct_await(if_block);

    let callback_prefix = if async_callback { "async " } else { "" };

    let mut output = String::new();
    output.push_str(&format!(
        "$$renderer.async_block([$$promises[{last_promise_idx}]], {callback_prefix}($$renderer) => {{\n"
    ));

    // Compile the if-chain inside the async_block
    let inner = self.compile_if_block_for_async_block(if_block, run_info)?;
    for line in inner.lines() {
        if line.is_empty() {
            output.push('\n');
        } else {
            output.push('\t');
            output.push_str(line);
            output.push('\n');
        }
    }

    output.push_str("});\n\n");

    Some(output)
}

} // end impl ServerContext

/// Check if an if-chain has direct `await` in any test expression
pub(super) fn if_chain_has_direct_await(if_block: &IfBlock) -> bool {
    let mut current = if_block;
    loop {
        if let Some(test) = current.test.render()
            && test.contains("await ") {
                return true;
            }
        match &current.alternate {
            Some(alt) => match alt.as_ref() {
                crate::ast::modern::Alternate::IfBlock(inner) => current = inner,
                crate::ast::modern::Alternate::Fragment(frag) => {
                    if let Some(elseif_block) = extract_elseif_from_fragment(frag) {
                        current = elseif_block;
                    } else {
                        return false;
                    }
                }
            },
            None => return false,
        }
    }
}

impl ServerContext<'_> {


/// Compile an if block for use inside $$renderer.async_block()
fn compile_if_block_for_async_block(&self,
    if_block: &IfBlock,
    run_info: &ServerAsyncRunInfo,
) -> Option<String> {
    let test = if_block.test.render()?;

    // Transform test: await → $.save, async-derived vars → function calls
    let test = transform_test_for_async_block(&test, run_info);

    let mut output = String::new();
    output.push_str(&format!("if ({test}) {{\n"));
    output.push_str("\t$$renderer.push('<!--[0-->');\n");

    let consequent = self.compile_fragment(&if_block.consequent)?;
    if !consequent.is_empty() {
        for line in consequent.lines() {
            if line.is_empty() {
                output.push('\n');
            } else {
                output.push('\t');
                output.push_str(line);
                output.push('\n');
            }
        }
    }

    // Handle else/else-if
    let mut branch_idx = 1i32;
    self.compile_if_alternate_for_async_block(&if_block.alternate, &mut output, &mut branch_idx, run_info)?;

    Some(output)
}

} // end impl ServerContext

/// Transform a test expression for use in async_block:
/// - `await expr` → `(await $.save(expr))()`
/// - async-derived vars → called as functions: `blocking` → `blocking()`
pub(super) fn transform_test_for_async_block(test: &str, run_info: &ServerAsyncRunInfo) -> String {
    let mut result = if test.contains("await ") {
        transform_await_in_expr(test)
    } else {
        test.to_string()
    };

    // Replace async-derived var references with function calls
    for var in &run_info.async_vars {
        // Simple word-boundary replacement: var → var()
        // Need to be careful not to replace inside longer identifiers
        let pattern = var.as_str();
        let replacement = format!("{var}()");
        result = replace_word_boundary(&result, pattern, &replacement);
    }

    result
}

impl ServerContext<'_> {


/// Handle alternates for if-blocks inside async_block.
/// When a later else-if has `await` in its test, nest it in a child_block/async_block.
fn compile_if_alternate_for_async_block(&self,
    alternate: &Option<Box<crate::ast::modern::Alternate>>,
    output: &mut String,
    branch_idx: &mut i32,
    run_info: &ServerAsyncRunInfo,
) -> Option<()> {
    if let Some(alt) = alternate {
        match alt.as_ref() {
            crate::ast::modern::Alternate::IfBlock(else_if) => {
                let test = else_if.test.render()?;
                let test_has_await = test.contains("await ");
                if test_has_await {
                    // Use async_block if test references state/async vars, else child_block
                    let refs_reactive = run_info.state_vars.iter().chain(run_info.async_vars.iter())
                        .any(|v| test.contains(v.as_str()));
                    self.emit_nested_await_branch(output, else_if, run_info, refs_reactive)?;
                } else {
                    let test = transform_test_for_async_block(&test, run_info);
                    output.push_str(&format!("}} else if ({test}) {{\n"));
                    output.push_str(&format!("\t$$renderer.push('<!--[{branch_idx}-->');\n"));
                    *branch_idx += 1;
                    self.emit_fragment_indented(output, &else_if.consequent)?;
                    self.compile_if_alternate_for_async_block(&else_if.alternate, output, branch_idx, run_info)?;
                }
            }
            crate::ast::modern::Alternate::Fragment(frag) => {
                if let Some(elseif_block) = extract_elseif_from_fragment(frag) {
                    let test = elseif_block.test.render()?;
                    let test_has_await = test.contains("await ");
                    if test_has_await {
                        let refs_reactive = run_info.state_vars.iter().chain(run_info.async_vars.iter())
                            .any(|v| test.contains(v.as_str()));
                        self.emit_nested_await_branch(output, elseif_block, run_info, refs_reactive)?;
                    } else {
                        let test = transform_test_for_async_block(&test, run_info);
                        output.push_str(&format!("}} else if ({test}) {{\n"));
                        output.push_str(&format!("\t$$renderer.push('<!--[{branch_idx}-->');\n"));
                        *branch_idx += 1;
                        self.emit_fragment_indented(output, &elseif_block.consequent)?;
                        self.compile_if_alternate_for_async_block(&elseif_block.alternate, output, branch_idx, run_info)?;
                    }
                } else {
                    output.push_str("} else {\n");
                    output.push_str("\t$$renderer.push('<!--[-1-->');\n");
                    self.emit_fragment_indented(output, frag)?;
                    output.push_str("}\n");
                }
            }
        }
    } else {
        output.push_str("} else {\n");
        output.push_str("\t$$renderer.push('<!--[-1-->');\n");
        output.push_str("}\n");
    }
    Some(())
}

/// Emit a fragment body with one level of indentation
fn emit_fragment_indented(&self, output: &mut String, fragment: &Fragment) -> Option<()> {
    let body = self.compile_fragment(fragment)?;
    if !body.is_empty() {
        for line in body.lines() {
            if line.is_empty() {
                output.push('\n');
            } else {
                output.push('\t');
                output.push_str(line);
                output.push('\n');
            }
        }
    }
    Some(())
}

/// Emit a nested child_block or async_block for an else-if branch with await in test.
/// Wraps the remaining if-chain in an else { child_block(...) } pattern.
fn emit_nested_await_branch(&self,
    output: &mut String,
    if_block: &IfBlock,
    run_info: &ServerAsyncRunInfo,
    use_async_block: bool,
) -> Option<()> {
    output.push_str("} else {\n");
    output.push_str("\t$$renderer.push('<!--[-1-->');\n\n");

    // Determine wrapper type
    if use_async_block {
        let last_idx = run_info.run_slot_count.saturating_sub(1);
        output.push_str(&format!("\t$$renderer.async_block([$$promises[{last_idx}]], async ($$renderer) => {{\n"));
    } else {
        output.push_str("\t$$renderer.child_block(async ($$renderer) => {\n");
    }

    // Compile the nested if-chain
    let test = if_block.test.render()?;
    let test = transform_await_in_expr(&test);
    output.push_str(&format!("\t\tif ({test}) {{\n"));
    output.push_str("\t\t\t$$renderer.push('<!--[0-->');\n");

    let consequent = self.compile_fragment(&if_block.consequent)?;
    if !consequent.is_empty() {
        for line in consequent.lines() {
            if line.is_empty() {
                output.push('\n');
            } else {
                output.push_str("\t\t\t");
                output.push_str(line);
                output.push('\n');
            }
        }
    }

    // Handle the remaining alternate of the nested block
    if let Some(alt) = &if_block.alternate {
        match alt.as_ref() {
            crate::ast::modern::Alternate::Fragment(frag) => {
                if let Some(elseif_block) = extract_elseif_from_fragment(frag) {
                    let inner_test = elseif_block.test.render()?;
                    let inner_has_await = inner_test.contains("await ");
                    if inner_has_await {
                        // Another level of nesting — rare but handle it
                        let inner_test = transform_await_in_expr(&inner_test);
                        output.push_str(&format!("\t\t}} else if ({inner_test}) {{\n"));
                        output.push_str("\t\t\t$$renderer.push('<!--[1-->');\n");
                        let inner_consequent = self.compile_fragment(&elseif_block.consequent)?;
                        if !inner_consequent.is_empty() {
                            for line in inner_consequent.lines() {
                                if line.is_empty() {
                                    output.push('\n');
                                } else {
                                    output.push_str("\t\t\t");
                                    output.push_str(line);
                                    output.push('\n');
                                }
                            }
                        }
                        // Handle rest of chain
                        if let Some(inner_alt) = &elseif_block.alternate {
                            match inner_alt.as_ref() {
                                crate::ast::modern::Alternate::Fragment(f) => {
                                    output.push_str("\t\t} else {\n");
                                    output.push_str("\t\t\t$$renderer.push('<!--[-1-->');\n");
                                    let else_body = self.compile_fragment(f)?;
                                    if !else_body.is_empty() {
                                        for line in else_body.lines() {
                                            if line.is_empty() {
                                                output.push('\n');
                                            } else {
                                                output.push_str("\t\t\t");
                                                output.push_str(line);
                                                output.push('\n');
                                            }
                                        }
                                    }
                                    output.push_str("\t\t}\n");
                                }
                                _ => {
                                    output.push_str("\t\t} else {\n");
                                    output.push_str("\t\t\t$$renderer.push('<!--[-1-->');\n");
                                    output.push_str("\t\t}\n");
                                }
                            }
                        } else {
                            output.push_str("\t\t} else {\n");
                            output.push_str("\t\t\t$$renderer.push('<!--[-1-->');\n");
                            output.push_str("\t\t}\n");
                        }
                    } else {
                        // No await — shouldn't normally happen in a nested block, but handle it
                        output.push_str("\t\t} else {\n");
                        output.push_str("\t\t\t$$renderer.push('<!--[-1-->');\n");
                        output.push_str("\t\t}\n");
                    }
                } else {
                    // Plain else
                    output.push_str("\t\t} else {\n");
                    output.push_str("\t\t\t$$renderer.push('<!--[-1-->');\n");
                    let else_body = self.compile_fragment(frag)?;
                    if !else_body.is_empty() {
                        for line in else_body.lines() {
                            if line.is_empty() {
                                output.push('\n');
                            } else {
                                output.push_str("\t\t\t");
                                output.push_str(line);
                                output.push('\n');
                            }
                        }
                    }
                    output.push_str("\t\t}\n");
                }
            }
            _ => {
                output.push_str("\t\t} else {\n");
                output.push_str("\t\t\t$$renderer.push('<!--[-1-->');\n");
                output.push_str("\t\t}\n");
            }
        }
    } else {
        output.push_str("\t\t} else {\n");
        output.push_str("\t\t\t$$renderer.push('<!--[-1-->');\n");
        output.push_str("\t\t}\n");
    }

    output.push_str("\t});\n\n");
    output.push_str("\t$$renderer.push(`<!--]-->`);\n");
    output.push_str("}\n");
    Some(())
}

/// Compile an if block for server
fn compile_if_block(&self, if_block: &IfBlock) -> Option<String> {
    let test = if_block.test.render()?;

    // Detect if any branch in the if chain uses await expressions
    let has_await = has_await_in_if_chain(if_block);

    let mut output = String::new();

    // If the if chain contains await, wrap in $$renderer.child_block(async ...)
    if has_await {
        output.push_str("$$renderer.child_block(async ($$renderer) => {\n");
    }

    let indent = if has_await { "\t" } else { "" };

    // Transform the test expression: await expr → (await $.save(expr))()
    let test = if test.contains("await ") {
        transform_await_in_expr(&test)
    } else {
        test
    };

    output.push_str(&format!("{indent}if ({test}) {{\n"));
    output.push_str(&format!("{indent}\t$$renderer.push('<!--[0-->');\n"));

    let consequent = self.compile_fragment_async(&if_block.consequent, has_await)?;
    if !consequent.is_empty() {
        for line in consequent.lines() {
            if line.is_empty() {
                output.push('\n');
            } else {
                output.push_str(indent);
                output.push('\t');
                output.push_str(line);
                output.push('\n');
            }
        }
    }

    // Handle else/else if
    let mut branch_idx = 1i32;
    self.compile_if_alternate(&if_block.alternate, &mut output, &mut branch_idx, has_await, indent)?;

    if has_await {
        output.push_str("});\n");
    }

    output.push('\n');
    output.push_str("$$renderer.push(`<!--]-->`);\n");

    Some(output)
}

} // end impl ServerContext

/// Extract an else-if IfBlock from a Fragment alternate, or None if it's a plain else.
/// The upstream Svelte parser wraps {:else if} in a Fragment containing a single IfBlock.
pub(super) fn extract_elseif_from_fragment(frag: &Fragment) -> Option<&IfBlock> {
    if frag.nodes.len() == 1
        && let Some(Node::IfBlock(if_block)) = frag.nodes.first()
        && if_block.elseif {
            return Some(if_block);
        }
    None
}

impl ServerContext<'_> {


/// Handle the alternate of an if block (else/else-if/none)
fn compile_if_alternate(&self,
    alternate: &Option<Box<crate::ast::modern::Alternate>>,
    output: &mut String,
    branch_idx: &mut i32,
    is_async: bool,
    indent: &str,
) -> Option<()> {
    if let Some(alt) = alternate {
        match alt.as_ref() {
            crate::ast::modern::Alternate::IfBlock(else_if) => {
                output.push_str(&format!("{indent}}} else "));
                let else_code = self.compile_if_block_inner_async(else_if, branch_idx, is_async, indent)?;
                output.push_str(&else_code);
            }
            crate::ast::modern::Alternate::Fragment(frag) => {
                // Check if this Fragment wraps an {:else if} IfBlock
                if let Some(elseif_block) = extract_elseif_from_fragment(frag) {
                    output.push_str(&format!("{indent}}} else "));
                    let else_code = self.compile_if_block_inner_async(elseif_block, branch_idx, is_async, indent)?;
                    output.push_str(&else_code);
                } else {
                    output.push_str(&format!("{indent}}} else {{\n"));
                    output.push_str(&format!("{indent}\t$$renderer.push('<!--[-1-->');\n"));
                    let else_body = self.compile_fragment_async(frag, is_async)?;
                    if !else_body.is_empty() {
                        for line in else_body.lines() {
                            if line.is_empty() {
                                output.push('\n');
                            } else {
                                output.push_str(indent);
                                output.push('\t');
                                output.push_str(line);
                                output.push('\n');
                            }
                        }
                    }
                    output.push_str(&format!("{indent}}}\n"));
                }
            }
        }
    } else {
        output.push_str(&format!("{indent}}} else {{\n"));
        output.push_str(&format!("{indent}\t$$renderer.push('<!--[-1-->');\n"));
        output.push_str(&format!("{indent}}}\n"));
    }
    Some(())
}

/// Inner helper for else-if chains that tracks branch indices (with async support)
fn compile_if_block_inner_async(&self,
    if_block: &IfBlock,
    branch_idx: &mut i32,
    is_async: bool,
    indent: &str,
) -> Option<String> {
    let test = if_block.test.render()?;
    let test = if is_async && test.contains("await ") {
        transform_await_in_expr(&test)
    } else {
        test
    };
    let mut output = String::new();

    output.push_str(&format!("if ({test}) {{\n"));
    output.push_str(&format!("{indent}\t$$renderer.push('<!--[{branch_idx}-->');\n"));
    *branch_idx += 1;

    let consequent = self.compile_fragment_async(&if_block.consequent, is_async)?;
    if !consequent.is_empty() {
        for line in consequent.lines() {
            if line.is_empty() {
                output.push('\n');
            } else {
                output.push_str(indent);
                output.push('\t');
                output.push_str(line);
                output.push('\n');
            }
        }
    }

    self.compile_if_alternate(&if_block.alternate, &mut output, branch_idx, is_async, indent)?;

    Some(output)
}

fn serialize_element(&self,
    element: &RegularElement,
    parts: &mut ServerTemplateParts,
) -> Option<()> {
    let is_svg = is_svg_element(&element.name);
    parts.push_static("<");
    parts.push_static(&element.name);

    for attr in element.attributes.iter() {
        match attr {
            Attribute::Attribute(attr) => {
                // Skip event handlers on server
                if attr.name.starts_with("on") {
                    continue;
                }
                // Lowercase attribute names for non-SVG elements
                let attr_name = if is_svg {
                    attr.name.to_string()
                } else {
                    attr.name.to_lowercase()
                };
                // Dynamic attribute values use $.attr() interpolation
                if is_dynamic_attribute_value(&attr.value) {
                    let expr_text = render_attribute_value_dynamic(&attr.value);
                    if let Some(expr) = expr_text {
                        parts.push_interpolation(&format!("$.attr('{attr_name}', {expr})"));
                    }
                    continue;
                }
                parts.push_static(" ");
                parts.push_static(&attr_name);
                match &attr.value {
                    AttributeValueKind::Boolean(true) => {
                        parts.push_static("=\"\"");
                    }
                    AttributeValueKind::Boolean(false) => {}
                    _ => {
                        parts.push_static("=\"");
                        parts.push_static(&render_attribute_value_static(&attr.value, self.source));
                        parts.push_static("\"");
                    }
                }
            }
            Attribute::BindDirective(bind) => {
                // bind:value={expr} → ${$.attr('value', expr)} on server
                if let Some(expr_text) = bind.expression.render() {
                    parts.push_interpolation(&format!("$.attr('{}', {})", bind.name, expr_text));
                }
            }
            _ => {}
        }
    }

    if is_void_element(&element.name) {
        parts.push_static("/>");
    } else {
        parts.push_static(">");
        // Find first and last non-whitespace children for whitespace trimming
        let children = &element.fragment.nodes;
        let first_non_ws = children.iter().position(|n| !is_whitespace_text(n));
        let last_non_ws = children.iter().rposition(|n| !is_whitespace_text(n));
        let mut elem_last_was_comment = false;
        for (ci, child) in children.iter().enumerate() {
            match child {
                Node::Text(text) => {
                    if text.data.trim().is_empty() {
                        // Pure whitespace text
                        let is_before_first = first_non_ws.is_none_or(|f| ci < f);
                        let is_after_last = last_non_ws.is_none_or(|l| ci > l);
                        if is_before_first || is_after_last || elem_last_was_comment {
                            // Leading/trailing whitespace or after comment → strip
                        } else {
                            // Between siblings — collapse to single space
                            parts.push_static(" ");
                        }
                    } else {
                        let mut collapsed = collapse_template_whitespace(&text.data);
                        // Trim leading ws if this is the first child or follows only whitespace
                        let is_first_sig = first_non_ws == Some(ci);
                        let is_last_sig = last_non_ws == Some(ci);
                        if is_first_sig {
                            collapsed = collapsed.trim_start().to_string();
                        }
                        if is_last_sig {
                            collapsed = collapsed.trim_end().to_string();
                        }
                        parts.push_static(&collapsed);
                    }
                }
                Node::RegularElement(el) => {
                    self.serialize_element(el, parts)?;
                }
                Node::ExpressionTag(tag) => {
                    // Try constant propagation first
                    if let Some(value) = try_resolve_constant_binding(&tag.expression, self.constant_bindings) {
                        parts.push_static(&value);
                    }
                    // Then try pure expression constant folding
                    else if let Some(folded) = try_fold_expression_to_string(&tag.expression) {
                        parts.push_static(&folded);
                    } else if let Some(expr_text) = tag.expression.render() {
                        parts.push_interpolation(&format!("$.escape({expr_text})"));
                    }
                }
                Node::Comment(_) => {
                    // HTML comments stripped from server output
                    elem_last_was_comment = true;
                    continue;
                }
                Node::HtmlTag(tag) => {
                    if let Some(expr) = tag.expression.render() {
                        parts.push_interpolation(&format!("$.html({expr})"));
                    }
                }
                _ => return None,
            }
            elem_last_was_comment = false;
        }
        parts.push_static(&format!("</{}>", element.name));
    }

    Some(())
}


/// Serialize a <select> element with special $$renderer.option() handling.
/// Works with the caller's parts and output_lines to allow static content
/// consolidation across select boundaries.
fn serialize_select_element(&self,
    element: &RegularElement,
    parts: &mut ServerTemplateParts,
    output_lines: &mut Vec<String>,
    each_counter: &mut usize,
) -> Option<()> {
    parts.push_static("<select");
    for attr in element.attributes.iter() {
        if let Attribute::Attribute(attr) = attr {
            if attr.name.starts_with("on") {
                continue;
            }
            parts.push_static(" ");
            parts.push_static(&attr.name.to_lowercase());
            match &attr.value {
                AttributeValueKind::Boolean(true) => {
                    parts.push_static("=\"\"");
                }
                AttributeValueKind::Boolean(false) => {}
                _ => {
                    parts.push_static("=\"");
                    parts.push_static(&render_attribute_value_static(&attr.value, self.source));
                    parts.push_static("\"");
                }
            }
        }
    }
    parts.push_static(">");

    self.compile_select_children(&element.fragment.nodes, parts, output_lines, each_counter, false)?;
    Some(())
}

/// Compile children of a <select> or <optgroup> element for server rendering.
/// Puts markers and static content into `parts`, imperative code into `output_lines`.
/// `in_nested_block` suppresses `<!----><!>` markers for render/component calls inside each/if/key blocks.
fn compile_select_children(&self,
    children: &[Node],
    parts: &mut ServerTemplateParts,
    output_lines: &mut Vec<String>,
    each_counter: &mut usize,
    in_nested_block: bool,
) -> Option<()> {
    for child in children.iter() {
        match child {
            Node::Text(text) if text.data.trim().is_empty() => {
                // Skip whitespace between options
            }
            Node::Comment(_) => {
                // Skip HTML comments
            }
            Node::RegularElement(el) if &*el.name == "option" => {
                // Flush parts, then emit $$renderer.option() call
                let flushed = parts.to_template_literal();
                if !flushed.is_empty() {
                    output_lines.push(format!("$$renderer.push(`{flushed}`);\n"));
                    *parts = ServerTemplateParts::new();
                }
                let option_call = self.serialize_option_element(el)?;
                output_lines.push(option_call);
            }
            Node::RegularElement(el) if &*el.name == "optgroup" => {
                // Add optgroup opening to parts
                parts.push_static("<optgroup");
                for attr in el.attributes.iter() {
                    if let Attribute::Attribute(attr) = attr {
                        parts.push_static(&format!(" {}=\"", attr.name.to_lowercase()));
                        parts.push_static(&render_attribute_value_static(&attr.value, self.source));
                        parts.push_static("\"");
                    }
                }
                parts.push_static(">");
                // Process optgroup children recursively (same nesting level as parent)
                self.compile_select_children(&el.fragment.nodes, parts, output_lines, each_counter, in_nested_block)?;
                parts.push_static("</optgroup>");
            }
            Node::EachBlock(each) => {
                // Add <!--[--> to parts, flush, then generate loop code
                parts.push_static("<!--[-->");
                let flushed = parts.to_template_literal();
                if !flushed.is_empty() {
                    output_lines.push(format!("$$renderer.push(`{flushed}`);\n"));
                    *parts = ServerTemplateParts::new();
                }
                let each_code = self.compile_each_in_select(each, each_counter)?;
                output_lines.push(each_code);
                // <!--]--> goes into parts for consolidation with next content
                parts.push_static("<!--]-->");
            }
            Node::IfBlock(if_block) => {
                // Flush parts, then generate if code
                let flushed = parts.to_template_literal();
                if !flushed.is_empty() {
                    output_lines.push(format!("$$renderer.push(`{flushed}`);\n"));
                    *parts = ServerTemplateParts::new();
                }
                let if_code = self.compile_if_in_select(if_block, each_counter)?;
                output_lines.push(if_code);
                // <!--]--> goes into parts for consolidation
                parts.push_static("<!--]-->");
            }
            Node::KeyBlock(key) => {
                // <!----> goes into parts
                parts.push_static("<!---->");
                let flushed = parts.to_template_literal();
                if !flushed.is_empty() {
                    output_lines.push(format!("$$renderer.push(`{flushed}`);\n"));
                    *parts = ServerTemplateParts::new();
                }
                // Key block body - use indented helper
                let body = self.render_select_children_indented(&key.fragment.nodes, each_counter, "\t")?;
                let trimmed_body = body.trim_start_matches('\n');
                output_lines.push(format!("\n{{\n{trimmed_body}}}\n"));
                // <!----> goes into parts
                parts.push_static("<!---->");
            }
            Node::RenderTag(render) => {
                // Flush parts, then emit render call
                let flushed = parts.to_template_literal();
                if !flushed.is_empty() {
                    output_lines.push(format!("$$renderer.push(`{flushed}`);\n"));
                    *parts = ServerTemplateParts::new();
                }
                if let Some(expr) = render.expression.render() {
                    let (fn_name, args) = parse_render_call_expr(&expr);
                    output_lines.push(format!("{fn_name}($$renderer{args});\n"));
                }
                // <!----><!> goes into parts for consolidation (only at top level)
                if !in_nested_block {
                    parts.push_static("<!----><!>");
                }
            }
            Node::Component(comp) => {
                // Flush parts, then emit component call
                let flushed = parts.to_template_literal();
                if !flushed.is_empty() {
                    output_lines.push(format!("$$renderer.push(`{flushed}`);\n"));
                    *parts = ServerTemplateParts::new();
                }
                let comp_code = self.compile_component(comp)?;
                output_lines.push(comp_code);
                // <!----><!> goes into parts for consolidation (only at top level)
                if !in_nested_block {
                    parts.push_static("<!----><!>");
                }
            }
            Node::HtmlTag(tag) => {
                // ${$.html(expr)}<!> goes into parts (it's all interpolation)
                if let Some(expr) = tag.expression.render() {
                    parts.push_interpolation(&format!("$.html({expr})"));
                    parts.push_static("<!>");
                }
            }
            Node::SvelteBoundary(boundary) => {
                // Flush current parts first
                let flushed = parts.to_template_literal();
                if !flushed.is_empty() {
                    output_lines.push(format!("$$renderer.push(`{flushed}`);\n"));
                    *parts = ServerTemplateParts::new();
                }
                // Emit separate <!--[--> push
                output_lines.push("$$renderer.push(`<!--[-->`);\n".to_string());
                // Boundary body
                let body = self.render_select_children_indented(&boundary.fragment.nodes, each_counter, "\t")?;
                let trimmed_body = body.trim_start_matches('\n');
                output_lines.push(format!("\n{{\n{trimmed_body}}}\n"));
                // Emit separate <!--]--> push
                output_lines.push("\n$$renderer.push(`<!--]-->`);\n".to_string());
            }
            _ => {
                // Unknown child type in select - skip
            }
        }
    }
    Some(())
}

/// Compile {#each} block inside a <select> for server rendering
fn compile_each_in_select(&self,
    each: &EachBlock,
    each_counter: &mut usize,
) -> Option<String> {
    let raw_expr = render_expression_from_source(&each.expression)
        .or_else(|| each.expression.render())?;

    let (expr, inferred_index) = if !each.has_as_clause {
        if let Some(OxcExpression::SequenceExpression(seq)) = each.expression.oxc_expression() {
            if seq.expressions.len() == 2 {
                if let OxcExpression::Identifier(id) = &seq.expressions[1] {
                    let idx_name = id.name.to_string();
                    let collection = if let Some((coll, _)) = raw_expr.rsplit_once(',') {
                        coll.trim().to_string()
                    } else {
                        raw_expr.clone()
                    };
                    (collection, Some(idx_name))
                } else {
                    (raw_expr, None)
                }
            } else {
                (raw_expr, None)
            }
        } else {
            (raw_expr, None)
        }
    } else {
        (raw_expr, None)
    };

    let context_name = each.context.as_ref()
        .and_then(|c| c.render())
        .unwrap_or_else(|| "$$item".to_string());

    let suffix = if *each_counter == 0 { String::new() } else { format!("_{each_counter}") };
    *each_counter += 1;

    let idx_var = if let Some(ref _idx) = inferred_index {
        format!("$$index{suffix}")
    } else {
        format!("$$index{suffix}")
    };

    let mut output = String::new();
    output.push_str(&format!("\nconst each_array{suffix} = $.ensure_array_like({expr});\n\n"));
    output.push_str(&format!("for (let {idx_var} = 0, $$length = each_array{suffix}.length; {idx_var} < $$length; {idx_var}++) {{\n"));
    output.push_str(&format!("\tlet {context_name} = each_array{suffix}[{idx_var}];\n"));

    // Process @const declarations first
    let mut body_nodes = Vec::new();
    for node in &each.body.nodes {
        if let Node::ConstTag(const_tag) = node {
            if let Some(decl_text) = const_tag.declaration.render() {
                output.push_str(&format!("\t{decl_text};\n"));
            }
        } else {
            body_nodes.push(node);
        }
    }

    // Process body: look for option elements and other content
    let mut inner = String::new();
    let sig_children: Vec<&Node> = body_nodes.iter()
        .filter(|n| !is_whitespace_text(n))
        .copied()
        .collect();

    for child in &sig_children {
        match child {
            Node::RegularElement(el) if &*el.name == "option" => {
                let option_call = self.serialize_option_element(el)?;
                inner.push_str(&option_call);
            }
            Node::Component(comp) => {
                let comp_code = self.compile_component(comp)?;
                inner.push('\n');
                inner.push_str(&comp_code);
            }
            Node::RenderTag(render) => {
                if let Some(expr) = render.expression.render() {
                    let (fn_name, args) = parse_render_call_expr(&expr);
                    inner.push_str(&format!("\n{fn_name}($$renderer{args});\n"));
                }
            }
            _ => {}
        }
    }

    for line in inner.lines() {
        if line.is_empty() {
            output.push('\n');
        } else {
            output.push('\t');
            output.push_str(line);
            output.push('\n');
        }
    }
    output.push_str("}\n");

    Some(output)
}

/// Render select children into a string with each line indented
fn render_select_children_indented(&self,
    nodes: &[Node],
    each_counter: &mut usize,
    indent: &str,
) -> Option<String> {
    let mut inner_parts = ServerTemplateParts::new();
    let mut inner_lines = Vec::new();
    self.compile_select_children(nodes, &mut inner_parts, &mut inner_lines, each_counter, true)?;
    let flushed = inner_parts.to_template_literal();
    if !flushed.is_empty() {
        inner_lines.push(format!("$$renderer.push(`{flushed}`);\n"));
    }
    let joined = inner_lines.join("");
    let normalized = normalize_server_select_blank_lines(&joined);
    let mut result = String::new();
    for line in normalized.lines() {
        if line.is_empty() {
            result.push('\n');
        } else {
            result.push_str(indent);
            result.push_str(line);
            result.push('\n');
        }
    }
    Some(result)
}

/// Compile {#if} block inside a <select> for server rendering
fn compile_if_in_select(&self,
    if_block: &IfBlock,
    each_counter: &mut usize,
) -> Option<String> {
    let condition = if_block.test.render()?;

    let mut output = String::new();
    output.push_str(&format!("\nif ({condition}) {{\n"));
    output.push_str("\t$$renderer.push('<!--[0-->');\n");

    // Consequent body
    let body = self.render_select_children_indented(&if_block.consequent.nodes, each_counter, "\t")?;
    output.push_str(&body);

    // Check for else/else-if
    if let Some(ref alternate) = if_block.alternate {
        match &**alternate {
            Alternate::IfBlock(nested_if) => {
                output.push_str("} else ");
                let nested = self.compile_if_in_select(nested_if, each_counter)?;
                // Strip leading \n from nested
                output.push_str(nested.trim_start_matches('\n'));
            }
            Alternate::Fragment(frag) => {
                output.push_str("} else {\n");
                output.push_str("\t$$renderer.push('<!--[-1-->');\n");
                let body = self.render_select_children_indented(&frag.nodes, each_counter, "\t")?;
                output.push_str(&body);
                output.push_str("}\n");
            }
        }
    } else {
        output.push_str("} else {\n");
        output.push_str("\t$$renderer.push('<!--[-1-->');\n");
        output.push_str("}\n");
    }

    Some(output)
}

} // end impl ServerContext

/// Parse a render tag expression like "opt()" into ("opt", "")
/// or "snippet(arg1, arg2)" into ("snippet", ", arg1, arg2")
pub(super) fn parse_render_call_expr(expr: &str) -> (String, String) {
    if let Some(paren_pos) = expr.find('(') {
        let fn_name = expr[..paren_pos].to_string();
        let args_part = &expr[paren_pos + 1..expr.len().saturating_sub(1)]; // strip ()
        let args = if args_part.trim().is_empty() {
            String::new()
        } else {
            format!(", {args_part}")
        };
        (fn_name, args)
    } else {
        (expr.to_string(), String::new())
    }
}

impl ServerContext<'_> {


/// Serialize a server <option> element as $$renderer.option() call
fn serialize_option_element(&self, element: &RegularElement) -> Option<String> {
    let attrs = self.build_option_attrs(element);

    // Get the content of the option
    let children = &element.fragment.nodes;
    let has_rich_content = server_option_has_rich_content(children);

    if has_rich_content {
        // Rich content: use callback with `true` flag at end
        let mut content_parts = ServerTemplateParts::new();
        for child in children.iter() {
            match child {
                Node::Text(text) => {
                    // Preserve significant whitespace (e.g., " text" after </em>)
                    let data = &*text.data;
                    if !data.trim().is_empty() {
                        content_parts.push_static(data);
                    }
                }
                Node::RegularElement(el) => {
                    self.serialize_element(el, &mut content_parts)?;
                }
                Node::ExpressionTag(tag) => {
                    if let Some(expr_text) = tag.expression.render() {
                        content_parts.push_interpolation(&format!("$.escape({expr_text})"));
                    }
                }
                Node::HtmlTag(tag) => {
                    if let Some(expr) = tag.expression.render() {
                        content_parts.push_interpolation(&format!("$.html({expr})"));
                    }
                }
                _ => {}
            }
        }
        let template = content_parts.to_template_literal();
        Some(format!("\n$$renderer.option(\n\t{attrs},\n\t($$renderer) => {{\n\t\t$$renderer.push(`{template}`);\n\t}},\n\tvoid 0,\n\tvoid 0,\n\tvoid 0,\n\tvoid 0,\n\ttrue\n);\n"))
    } else {
        // Simple text/expression content → $$renderer.option({}, value)
        // Check if it's a simple expression (just a variable reference)
        let sig_children: Vec<&Node> = children.iter()
            .filter(|n| !is_whitespace_text(n))
            .collect();

        // Single expression tag → use the expression directly as value
        if sig_children.len() == 1
            && let Node::ExpressionTag(tag) = sig_children[0]
            && let Some(expr_text) = tag.expression.render() {
                return Some(format!("\n$$renderer.option({attrs}, {expr_text});\n"));
            }

        // Text content → use callback with push
        let text_content: String = children.iter().filter_map(|n| {
            match n {
                Node::Text(text) => {
                    let trimmed = text.data.trim();
                    if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
                }
                _ => None,
            }
        }).collect::<Vec<_>>().join("");

        if text_content.is_empty() {
            Some(format!("\n$$renderer.option({attrs});\n"))
        } else {
            Some(format!("\n$$renderer.option({attrs}, ($$renderer) => {{\n\t$$renderer.push(`{text_content}`);\n}});\n"))
        }
    }
}

/// Build the attributes object string for a server-side $$renderer.option() call
fn build_option_attrs(&self, element: &RegularElement) -> String {
    let mut attrs = String::from("{");
    let mut first = true;

    for attr in element.attributes.iter() {
        if let Attribute::Attribute(attr) = attr {
            if !first { attrs.push(','); }
            first = false;
            attrs.push_str(&format!(" {}: ", attr.name));
            match &attr.value {
                AttributeValueKind::Boolean(true) => attrs.push_str("true"),
                _ => {
                    let val = render_attribute_value_static(&attr.value, self.source);
                    attrs.push_str(&format!("'{val}'"));
                }
            }
        }
    }
    if first {
        attrs.push_str("{}");
        return attrs[1..].to_string(); // strip leading {
    }
    attrs.push_str(" }");
    attrs
}

} // end impl ServerContext

/// Check if option children include rich content (HTML elements, @html, etc.)
pub(super) fn server_option_has_rich_content(children: &[Node]) -> bool {
    children.iter().any(|n| matches!(n, Node::RegularElement(_) | Node::HtmlTag(_) | Node::Component(_)))
}


pub(super) fn collect_server_snippet_functions(
    fragment: &Fragment,
    source: &str,
) -> Vec<String> {
    let mut snippets = Vec::new();
    for node in &fragment.nodes {
        if let Node::SnippetBlock(snippet) = node {
            let name = snippet.expression.render().unwrap_or_default();
            if name.is_empty() {
                continue;
            }

            // Compile snippet body for server
            let sctx = ServerContext::source_only(source);
            let body_code = sctx.compile_snippet_body(&snippet.body);
            let mut snippet_fn = format!("function {name}($$renderer) {{\n");
            snippet_fn.push_str(&body_code);
            snippet_fn.push_str("}\n");
            snippets.push(snippet_fn);
        }
    }
    snippets
}

impl ServerContext<'_> {

/// Compile the body of a server-side snippet.
fn compile_snippet_body(&self, fragment: &Fragment) -> String {
    let has_option = fragment.nodes.iter().any(|n| {
        matches!(n, Node::RegularElement(el) if &*el.name == "option")
    });

    if has_option {
        // Snippet body contains options → use $$renderer.option() calls
        let mut output = String::new();
        for node in &fragment.nodes {
            match node {
                Node::Text(text) if text.data.trim().is_empty() => {}
                Node::RegularElement(el) if &*el.name == "option" => {
                    if let Some(option_call) = self.serialize_option_element(el) {
                        for line in option_call.trim().lines() {
                            output.push('\t');
                            output.push_str(line);
                            output.push('\n');
                        }
                    }
                }
                _ => {}
            }
        }
        output
    } else {
        let mut parts = ServerTemplateParts::new();
        parts.push_static("<!---->");

        for node in &fragment.nodes {
            match node {
                Node::Text(text) => {
                    if !text.data.trim().is_empty() {
                        parts.push_static(text.data.trim());
                    }
                }
                Node::ExpressionTag(tag) => {
                    if let Some(expr) = tag.expression.render() {
                        parts.push_interpolation(&format!("$.escape({expr})"));
                    }
                }
                _ => {}
            }
        }

        let template = parts.to_template_literal();
        if template.is_empty() {
            String::new()
        } else {
            format!("\t$$renderer.push(`{template}`);\n")
        }
    }
}

} // impl ServerContext


pub(super) fn render_props_declaration_server(
    statement: &OxcStatement<'_>,
    props_name: &str,
    snippet: &str,
) -> Option<String> {
    let decl = match statement {
        OxcStatement::VariableDeclaration(d) => &**d,
        _ => return None,
    };
    for declarator in &decl.declarations {
        let Some(init) = declarator.init.as_ref() else { continue };
        if let OxcExpression::CallExpression(call) = init.get_inner_expression()
            && let OxcExpression::Identifier(id) = call.callee.get_inner_expression()
            && id.name.as_str() == "$props" {
                use oxc_ast::ast::BindingPattern;
                match &declarator.id {
                    BindingPattern::BindingIdentifier(_) => {
                        return Some(format!(
                            "let {{ $$slots, $$events, ...{props_name} }} = $$props;"
                        ));
                    }
                    BindingPattern::ObjectPattern(_) => {
                        // Render the destructuring pattern from source, replacing $props() with $$props
                        let decl_span = decl.span();
                        let decl_text = snippet
                            .get(decl_span.start as usize..decl_span.end as usize)
                            .unwrap_or("");
                        // Replace $props() with $$props
                        let result = decl_text.replace("$props()", "$$props");
                        return Some(result.trim().to_string());
                    }
                    _ => {}
                }
            }
    }
    None
}


