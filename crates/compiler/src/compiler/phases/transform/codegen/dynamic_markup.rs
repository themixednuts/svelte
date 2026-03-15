use std::collections::BTreeSet;

use camino::Utf8Path;
use oxc_ast::ast::{Declaration, Expression as OxcExpression, Statement as OxcStatement};
use oxc_codegen::{Codegen, Context, Gen};
use oxc_span::GetSpan;

use crate::api::GenerateTarget;
use crate::ast::modern::{
    Alternate, Attribute, AttributeValue, AttributeValueList, Component as ComponentNode,
    EachBlock, Fragment, IfBlock, Node,
    RegularElement, Root, Script, SnippetBlock, SvelteBoundary, SvelteElement,
};
use crate::js::{Render, codegen_options};

use super::{
    oxc_state_call_argument, render_state_declaration_statement, replace_source_ranges,
};
use super::static_markup::{component_name_from_filename, escape_js_template_literal};

pub(crate) fn compile_dynamic_markup_js(
    source: &str,
    target: GenerateTarget,
    root: &Root,
    runes_mode: bool,
    hmr: bool,
    filename: Option<&Utf8Path>,
) -> Option<String> {
    let _ = hmr; // TODO: HMR support
    let component_name = component_name_from_filename(filename);

    match target {
        GenerateTarget::Client => compile_client(source, root, runes_mode, &component_name),
        GenerateTarget::Server => compile_server(source, root, runes_mode, &component_name),
        GenerateTarget::None => Some(String::new()),
    }
}

// ---------------------------------------------------------------------------
// Client codegen
// ---------------------------------------------------------------------------

fn compile_client(
    source: &str,
    root: &Root,
    runes_mode: bool,
    component_name: &str,
) -> Option<String> {
    let mut ctx = ClientContext::new(runes_mode);

    // Collect local function names for getter optimization
    if let Some(instance) = root.instance.as_ref() {
        ctx.local_functions = collect_local_function_names(instance);
        ctx.constant_bindings = collect_constant_bindings(instance);
        // Collect state bindings that are mutated (need $.get()/$set()/$update())
        let all_state_bindings = collect_instance_state_bindings(instance);
        let mutated = collect_mutated_state_bindings(instance, &all_state_bindings, Some(root));
        let proxy_bindings = collect_proxy_bindings(instance, &mutated);
        ctx.proxy_bindings = proxy_bindings.clone();
        // state_bindings should only contain non-proxy mutated state (source signals)
        ctx.state_bindings = mutated.difference(&proxy_bindings).cloned().collect();
        ctx.derived_bindings = collect_derived_bindings(instance);
    }

    // Determine if component needs $$props parameter
    let has_props = has_props_rune(root) || has_class_rune_fields(root);

    // Check if props come only from destructured $props() pattern with no defaults
    let mut props_are_destructured_only = runes_mode
        && has_props
        && !has_class_rune_fields(root)
        && root.instance.as_ref().map_or(false, |inst| {
            detect_props_binding(inst) == Some("$$destructured_props".to_string())
        });

    // Collect destructured prop names for direct $$props access
    if props_are_destructured_only {
        if let Some(instance) = root.instance.as_ref() {
            let names = collect_destructured_prop_names(instance);
            if names.is_empty() {
                // Has defaults or rest — can't use direct access
                props_are_destructured_only = false;
            } else {
                ctx.destructured_props = names;
            }
        }
    }

    // Build instance script body FIRST (to get async_run_info before fragment compilation)
    let (script_body, client_async_run_info) = if let Some(instance) = root.instance.as_ref() {
        compile_instance_script_client(source, instance, runes_mode, root)?
    } else {
        (String::new(), None)
    };

    // If props are destructured only, filter out $.prop() declarations from script body
    let script_body = if props_are_destructured_only {
        script_body.lines()
            .filter(|line| !line.trim_start().starts_with("let ") || !line.contains("$.prop("))
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        script_body
    };

    // Sync derived needs $$props and push/pop on client too
    let has_props = has_props || client_async_run_info.as_ref().map_or(false, |info| info.has_sync_derived);

    // Set async_run_info on context before fragment compilation
    ctx.async_run_info = client_async_run_info;

    // Build template and body from fragment (after async_run_info is set)
    let fragment_code = ctx.compile_fragment(&root.fragment, source)?;

    // Replace destructured prop references with $$props.propName in fragment code
    let fragment_code = if !ctx.destructured_props.is_empty() {
        let mut code = fragment_code;
        for prop in &ctx.destructured_props {
            code = replace_word_with(&code, prop, &format!("$$props.{prop}"));
        }
        code
    } else {
        fragment_code
    };

    // Assemble the output
    let mut output = String::new();

    // Imports
    let uses_async = has_async_content_with_source(root, source);
    output.push_str("import 'svelte/internal/disclose-version';\n");
    if uses_async {
        output.push_str("import 'svelte/internal/flags/async';\n");
    } else if !runes_mode {
        output.push_str("import 'svelte/internal/flags/legacy';\n");
    }
    output.push_str("import * as $ from 'svelte/internal/client';\n");

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

    // Blank line between imports and var declarations (but not before
    // snippets — they already emit their own leading newline)
    if !ctx.templates.is_empty() && ctx.hoisted_snippets.is_empty() {
        output.push('\n');
    }

    // Hoisted snippet functions (before templates)
    for snippet_fn in &ctx.hoisted_snippets {
        output.push('\n');
        output.push_str(snippet_fn);
    }
    // Extra blank line after snippets before templates
    if !ctx.hoisted_snippets.is_empty() {
        output.push('\n');
    }

    // Hoisted templates — emit non-root templates first, then root last
    let mut non_root: Vec<&HoistedTemplate> = Vec::new();
    let mut root_template: Option<&HoistedTemplate> = None;
    for template in &ctx.templates {
        if template.name == "root" {
            root_template = Some(template);
        } else {
            non_root.push(template);
        }
    }
    let ordered: Vec<&HoistedTemplate> = non_root.iter().copied()
        .chain(root_template.into_iter())
        .collect();
    for template in &ordered {
        let flags = template.flags;
        let flags_arg = if flags != 0 {
            format!(", {flags}")
        } else {
            String::new()
        };
        output.push_str(&format!(
            "var {} = $.from_html(`{}`{flags_arg});\n",
            template.name,
            escape_js_template_literal(&template.html),
        ));
    }

    // Component function
    output.push('\n');
    let props_param = if has_props {
        if runes_mode {
            ", $$props"
        } else {
            ", $$props"
        }
    } else {
        ""
    };
    output.push_str(&format!(
        "export default function {component_name}($$anchor{props_param}) {{\n"
    ));

    // $.push / $.pop — not needed in runes mode when only $props() is used (no other reactive primitives)
    let only_props_rune = runes_mode
        && has_props
        && !has_class_rune_fields(root)
        && root.instance.as_ref().map_or(false, |inst| {
            detect_props_binding(inst) == Some("$$destructured_props".to_string())
        })
        && !script_body.contains("$.state(")
        && !script_body.contains("$.derived(")
        && !script_body.contains("$.proxy(");
    let needs_push_pop = has_props && !only_props_rune;
    if needs_push_pop {
        let push_arg = if runes_mode { ", true" } else { "" };
        output.push_str(&format!("\t$.push($$props{push_arg});\n\n"));
    }

    // Instance script body
    if !script_body.is_empty() {
        for line in script_body.lines() {
            if line.is_empty() {
                output.push('\n');
            } else {
                output.push('\t');
                output.push_str(line);
                output.push('\n');
            }
        }
        if !fragment_code.is_empty() {
            // Add blank line between script body and template when:
            // 1. push/pop wrapping is present, OR
            // 2. script has multi-line constructs (function/class)
            // Skip blank line when script ends with single-line $.run() without push/pop
            let has_block_constructs = script_body.contains("function ") || script_body.contains("class ");
            let ends_with_single_line_run = script_body.trim_end().ends_with("]);")
                && !script_body.contains("$.run([\n");
            let skip_gap = ends_with_single_line_run && !needs_push_pop;
            if (needs_push_pop || has_block_constructs) && !skip_gap {
                output.push('\n');
            }
        }
    }

    // Template body
    if !fragment_code.is_empty() {
        for line in fragment_code.lines() {
            if line.is_empty() {
                output.push('\n');
            } else {
                output.push('\t');
                output.push_str(line);
                output.push('\n');
            }
        }
    }

    if needs_push_pop {
        // Blank line before $.pop() only if the preceding code ends with
        // a multi-line construct like a class or function definition.
        let needs_gap = script_body.trim_end().ends_with('}')
            && (script_body.contains("class ")
                || script_body.contains("function "));
        if needs_gap {
            output.push('\n');
        }
        output.push_str("\t$.pop();\n");
    }

    output.push_str("}");

    // Normalize blank lines in the output
    output = normalize_client_blank_lines(&output);

    // Delegated events
    if !ctx.delegated_events.is_empty() {
        let events: Vec<String> = ctx
            .delegated_events
            .iter()
            .map(|e| format!("'{e}'"))
            .collect();
        output.push_str(&format!("\n\n$.delegate([{}]);\n", events.join(", ")));
    }

    Some(output)
}

// ---------------------------------------------------------------------------
// Server codegen
// ---------------------------------------------------------------------------

fn compile_server(
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
        && root.instance.as_ref().map_or(false, |inst| {
            detect_props_binding(inst) == Some("$$destructured_props".to_string())
        });

    let needs_component_wrapper = has_props && !props_are_destructured_only;

    // Detect if any component in the fragment has bind: directives → $$settled pattern
    let has_component_bindings = fragment_has_component_bindings(&root.fragment);

    // Build instance script body
    let (script_body, async_run_info): (String, Option<ServerAsyncRunInfo>) = if let Some(instance) = root.instance.as_ref() {
        compile_instance_script_server(source, instance, runes_mode)?
    } else {
        (String::new(), None)
    };

    // Sync derived ($derived.by, $derived with non-await fn) needs component wrapper + $$props
    let has_props = has_props || async_run_info.as_ref().map_or(false, |info| info.has_sync_derived);
    let needs_component_wrapper = needs_component_wrapper || async_run_info.as_ref().map_or(false, |info| info.has_sync_derived);

    // Collect constant bindings for constant propagation in templates
    let constant_bindings = if let Some(instance) = root.instance.as_ref() {
        collect_constant_bindings(instance)
    } else {
        std::collections::HashMap::new()
    };

    // Build template output
    let template_code = if let Some(ref run_info) = async_run_info {
        compile_server_fragment_with_script_run(&root.fragment, source, &constant_bindings, run_info)?
    } else {
        compile_server_fragment(&root.fragment, source, &constant_bindings)?
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

// ---------------------------------------------------------------------------
// Instance script compilation
// ---------------------------------------------------------------------------

fn compile_instance_script_client(
    source: &str,
    script: &Script,
    runes_mode: bool,
    root: &Root,
) -> Option<(String, Option<ServerAsyncRunInfo>)> {
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
                if let Some(ref props_name) = props_binding {
                    if let Some(rendered) = render_props_declaration_client(statement, props_name, snippet, runes_mode) {
                        statements.push(prepend_comments(&leading_comments, &rendered));
                        continue;
                    }
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

    Some((join_statements_with_blank_lines(&statements), None))
}

/// Rewrite `$effect(` → `$.user_effect(` and `$effect.pre(` → `$.user_pre_effect(`.
fn rewrite_effect_calls(source: &str) -> String {
    source
        .replace("$effect.pre(", "$.user_pre_effect(")
        .replace("$effect(", "$.user_effect(")
}

/// Render a statement using OXC codegen for consistent formatting,
/// with fallback to source extraction.
fn render_statement_via_codegen(snippet: &str, statement: &OxcStatement<'_>) -> String {
    let mut codegen = Codegen::new()
        .with_options(codegen_options())
        .with_source_text(snippet);
    statement.print(&mut codegen, Context::default());
    let text = codegen.into_source_text();
    text.trim().to_string()
}

/// Info about script-level async run pattern, passed from script to template compilation.
struct ServerAsyncRunInfo {
    /// Number of run slots (including empty ones)
    run_slot_count: usize,
    /// Variable names that are assigned via async run closures
    async_vars: Vec<String>,
    /// Variable names that are reactive state ($state)
    state_vars: Vec<String>,
    /// Whether any sync $.derived() calls exist (needs component wrapper)
    has_sync_derived: bool,
    /// Promise variable name ("$$promises" for top-level script, "promises" for @const run)
    promise_var: String,
}

fn compile_instance_script_server(
    source: &str,
    script: &Script,
    _runes_mode: bool,
) -> Option<(String, Option<ServerAsyncRunInfo>)> {
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
                if let Some(ref props_name) = props_binding {
                    if let Some(rendered) = render_props_declaration_server(statement, props_name, snippet) {
                        statements.push(prepend_comments(&leading_comments, &rendered));
                        continue;
                    }
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

    Some((join_statements_with_blank_lines(&statements), None))
}

/// Check if a script program has any top-level await expressions (not inside async functions/arrows)
fn script_has_top_level_await(
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
                        if let OxcExpression::CallExpression(call) = init.get_inner_expression() {
                            if let OxcExpression::Identifier(id) = call.callee.get_inner_expression() {
                                if id.name.as_str() == "$derived" {
                                    if let Some(arg) = call.arguments.first() {
                                        if let Some(expr) = arg.as_expression() {
                                            let arg_text = &snippet[expr.span().start as usize..expr.span().end as usize];
                                            if arg_text.starts_with("await ") || arg_text.contains(" await ") {
                                                return true;
                                            }
                                        }
                                    }
                                }
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
fn compile_instance_script_client_async_run(
    snippet: &str,
    program: &oxc_ast::ast::Program<'_>,
    props_binding: &Option<String>,
) -> Option<(String, Option<ServerAsyncRunInfo>)> {
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
                if let Some(props_name) = props_binding {
                    if let Some(rendered) = render_props_declaration_server(statement, props_name, snippet) {
                        non_run_statements.push(rendered);
                        continue;
                    }
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
                let next_is_multiline = run_closures.get(i + 1).map_or(false, |c| c.contains('\n'));
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

    Some((output, Some(info)))
}

/// Compile script declarations with top-level await into $renderer.run() pattern
fn compile_instance_script_server_async_run(
    snippet: &str,
    program: &oxc_ast::ast::Program<'_>,
    props_binding: &Option<String>,
) -> Option<(String, Option<ServerAsyncRunInfo>)> {
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
                if let Some(props_name) = props_binding {
                    if let Some(rendered) = render_props_declaration_server(statement, props_name, snippet) {
                        non_run_statements.push(rendered);
                        continue;
                    }
                }
                let rendered = render_statement_via_codegen(snippet, statement);
                let trimmed = rendered.trim();
                // $inspect() calls are dropped on server — add empty run slot
                if trimmed.starts_with("$inspect(") || trimmed.starts_with("$inspect.") {
                    run_closures.push(String::new()); // empty slot
                    continue;
                }
                // Function declarations go before run
                if let OxcStatement::FunctionDeclaration(_) = statement {
                    non_run_statements.push(rendered);
                } else {
                    non_run_statements.push(rendered);
                }
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
                let next_is_multiline = run_closures.get(i + 1).map_or(false, |c| c.contains('\n'));
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

    Some((output, Some(info)))
}

/// Extract the value from a $state() call for server: $state(val) → val
fn extract_state_call_value<'a>(
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
enum DerivedRunKind {
    /// `$derived(await expr)` → `async () => name = await $.async_derived(() => expr)`
    AsyncDerived(String),
    /// `$derived.by(fn)` → `() => name = $.derived(fn)` or `$derived(fn)` → `() => name = $.derived(() => fn)`
    SyncDerived(String),
}

/// Extract `$derived(...)` / `$derived.by(...)` patterns and generate run closures
fn extract_derived_run(
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
        if let OxcExpression::Identifier(id) = member.object.get_inner_expression() {
            if id.name.as_str() == "$derived" && member.property.name.as_str() == "by" {
                let arg = call.arguments.first()?.as_expression()?;
                let arg_text = &snippet[arg.span().start as usize..arg.span().end as usize];
                let arg_reindented = reindent_block(arg_text.trim());
                return Some(DerivedRunKind::SyncDerived(
                    format!("() => {binding_name} = $.derived({arg_reindented})")
                ));
            }
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

fn render_instance_declaration_client(
    snippet: &str,
    decl: &Declaration<'_>,
    _state_bindings: &BTreeSet<String>,
) -> Option<String> {
    Some(render_declaration_from_snippet(snippet, decl))
}

fn render_instance_declaration_server(
    snippet: &str,
    decl: &Declaration<'_>,
) -> Option<String> {
    Some(render_declaration_from_snippet(snippet, decl))
}

fn render_declaration_from_snippet(snippet: &str, decl: &Declaration<'_>) -> String {
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

/// Re-indent a multi-line block to have no base indentation.
fn reindent_block(text: &str) -> String {
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

/// Render $state() declarations on server: always strip to plain value with proper formatting.
fn render_state_declaration_server_formatted(
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

fn render_state_declaration_in_instance(
    snippet: &str,
    statement: &OxcStatement<'_>,
    target: GenerateTarget,
) -> Option<String> {
    // Reuse the module-level state declaration renderer
    render_state_declaration_statement(snippet, statement, target)
        .ok()
        .flatten()
}

/// Render a $state() declaration with mutation analysis:
/// - If the binding IS mutated → keep as `$.state(x)` (or `$.proxy(x)`)
/// - If the binding is NOT mutated → strip to plain `let name = x`
fn render_state_declaration_with_mutation_analysis(
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
        if let Some(ref name) = binding_name {
            if mutated_bindings.contains(name) {
                all_unmutated = false;
            }
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

            replacements.push((init.span(), replacement));
        }

        let declaration_span = declaration.span();
        replace_source_ranges(
            snippet,
            declaration_span,
            replacements
                .into_iter()
                .map(|(span, replacement)| (span.start as usize, span.end as usize, replacement))
                .collect(),
        )
        .ok()
    }
}

/// Render `$derived(expr)` → `$.derived(() => expr)` or `$derived.by(fn)` → `$.derived(fn)`
fn render_derived_declaration(
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
                if let Some(arg) = call.arguments.first() {
                    if let Some(expr) = arg.as_expression() {
                        let mut codegen = Codegen::new()
                            .with_options(codegen_options())
                            .with_source_text(snippet);
                        codegen.print_expression(expr);
                        let arg_text = codegen.into_source_text();
                        let arg_text = super::strip_outer_parens(&arg_text);
                        replacements.push((init.span(), format!("$.derived(() => {arg_text})")));
                    }
                }
            }
            OxcExpression::StaticMemberExpression(member) => {
                if let OxcExpression::Identifier(id) = &member.object {
                    if id.name.as_str() == "$derived" && member.property.name.as_str() == "by" {
                        has_derived = true;
                        // $derived.by(fn) → $.derived(fn)
                        if let Some(arg) = call.arguments.first() {
                            if let Some(expr) = arg.as_expression() {
                                let mut codegen = Codegen::new()
                                    .with_options(codegen_options())
                                    .with_source_text(snippet);
                                codegen.print_expression(expr);
                                let arg_text = codegen.into_source_text();
                                replacements.push((init.span(), format!("$.derived({arg_text})")));
                            }
                        }
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
    let mut result = replace_source_ranges(
        snippet,
        declaration_span,
        replacements
            .into_iter()
            .map(|(span, replacement)| (span.start as usize, span.end as usize, replacement))
            .collect(),
    )
    .ok()?;
    // Ensure trailing semicolon
    if !result.trim_end().ends_with(';') {
        result = format!("{};", result.trim_end());
    }
    Some(result)
}

fn render_class_with_rune_fields(
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
        if let ClassElement::PropertyDefinition(prop) = element {
            if let Some(init) = &prop.value {
                return is_state_or_derived_call(init);
            }
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
        if let ClassElement::PropertyDefinition(prop) = element {
            if let Some(init) = &prop.value {
                if is_state_or_derived_call(init) {
                    // For client: public fields become private, private fields stay private
                    if let oxc_ast::ast::PropertyKey::PrivateIdentifier(id) = &prop.key {
                        private_state_fields.insert(id.name.to_string());
                    } else if target == GenerateTarget::Client {
                        if let Some(name) = prop.key.static_name() {
                            private_state_fields.insert(name.to_string());
                        }
                    }
                }
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

                if let Some(init) = &prop.value {
                    if let Some((rune, arg_text)) = extract_rune_call_info(snippet, init) {
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
                                } else {
                                    if arg_text.is_empty() {
                                        output.push_str(&format!(
                                            "\t{static_prefix}{actual_name};\n"
                                        ));
                                    } else {
                                        output.push_str(&format!(
                                            "\t{static_prefix}{actual_name} = {arg_text};\n"
                                        ));
                                    }
                                }
                            }
                            GenerateTarget::None => {}
                        }
                        continue;
                    }
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

fn is_constructor_method(text: &str) -> bool {
    text.starts_with("constructor(") || text.starts_with("constructor (")
}

/// Rewrite constructor body: `this.#field = value` → `$.set(this.#field, value)`
/// for private state fields only.
fn rewrite_constructor_for_client(text: &str, private_state_fields: &BTreeSet<String>) -> String {
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

fn try_rewrite_private_assignment(line: &str, private_state_fields: &BTreeSet<String>) -> Option<String> {
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

fn is_state_or_derived_call(expr: &OxcExpression<'_>) -> bool {
    if let OxcExpression::CallExpression(call) = expr.get_inner_expression() {
        if let OxcExpression::Identifier(id) = call.callee.get_inner_expression() {
            return matches!(id.name.as_str(), "$state" | "$derived" | "$derived.by");
        }
    }
    false
}

fn extract_rune_call_info(snippet: &str, expr: &OxcExpression<'_>) -> Option<(&'static str, String)> {
    let OxcExpression::CallExpression(call) = expr.get_inner_expression() else {
        return None;
    };
    let rune_name = match call.callee.get_inner_expression() {
        OxcExpression::Identifier(id) => match id.name.as_str() {
            "$state" => "$state",
            "$derived" => "$derived",
            _ => return None,
        },
        _ => return None,
    };

    let arg_text = if let Some(arg) = call.arguments.first() {
        if let Some(expr) = arg.as_expression() {
            let mut codegen = Codegen::new()
                .with_options(codegen_options())
                .with_source_text(snippet);
            codegen.print_expression(expr);
            let text = codegen.into_source_text();
            super::strip_outer_parens(&text)
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    Some((rune_name, arg_text))
}

fn collect_instance_state_bindings(script: &Script) -> BTreeSet<String> {
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
fn collect_proxy_bindings(script: &Script, mutated: &BTreeSet<String>) -> BTreeSet<String> {
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
            if let Some(arg) = oxc_state_call_argument(init) {
                if arg.is_proxy_like() {
                    proxies.insert(name);
                }
            }
        }
    }
    proxies
}

/// Collect $derived() binding names — these are signals that need $.get() wrapping.
fn collect_derived_bindings(script: &Script) -> BTreeSet<String> {
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
fn is_derived_call(expr: &OxcExpression<'_>) -> bool {
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
fn collect_mutated_state_bindings(
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

fn scan_fragment_for_mutations(
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
                            if let Some(inner_alt) = &inner_if.alternate {
                                match inner_alt.as_ref() {
                                    crate::ast::modern::Alternate::Fragment(f) => {
                                        scan_fragment_for_mutations(f, state_bindings, mutated);
                                    }
                                    _ => {}
                                }
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
                            if let Some(expr_str) = bind.expression.render() {
                                if state_bindings.contains(expr_str.as_str()) {
                                    mutated.insert(expr_str);
                                }
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

fn scan_attribute_value_for_mutations(
    value: &AttributeValueList,
    state_bindings: &BTreeSet<String>,
    mutated: &mut BTreeSet<String>,
) {
    match value {
        AttributeValueList::Values(parts) => {
            for part in parts.iter() {
                if let AttributeValue::ExpressionTag(tag) = part {
                    if let Some(oxc_expr) = tag.expression.oxc_expression() {
                        scan_expression_for_mutations(oxc_expr, state_bindings, mutated);
                    }
                }
            }
        }
        AttributeValueList::ExpressionTag(tag) => {
            if let Some(oxc_expr) = tag.expression.oxc_expression() {
                scan_expression_for_mutations(oxc_expr, state_bindings, mutated);
            }
        }
        _ => {}
    }
}

fn scan_statement_for_mutations(
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

fn scan_expression_for_mutations(
    expr: &OxcExpression<'_>,
    state_bindings: &BTreeSet<String>,
    mutated: &mut BTreeSet<String>,
) {
    match expr {
        OxcExpression::AssignmentExpression(assign) => {
            // Check if left side is a state binding
            if let Some(name) = assignment_target_name(&assign.left) {
                if state_bindings.contains(&name) {
                    mutated.insert(name);
                }
            }
            scan_expression_for_mutations(&assign.right, state_bindings, mutated);
        }
        OxcExpression::UpdateExpression(update) => {
            if let Some(name) = simple_assignment_target_name(&update.argument) {
                if state_bindings.contains(&name) {
                    mutated.insert(name);
                }
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

fn assignment_target_name(target: &oxc_ast::ast::AssignmentTarget<'_>) -> Option<String> {
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

fn simple_assignment_target_name(target: &oxc_ast::ast::SimpleAssignmentTarget<'_>) -> Option<String> {
    match target {
        oxc_ast::ast::SimpleAssignmentTarget::AssignmentTargetIdentifier(id) => {
            Some(id.name.to_string())
        }
        _ => None,
    }
}

fn collect_local_function_names(script: &Script) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for statement in &script.oxc_program().body {
        match statement {
            OxcStatement::FunctionDeclaration(func) => {
                if let Some(id) = &func.id {
                    names.insert(id.name.to_string());
                }
            }
            OxcStatement::ExportNamedDeclaration(export) => {
                if let Some(Declaration::FunctionDeclaration(func)) = &export.declaration {
                    if let Some(id) = &func.id {
                        names.insert(id.name.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    names
}

fn collect_instance_imports(source: &str, script: &Script) -> Vec<String> {
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

fn collect_module_statements(source: &str, script: &Script) -> Vec<String> {
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

// ---------------------------------------------------------------------------
// Client template + DOM traversal
// ---------------------------------------------------------------------------

struct ClientContext {
    templates: Vec<HoistedTemplate>,
    template_counter: usize,
    /// Counters for named template prefixes (option_content, select_content, etc.)
    named_template_counters: std::collections::HashMap<String, usize>,
    var_counter: VarCounter,
    delegated_events: BTreeSet<String>,
    runes_mode: bool,
    /// Names of local function declarations (for getter optimization)
    local_functions: BTreeSet<String>,
    /// Compile-time constant bindings (variable name → string value)
    constant_bindings: std::collections::HashMap<String, String>,
    /// State bindings that were mutated (kept as $.state() signals) — need $.get() wrapping
    state_bindings: BTreeSet<String>,
    /// State bindings using $.proxy() — do NOT need $.get() wrapping
    proxy_bindings: BTreeSet<String>,
    /// Derived bindings ($derived) — need $.get() wrapping
    derived_bindings: BTreeSet<String>,
    /// Deferred effects/handlers — emitted after DOM traversal
    deferred_effects: Vec<String>,
    /// Impure attribute effects — batched into template_effects at flush time
    impure_attr_effects: Vec<ImpureAttrEffect>,
    /// Hoisted snippet functions (emitted before templates at module level)
    hoisted_snippets: Vec<String>,
    /// Info about script-level async run (for template promise dependencies)
    async_run_info: Option<ServerAsyncRunInfo>,
    /// Destructured prop names that should be accessed as $$props.propName
    destructured_props: BTreeSet<String>,
    /// Whether we're currently inside a select/optgroup element (for each flag selection)
    in_select_context: bool,
}

struct ImpureAttrEffect {
    el_var: String,
    attr_name: String,
    dep: String,      // dependency function ref (e.g., "y")
    is_custom: bool,  // custom element → separate template_effect
}

struct HoistedTemplate {
    name: String,
    html: String,
    flags: u32,
}

struct VarCounter {
    counts: std::collections::HashMap<String, usize>,
}

impl VarCounter {
    fn new() -> Self {
        Self {
            counts: std::collections::HashMap::new(),
        }
    }

    fn next(&mut self, base: &str) -> String {
        let count = self.counts.entry(base.to_string()).or_insert(0);
        let name = if *count == 0 {
            base.to_string()
        } else {
            format!("{}_{}", base, count)
        };
        *count += 1;
        name
    }
}

impl ClientContext {
    fn new(runes_mode: bool) -> Self {
        Self {
            templates: Vec::new(),
            template_counter: 0,
            named_template_counters: std::collections::HashMap::new(),
            var_counter: VarCounter::new(),
            delegated_events: BTreeSet::new(),
            runes_mode,
            local_functions: BTreeSet::new(),
            constant_bindings: std::collections::HashMap::new(),
            state_bindings: BTreeSet::new(),
            proxy_bindings: BTreeSet::new(),
            derived_bindings: BTreeSet::new(),
            deferred_effects: Vec::new(),
            impure_attr_effects: Vec::new(),
            hoisted_snippets: Vec::new(),
            async_run_info: None,
            destructured_props: BTreeSet::new(),
            in_select_context: false,
        }
    }

    /// Rewrite expression to use $.get() for state/derived bindings.
    fn maybe_rewrite_state_expr(&self, expr: &str) -> String {
        if self.state_bindings.contains(expr) || self.derived_bindings.contains(expr) {
            format!("$.get({expr})")
        } else {
            expr.to_string()
        }
    }

    /// Get all signal bindings that need $.get() wrapping (state + derived, NOT proxy).
    fn signal_bindings(&self) -> BTreeSet<String> {
        self.state_bindings.union(&self.derived_bindings).cloned().collect()
    }

    fn add_template(&mut self, html: String, flags: u32) -> String {
        let name = if self.template_counter == 0 {
            "root".to_string()
        } else {
            format!("root_{}", self.template_counter)
        };
        self.template_counter += 1;
        self.templates.push(HoistedTemplate {
            name: name.clone(),
            html,
            flags,
        });
        name
    }

    /// Add the main root template — always named "root", inserted at a position
    /// so it appears after snippet templates but with the name "root".
    fn add_root_template(&mut self, html: String, flags: u32) -> String {
        // The main root template should always be named "root"
        self.templates.push(HoistedTemplate {
            name: "root".to_string(),
            html,
            flags,
        });
        "root".to_string()
    }

    /// Add a template with a custom name prefix (e.g., "option_content", "select_content").
    /// Returns "prefix" for the first, "prefix_1" for the second, etc.
    fn add_named_template(&mut self, prefix: &str, html: String, flags: u32) -> String {
        let count = self.named_template_counters.entry(prefix.to_string()).or_insert(0);
        let name = if *count == 0 {
            prefix.to_string()
        } else {
            format!("{prefix}_{count}")
        };
        *count += 1;
        self.templates.push(HoistedTemplate {
            name: name.clone(),
            html,
            flags,
        });
        name
    }

    fn compile_fragment(&mut self, fragment: &Fragment, source: &str) -> Option<String> {
        if fragment.nodes.is_empty() {
            return Some(String::new());
        }

        // Check if fragment is purely static (no dynamic content)
        // If so, defer to static_markup (return None)
        if !has_dynamic_content(fragment) {
            return None;
        }

        // Extract template HTML and build DOM traversal code
        let mut template_html = String::new();
        let mut body = String::new();
        let mut _dynamic_count = 0;

        // Count significant nodes for template flags
        // SnippetBlocks and ConstTags are declarations and don't produce DOM nodes
        let is_dom_node = |n: &Node| !is_whitespace_text(n) && !matches!(n, Node::SnippetBlock(_) | Node::ConstTag(_));
        let significant_nodes: Vec<&Node> = fragment
            .nodes
            .iter()
            .filter(|n| is_dom_node(n))
            .collect();

        // Find indices of first and last significant (non-whitespace, non-snippet, non-const) nodes
        let is_sig = is_dom_node;
        let first_sig = fragment.nodes.iter().position(|n| is_sig(n));
        let last_sig = fragment.nodes.iter().rposition(|n| is_sig(n));
        let (first_sig, last_sig) = match (first_sig, last_sig) {
            (Some(f), Some(l)) => (f, l),
            _ => return Some(String::new()),
        };

        // Special case: single Component node — call directly with $$anchor (no template needed)
        if significant_nodes.len() == 1 {
            if let Node::Component(_) = significant_nodes[0] {
                let mut body = String::new();
                self.compile_single_dynamic_node(significant_nodes[0], "$$anchor", source, &mut body)?;
                return Some(body);
            }
        }

        // Check for fragment that is only dynamic comment anchors (e.g. single EachBlock)
        let all_dynamic = significant_nodes.iter().all(|n| matches!(n,
            Node::Component(_) | Node::SvelteElement(_) | Node::SvelteSelf(_)
            | Node::EachBlock(_) | Node::IfBlock(_) | Node::AwaitBlock(_)
            | Node::KeyBlock(_) | Node::HtmlTag(_)
            | Node::RenderTag(_) | Node::SvelteBoundary(_)
        ));
        if all_dynamic && significant_nodes.len() == 1 {
            // Single dynamic block — use $.comment() as anchor
            // Reserve the root template counter so nested templates start at root_1
            if self.template_counter == 0 {
                self.template_counter = 1;
            } else {
                // Consume a template counter slot (matching upstream behavior where
                // the comment placeholder still advances the counter)
                self.template_counter += 1;
            }
            let frag_var = self.var_counter.next("fragment");
            let node_var = self.var_counter.next("node");
            let mut body = String::new();
            body.push_str(&format!("var {frag_var} = $.comment();\n"));
            body.push_str(&format!("var {node_var} = $.first_child({frag_var});\n\n"));
            self.compile_single_dynamic_node(significant_nodes[0], &node_var, source, &mut body)?;
            // Add blank line before $.append if the body ends with a multi-line block (e.g. });)
            if body.trim_end().ends_with("});") || body.trim_end().ends_with('}') {
                body.push_str(&format!("\n$.append($$anchor, {frag_var});\n"));
            } else {
                body.push_str(&format!("$.append($$anchor, {frag_var});\n"));
            }
            return Some(body);
        }

        // Process all snippet blocks first (they're hoisted, not DOM nodes)
        let has_snippets = fragment.nodes.iter().any(|n| matches!(n, Node::SnippetBlock(_)));
        let is_top_level = self.template_counter == 0 && has_snippets;
        if is_top_level {
            // Reserve counter 0 ("root") for the main template — snippets start at root_1
            self.template_counter = 1;
        }
        for node in &fragment.nodes {
            if let Node::SnippetBlock(snippet) = node {
                self.compile_snippet_block(snippet, source);
            }
        }

        // Pre-compute which expression tags are part of a text+expression run
        // (i.e., adjacent to a Text node — they share a text anchor instead of getting <!>)
        let text_expr_run_indices = detect_text_expr_runs(&fragment.nodes, first_sig, last_sig);

        let mut last_was_comment = false;
        for (i, node) in fragment.nodes.iter().enumerate() {
            // Skip leading and trailing whitespace text nodes
            if i < first_sig || i > last_sig {
                continue;
            }
            match node {
                Node::Text(text) => {
                    if text_expr_run_indices.contains(&i) {
                        // Part of a text+expression run — emit just a space as text anchor
                        template_html.push(' ');
                    } else if text.data.trim().is_empty() {
                        // Skip whitespace after a stripped comment
                        if last_was_comment {
                            // skip
                        } else {
                            // Check if next significant node is a standalone ExpressionTag
                            // If so, skip this whitespace — the expression's text anchor provides separation
                            let next_is_standalone_expr = fragment.nodes[i+1..].iter().enumerate().find(|(_, n)| {
                                !is_whitespace_text(n) && !matches!(n, Node::SnippetBlock(_) | Node::ConstTag(_))
                            }).map(|(_, n)| matches!(n, Node::ExpressionTag(_)) && !text_expr_run_indices.contains(&(i + 1)))
                            .unwrap_or(false);
                            if !next_is_standalone_expr {
                                template_html.push(' ');
                            }
                        }
                    } else {
                        template_html.push_str(&collapse_template_whitespace(&text.data));
                    }
                    last_was_comment = false;
                }
                Node::RegularElement(element) => {
                    self.serialize_element_to_template(element, &mut template_html, source);
                    last_was_comment = false;
                }
                Node::ExpressionTag(_) => {
                    if text_expr_run_indices.contains(&i) {
                        // Part of a text+expression run — text anchor already emitted
                    } else {
                        // Standalone expression tag — emit space as text anchor
                        template_html.push(' ');
                        _dynamic_count += 1;
                    }
                    last_was_comment = false;
                }
                Node::Comment(_comment) => {
                    // HTML comments are stripped from client template
                    last_was_comment = true;
                }
                Node::ConstTag(_) => {
                    // {@const} declarations don't produce DOM nodes — handled in code generation
                }
                Node::SnippetBlock(_) => {
                    // Already processed above — skip
                }
                Node::Component(_)
                | Node::SvelteElement(_)
                | Node::SvelteSelf(_)
                | Node::EachBlock(_)
                | Node::IfBlock(_)
                | Node::AwaitBlock(_)
                | Node::KeyBlock(_)
                | Node::HtmlTag(_)
                | Node::RenderTag(_)
                | Node::SvelteBoundary(_) => {
                    // Dynamic content — insert comment anchor
                    template_html.push_str("<!>");
                    _dynamic_count += 1;
                    last_was_comment = false;
                }
                _ => {
                    // For unsupported nodes, bail out to generic renderer
                    return None;
                }
            }
        }

        if template_html.is_empty() {
            return Some(String::new());
        }

        // Determine template flags
        // 1 = TEMPLATE_FRAGMENT (multiple root nodes)
        // 2 = USE_IMPORT_NODE (template contains custom elements)
        // 3 = TEMPLATE_FRAGMENT | USE_IMPORT_NODE
        let is_fragment = significant_nodes.len() > 1;
        let has_custom_elements = fragment.nodes.iter().any(|n| {
            fn check_custom_elements(node: &Node) -> bool {
                match node {
                    Node::RegularElement(el) => {
                        is_custom_element(&el.name)
                            || el.fragment.nodes.iter().any(|c| check_custom_elements(c))
                    }
                    _ => false,
                }
            }
            check_custom_elements(n)
        });
        let flags = match (is_fragment, has_custom_elements) {
            (true, true) => 3,
            (true, false) => 1,
            (false, true) => 2,
            (false, false) => 0,
        };

        let template_name = if is_top_level {
            self.add_root_template(template_html, flags)
        } else {
            self.add_template(template_html, flags)
        };

        // Generate DOM traversal
        let root_var = if significant_nodes.len() == 1 {
            match significant_nodes[0] {
                Node::RegularElement(el) => self.var_counter.next(&el.name),
                _ => self.var_counter.next("fragment"),
            }
        } else {
            self.var_counter.next("fragment")
        };

        body.push_str(&format!("var {root_var} = {template_name}();\n"));

        // Walk nodes and generate DOM access code
        let is_single_element = significant_nodes.len() == 1;
        if is_single_element {
            if let Some(Node::RegularElement(element)) = significant_nodes.first() {
                self.compile_element_attributes(element, &root_var, source, &mut body);
                self.compile_element_children(element, &root_var, source, &mut body)?;
            } else {
                // Single non-element node (ExpressionTag, etc.)
                self.compile_fragment_children(fragment, &root_var, source, &mut body)?;
            }
        } else {
            // Multiple top-level nodes — traverse as fragment children
            self.compile_fragment_children(fragment, &root_var, source, &mut body)?;
        }

        // Flush impure attribute effects as batched template_effects
        let impure_effects = std::mem::take(&mut self.impure_attr_effects);
        if !impure_effects.is_empty() {
            let batched = flush_impure_attr_effects(&impure_effects);
            for effect in batched {
                self.deferred_effects.push(effect);
            }
        }

        // Flush deferred effects (template_effects, event handlers) after DOM traversal
        if !self.deferred_effects.is_empty() {
            // Add blank line separator if body ends with a var declaration line
            let last_line = body.trim_end_matches('\n').lines().last().unwrap_or("");
            if last_line.trim_start().starts_with("var ") {
                body.push('\n');
            }
            let effects = std::mem::take(&mut self.deferred_effects);

            // Batch bare $.set_text() calls into a single $.template_effect
            let mut set_text_calls: Vec<String> = Vec::new();
            let mut other_effects: Vec<String> = Vec::new();
            for effect in &effects {
                let trimmed = effect.trim();
                if trimmed.starts_with("$.set_text(") && !trimmed.contains("$.template_effect") {
                    set_text_calls.push(trimmed.trim_end_matches(';').to_string());
                } else {
                    other_effects.push(effect.clone());
                }
            }

            if !set_text_calls.is_empty() {
                if set_text_calls.len() == 1 {
                    // Single set_text — wrap in template_effect on one line
                    body.push_str(&format!("$.template_effect(() => {});\n", set_text_calls[0]));
                } else {
                    // Multiple set_text — batch into multi-line template_effect
                    body.push_str("$.template_effect(() => {\n");
                    for call in &set_text_calls {
                        body.push_str(&format!("\t{call};\n"));
                    }
                    body.push_str("});\n");
                }
            }

            if set_text_calls.len() > 1 && !other_effects.is_empty() {
                body.push('\n');
            }
            for effect in other_effects {
                body.push_str(&effect);
            }
        }

        // Add blank line before $.append if body ends with a multi-line construct
        let last_line = body.trim_end_matches('\n').lines().last().unwrap_or("");
        if last_line.trim() == ");" || last_line.trim() == "});" {
            body.push('\n');
        }
        body.push_str(&format!("$.append($$anchor, {root_var});\n"));
        Some(body)
    }

    fn serialize_element_to_template(
        &self,
        element: &RegularElement,
        html: &mut String,
        source: &str,
    ) {
        html.push('<');
        html.push_str(&element.name);

        // Static attributes — skip certain attributes that are handled dynamically
        let is_custom = is_custom_element(&element.name);
        for attr in element.attributes.iter() {
            match attr {
                Attribute::Attribute(attr) => {
                    // Skip ALL attributes on custom elements (handled via $.set_custom_element_data)
                    if is_custom {
                        continue;
                    }
                    // Skip event handlers — they're handled dynamically
                    if attr.name.starts_with("on") {
                        continue;
                    }
                    // Skip attributes with dynamic (expression) values
                    if is_dynamic_attribute_value(&attr.value) {
                        continue;
                    }
                    // Skip attributes handled as JS properties
                    if &*attr.name == "autofocus" || &*attr.name == "muted" {
                        continue;
                    }
                    // Skip value attribute on option elements (handled via JS)
                    if &*attr.name == "value" && &*element.name == "option" {
                        continue;
                    }
                    html.push(' ');
                    html.push_str(&attr.name);
                    match &attr.value {
                        AttributeValueList::Boolean(true) => {
                            // Boolean attribute: <input disabled>
                        }
                        AttributeValueList::Boolean(false) => {}
                        _ => {
                            html.push_str("=\"");
                            let rendered = render_attribute_value_static(&attr.value, source);
                            html.push_str(&rendered);
                            html.push('"');
                        }
                    }
                }
                _ => {
                    // Dynamic attributes — skip in template
                }
            }
        }

        // Check if void element
        if is_void_element(&element.name) {
            html.push_str("/>");
        } else {
            html.push('>');

            // Check if all children are pure (text + pure expressions)
            // If so, emit empty element (textContent will be set in JS)
            let all_children_pure = element.fragment.nodes.iter().all(|child| match child {
                Node::Text(_) => true,
                Node::ExpressionTag(tag) => {
                    is_pure_expression(&tag.expression)
                        || try_resolve_constant_binding(&tag.expression, &self.constant_bindings).is_some()
                }
                _ => false,
            });
            let has_expr_tags = element.fragment.nodes.iter().any(|n| matches!(n, Node::ExpressionTag(_)));

            if all_children_pure && has_expr_tags {
                // Empty element — textContent will be set dynamically
            } else if has_expr_tags {
                // Has expression tags but not all pure — use single space as text anchor
                // The full content will be set via $.set_text() with a template literal
                let only_text_and_expr = element.fragment.nodes.iter().all(|c| matches!(c, Node::Text(_) | Node::ExpressionTag(_)));
                if only_text_and_expr {
                    html.push(' ');
                } else {
                    // Mixed content with nested elements — emit children normally
                    let mut has_expression_anchor = false;
                    for child in &element.fragment.nodes {
                        match child {
                            Node::Text(text) => {
                                if !text.data.trim().is_empty() {
                                    html.push_str(&collapse_template_whitespace(&text.data));
                                }
                                has_expression_anchor = false;
                            }
                            Node::RegularElement(el) => {
                                self.serialize_element_to_template(el, html, source);
                                has_expression_anchor = false;
                            }
                            Node::ExpressionTag(_) => {
                                if !has_expression_anchor {
                                    html.push(' ');
                                    has_expression_anchor = true;
                                }
                            }
                            Node::Comment(_) => {}
                            _ => {
                                html.push_str("<!>");
                                has_expression_anchor = false;
                            }
                        }
                    }
                }
            } else if &*element.name == "select" || &*element.name == "optgroup" {
                // Special serialization for select/optgroup children
                self.serialize_select_children_to_template(&element.fragment.nodes, html, source);
            } else {
                // No expression tags — emit children, collapsing whitespace
                let child_first_sig = element.fragment.nodes.iter().position(|n| !is_whitespace_text(n) && !matches!(n, Node::Comment(_)));
                let child_last_sig = element.fragment.nodes.iter().rposition(|n| !is_whitespace_text(n) && !matches!(n, Node::Comment(_)));
                let mut last_was_comment = false;
                for (ci, child) in element.fragment.nodes.iter().enumerate() {
                    // Skip leading/trailing whitespace
                    if let (Some(f), Some(l)) = (child_first_sig, child_last_sig) {
                        if ci < f || ci > l {
                            continue;
                        }
                    }
                    match child {
                        Node::Text(text) => {
                            if text.data.trim().is_empty() {
                                if !last_was_comment {
                                    // Whitespace between elements → single space
                                    html.push(' ');
                                }
                            } else {
                                html.push_str(&collapse_template_whitespace(&text.data));
                            }
                            last_was_comment = false;
                        }
                        Node::RegularElement(el) => {
                            self.serialize_element_to_template(el, html, source);
                            last_was_comment = false;
                        }
                        Node::Comment(_) => {
                            last_was_comment = true;
                        }
                        _ => {
                            html.push_str("<!>");
                            last_was_comment = false;
                        }
                    }
                }
            }

            html.push_str(&format!("</{}>", element.name));
        }
    }

    /// Serialize children of a `<select>` or `<optgroup>` to template HTML.
    /// - Rich options → `<option><!></option>`
    /// - Each blocks → empty (compiled separately with own template)
    /// - If/Key/Boundary/Component/RenderTag/HtmlTag → `<!>`
    /// - If the select needs a full customizable_select wrapper → entire content is `<!>`
    fn serialize_select_children_to_template(&self, children: &[Node], html: &mut String, source: &str) {
        let sig: Vec<&Node> = children.iter()
            .filter(|n| !is_whitespace_text(n) && !matches!(n, Node::Comment(_)))
            .collect();

        if select_children_need_wrapper(&sig) {
            html.push_str("<!>");
            return;
        }

        for child in &sig {
            match child {
                Node::RegularElement(el) if &*el.name == "option" => {
                    if option_has_rich_content(el) {
                        html.push_str("<option><!></option>");
                    } else {
                        self.serialize_element_to_template(el, html, source);
                    }
                }
                Node::RegularElement(el) => {
                    // optgroup or other element — recurse normally
                    self.serialize_element_to_template(el, html, source);
                }
                Node::EachBlock(_) => {
                    // Each block → empty (compiled separately with its own template)
                }
                _ => {
                    // If, Key, Boundary → <!>
                    html.push_str("<!>");
                }
            }
        }
    }

    /// Serialize a fragment to static HTML (for use in template roots of static content).
    fn serialize_fragment_to_static_html(&self, fragment: &Fragment, source: &str) -> String {
        let mut html = String::new();
        for node in &fragment.nodes {
            match node {
                Node::Text(text) => {
                    if !text.data.trim().is_empty() {
                        html.push_str(&collapse_template_whitespace(&text.data));
                    }
                }
                Node::RegularElement(el) => {
                    self.serialize_element_to_template(el, &mut html, source);
                }
                Node::Comment(_) => {}
                _ => {}
            }
        }
        html
    }

    /// Guess the first element's tag name in a fragment (for variable naming).
    fn guess_first_element_name(&self, fragment: &Fragment) -> String {
        for node in &fragment.nodes {
            if let Node::RegularElement(el) = node {
                return el.name.to_string();
            }
        }
        "fragment".to_string()
    }

    fn compile_element_children(
        &mut self,
        element: &RegularElement,
        element_var: &str,
        _source: &str,
        body: &mut String,
    ) -> Option<()> {
        self.compile_element_children_inner(element, element_var, _source, body, false)
    }

    fn compile_element_children_nonreactive(
        &mut self,
        element: &RegularElement,
        element_var: &str,
        source: &str,
        body: &mut String,
    ) -> Option<()> {
        self.compile_element_children_inner(element, element_var, source, body, true)
    }

    fn compile_element_children_inner(
        &mut self,
        element: &RegularElement,
        element_var: &str,
        _source: &str,
        body: &mut String,
        nonreactive: bool,
    ) -> Option<()> {
        let children = &element.fragment.nodes;
        if children.is_empty() {
            return Some(());
        }

        // Collect expression tags
        let expr_tags: Vec<&crate::ast::modern::ExpressionTag> = children
            .iter()
            .filter_map(|n| {
                if let Node::ExpressionTag(tag) = n {
                    Some(tag)
                } else {
                    None
                }
            })
            .collect();

        if expr_tags.is_empty() {
            // No expressions — check for nested elements with dynamic content
            return Some(());
        }

        // Check if all children (text + expressions) are pure/constant or non-reactive
        // If so, we can set textContent directly instead of using template_effect
        let all_pure = nonreactive || children.iter().all(|child| match child {
            Node::Text(_) => true,
            Node::ExpressionTag(tag) => {
                is_pure_expression(&tag.expression)
                    || try_resolve_constant_binding(&tag.expression, &self.constant_bindings).is_some()
            }
            _ => false,
        });

        if all_pure {
            // Check if ALL expressions can be fully evaluated at compile time
            let mut all_constant = true;
            let mut has_text = false;
            let mut has_expr = false;
            for child in children.iter() {
                match child {
                    Node::Text(_) => has_text = true,
                    Node::ExpressionTag(tag) => {
                        has_expr = true;
                        if let Some(oxc_expr) = tag.expression.oxc_expression() {
                            if try_eval_constant(oxc_expr).is_none()
                                && try_fold_expression_to_string(&tag.expression).is_none()
                                && try_resolve_constant_binding(&tag.expression, &self.constant_bindings).is_none()
                            {
                                all_constant = false;
                            }
                        } else {
                            all_constant = false;
                        }
                    }
                    _ => {}
                }
            }

            if all_constant {
                // All parts can be evaluated at compile time → string literal
                let mut const_parts = Vec::new();
                for child in children.iter() {
                    match child {
                        Node::Text(t) => const_parts.push(t.data.to_string()),
                        Node::ExpressionTag(tag) => {
                            if let Some(oxc_expr) = tag.expression.oxc_expression() {
                                if let Some(evaled) = try_eval_constant(oxc_expr) {
                                    const_parts.push(evaled);
                                } else if let Some(folded) = try_fold_expression_to_string(&tag.expression) {
                                    const_parts.push(folded);
                                } else if let Some(resolved) = try_resolve_constant_binding(&tag.expression, &self.constant_bindings) {
                                    const_parts.push(resolved);
                                }
                            }
                        }
                        _ => {}
                    }
                }
                let value = const_parts.join("");
                body.push_str(&format!(
                    "\n{element_var}.textContent = '{}';\n",
                    value.replace('\'', "\\'")
                ));
            } else if has_expr && !has_text && expr_tags.len() == 1 {
                // Single non-constant expression, no surrounding text
                if let Some(rendered) = expr_tags[0].expression.render() {
                    body.push_str(&format!(
                        "\n{element_var}.textContent = {rendered};\n"
                    ));
                }
            } else {
                // Mixed text + non-constant expressions → template literal
                let children_refs: Vec<&Node> = children.iter().collect();
                let template_str = build_template_string_no_null_coalesce(&children_refs);
                body.push_str(&format!(
                    "\n{element_var}.textContent = `{template_str}`;\n"
                ));
            }
            return Some(());
        }

        // Dynamic content — use template_effect
        let text_var = self.var_counter.next("text");
        // Use $.child(el, true) when element has exactly one expression and no literal text
        let has_literal_text = children.iter().any(|c| matches!(c, Node::Text(t) if !t.data.trim().is_empty()));
        let child_true = if !has_literal_text && expr_tags.len() == 1 { ", true" } else { "" };
        body.push_str(&format!("var {text_var} = $.child({element_var}{child_true});\n\n"));
        body.push_str(&format!("$.reset({element_var});\n"));

        // Collect expression texts for the template_effect
        // Apply getter optimization: if expression is `fn()` where fn is a local function,
        // pass the function reference instead of calling it
        let mut expr_texts: Vec<String> = Vec::new();
        for tag in &expr_tags {
            if let Some(oxc_expr) = tag.expression.oxc_expression() {
                if let OxcExpression::CallExpression(call) = oxc_expr {
                    if call.arguments.is_empty() {
                        if let OxcExpression::Identifier(id) = &call.callee {
                            if self.local_functions.contains(id.name.as_str()) {
                                expr_texts.push(id.name.to_string());
                                continue;
                            }
                        }
                    }
                }
            }
            if let Some(expr_text) = tag.expression.render() {
                expr_texts.push(expr_text);
            }
        }

        if expr_texts.len() == 1 {
            // Single expression: simple template_effect (deferred)
            // Check if we're in async context and the expression references an async var
            let async_promise_deps = if let Some(ref info) = self.async_run_info {
                let last_idx = info.run_slot_count.saturating_sub(1);
                let expr_refs_async = info.async_vars.iter().any(|v| expr_texts[0].contains(v.as_str()));
                if expr_refs_async {
                    let pvar = &info.promise_var;
                    Some(format!(", void 0, void 0, [{pvar}[{last_idx}]]"))
                } else {
                    None
                }
            } else {
                None
            };

            if let Some(ref promise_deps) = async_promise_deps {
                // Async context: wrap vars in $.get() only for @const run context
                // (top-level script vars are plain values, not signals)
                let mut expr = expr_texts[0].clone();
                if let Some(ref info) = self.async_run_info {
                    if info.promise_var == "promises" {
                        // @const run: vars are signals/deriveds, need $.get()
                        for v in &info.async_vars {
                            expr = replace_var_with_get(&expr, v);
                        }
                    }
                }
                self.deferred_effects.push(format!(
                    "$.template_effect(() => $.set_text({text_var}, {expr}){promise_deps});\n"
                ));
            } else {
                // Check if single expression with no surrounding text → use expression directly
                let has_surrounding_text = children.iter().any(|c| matches!(c, Node::Text(t) if !t.data.trim().is_empty()));
                if !has_surrounding_text && expr_texts.len() == 1 {
                    let expr = rewrite_state_accesses(&expr_texts[0], &self.signal_bindings());
                    self.deferred_effects.push(format!(
                        "$.set_text({text_var}, {expr})"
                    ));
                } else {
                    let children_refs: Vec<&Node> = children.iter().collect();
                    let template_str = build_template_string_with_folding(&children_refs);
                    let template_str = rewrite_state_accesses(&template_str, &self.signal_bindings());
                    // Push bare set_text — will be batched into $.template_effect at flush time
                    self.deferred_effects.push(format!(
                        "$.set_text({text_var}, `{template_str}`)"
                    ));
                }
            }
        } else if expr_texts.len() > 1 {
            // Multiple expressions: batched template_effect with parameters (deferred)
            let params: Vec<String> = (0..expr_texts.len())
                .map(|i| format!("${i}"))
                .collect();
            let children_refs: Vec<&Node> = children.iter().collect();
            let template_str = build_template_string_with_folding_params(&children_refs);
            let deps = expr_texts.join(", ");
            self.deferred_effects.push(format!(
                "$.template_effect(({}) => $.set_text({text_var}, `{template_str}`), [{deps}]);\n",
                params.join(", ")
            ));
        }

        Some(())
    }

    /// Deep element traversal: enter element's children with $.child(), handle
    /// nested dynamic content, then $.reset() to exit.
    fn compile_element_deep(
        &mut self,
        element: &RegularElement,
        element_var: &str,
        source: &str,
        body: &mut String,
    ) -> Option<()> {
        // Special handling for select elements
        if &*element.name == "select" {
            return self.compile_select_element(element, element_var, source, body);
        }

        let children = &element.fragment.nodes;
        if children.is_empty() {
            return Some(());
        }

        // Check if children have expression tags (text interpolation case)
        let has_expr_tags = children.iter().any(|n| matches!(n, Node::ExpressionTag(_)));
        let only_text_and_expr = children.iter().all(|c| matches!(c, Node::Text(_) | Node::ExpressionTag(_)));

        if has_expr_tags && only_text_and_expr {
            // Text interpolation — delegate to existing handler
            return self.compile_element_children(element, element_var, source, body);
        }

        // Check if there are nested elements or dynamic nodes that need traversal
        let sig_children: Vec<(usize, &Node)> = children.iter().enumerate()
            .filter(|(_, n)| !is_whitespace_text(n) && !matches!(n, Node::Comment(_)))
            .collect();

        if sig_children.is_empty() {
            return Some(());
        }

        // Check which children need JS traversal
        let any_needs_traverse = sig_children.iter().any(|(_, n)| match n {
            Node::RegularElement(el) => element_needs_js_traversal(el),
            Node::ExpressionTag(_) | Node::HtmlTag(_) | Node::IfBlock(_)
            | Node::EachBlock(_) | Node::AwaitBlock(_) | Node::KeyBlock(_)
            | Node::Component(_) | Node::RenderTag(_) | Node::SvelteBoundary(_) => true,
            _ => false,
        });

        if !any_needs_traverse {
            // All children are static — no traversal needed
            return Some(());
        }

        // Deep traversal: enter with $.child(), visit dynamic children, $.reset()
        let mut first_child_var = String::new();
        let mut prev_child_var = String::new();
        let mut has_prev = false;
        let mut last_visited_idx: Option<usize> = None;

        for (si, (_, child)) in sig_children.iter().enumerate() {
            let child_needs_traverse = match child {
                Node::RegularElement(el) => element_needs_js_traversal(el),
                Node::ExpressionTag(_) | Node::HtmlTag(_) | Node::IfBlock(_)
                | Node::EachBlock(_) | Node::AwaitBlock(_) | Node::KeyBlock(_)
                | Node::Component(_) | Node::RenderTag(_) | Node::SvelteBoundary(_) => true,
                _ => false,
            };

            if !child_needs_traverse {
                continue;
            }

            let skip = if !has_prev {
                si * 2 // skip leading static siblings
            } else {
                let prev_idx = last_visited_idx.unwrap_or(0);
                (si - prev_idx) * 2
            };

            match child {
                Node::RegularElement(child_el) => {
                    let child_var = self.var_counter.next(&sanitize_var_name(&child_el.name));
                    if !has_prev {
                        if skip == 0 {
                            body.push_str(&format!("var {child_var} = $.child({element_var});\n"));
                        } else {
                            body.push_str(&format!("var {child_var} = $.sibling($.child({element_var}), {skip});\n"));
                        }
                        first_child_var = child_var.clone();
                    } else {
                        let sep = if body.trim_end_matches('\n').lines().last().unwrap_or("").starts_with("var ") { "" } else { "\n" };
                        body.push_str(&format!("{sep}var {child_var} = $.sibling({prev_child_var}, {skip});\n"));
                    }

                    // Recursively handle this child's content
                    if self.compile_element_deep(child_el, &child_var, source, body).is_none() {
                        return None;
                    }

                    // Handle dynamic attributes on this child element
                    self.compile_element_dynamic_attrs(child_el, &child_var, body);

                    prev_child_var = child_var;
                    has_prev = true;
                }
                Node::HtmlTag(html_tag) => {
                    let node_var = self.var_counter.next("node");
                    if !has_prev {
                        if skip == 0 {
                            body.push_str(&format!("var {node_var} = $.child({element_var});\n"));
                        } else {
                            body.push_str(&format!("var {node_var} = $.sibling($.child({element_var}), {skip});\n"));
                        }
                        first_child_var = node_var.clone();
                    } else {
                        body.push_str(&format!("\nvar {node_var} = $.sibling({prev_child_var}, {skip});\n"));
                    }

                    // Emit $.html() call
                    if let Some(expr) = html_tag.expression.render() {
                        let expr = self.maybe_rewrite_state_expr(&expr);
                        body.push_str(&format!("\n$.html({node_var}, () => {expr});\n"));
                    }

                    // Count remaining siblings after this node for $.next()
                    let remaining = sig_children.len() - si - 1;
                    let remaining_static = sig_children[si+1..].iter()
                        .filter(|(_, n)| match n {
                            Node::RegularElement(el) => !element_needs_js_traversal(el),
                            Node::Text(_) => true,
                            _ => false,
                        })
                        .count();
                    if remaining_static > 0 {
                        let next_count = remaining_static * 2;
                        body.push_str(&format!("$.next({next_count});\n"));
                    }

                    prev_child_var = node_var;
                    has_prev = true;
                }
                Node::ExpressionTag(_) | Node::IfBlock(_) | Node::EachBlock(_)
                | Node::AwaitBlock(_) | Node::KeyBlock(_) | Node::Component(_)
                | Node::RenderTag(_) | Node::SvelteBoundary(_) => {
                    let node_var = self.var_counter.next("node");
                    if !has_prev {
                        if skip == 0 {
                            body.push_str(&format!("var {node_var} = $.child({element_var});\n"));
                        } else {
                            body.push_str(&format!("var {node_var} = $.sibling($.child({element_var}), {skip});\n"));
                        }
                        first_child_var = node_var.clone();
                    } else {
                        body.push_str(&format!("\nvar {node_var} = $.sibling({prev_child_var}, {skip});\n"));
                    }
                    body.push('\n');
                    if self.compile_single_dynamic_node(*child, &node_var, source, body).is_none() {
                        return None;
                    }
                    prev_child_var = node_var;
                    has_prev = true;
                }
                _ => {}
            }

            last_visited_idx = Some(si);
        }

        // $.reset() to exit the element
        if has_prev {
            body.push_str(&format!("$.reset({element_var});\n"));
        }

        Some(())
    }

    /// Compile a `<select>` element — handles rich content detection,
    /// `$.customizable_select()`, each flag 5, `$.get()` wrapping, and `__value` tracking.
    fn compile_select_element(
        &mut self,
        element: &RegularElement,
        select_var: &str,
        source: &str,
        body: &mut String,
    ) -> Option<()> {
        let children = &element.fragment.nodes;
        if children.is_empty() {
            return Some(());
        }

        let sig_children: Vec<&Node> = children.iter()
            .filter(|n| !is_whitespace_text(n) && !matches!(n, Node::Comment(_)))
            .collect();

        if sig_children.is_empty() {
            return Some(());
        }

        // Determine what kind of select content we have
        let old_in_select = self.in_select_context;
        self.in_select_context = true;

        // Pre-determine if select needs full customizable_select wrapping
        if select_children_need_wrapper(&sig_children) {
            self.compile_customizable_select_wrapper(
                select_var, "select", &sig_children, source, body
            )?;
            self.in_select_context = old_in_select;
            return Some(());
        }

        // Normal processing — iterate children
        let mut has_prev = false;
        let mut prev_var = String::new();

        for child in &sig_children {
            match child {
                Node::RegularElement(opt_el) if &*opt_el.name == "option" => {
                    let opt_var = self.var_counter.next("option");
                    if !has_prev {
                        body.push_str(&format!("var {opt_var} = $.child({select_var});\n"));
                    } else {
                        body.push_str(&format!("\nvar {opt_var} = $.sibling({prev_var}, 2);\n"));
                    }

                    if option_has_rich_content(opt_el) {
                        self.compile_customizable_select_option(opt_el, &opt_var, source, body);
                    } else {
                        self.compile_element_children(opt_el, &opt_var, source, body);
                    }

                    // Handle option value attribute
                    self.compile_element_dynamic_attrs(opt_el, &opt_var, body);

                    prev_var = opt_var;
                    has_prev = true;
                }
                Node::RegularElement(og_el) if &*og_el.name == "optgroup" => {
                    let og_var = self.var_counter.next("optgroup");
                    if !has_prev {
                        body.push_str(&format!("var {og_var} = $.child({select_var});\n"));
                    } else {
                        body.push_str(&format!("\nvar {og_var} = $.sibling({prev_var}, 2);\n"));
                    }

                    self.compile_optgroup_element(og_el, &og_var, source, body)?;

                    prev_var = og_var;
                    has_prev = true;
                }
                Node::EachBlock(each) => {
                    body.push('\n');
                    self.compile_select_each_block(each, select_var, source, body, false)?;
                    has_prev = true;
                    prev_var = select_var.to_string();
                }
                Node::IfBlock(if_block) => {
                    let node_var = self.var_counter.next("node");
                    if !has_prev {
                        body.push_str(&format!("var {node_var} = $.child({select_var});\n"));
                    } else {
                        body.push_str(&format!("\nvar {node_var} = $.sibling({prev_var}, 2);\n"));
                    }
                    body.push('\n');
                    self.compile_if_block(if_block, &node_var, source, body)?;
                    prev_var = node_var;
                    has_prev = true;
                }
                Node::KeyBlock(key) => {
                    let node_var = self.var_counter.next("node");
                    if !has_prev {
                        body.push_str(&format!("var {node_var} = $.child({select_var});\n"));
                    } else {
                        body.push_str(&format!("\nvar {node_var} = $.sibling({prev_var}, 2);\n"));
                    }
                    body.push('\n');
                    self.compile_key_block(key, &node_var, source, body)?;
                    prev_var = node_var;
                    has_prev = true;
                }
                Node::SvelteBoundary(boundary) => {
                    let node_var = self.var_counter.next("node");
                    if !has_prev {
                        body.push_str(&format!("var {node_var} = $.child({select_var});\n"));
                    } else {
                        body.push_str(&format!("\nvar {node_var} = $.sibling({prev_var}, 2);\n"));
                    }
                    body.push('\n');
                    self.compile_svelte_boundary(boundary, &node_var, source, body)?;
                    prev_var = node_var;
                    has_prev = true;
                }
                _ => {}
            }
        }

        if has_prev {
            // Add blank line before $.reset() for readability
            if body.trim_end().ends_with("});") || body.trim_end().ends_with('}') {
                body.push('\n');
            }
            body.push_str(&format!("$.reset({select_var});\n"));
        }

        self.in_select_context = old_in_select;
        Some(())
    }

    /// Compile a `$.customizable_select(target, () => { ... })` wrapper for rich content.
    fn compile_customizable_select_option(
        &mut self,
        option_el: &RegularElement,
        option_var: &str,
        source: &str,
        body: &mut String,
    ) {
        // Extract rich content and create a named template
        let content_html = self.serialize_option_rich_content(option_el, source);
        let content_template = self.add_named_template("option_content", content_html, 1);
        self.compile_customizable_select_option_with_template(option_el, option_var, &content_template, source, body);
    }

    fn compile_customizable_select_option_with_template(
        &mut self,
        option_el: &RegularElement,
        option_var: &str,
        content_template: &str,
        source: &str,
        body: &mut String,
    ) {
        let content_template = content_template.to_string(); // own it for format strings

        let anchor_var = self.var_counter.next("anchor");
        let fragment_var = self.var_counter.next("fragment");

        body.push_str(&format!("$.customizable_select({option_var}, () => {{\n"));
        body.push_str(&format!("\tvar {anchor_var} = $.child({option_var});\n"));
        body.push_str(&format!("\tvar {fragment_var} = {content_template}();\n"));

        // Check if content has dynamic expressions that need traversal
        let sig_children: Vec<&Node> = option_el.fragment.nodes.iter()
            .filter(|n| !is_whitespace_text(n))
            .collect();

        let has_dynamic = sig_children.iter().any(|n| match n {
            Node::ExpressionTag(_) | Node::HtmlTag(_) | Node::Component(_) | Node::RenderTag(_) => true,
            Node::RegularElement(el) => has_dynamic_content(&el.fragment),
            _ => false,
        });

        if has_dynamic {
            // Traverse the fragment content for dynamic nodes
            let mut inner_body = String::new();
            let saved_effects = std::mem::take(&mut self.deferred_effects);

            // Find elements in the rich content that need traversal
            for child in &sig_children {
                match child {
                    Node::RegularElement(el) if has_dynamic_content(&el.fragment) => {
                        let el_var = self.var_counter.next(&sanitize_var_name(&el.name));
                        inner_body.push_str(&format!("var {el_var} = $.first_child({fragment_var});\n"));
                        // Handle text expressions inside the element
                        self.compile_element_children(el, &el_var, source, &mut inner_body);
                    }
                    Node::RegularElement(el) => {
                        let el_var = self.var_counter.next(&sanitize_var_name(&el.name));
                        inner_body.push_str(&format!("var {el_var} = $.first_child({fragment_var});\n"));
                        self.compile_element_children(el, &el_var, source, &mut inner_body);
                    }
                    Node::HtmlTag(html_tag) => {
                        let node_var = self.var_counter.next("node");
                        inner_body.push_str(&format!("var {node_var} = $.first_child({fragment_var});\n\n"));
                        if let Some(expr) = html_tag.expression.render() {
                            inner_body.push_str(&format!("$.html({node_var}, () => {expr});\n"));
                        }
                    }
                    _ => {}
                }
            }

            // Flush effects — wrap each in $.template_effect
            let flushed = std::mem::take(&mut self.deferred_effects);
            for effect in &flushed {
                inner_body.push_str(&format!("$.template_effect(() => {effect});\n"));
            }
            self.deferred_effects = saved_effects;

            for line in inner_body.lines() {
                if !line.is_empty() {
                    body.push_str(&format!("\t{line}\n"));
                }
            }
        }

        // Check if we need $.next() (when content has trailing text like "text" in <em>Italic</em> text)
        let has_trailing_text = option_el.fragment.nodes.iter().any(|n| {
            if let Node::Text(t) = n {
                !t.data.trim().is_empty()
            } else {
                false
            }
        }) && option_el.fragment.nodes.iter().any(|n| matches!(n, Node::RegularElement(_)));
        if has_trailing_text {
            body.push_str(&format!("\t$.next();\n"));
        }

        body.push_str(&format!("\t$.append({anchor_var}, {fragment_var});\n"));
        body.push_str("});\n");
    }

    /// Check if a fragment contains a single render tag (for counter pre-consumption).
    fn if_branch_has_single_render_tag(&self, fragment: &Fragment) -> bool {
        let significant: Vec<&Node> = fragment.nodes.iter()
            .filter(|n| !is_whitespace_text(n))
            .collect();
        significant.len() == 1 && matches!(significant[0], Node::RenderTag(_))
    }

    /// Compile a `$.customizable_select()` wrapper that wraps the entire select or optgroup.
    fn compile_customizable_select_wrapper(
        &mut self,
        target_var: &str,
        target_type: &str,
        children: &[&Node],
        source: &str,
        body: &mut String,
    ) -> Option<()> {
        // Pre-consume fragment counter slots for any if-blocks with single render tags
        // (matching upstream behavior where compile_fragment would allocate but not emit)
        for child in children {
            if let Node::IfBlock(if_block) = child {
                if self.if_branch_has_single_render_tag(&if_block.consequent) {
                    self.var_counter.next("fragment");
                }
            }
        }

        // Create a content template — just `<!>` as a fragment anchor
        let content_template = self.add_named_template(
            &format!("{target_type}_content"), "<!>".to_string(), 1
        );

        let anchor_var = self.var_counter.next("anchor");
        let fragment_var = self.var_counter.next("fragment");
        let node_var = self.var_counter.next("node");

        body.push_str(&format!("\n$.customizable_select({target_var}, () => {{\n"));
        body.push_str(&format!("\tvar {anchor_var} = $.child({target_var});\n"));
        body.push_str(&format!("\tvar {fragment_var} = {content_template}();\n"));
        body.push_str(&format!("\tvar {node_var} = $.first_child({fragment_var});\n\n"));

        // Compile children inside the customizable_select callback
        for child in children {
            match child {
                Node::Component(comp) => {
                    let saved_effects = std::mem::take(&mut self.deferred_effects);
                    let mut inner = String::new();
                    self.compile_component_call(comp, &node_var, source, &mut inner)?;
                    let flushed = std::mem::take(&mut self.deferred_effects);
                    self.deferred_effects = saved_effects;
                    for line in inner.lines() {
                        body.push_str(&format!("\t{line}\n"));
                    }
                    for effect in flushed {
                        for line in effect.lines() {
                            body.push_str(&format!("\t{line}\n"));
                        }
                    }
                }
                Node::RenderTag(render) => {
                    let mut inner = String::new();
                    self.compile_render_tag(render, &node_var, source, &mut inner)?;
                    for line in inner.lines() {
                        body.push_str(&format!("\t{line}\n"));
                    }
                }
                Node::HtmlTag(html_tag) => {
                    if let Some(expr) = html_tag.expression.render() {
                        body.push_str(&format!("\t$.html({node_var}, () => {expr});\n"));
                    }
                }
                Node::EachBlock(each) => {
                    let saved_effects = std::mem::take(&mut self.deferred_effects);
                    let mut inner = String::new();
                    // Use flag 1 for each inside customizable_select
                    self.compile_select_each_block(each, &node_var, source, &mut inner, true)?;
                    let flushed = std::mem::take(&mut self.deferred_effects);
                    self.deferred_effects = saved_effects;
                    for line in inner.lines() {
                        body.push_str(&format!("\t{line}\n"));
                    }
                    for effect in flushed {
                        for line in effect.lines() {
                            body.push_str(&format!("\t{line}\n"));
                        }
                    }
                }
                Node::IfBlock(if_block) => {
                    let mut inner = String::new();
                    self.compile_if_block(if_block, &node_var, source, &mut inner)?;
                    for line in inner.lines() {
                        body.push_str(&format!("\t{line}\n"));
                    }
                }
                _ => {}
            }
        }

        body.push_str(&format!("\t$.append({anchor_var}, {fragment_var});\n"));
        body.push_str("});\n");
        Some(())
    }

    /// Compile an optgroup element's children
    fn compile_optgroup_element(
        &mut self,
        element: &RegularElement,
        element_var: &str,
        source: &str,
        body: &mut String,
    ) -> Option<()> {
        let sig_children: Vec<&Node> = element.fragment.nodes.iter()
            .filter(|n| !is_whitespace_text(n) && !matches!(n, Node::Comment(_)))
            .collect();

        // Check if optgroup has rich content that needs customizable_select
        let optgroup_rich = sig_children.iter().any(|child| match child {
            Node::Component(_) | Node::RenderTag(_) | Node::HtmlTag(_) => true,
            Node::EachBlock(each) => each_body_has_rich_content(&each.body),
            _ => false,
        });

        if optgroup_rich {
            // Wrap entire optgroup content in $.customizable_select
            self.compile_customizable_select_wrapper(
                element_var, "optgroup", &sig_children, source, body
            )?;
            return Some(());
        }

        // Regular optgroup handling
        for child in &sig_children {
            match child {
                Node::RegularElement(opt_el) if &*opt_el.name == "option" => {
                    if option_has_rich_content(opt_el) {
                        let opt_var = self.var_counter.next("option");
                        body.push_str(&format!("var {opt_var} = $.child({element_var});\n"));
                        self.compile_customizable_select_option(opt_el, &opt_var, source, body);
                    }
                }
                Node::EachBlock(each) => {
                    // For optgroup's each, it's not nested in customizable_select → flag 5
                    body.push_str("\n");
                    self.compile_select_each_block(each, element_var, source, body, false)?;
                }
                _ => {}
            }
        }

        body.push_str(&format!("$.reset({element_var});\n"));
        Some(())
    }

    /// Serialize the rich content of an option element for a named template.
    fn serialize_option_rich_content(&self, option_el: &RegularElement, source: &str) -> String {
        let mut html = String::new();
        for child in &option_el.fragment.nodes {
            match child {
                Node::Text(text) => {
                    let trimmed = text.data.trim();
                    if !trimmed.is_empty() {
                        html.push_str(&collapse_template_whitespace(&text.data));
                    }
                }
                Node::RegularElement(el) => {
                    self.serialize_element_to_template(el, &mut html, source);
                }
                Node::HtmlTag(_) | Node::ExpressionTag(_) => {
                    html.push_str("<!>");
                }
                _ => {}
            }
        }
        html
    }

    /// Compile an each block inside a select context with proper flag (5 or 1),
    /// `$.get(item)` wrapping, and `__value` tracking.
    fn compile_select_each_block(
        &mut self,
        each: &EachBlock,
        anchor_var: &str,
        source: &str,
        body: &mut String,
        inside_customizable: bool,
    ) -> Option<()> {
        let raw_expr = render_expression_from_source(&each.expression)
            .or_else(|| each.expression.render())?;

        let context_name = each.context.as_ref()
            .and_then(|c| c.render())
            .unwrap_or_else(|| "$$item".to_string());

        let flag = if inside_customizable { 1 } else { 5 };

        // Check for @const declarations in body
        let body_children: Vec<&Node> = each.body.nodes.iter()
            .filter(|n| !is_whitespace_text(n))
            .collect();

        let has_const = body_children.iter().any(|n| matches!(n, Node::ConstTag(_)));

        // Check if each body has Component (rich content inside select)
        let body_has_component = body_children.iter().any(|n| matches!(n, Node::Component(_)));

        // Check if body has option with rich content
        let body_has_rich_option = body_children.iter().any(|n| {
            if let Node::RegularElement(el) = n {
                &*el.name == "option" && option_has_rich_content(el)
            } else {
                false
            }
        });

        let mut each_body = String::new();

        if body_has_component {
            // Each body with Component — just call the component
            for child in &body_children {
                if let Node::Component(comp) = child {
                    let mut inner = String::new();
                    self.compile_component_call(comp, "$$anchor", source, &mut inner)?;
                    each_body.push_str(&inner);
                }
            }
        } else if body_has_rich_option {
            // Each body with rich option — need customizable_select per option
            // Temporarily add the each variable to state_bindings so $.get() wrapping works
            let had_binding = self.state_bindings.contains(&context_name);
            self.state_bindings.insert(context_name.clone());
            for child in &body_children {
                if let Node::RegularElement(opt_el) = child {
                    if &*opt_el.name == "option" && option_has_rich_content(opt_el) {
                        // Create content template FIRST (before option shell) to match expected ordering
                        let content_html = self.serialize_option_rich_content(opt_el, source);
                        let _content_template = self.add_named_template("option_content", content_html, 1);
                        // Then create template for option shell
                        let template_name = self.add_template("<option><!></option>".to_string(), 0);
                        let opt_var = self.var_counter.next("option");
                        each_body.push_str(&format!("var {opt_var} = {template_name}();\n"));
                        each_body.push('\n');
                        self.compile_customizable_select_option_with_template(opt_el, &opt_var, &_content_template, source, &mut each_body);
                        each_body.push('\n');
                        each_body.push_str(&format!("$.append($$anchor, {opt_var});\n"));
                    }
                }
            }
            if !had_binding {
                self.state_bindings.remove(&context_name);
            }
        } else {
            // Plain option in each body — standard pattern with $.get() and __value tracking
            let option_el = body_children.iter()
                .filter_map(|n| if let Node::RegularElement(el) = n { Some(el) } else { None })
                .find(|el| &*el.name == "option");

            if let Some(opt_el) = option_el {
                // Handle @const declarations
                if has_const {
                    for child in &body_children {
                        if let Node::ConstTag(const_tag) = child {
                            if let Some(decl_text) = const_tag.declaration.render() {
                                // Wrap in $.derived_safe_equal with $.get()
                                let decl_text = if decl_text.starts_with("const ") {
                                    decl_text.strip_prefix("const ").unwrap().to_string()
                                } else if decl_text.starts_with("let ") {
                                    decl_text.strip_prefix("let ").unwrap().to_string()
                                } else {
                                    decl_text.clone()
                                };
                                // Split name = expr
                                if let Some((name, expr)) = decl_text.split_once(" = ") {
                                    let name = name.trim();
                                    let expr = expr.trim();
                                    // Replace context_name with $.get(context_name) in expr
                                    let expr = replace_var_with_get(expr, &context_name);
                                    each_body.push_str(&format!("const {name} = $.derived_safe_equal(() => {expr});\n"));
                                }
                            }
                        }
                    }
                }

                // Create option template with space placeholder
                let template_name = self.add_template(format!("<option> </option>"), 0);
                let opt_var = self.var_counter.next("option");
                let text_var = self.var_counter.next("text");
                each_body.push_str(&format!("var {opt_var} = {template_name}();\n"));
                each_body.push_str(&format!("var {text_var} = $.child({opt_var}, true);\n\n"));
                each_body.push_str(&format!("$.reset({opt_var});\n\n"));

                // Option value tracking
                let value_tracker = format!("{opt_var}_value");
                each_body.push_str(&format!("var {value_tracker} = {{}};\n\n"));

                // Determine the value expression — from ExpressionTag in option children
                let value_expr = opt_el.fragment.nodes.iter()
                    .find_map(|n| if let Node::ExpressionTag(tag) = n { tag.expression.render() } else { None })
                    .unwrap_or_else(|| context_name.clone());

                // Wrap expression in $.get()
                let value_with_get = if has_const {
                    // For @const, the variable is already a derived signal
                    replace_var_with_get(&value_expr, &value_expr)
                } else {
                    replace_var_with_get(&value_expr, &context_name)
                };

                each_body.push_str("$.template_effect(() => {\n");
                each_body.push_str(&format!("\t$.set_text({text_var}, {value_with_get});\n\n"));
                each_body.push_str(&format!("\tif ({value_tracker} !== ({value_tracker} = {value_with_get})) {{\n"));
                each_body.push_str(&format!("\t\t{opt_var}.__value = {value_with_get};\n"));
                each_body.push_str("\t}\n");
                each_body.push_str("});\n\n");

                each_body.push_str(&format!("$.append($$anchor, {opt_var});\n"));
            } else {
                // Fallback — use generic compile_block_body_as_closure
                if let Some(inner) = self.compile_block_body_as_closure(&each.body, source) {
                    each_body.push_str(&inner);
                } else {
                    return None;
                }
            }
        }

        // Indent the each body
        let indented_body: String = each_body.lines()
            .map(|line| if line.is_empty() { String::new() } else { format!("\t{line}") })
            .collect::<Vec<_>>()
            .join("\n");

        let expr_in_arrow = if raw_expr.starts_with('{') {
            format!("({raw_expr})")
        } else {
            raw_expr.clone()
        };

        body.push_str(&format!(
            "$.each({anchor_var}, {flag}, () => {expr_in_arrow}, $.index, ($$anchor, {context_name}) => {{\n{indented_body}\n}});\n"
        ));

        Some(())
    }

    /// Handle dynamic attributes on an element (custom element data, autofocus, muted, option value).
    fn compile_element_dynamic_attrs(
        &self,
        element: &RegularElement,
        element_var: &str,
        body: &mut String,
    ) {
        let is_custom = is_custom_element(&element.name);

        for attr in &element.attributes {
            if let Attribute::Attribute(a) = attr {
                if is_custom {
                    // Custom element: $.set_custom_element_data()
                    let val = render_attribute_value_static(&a.value, "");
                    body.push_str(&format!(
                        "\n$.set_custom_element_data({element_var}, '{}', '{}');\n",
                        a.name, val
                    ));
                } else if &*a.name == "autofocus" {
                    body.push_str(&format!("\n$.autofocus({element_var}, true);\n"));
                } else if &*a.name == "muted" {
                    body.push_str(&format!("\n{element_var}.muted = true;\n"));
                } else if &*a.name == "value" && &*element.name == "option" {
                    let val = render_attribute_value_static(&a.value, "");
                    body.push_str(&format!("\n{element_var}.value = {element_var}.__value = '{val}';\n"));
                }
            }
        }
    }

    fn compile_fragment_children(
        &mut self,
        fragment: &Fragment,
        fragment_var: &str,
        source: &str,
        body: &mut String,
    ) -> Option<()> {
        let mut prev_var = String::new();
        let mut has_prev = false;

        // Pre-detect text+expression runs
        let first_sig = fragment.nodes.iter().position(|n| !is_whitespace_text(n) && !matches!(n, Node::SnippetBlock(_))).unwrap_or(0);
        let last_sig = fragment.nodes.iter().rposition(|n| !is_whitespace_text(n) && !matches!(n, Node::SnippetBlock(_))).unwrap_or(fragment.nodes.len().saturating_sub(1));
        let text_expr_run_indices = detect_text_expr_runs(&fragment.nodes, first_sig, last_sig);

        // Build list of significant nodes (non-whitespace, non-snippet, non-comment)
        let sig_nodes: Vec<(usize, &Node)> = fragment.nodes.iter().enumerate()
            .filter(|(_, n)| !is_whitespace_text(n) && !matches!(n, Node::SnippetBlock(_) | Node::Comment(_)))
            .collect();

        if sig_nodes.is_empty() {
            return Some(());
        }

        // Determine which significant nodes need JS traversal
        let needs_traverse: Vec<bool> = sig_nodes.iter().map(|(idx, node)| {
            if text_expr_run_indices.contains(idx) {
                return true;
            }
            match node {
                Node::RegularElement(el) => element_needs_js_traversal(el),
                Node::Text(_) => false,
                Node::ConstTag(_) => true,
                _ => true, // blocks, expressions, etc.
            }
        }).collect();

        // Track position in significant nodes for skip counting
        let mut last_visited_sig_idx: Option<usize> = None;

        let mut i = 0;
        let mut sig_iter_idx = 0;
        while i < fragment.nodes.len() {
            let node = &fragment.nodes[i];
            if is_whitespace_text(node) || matches!(node, Node::SnippetBlock(_) | Node::Comment(_)) {
                i += 1;
                continue;
            }

            // Find this node's position in sig_nodes
            let sig_idx = sig_iter_idx;
            sig_iter_idx += 1;

            if sig_idx >= sig_nodes.len() || sig_idx >= needs_traverse.len() {
                i += 1;
                continue;
            }

            if !needs_traverse[sig_idx] {
                i += 1;
                continue;
            }

            // Check if this node starts a text+expression run
            if text_expr_run_indices.contains(&i) {
                let mut run_texts = Vec::new();
                while i < fragment.nodes.len() && text_expr_run_indices.contains(&i) {
                    match &fragment.nodes[i] {
                        Node::Text(t) => {
                            let collapsed = collapse_template_whitespace(&t.data);
                            run_texts.push((false, collapsed));
                        }
                        Node::ExpressionTag(tag) => {
                            if let Some(expr) = tag.expression.render() {
                                let expr = self.maybe_rewrite_state_expr(&expr);
                                run_texts.push((true, expr));
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                    sig_iter_idx += 1; // skip the expression tags in sig_nodes
                }
                sig_iter_idx -= 1; // adjust for the outer increment

                let text_var = self.var_counter.next("text");
                if !has_prev {
                    body.push_str(&format!(
                        "var {text_var} = $.first_child({fragment_var});\n"
                    ));
                } else {
                    let sep = if body.trim_end_matches('\n').lines().last().unwrap_or("").starts_with("var ") { "" } else { "\n" };
                    body.push_str(&format!("{sep}var {text_var} = $.sibling({prev_var});\n"));
                }

                let mut template_str = String::new();
                for (is_expr, part) in &run_texts {
                    if *is_expr {
                        template_str.push_str(&format!("${{{part} ?? ''}}"));
                    } else {
                        template_str.push_str(part);
                    }
                }
                let template_str = template_str.trim_start_matches(|c: char| c == '\n' || c == '\r');
                self.deferred_effects.push(format!(
                    "$.template_effect(() => $.set_text({text_var}, `{template_str}`));\n"
                ));
                prev_var = text_var;
                has_prev = true;
                last_visited_sig_idx = Some(sig_idx);
                continue;
            }

            // Calculate skip count from previous visited node
            let skip = if !has_prev {
                // First traversable node — check if we need to skip leading static nodes
                if sig_idx == 0 {
                    0 // First significant node, no skip needed
                } else {
                    sig_idx * 2 // Each significant node = 2 DOM positions (element + text)
                }
            } else {
                // Distance from last visited to current
                let prev_idx = last_visited_sig_idx.unwrap_or(0);
                (sig_idx - prev_idx) * 2
            };

            // Create variable for this node
            match node {
                Node::RegularElement(element) => {
                    let var_base = sanitize_var_name(&element.name);
                    let el_var = self.var_counter.next(&var_base);
                    if !has_prev {
                        if skip == 0 {
                            body.push_str(&format!("var {el_var} = $.first_child({fragment_var});\n"));
                        } else {
                            body.push_str(&format!("var {el_var} = $.sibling($.first_child({fragment_var}), {skip});\n"));
                        }
                    } else {
                        let sep = if body.trim_end_matches('\n').lines().last().unwrap_or("").starts_with("var ") { "" } else { "\n" };
                        body.push_str(&format!("{sep}var {el_var} = $.sibling({prev_var}, {skip});\n"));
                    }

                    // Deep element traversal — enter element's children
                    if self.compile_element_deep(element, &el_var, source, body).is_none() {
                        return None;
                    }
                    self.compile_element_attributes(element, &el_var, source, body);
                    prev_var = el_var;
                    has_prev = true;
                }
                Node::ExpressionTag(tag) => {
                    let text_var = self.var_counter.next("text");
                    if !has_prev {
                        body.push_str(&format!("var {text_var} = $.first_child({fragment_var});\n"));
                    } else {
                        let sep = if body.trim_end_matches('\n').lines().last().unwrap_or("").starts_with("var ") { "" } else { "\n" };
                        body.push_str(&format!("{sep}var {text_var} = $.sibling({prev_var});\n"));
                    }
                    if let Some(expr) = tag.expression.render() {
                        let expr = self.maybe_rewrite_state_expr(&expr);
                        self.deferred_effects.push(format!("$.set_text({text_var}, ` ${{{expr} ?? ''}}`)" ));
                    }
                    prev_var = text_var;
                    has_prev = true;
                }
                Node::ConstTag(const_tag) => {
                    if let Some(decl_text) = const_tag.declaration.render() {
                        body.push_str(&format!("{decl_text};\n"));
                    }
                }
                Node::Component(_)
                | Node::SvelteElement(_)
                | Node::EachBlock(_)
                | Node::IfBlock(_)
                | Node::AwaitBlock(_)
                | Node::HtmlTag(_)
                | Node::KeyBlock(_)
                | Node::RenderTag(_)
                | Node::SvelteBoundary(_) => {
                    let node_var = self.var_counter.next("node");
                    if !has_prev {
                        if skip == 0 {
                            body.push_str(&format!("var {node_var} = $.first_child({fragment_var});\n"));
                        } else {
                            body.push_str(&format!("var {node_var} = $.sibling($.first_child({fragment_var}), {skip});\n"));
                        }
                    } else {
                        body.push_str(&format!("\nvar {node_var} = $.sibling({prev_var}, {skip});\n"));
                    }
                    body.push('\n');
                    self.compile_single_dynamic_node(node, &node_var, source, body)?;
                    prev_var = node_var;
                    has_prev = true;
                }
                Node::Text(_) => {
                    // Static text nodes are in the template, skip traversal
                }
                _ => {
                    return None; // Unsupported node type
                }
            }

            last_visited_sig_idx = Some(sig_idx);
            i += 1;
        }

        // After visiting all dynamic nodes, check for trailing static elements
        // that need a cursor advancement (like `var img = $.sibling(select, 2); $.next(2);`)
        if let Some(last_sig_idx) = last_visited_sig_idx {
            let trailing_static_count = sig_nodes.len() - last_sig_idx - 1;
            if trailing_static_count > 0 && has_prev {
                // Find the first trailing static element to create a var for
                let next_sig_idx = last_sig_idx + 1;
                if next_sig_idx < sig_nodes.len() {
                    let (_, next_node) = &sig_nodes[next_sig_idx];
                    if let Node::RegularElement(el) = next_node {
                        let skip = 2; // element + text gap
                        let var_name = self.var_counter.next(&sanitize_var_name(&el.name));
                        body.push_str(&format!("\nvar {var_name} = $.sibling({prev_var}, {skip});\n"));
                        // Count remaining static nodes after this one
                        let remaining_after = sig_nodes.len() - next_sig_idx - 1;
                        if remaining_after > 0 {
                            let next_count = remaining_after * 2;
                            body.push_str(&format!("\n$.next({next_count});\n"));
                        }
                    }
                }
            }
        }

        Some(())
    }

    /// Compile a snippet block into a hoisted module-level function.
    fn compile_snippet_block(&mut self, snippet: &SnippetBlock, source: &str) {
        let name = snippet.expression.render().unwrap_or_default();
        if name.is_empty() {
            return;
        }

        // Compile snippet body
        let body_text = self.compile_snippet_body(&snippet.body, source);

        let mut snippet_code = format!("const {name} = ($$anchor) => {{\n");
        snippet_code.push_str(&body_text);
        snippet_code.push_str("};\n");

        self.hoisted_snippets.push(snippet_code);
    }

    /// Compile the body of a snippet block.
    fn compile_snippet_body(&mut self, fragment: &Fragment, source: &str) -> String {
        let mut body = String::new();

        // Check if body is just text
        let significant: Vec<&Node> = fragment
            .nodes
            .iter()
            .filter(|n| !is_whitespace_text(n))
            .collect();

        if significant.is_empty() {
            return body;
        }

        // Simple case: body is just text nodes
        let all_text = significant.iter().all(|n| matches!(n, Node::Text(_)));
        if all_text {
            body.push_str("\t$.next();\n\n");
            let text_content: String = significant
                .iter()
                .filter_map(|n| match n {
                    Node::Text(t) => Some(t.data.trim()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(" ");
            let text_var = self.var_counter.next("text");
            body.push_str(&format!("\tvar {text_var} = $.text('{text_content}');\n\n"));
            body.push_str(&format!("\t$.append($$anchor, {text_var});\n"));
            return body;
        }

        // Complex case: compile as a fragment
        if let Some(fragment_code) = self.compile_fragment(fragment, source) {
            for line in fragment_code.lines() {
                if line.is_empty() {
                    body.push('\n');
                } else {
                    body.push('\t');
                    body.push_str(line);
                    body.push('\n');
                }
            }
        } else {
            // Static content — generate template root and $.append
            let template_html = self.serialize_fragment_to_static_html(fragment, source);
            if !template_html.is_empty() {
                let root_name = self.add_template(template_html, 0);
                let var_name = self.var_counter.next(&sanitize_var_name(&self.guess_first_element_name(fragment)));
                body.push_str(&format!("\tvar {var_name} = {root_name}();\n\n"));
                body.push_str(&format!("\t$.append($$anchor, {var_name});\n"));
            }
        }

        body
    }

    /// Compile a single dynamic node (Component, EachBlock, etc.) at the given anchor variable.
    fn compile_single_dynamic_node(
        &mut self,
        node: &Node,
        anchor_var: &str,
        source: &str,
        body: &mut String,
    ) -> Option<()> {
        match node {
            Node::Component(comp) => {
                self.compile_component_call(comp, anchor_var, source, body)?;
            }
            Node::EachBlock(each) => {
                if self.in_select_context {
                    self.compile_select_each_block(each, anchor_var, source, body, true)?;
                } else {
                    self.compile_each_block(each, anchor_var, source, body)?;
                }
            }
            Node::IfBlock(if_block) => {
                self.compile_if_block(if_block, anchor_var, source, body)?;
            }
            Node::AwaitBlock(await_block) => {
                self.compile_await_block(await_block, anchor_var, source, body)?;
            }
            Node::HtmlTag(tag) => {
                if let Some(expr) = tag.expression.render() {
                    body.push_str(&format!("$.html({anchor_var}, () => {expr});\n"));
                }
            }
            Node::SvelteElement(el) => {
                self.compile_svelte_element(el, anchor_var, source, body)?;
            }
            Node::KeyBlock(key) => {
                self.compile_key_block(key, anchor_var, source, body)?;
            }
            Node::RenderTag(render) => {
                self.compile_render_tag(render, anchor_var, source, body)?;
            }
            Node::SvelteBoundary(boundary) => {
                self.compile_svelte_boundary(boundary, anchor_var, source, body)?;
            }
            _ => {
                // TODO: handle more node types
                return None;
            }
        }
        Some(())
    }

    /// Compile a {#key expr}...{/key} block.
    fn compile_key_block(
        &mut self,
        key: &crate::ast::modern::KeyBlock,
        anchor_var: &str,
        source: &str,
        body: &mut String,
    ) -> Option<()> {
        let expr = key.expression.render().unwrap_or_default();
        let expr = self.maybe_rewrite_state_expr(&expr);

        // Compile key body
        let inner_body = self.compile_block_body_as_closure(&key.fragment, source)?;

        body.push_str(&format!("$.key({anchor_var}, () => {expr}, ($$anchor) => {{\n"));
        for line in inner_body.lines() {
            body.push_str(&format!("\t{line}\n"));
        }
        body.push_str("});\n");
        Some(())
    }

    /// Compile a {@render snippet()} tag.
    fn compile_render_tag(
        &mut self,
        render: &crate::ast::modern::RenderTag,
        anchor_var: &str,
        _source: &str,
        body: &mut String,
    ) -> Option<()> {
        let expr = render.expression.render().unwrap_or_default();
        let expr = self.maybe_rewrite_state_expr(&expr);

        // Render tags are calls: snippet_name(anchor)
        // The expression is typically a function call like `opt()` or `snippet(arg)`
        // We need to transform it to pass the anchor instead
        if expr.ends_with("()") {
            // Simple call with no args: `opt()` → `opt(anchor_var)`
            let fn_name = &expr[..expr.len() - 2];
            body.push_str(&format!("{fn_name}({anchor_var});\n"));
        } else if let Some(paren_pos) = expr.find('(') {
            // Call with args: `snippet(arg)` → keep as-is but pass anchor
            let fn_name = &expr[..paren_pos];
            let args = &expr[paren_pos + 1..expr.len() - 1];
            if args.is_empty() {
                body.push_str(&format!("{fn_name}({anchor_var});\n"));
            } else {
                body.push_str(&format!("{fn_name}({anchor_var}, {args});\n"));
            }
        } else {
            // Not a call — maybe optional: `snippet?.()` or just a reference
            body.push_str(&format!("{expr}({anchor_var});\n"));
        }
        Some(())
    }

    /// Compile a <svelte:boundary> block.
    fn compile_svelte_boundary(
        &mut self,
        boundary: &crate::ast::modern::SvelteBoundary,
        anchor_var: &str,
        source: &str,
        body: &mut String,
    ) -> Option<()> {
        // In select context, check if boundary body has options with rich content
        let inner_body = if self.in_select_context {
            self.compile_boundary_body_in_select_context(&boundary.fragment, source)?
        } else {
            self.compile_block_body_as_closure(&boundary.fragment, source)?
        };

        // Collect boundary props (failed, pending snippets)
        let mut props_entries = Vec::new();
        for attr in &boundary.attributes {
            if let Attribute::Attribute(a) = attr {
                let value = render_attribute_value_js(&a.value, source);
                props_entries.push(format!("{}: {}", a.name, value));
            }
        }
        let props_obj = if props_entries.is_empty() {
            "{}".to_string()
        } else {
            format!("{{ {} }}", props_entries.join(", "))
        };

        body.push_str(&format!("$.boundary({anchor_var}, {props_obj}, ($$anchor) => {{\n"));
        for line in inner_body.lines() {
            body.push_str(&format!("\t{line}\n"));
        }
        body.push_str("});\n");
        Some(())
    }

    /// Compile boundary body when inside a select context.
    /// Detects options with rich content and uses customizable_select.
    fn compile_boundary_body_in_select_context(
        &mut self,
        fragment: &Fragment,
        source: &str,
    ) -> Option<String> {
        let sig_children: Vec<&Node> = fragment.nodes.iter()
            .filter(|n| !is_whitespace_text(n) && !matches!(n, Node::Comment(_)))
            .collect();

        // Check if there's a single option with rich content
        if sig_children.len() == 1 {
            if let Node::RegularElement(el) = sig_children[0] {
                if &*el.name == "option" && option_has_rich_content(el) {
                    // Create content template FIRST (before option shell) to match expected ordering
                    let content_html = self.serialize_option_rich_content(el, source);
                    let content_template = self.add_named_template("option_content", content_html, 1);
                    // Then create <option><!></option> shell template
                    let root_name = self.add_template("<option><!></option>".to_string(), 0);
                    let option_var = self.var_counter.next("option");

                    let mut inner = String::new();
                    inner.push_str(&format!("var {option_var} = {root_name}();\n\n"));
                    self.compile_customizable_select_option_with_template(el, &option_var, &content_template, source, &mut inner);
                    inner.push_str(&format!("\n$.append($$anchor, {option_var});\n"));
                    return Some(inner);
                }
            }
        }

        // Fall back to normal compilation
        self.compile_block_body_as_closure(fragment, source)
    }

    /// Helper: compile a fragment as an inner block body for closures.
    /// Returns the code string to be indented and placed inside a closure.
    fn compile_block_body_as_closure(&mut self, fragment: &Fragment, source: &str) -> Option<String> {
        // Try dynamic compilation first
        if let Some(code) = self.compile_fragment(fragment, source) {
            return Some(code);
        }

        // Static content — generate template root and $.append
        let template_html = self.serialize_fragment_to_static_html(fragment, source);
        if template_html.is_empty() {
            return Some(String::new());
        }

        let root_name = self.add_template(template_html, 0);
        let var_name = self.var_counter.next(&sanitize_var_name(&self.guess_first_element_name(fragment)));
        let mut body = String::new();
        body.push_str(&format!("var {var_name} = {root_name}();\n\n"));
        body.push_str(&format!("$.append($$anchor, {var_name});\n"));
        Some(body)
    }

    /// Compile a component call: `ComponentName(anchor, { ...props })`
    fn compile_component_call(
        &mut self,
        comp: &ComponentNode,
        anchor_var: &str,
        source: &str,
        body: &mut String,
    ) -> Option<()> {
        let name = &comp.name;

        // Collect props from attributes
        let mut props = Vec::new();
        let mut has_bind_this = false;
        let mut bind_this_expr: Option<String> = None;

        for attr in comp.attributes.iter() {
            match attr {
                Attribute::Attribute(a) => {
                    let mut value = render_attribute_value_js(&a.value, source);
                    // Rewrite state accesses in prop values
                    if !self.state_bindings.is_empty() {
                        value = rewrite_state_accesses(&value, &self.state_bindings);
                    }
                    // Shorthand: {onmouseup} renders as just `onmouseup`
                    if value == a.name.as_ref() {
                        props.push(a.name.to_string());
                    } else {
                        props.push(format!("{}: {value}", a.name));
                    }
                }
                Attribute::BindDirective(bind) => {
                    if bind.name.as_ref() == "this" {
                        has_bind_this = true;
                        if let Some(expr) = bind.expression.render() {
                            bind_this_expr = Some(expr);
                        }
                    } else {
                        // bind:prop — generate get/set props with blank line between
                        if let Some(expr) = bind.expression.render() {
                            let (getter_body, setter_body) = if self.state_bindings.contains::<str>(&expr) {
                                (format!("$.get({expr})"), format!("$.set({expr}, $$value, true)"))
                            } else {
                                (expr.clone(), format!("{expr} = $$value"))
                            };
                            props.push(format!(
                                "get {}() {{\n\t\treturn {getter_body};\n\t}},\n\n\tset {}($$value) {{\n\t\t{setter_body};\n\t}}",
                                bind.name, bind.name
                            ));
                        }
                    }
                }
                Attribute::OnDirective(on) => {
                    if let Some(expr) = on.expression.render() {
                        props.push(format!("{}: {expr}", on.name));
                    }
                }
                _ => {}
            }
        }

        // Check for children (non-empty fragment)
        if !comp.fragment.nodes.is_empty() {
            let non_ws_children: Vec<&Node> = comp.fragment.nodes.iter()
                .filter(|n| !is_whitespace_text(n))
                .collect();
            if !non_ws_children.is_empty() {
                // Generate children function body
                let children_body = self.compile_component_children_client(&comp.fragment, source);
                if let Some(cb) = children_body {
                    let indented = cb.lines()
                        .map(|l| if l.is_empty() { String::new() } else { format!("\t\t{l}") })
                        .collect::<Vec<_>>()
                        .join("\n");
                    props.push(format!("children: ($$anchor, $$slotProps) => {{\n{indented}\n\t}}"));
                } else {
                    props.push("children: ($$anchor, $$slotProps) => {}".to_string());
                }
                props.push("$$slots: { default: true }".to_string());
            }
        }

        // Add $$legacy: true in legacy mode when no other user-defined props exist
        // and the component has bind:this (needs lifecycle tracking)
        if !self.runes_mode && props.is_empty() && has_bind_this {
            props.push("$$legacy: true".to_string());
        }

        // Format props — multi-line when any prop is complex or there are many props
        let has_complex = props.iter().any(|p| p.contains('\n') || p.contains("children:"));
        let props_str = if props.is_empty() {
            "{}".to_string()
        } else if has_complex || props.len() > 4 {
            let mut parts = String::from("{\n");
            for (i, prop) in props.iter().enumerate() {
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

        if has_bind_this {
            if let Some(bind_expr) = bind_this_expr {
                body.push_str(&format!(
                    "$.bind_this({name}({anchor_var}, {props_str}), ($$value) => {bind_expr} = $$value, () => {bind_expr});\n"
                ));
            }
        } else {
            body.push_str(&format!("{name}({anchor_var}, {props_str});\n"));
        }

        Some(())
    }

    /// Compile children of a component into a function body (client-side).
    fn compile_component_children_client(
        &mut self,
        fragment: &Fragment,
        _source: &str,
    ) -> Option<String> {
        let children: Vec<&Node> = fragment.nodes.iter()
            .filter(|n| !is_whitespace_text(n))
            .collect();

        if children.is_empty() {
            return None;
        }

        let mut body = String::new();

        // Check if all children are text/expression nodes
        let all_text_expr = children.iter().all(|n| matches!(n, Node::Text(_) | Node::ExpressionTag(_)));
        if all_text_expr {
            // Simple text/expression children → $.next(), $.text(), template_effect
            body.push_str("$.next();\n\nvar text = $.text();\n\n");

            // Build template string from children
            // First, collect raw parts with their types
            let mut raw_parts: Vec<(bool, String)> = Vec::new(); // (is_expr, text)
            for child in &children {
                match child {
                    Node::Text(t) => {
                        raw_parts.push((false, t.data.to_string()));
                    }
                    Node::ExpressionTag(tag) => {
                        if let Some(expr) = tag.expression.render() {
                            let expr = rewrite_state_accesses(&expr, &self.state_bindings);
                            raw_parts.push((true, format!("${{{expr} ?? ''}}")));
                        }
                    }
                    _ => {}
                }
            }
            // Combine: collapse whitespace in text nodes, preserving spacing
            let mut combined = String::new();
            for (is_expr, part) in &raw_parts {
                if *is_expr {
                    combined.push_str(part);
                } else {
                    // Collapse whitespace: replace newlines/tabs with spaces, then collapse runs
                    let collapsed = part.chars()
                        .map(|c| if c == '\n' || c == '\r' || c == '\t' { ' ' } else { c })
                        .collect::<String>();
                    let collapsed = collapse_spaces(&collapsed);
                    combined.push_str(&collapsed);
                }
            }
            // Trim leading/trailing whitespace from the combined result
            let template_str = combined.trim().to_string();
            body.push_str(&format!("$.template_effect(() => $.set_text(text, `{template_str}`));\n"));
            body.push_str("$.append($$anchor, text);\n");
            return Some(body);
        }

        // TODO: handle more complex children
        None
    }

    /// Compile `{#each expr as item, index}...{/each}`
    fn compile_each_block(
        &mut self,
        each: &EachBlock,
        anchor_var: &str,
        source: &str,
        body: &mut String,
    ) -> Option<()> {
        // Get expression — prefer source text to preserve formatting
        let raw_expr = render_expression_from_source(&each.expression)
            .or_else(|| each.expression.render())?;

        // Check for await in expression → use $.async() wrapper
        if raw_expr.contains("await ") {
            return self.compile_async_each_block(each, anchor_var, source, body, &raw_expr);
        }

        // For `{#each expr, index}` (no `as` clause), the expression may include the index
        // as a SequenceExpression. We need to extract: collection expression and index name.
        let (expr, inferred_index) = if !each.has_as_clause {
            // Check if the OXC expression is a SequenceExpression
            if let Some(oxc_expr) = each.expression.oxc_expression() {
                if let OxcExpression::SequenceExpression(seq) = oxc_expr {
                    if seq.expressions.len() == 2 {
                        // Last element is the index variable
                        if let OxcExpression::Identifier(id) = &seq.expressions[1] {
                            let idx_name = id.name.to_string();
                            // Strip the index from the raw expression text
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
                // Fallback: use declared index
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

        // Context variable name
        let context_name = each.context.as_ref()
            .and_then(|c| c.render())
            .unwrap_or_else(|| "$$item".to_string());

        // Index variable — use parser's index field or inferred from SequenceExpression
        let index_name = each.index.as_deref()
            .map(|s| s.to_string())
            .or(inferred_index);

        // Build callback parameters
        let mut params = vec!["$$anchor".to_string()];
        params.push(context_name.clone());
        if let Some(ref idx) = index_name {
            params.push(idx.clone());
        }

        // Compile each body
        let mut each_body = String::new();
        let body_children: Vec<&Node> = each.body.nodes.iter()
            .filter(|n| !is_whitespace_text(n))
            .collect();

        if body_children.len() == 1 {
            if let Node::RegularElement(el) = body_children[0] {
                // Single element in each body — create a template for it
                // In each body, expressions reference callback params (non-reactive)
                // so if there are expressions, emit empty element for textContent
                let has_exprs = el.fragment.nodes.iter().any(|n| matches!(n, Node::ExpressionTag(_)));
                let mut template_html = String::new();
                if has_exprs {
                    // Empty element — textContent will be set
                    template_html.push_str(&format!("<{0}></{0}>", el.name));
                } else {
                    self.serialize_element_to_template(el, &mut template_html, source);
                }
                let template_name = self.add_template(template_html, 0);
                let el_var = self.var_counter.next(&el.name);
                each_body.push_str(&format!("var {el_var} = {template_name}();\n"));
                // Save deferred effects — each block has its own scope
                let saved_effects = std::mem::take(&mut self.deferred_effects);
                self.compile_element_children_nonreactive(el, &el_var, source, &mut each_body)?;
                let had_attrs = self.compile_element_attributes(el, &el_var, source, &mut each_body);
                if had_attrs {
                    each_body.push('\n');
                }
                // Flush effects generated within this each block
                let flushed = std::mem::take(&mut self.deferred_effects);
                let had_effects = !flushed.is_empty();
                for effect in flushed {
                    each_body.push_str(&effect);
                }
                // Restore parent scope's deferred effects
                self.deferred_effects = saved_effects;
                if had_effects {
                    each_body.push('\n');
                }
                each_body.push_str(&format!("$.append($$anchor, {el_var});\n"));
            } else if let Node::ExpressionTag(tag) = body_children[0] {
                // Single expression in each body
                if let Some(expr_text) = tag.expression.render() {
                    each_body.push_str("$.next();\n\nvar text = $.text();\n\n");
                    each_body.push_str(&format!(
                        "$.template_effect(() => $.set_text(text, `${{{}}}`);\n",
                        format!("{expr_text} ?? ''")
                    ));
                    each_body.push_str("$.append($$anchor, text);\n");
                }
            } else {
                // Other single-node body types (Component, blocks, etc.)
                // Use compile_block_body_as_closure for these
                if let Some(inner) = self.compile_block_body_as_closure(&each.body, source) {
                    each_body.push_str(&inner);
                } else {
                    return None;
                }
            }
        } else if body_children.len() > 1 {
            // Multiple children — check if they're all text/expression (inline template)
            let has_elements = body_children.iter().any(|n| matches!(n, Node::RegularElement(_)));
            let has_const_tags = body_children.iter().any(|n| matches!(n, Node::ConstTag(_)));
            if !has_elements {
                // Text and expression tags only — build template string with constant folding
                each_body.push_str("$.next();\n\nvar text = $.text();\n\n");
                let template_str = build_template_string_with_folding(&body_children);
                each_body.push_str(&format!(
                    "$.template_effect(() => $.set_text(text, `{template_str}`));\n"
                ));
                each_body.push_str("$.append($$anchor, text);\n");
            } else if has_const_tags {
                // Has @const declarations + elements — emit const declarations first,
                // then handle the element(s) like a single element case
                for child in &body_children {
                    if let Node::ConstTag(const_tag) = child {
                        if let Some(decl_text) = const_tag.declaration.render() {
                            // render() may already include 'const' prefix
                            if decl_text.starts_with("const ") || decl_text.starts_with("let ") || decl_text.starts_with("var ") {
                                each_body.push_str(&format!("{decl_text};\n"));
                            } else {
                                each_body.push_str(&format!("const {decl_text};\n"));
                            }
                        }
                    }
                }
                // Find the single element
                let elements: Vec<&&Node> = body_children.iter().filter(|n| matches!(n, Node::RegularElement(_))).collect();
                if elements.len() == 1 {
                    if let Node::RegularElement(el) = elements[0] {
                        let has_exprs = el.fragment.nodes.iter().any(|n| matches!(n, Node::ExpressionTag(_)));
                        let mut template_html = String::new();
                        if has_exprs {
                            template_html.push_str(&format!("<{0}></{0}>", el.name));
                        } else {
                            self.serialize_element_to_template(el, &mut template_html, source);
                        }
                        let template_name = self.add_template(template_html, 0);
                        let el_var = self.var_counter.next(&el.name);
                        each_body.push_str(&format!("var {el_var} = {template_name}();\n"));
                        let saved_effects = std::mem::take(&mut self.deferred_effects);
                        self.compile_element_children_nonreactive(el, &el_var, source, &mut each_body)?;
                        let had_attrs = self.compile_element_attributes(el, &el_var, source, &mut each_body);
                        if had_attrs {
                            each_body.push('\n');
                        }
                        let flushed = std::mem::take(&mut self.deferred_effects);
                        let had_effects = !flushed.is_empty();
                        for effect in flushed {
                            each_body.push_str(&effect);
                        }
                        self.deferred_effects = saved_effects;
                        if had_effects {
                            each_body.push('\n');
                        }
                        each_body.push_str(&format!("$.append($$anchor, {el_var});\n"));
                    }
                } else {
                    // Use compile_block_body_as_closure for complex cases
                    if let Some(inner) = self.compile_block_body_as_closure(&each.body, source) {
                        each_body.push_str(&inner);
                    }
                }
            } else {
                // Complex multi-element body — use compile_block_body_as_closure
                if let Some(inner) = self.compile_block_body_as_closure(&each.body, source) {
                    each_body.push_str(&inner);
                } else {
                    return None;
                }
            }
        }

        // Indent the each body
        let indented_body: String = each_body.lines()
            .map(|line| if line.is_empty() { String::new() } else { format!("\t{line}") })
            .collect::<Vec<_>>()
            .join("\n");

        // If expr starts with '{', wrap in parens so arrow function returns object literal
        let expr_in_arrow = if expr.starts_with('{') {
            format!("({expr})")
        } else {
            expr.clone()
        };

        body.push_str(&format!(
            "$.each({anchor_var}, 0, () => {expr_in_arrow}, $.index, ({}) => {{\n{indented_body}\n}});\n",
            params.join(", ")
        ));

        Some(())
    }

    /// Compile `{#each await expr as item}...{/each}` with `$.async()` wrapper.
    fn compile_async_each_block(
        &mut self,
        each: &EachBlock,
        anchor_var: &str,
        source: &str,
        body: &mut String,
        raw_expr: &str,
    ) -> Option<()> {
        // Extract await expression: `await expr` → `expr`
        let await_expr = raw_expr.trim().strip_prefix("await ").unwrap_or(raw_expr.trim());

        // Get context (item) name
        let context_name = each.context.as_ref()
            .and_then(|c| c.render())
            .unwrap_or_else(|| "$$item".to_string());

        let has_fallback = each.fallback.is_some();
        // Flag 17 = no fallback, 16 = has fallback
        let flags = if has_fallback { 16 } else { 17 };

        // Build each body closure content
        let each_body = self.compile_async_each_branch_body(&each.body, source, "text");

        // Build fallback closure content if present
        let fallback_body = each.fallback.as_ref().map(|fb| {
            self.compile_async_each_branch_body(fb, source, "text_1")
        });

        // Build $.async() wrapping $.each()
        body.push_str(&format!(
            "$.async({anchor_var}, [], [() => {await_expr}], ({anchor_var}, $$collection) => {{\n"
        ));

        if has_fallback {
            // Multi-line $.each() with each arg on its own line
            let indent2 = "\t\t";
            let indent3 = "\t\t\t";
            body.push_str(&format!("\t$.each(\n"));
            body.push_str(&format!("{indent2}{anchor_var},\n"));
            body.push_str(&format!("{indent2}{flags},\n"));
            body.push_str(&format!("{indent2}() => $.get($$collection),\n"));
            body.push_str(&format!("{indent2}$.index,\n"));

            // Main body closure
            body.push_str(&format!("{indent2}($$anchor, {context_name}) => {{\n"));
            for line in each_body.lines() {
                if line.is_empty() {
                    body.push('\n');
                } else {
                    body.push_str(&format!("{indent3}{line}\n"));
                }
            }
            body.push_str(&format!("{indent2}}},\n"));

            // Fallback closure
            if let Some(fb_body) = &fallback_body {
                body.push_str(&format!("{indent2}($$anchor) => {{\n"));
                for line in fb_body.lines() {
                    if line.is_empty() {
                        body.push('\n');
                    } else {
                        body.push_str(&format!("{indent3}{line}\n"));
                    }
                }
                body.push_str(&format!("{indent2}}}\n"));
            }

            body.push_str("\t);\n");
        } else {
            // Single-line $.each() args (compact)
            let indented_each: String = each_body.lines()
                .map(|line| if line.is_empty() { String::new() } else { format!("\t\t{line}") })
                .collect::<Vec<_>>()
                .join("\n");

            body.push_str(&format!(
                "\t$.each({anchor_var}, {flags}, () => $.get($$collection), $.index, ($$anchor, {context_name}) => {{\n{indented_each}\n\t}});\n"
            ));
        }

        body.push_str("});\n");

        Some(())
    }

    /// Compile the body of an async each branch (main or fallback).
    /// Returns the body lines (not indented).
    fn compile_async_each_branch_body(
        &mut self,
        fragment: &Fragment,
        source: &str,
        text_var: &str,
    ) -> String {
        let body_children: Vec<&Node> = fragment.nodes.iter()
            .filter(|n| !is_whitespace_text(n))
            .collect();

        let mut result = String::new();

        if body_children.len() == 1 {
            if let Node::ExpressionTag(tag) = body_children[0] {
                if let Some(expr_text) = tag.expression.render() {
                    result.push_str(&format!("$.next();\n\nvar {text_var} = $.text();\n\n"));
                    // Handle {await expr} → $.template_effect with lazy dep
                    if let Some(inner) = expr_text.trim().strip_prefix("await ") {
                        // Simple identifier → $.get(id), complex expr → just expr
                        let lazy = if is_simple_identifier(inner) {
                            format!("$.get({inner})")
                        } else {
                            inner.to_string()
                        };
                        result.push_str(&format!(
                            "$.template_effect(($0) => $.set_text({text_var}, $0), void 0, [() => {lazy}]);\n"
                        ));
                    } else {
                        result.push_str(&format!(
                            "$.template_effect(() => $.set_text({text_var}, {expr_text}));\n"
                        ));
                    }
                    result.push_str(&format!("$.append($$anchor, {text_var});\n"));
                    return result;
                }
            }
        }

        // Fallback: compile fragment generically
        self.compile_if_branch_body(fragment, source)
    }

    /// Compile `{#if test}...{:else}...{/if}`
    fn compile_if_block(
        &mut self,
        if_block: &IfBlock,
        anchor_var: &str,
        source: &str,
        body: &mut String,
    ) -> Option<()> {
        // Check if the if-block test contains await → use $.async() wrapper
        let test_text = if_block.test.render()?;
        if test_text.contains("await ") {
            return self.compile_async_if_block(if_block, anchor_var, source, body);
        }

        // Collect all branches: (test_expr, fragment) pairs, plus optional else fragment
        let mut branches: Vec<(String, &Fragment)> = Vec::new();
        let mut else_fragment: Option<&Fragment> = None;

        // Walk the if/else-if chain
        let mut current = if_block;
        loop {
            let test = current.test.render()?;
            branches.push((test, &current.consequent));

            match &current.alternate {
                Some(alt) => match alt.as_ref() {
                    crate::ast::modern::Alternate::IfBlock(inner_if) => {
                        current = inner_if;
                    }
                    crate::ast::modern::Alternate::Fragment(frag) => {
                        // Check if this Fragment wraps an {:else if} IfBlock
                        if let Some(elseif_block) = extract_elseif_from_fragment(frag) {
                            current = elseif_block;
                        } else {
                            else_fragment = Some(frag);
                            break;
                        }
                    }
                },
                None => break,
            }
        }

        // Check if any branch test references async_vars or state_vars → needs $.async() wrapping
        let needs_async_wrap = if let Some(ref info) = self.async_run_info {
            let all_vars: Vec<&String> = info.state_vars.iter().chain(info.async_vars.iter()).collect();
            branches.iter().any(|(test, _)| {
                all_vars.iter().any(|v| contains_word(test, v))
            })
        } else {
            false
        };

        // Both paths use same indent (1 tab for closure body, 2 tabs for inner)
        let ind = "\t";
        let ind2 = "\t\t";

        // Open block or $.async() wrapper
        if needs_async_wrap {
            let promise_var = self.async_run_info.as_ref().map(|i| &i.promise_var).cloned().unwrap_or_else(|| "$$promises".to_string());
            body.push_str(&format!("$.async({anchor_var}, [{promise_var}[0]], void 0, ({anchor_var}) => {{\n"));
        } else {
            body.push_str("{\n");
        }

        let mut branch_names = Vec::new();
        let mut derived_vars: Vec<(usize, String, String)> = Vec::new(); // (branch_idx, var_name, expr)

        for (_i, (_test, fragment)) in branches.iter().enumerate() {
            let branch_name = self.var_counter.next("consequent");

            body.push_str(&format!("{ind}var {branch_name} = ($$anchor) => {{\n"));

            let branch_body = self.compile_if_branch_body(fragment, source);
            for line in branch_body.lines() {
                if line.is_empty() {
                    body.push('\n');
                } else {
                    body.push_str(ind2);
                    body.push_str(line);
                    body.push('\n');
                }
            }

            body.push_str(&format!("{ind}}};\n\n"));
            branch_names.push(branch_name);
        }

        // Emit $.derived() for complex test expressions (containing function calls)
        for (i, (test, _)) in branches.iter().enumerate() {
            if test_needs_derived(test) {
                let d_var = self.var_counter.next("d");
                body.push_str(&format!("{ind}var {d_var} = $.derived(() => {test});\n\n"));
                derived_vars.push((i, d_var, test.clone()));
            }
        }

        // Generate else branch if present
        let else_name = if let Some(frag) = else_fragment {
            let name = self.var_counter.next("alternate");
            body.push_str(&format!("{ind}var {name} = ($$anchor) => {{\n"));
            let branch_body = self.compile_if_branch_body(frag, source);
            for line in branch_body.lines() {
                if line.is_empty() {
                    body.push('\n');
                } else {
                    body.push_str(ind2);
                    body.push_str(line);
                    body.push('\n');
                }
            }
            body.push_str(&format!("{ind}}};\n\n"));
            Some(name)
        } else {
            None
        };

        // Build $.if() call — all branches on a single line
        // For async-wrapped blocks, apply $.get() to async_var references in tests
        let async_vars_for_get: Vec<String> = if needs_async_wrap {
            self.async_run_info.as_ref().map(|i| i.async_vars.clone()).unwrap_or_default()
        } else {
            Vec::new()
        };

        let mut render_parts: Vec<String> = Vec::new();
        for (i, (test, _)) in branches.iter().enumerate() {
            let keyword = if i == 0 { "if" } else { " else if" };
            let idx_arg = if i > 0 { format!(", {i}") } else { String::new() };
            let test_expr = if let Some((_, d_var, _)) = derived_vars.iter().find(|(idx, _, _)| *idx == i) {
                format!("$.get({d_var})")
            } else {
                // Apply $.get() wrapping for async vars
                let mut expr = test.clone();
                for av in &async_vars_for_get {
                    if contains_word(&expr, av) {
                        expr = replace_var_with_get(&expr, av);
                    }
                }
                expr
            };
            render_parts.push(format!("{keyword} ({test_expr}) $$render({}{idx_arg});", branch_names[i]));
        }
        if let Some(ref else_name) = else_name {
            render_parts.push(format!(" else $$render({else_name}, -1);"));
        }
        let render_line = render_parts.join("");
        body.push_str(&format!("{ind}$.if({anchor_var}, ($$render) => {{\n"));
        body.push_str(&format!("{ind}\t{render_line}\n"));
        body.push_str(&format!("{ind}}});\n"));

        if needs_async_wrap {
            body.push_str("});\n");
        } else {
            body.push_str("}\n");
        }

        Some(())
    }

    /// Compile an if-block whose test contains `await` using `$.async()` wrapper.
    /// When multiple branches have `await` in their tests, the chain is split at each
    /// await boundary, nesting the remaining chain inside an alternate closure.
    fn compile_async_if_block(
        &mut self,
        if_block: &IfBlock,
        anchor_var: &str,
        source: &str,
        body: &mut String,
    ) -> Option<()> {
        // Collect all branches with their await status
        let mut branches: Vec<(String, &Fragment, bool)> = Vec::new(); // (test, frag, has_await)
        let mut else_fragment: Option<&Fragment> = None;

        let mut current = if_block;
        loop {
            let t = current.test.render()?;
            let has_await = t.contains("await ");
            branches.push((t, &current.consequent, has_await));
            match &current.alternate {
                Some(alt) => match alt.as_ref() {
                    crate::ast::modern::Alternate::IfBlock(inner_if) => {
                        current = inner_if;
                    }
                    crate::ast::modern::Alternate::Fragment(frag) => {
                        if let Some(elseif_block) = extract_elseif_from_fragment(frag) {
                            current = elseif_block;
                        } else {
                            else_fragment = Some(frag);
                            break;
                        }
                    }
                },
                None => break,
            }
        }

        let segment_output = self.compile_async_if_segment(&branches, else_fragment, anchor_var, source, false);
        body.push_str(&segment_output);
        Some(())
    }

    /// Compile one segment of an async if-chain. Always generates at zero-relative indent.
    /// A segment starts with an `await` branch and includes all following non-await branches.
    /// If another `await` branch follows, it becomes a nested `$.async()` inside an alternate.
    fn compile_async_if_segment(
        &mut self,
        branches: &[(String, &Fragment, bool)],
        else_fragment: Option<&Fragment>,
        anchor_var: &str,
        source: &str,
        is_nested: bool,
    ) -> String {
        let mut body = String::new();

        // First branch must have await
        let first_test = &branches[0].0;
        let (async_expr_list, promise_deps) = self.build_async_expr_and_deps(first_test);

        body.push_str(&format!(
            "$.async({anchor_var}, {promise_deps}, {async_expr_list}, ({anchor_var}, $$condition) => {{\n"
        ));

        // Find the split point: next await branch after the first one
        let split_at = branches[1..].iter().position(|(_, _, has_await)| *has_await).map(|i| i + 1);
        let this_level_count = split_at.unwrap_or(branches.len());

        // Generate branch closures for this level
        let mut branch_names = Vec::new();
        for i in 0..this_level_count {
            let (_test, fragment, _) = &branches[i];
            let branch_name = self.var_counter.next("consequent");

            body.push_str(&format!("\tvar {branch_name} = ($$anchor) => {{\n"));
            let branch_body = self.compile_async_branch_body(fragment, source);
            for line in branch_body.lines() {
                if line.is_empty() { body.push('\n'); }
                else { body.push_str(&format!("\t\t{line}\n")); }
            }
            body.push_str("\t};\n\n");
            branch_names.push(branch_name);
        }

        // Handle remaining branches: either nested async or else
        // For correct naming order, compile nested body FIRST (to allocate inner names),
        // then allocate the outer alternate name.
        let else_name = if let Some(split_pos) = split_at {
            let remaining = &branches[split_pos..];

            // Compile nested segment first (allocates inner names like alternate_1)
            let frag_var = self.var_counter.next("fragment");
            let inner_node = self.var_counter.next("node");
            let nested_output = self.compile_async_if_segment(remaining, else_fragment, &inner_node, source, true);

            // Now allocate the outer alternate name (gets higher index)
            let alt_name = self.var_counter.next("alternate");

            body.push_str(&format!("\tvar {alt_name} = ($$anchor) => {{\n"));
            body.push_str(&format!("\t\tvar {frag_var} = $.comment();\n"));
            body.push_str(&format!("\t\tvar {inner_node} = $.first_child({frag_var});\n\n"));

            // Write nested body indented by 2 tabs
            for line in nested_output.lines() {
                if line.is_empty() { body.push('\n'); }
                else { body.push_str(&format!("\t\t{line}\n")); }
            }

            body.push_str(&format!("\n\t\t$.append($$anchor, {frag_var});\n"));
            body.push_str("\t};\n\n");
            Some(alt_name)
        } else if let Some(frag) = else_fragment {
            let name = self.var_counter.next("alternate");
            body.push_str(&format!("\tvar {name} = ($$anchor) => {{\n"));
            let branch_body = self.compile_async_branch_body(frag, source);
            for line in branch_body.lines() {
                if line.is_empty() { body.push('\n'); }
                else { body.push_str(&format!("\t\t{line}\n")); }
            }
            body.push_str("\t};\n\n");
            Some(name)
        } else {
            None
        };

        // Build $.if() call
        if is_nested {
            // Nested: multi-line $.if() with `true` third arg
            let render_line = self.build_async_render_line(&branches[..this_level_count], &branch_names, &else_name);
            body.push_str(&format!("\t$.if(\n"));
            body.push_str(&format!("\t\t{anchor_var},\n"));
            body.push_str(&format!("\t\t($$render) => {{\n"));
            body.push_str(&format!("\t\t\t{render_line}\n"));
            body.push_str(&format!("\t\t}},\n"));
            body.push_str(&format!("\t\ttrue\n"));
            body.push_str(&format!("\t);\n"));
        } else {
            // Top level: single-line $.if()
            let render_line = self.build_async_render_line(&branches[..this_level_count], &branch_names, &else_name);
            body.push_str(&format!("\t$.if({anchor_var}, ($$render) => {{\n"));
            body.push_str(&format!("\t\t{render_line}\n"));
            body.push_str("\t});\n");
        }

        // Close $.async() wrapper
        body.push_str("});\n");
        body
    }

    /// Build the async expression list and promise deps for a $.async() call.
    /// Returns (expr_list, promise_deps).
    fn build_async_expr_and_deps(&self, test: &str) -> (String, String) {
        // Check if this is a simple `await expr` or `await expr OP rest` (compound)
        let (await_target, is_compound) = if let Some(rest) = test.strip_prefix("await ") {
            // Starts with `await ` — check if there's more after the expression
            // Find end of the awaited expression (before operator)
            let end = rest.find(|c: char| c == ' ' || c == '>' || c == '<' || c == '=' || c == '!' || c == '+' || c == '-' || c == '*' || c == '/' || c == '%' || c == '&' || c == '|').unwrap_or(rest.len());
            let target = rest[..end].trim().to_string();
            let is_compound = end < rest.len();
            (target, is_compound)
        } else {
            // Contains `await` somewhere inside
            let pos = test.find("await ").unwrap_or(0);
            let after = &test[pos + 6..];
            let end = after.find(|c: char| c == ' ' || c == '>' || c == '<').unwrap_or(after.len());
            (after[..end].trim().to_string(), true)
        };

        // Compute promise deps: check if the expression references state/async vars
        let promise_deps = if let Some(ref info) = self.async_run_info {
            let all_vars: Vec<&String> = info.state_vars.iter().chain(info.async_vars.iter()).collect();
            let refs_promise_var = all_vars.iter().any(|v| {
                contains_word(&await_target, v) || contains_word(test, v)
            });
            if refs_promise_var {
                let last_idx = info.run_slot_count.saturating_sub(1);
                let pvar = &info.promise_var;
                format!("[{pvar}[{last_idx}]]")
            } else {
                "[]".to_string()
            }
        } else {
            "[]".to_string()
        };

        let expr_list = if is_compound {
            // Compound: `[async () => (await $.save(target))() OP rest]`
            let transformed = transform_await_with_save(test);
            format!("[async () => {transformed}]")
        } else {
            format!("[() => {await_target}]")
        };

        (expr_list, promise_deps)
    }

    /// Build the render line for $.if() inside an async block
    fn build_async_render_line(
        &self,
        branches: &[(String, &Fragment, bool)],
        branch_names: &[String],
        else_name: &Option<String>,
    ) -> String {
        let mut parts: Vec<String> = Vec::new();
        for (i, (test, _, has_await)) in branches.iter().enumerate() {
            let keyword = if i == 0 { "if" } else { " else if" };
            let transformed_test = if *has_await {
                "$.get($$condition)".to_string()
            } else {
                test.clone()
            };
            let idx_arg = if i > 0 { format!(", {i}") } else { String::new() };
            parts.push(format!("{keyword} ({transformed_test}) $$render({}{idx_arg});", branch_names[i]));
        }
        if let Some(name) = else_name {
            parts.push(format!(" else $$render({name}, -1);"));
        }
        parts.join("")
    }

    /// Compile a branch body for async if-blocks.
    /// Handles `{await expr}` expression tags with `$.template_effect` lazy deps.
    fn compile_async_branch_body(&mut self, fragment: &Fragment, source: &str) -> String {
        let significant: Vec<&Node> = fragment.nodes.iter()
            .filter(|n| !is_whitespace_text(n))
            .collect();

        if significant.is_empty() {
            return String::new();
        }

        // Check if all significant nodes are expression tags with await
        let all_await_exprs: Vec<&crate::ast::modern::ExpressionTag> = significant.iter()
            .filter_map(|n| match n {
                Node::ExpressionTag(tag) => Some(tag),
                _ => None,
            })
            .collect();

        if all_await_exprs.len() == significant.len() && !all_await_exprs.is_empty() {
            // All nodes are expression tags — generate $.text() + $.template_effect with lazy deps
            let mut out = String::new();
            for (i, tag) in all_await_exprs.iter().enumerate() {
                if let Some(expr_text) = tag.expression.render() {
                    let text_var = self.var_counter.next("text");
                    let suffix = if i > 0 { format!("_{}", i) } else { String::new() };
                    let text_var_name = if i > 0 { format!("text{suffix}") } else { "text".to_string() };
                    // Use the already-generated var name from var_counter
                    out.push_str(&format!("var {text_var} = $.text();\n\n"));

                    // Extract the expression: if it's `await expr`, make it lazy `() => expr`
                    let (lazy_expr, is_await) = if let Some(inner) = expr_text.trim().strip_prefix("await ") {
                        (inner.to_string(), true)
                    } else {
                        (expr_text.clone(), false)
                    };

                    if is_await {
                        out.push_str(&format!(
                            "$.template_effect(($0) => $.set_text({text_var}, $0), void 0, [() => {lazy_expr}]);\n"
                        ));
                    } else {
                        out.push_str(&format!(
                            "$.template_effect(() => $.set_text({text_var}, {expr_text}));\n"
                        ));
                    }
                    out.push_str(&format!("$.append($$anchor, {text_var});\n"));
                }
            }
            return out;
        }

        // Fallback: use normal compilation
        self.compile_if_branch_body(fragment, source)
    }

    /// Compile the body of an {#if}/{:else} branch into code that gets placed inside
    /// the branch closure `($$anchor) => { ... }`.
    fn compile_if_branch_body(&mut self, fragment: &Fragment, source: &str) -> String {
        let significant: Vec<&Node> = fragment
            .nodes
            .iter()
            .filter(|n| !is_whitespace_text(n))
            .collect();

        if significant.is_empty() {
            return String::new();
        }

        // Inside customizable_select, a single render tag can be called directly
        if self.in_select_context && significant.len() == 1 {
            if let Node::RenderTag(render) = significant[0] {
                let mut body = String::new();
                self.compile_render_tag(render, "$$anchor", source, &mut body);
                return body;
            }
        }

        // Check for @const tags with await — needs client $.run() pattern
        if let Some(const_run_code) = self.compile_client_const_run(fragment, source) {
            return const_run_code;
        }

        // Try to compile as a full fragment (handles template hoisting, etc.)
        if let Some(fragment_code) = self.compile_fragment(fragment, source) {
            return fragment_code;
        }

        // Fallback: handle text-only branches → $.text('content') + $.append()
        if significant.len() == 1 {
            if let Node::Text(text) = significant[0] {
                let data = text.data.trim();
                if !data.is_empty() {
                    let text_var = self.var_counter.next("text");
                    let mut result = format!("var {text_var} = $.text('{data}');\n\n");
                    result.push_str(&format!("$.append($$anchor, {text_var});\n"));
                    return result;
                }
            }
        }

        // Fallback for static element content — create template and append
        let template_html = self.serialize_fragment_to_static_html(fragment, source);
        if !template_html.is_empty() {
            let root_name = self.add_template(template_html, 0);
            let var_name = self.var_counter.next(&sanitize_var_name(&self.guess_first_element_name(fragment)));
            let mut result = format!("var {var_name} = {root_name}();\n\n");
            result.push_str(&format!("$.append($$anchor, {var_name});\n"));
            return result;
        }

        String::new()
    }

    /// Compile @const tags inside a client if-block branch into $.run() pattern.
    /// Returns None if no @const tags are present.
    fn compile_client_const_run(&mut self, fragment: &Fragment, source: &str) -> Option<String> {
        // Collect @const declarations
        let mut const_entries: Vec<(String, String, bool)> = Vec::new(); // (name, init, has_top_level_await)
        for node in &fragment.nodes {
            if let Node::ConstTag(ct) = node {
                if let Some(decl_text) = ct.declaration.render() {
                    if let Some((name, init)) = parse_const_declaration(&decl_text) {
                        let has_top_level_await = init.contains("await ") && !init.trim_start().starts_with("(async ");
                        const_entries.push((name, init, has_top_level_await));
                    }
                }
            }
        }

        if const_entries.is_empty() {
            return None;
        }

        let mut result = String::new();

        // Collect async var names (for $.get() replacement)
        let async_var_names: Vec<String> = const_entries.iter()
            .filter(|(_, _, has_await)| *has_await)
            .map(|(name, _, _)| name.clone())
            .collect();

        // Emit let declarations
        for (name, _, _) in &const_entries {
            result.push_str(&format!("let {name};\n"));
        }
        result.push('\n');

        // Build $.run() array
        result.push_str("var promises = $.run([\n");
        for (i, (name, init, has_await)) in const_entries.iter().enumerate() {
            if *has_await {
                // Transform await expressions: `await X` → `(await $.save(X))()`
                let transformed_init = transform_await_in_expr(init);
                // Wrap in $.async_derived, then in (await $.save(...))()
                // Check if inner has nested await to decide if async_derived callback needs async
                let inner_has_await = transformed_init.contains("await ");
                let async_kw = if inner_has_await { "async " } else { "" };
                result.push_str(&format!(
                    "\tasync () => {name} = (await $.save($.async_derived({async_kw}() => {transformed_init})))()"
                ));
            } else {
                // Sync const — wrap in $.derived()
                // Replace refs to async vars with $.get()
                let mut transformed = init.clone();
                for async_var in &async_var_names {
                    transformed = replace_var_with_get(&transformed, async_var);
                }
                // Re-indent to remove source indentation
                let transformed = reindent_block(transformed.trim());
                // Handle multi-line IIFE patterns
                let init_lines: Vec<&str> = transformed.lines().collect();
                if init_lines.len() > 1 {
                    result.push_str(&format!("\t() => {name} = $.derived(() => {}", init_lines[0]));
                    for line in &init_lines[1..] {
                        result.push_str(&format!("\n\t{line}"));
                    }
                    result.push(')');
                } else {
                    result.push_str(&format!("\t() => {name} = $.derived(() => {transformed})"));
                }
            }

            let is_multiline = init.lines().count() > 1 && !*has_await;
            if i < const_entries.len() - 1 {
                result.push_str(",\n");
                // Add blank line after multi-line closure if next is also multi-line
                if is_multiline {
                    let next_is_multiline = const_entries.get(i + 1)
                        .map(|(_, next_init, next_await)| next_init.lines().count() > 1 && !*next_await)
                        .unwrap_or(false);
                    if next_is_multiline {
                        result.push('\n');
                    }
                }
            }
        }
        result.push_str("\n]);\n");

        // Non-const nodes (template content)
        let non_const_nodes: Vec<&Node> = fragment.nodes.iter()
            .filter(|n| !matches!(n, Node::ConstTag(_)) && !is_whitespace_text(n))
            .collect();

        if !non_const_nodes.is_empty() {
            result.push('\n');
            // Build a sub-fragment without @const nodes
            let sub_fragment = Fragment {
                nodes: fragment.nodes.iter()
                    .filter(|n| !matches!(n, Node::ConstTag(_)))
                    .cloned()
                    .collect(),
                ..fragment.clone()
            };

            // Save current async_run_info and set up one for const context
            // All const vars are signals/deriveds needing $.get() wrapping
            let all_const_names: Vec<String> = const_entries.iter().map(|(n, _, _)| n.clone()).collect();
            let old_info = self.async_run_info.take();
            self.async_run_info = Some(ServerAsyncRunInfo {
                run_slot_count: const_entries.len(),
                async_vars: all_const_names,
                state_vars: Vec::new(),
                has_sync_derived: false,
                promise_var: "promises".to_string(),
            });

            if let Some(fragment_code) = self.compile_fragment(&sub_fragment, source) {
                // The fragment code should use $.child(p, true) and $.template_effect with promises[N]
                result.push_str(&fragment_code);
            }

            self.async_run_info = old_info;
        }

        Some(result)
    }

    /// Compile `{#await expr}...{:then val}...{:catch err}...{/await}` for client
    fn compile_await_block(
        &mut self,
        await_block: &crate::ast::modern::AwaitBlock,
        anchor_var: &str,
        source: &str,
        body: &mut String,
    ) -> Option<()> {
        let expr = await_block.expression.render()?;
        let expr = self.maybe_rewrite_state_expr(&expr);

        // pending callback
        let pending_fn = if let Some(ref pending_frag) = await_block.pending {
            let has_content = pending_frag.nodes.iter().any(|n| !is_whitespace_text(n));
            if has_content {
                let branch_body = self.compile_if_branch_body(pending_frag, source);
                if branch_body.is_empty() {
                    "null".to_string()
                } else {
                    let indented: String = branch_body.lines()
                        .map(|l| if l.is_empty() { String::new() } else { format!("\t{l}") })
                        .collect::<Vec<_>>()
                        .join("\n");
                    format!("($$anchor) => {{\n{indented}\n}}")
                }
            } else {
                "null".to_string()
            }
        } else {
            "null".to_string()
        };

        // then callback
        let then_fn = if let Some(ref then_frag) = await_block.then {
            let val_name = await_block.value.as_ref()
                .and_then(|v| v.render())
                .unwrap_or_default();
            let has_content = then_frag.nodes.iter().any(|n| !is_whitespace_text(n));
            if has_content {
                let branch_body = self.compile_if_branch_body(then_frag, source);
                let indented: String = branch_body.lines()
                    .map(|l| if l.is_empty() { String::new() } else { format!("\t{l}") })
                    .collect::<Vec<_>>()
                    .join("\n");
                format!("($$anchor, {val_name}) => {{\n{indented}\n}}")
            } else {
                let params = if val_name.is_empty() {
                    "$$anchor".to_string()
                } else {
                    format!("$$anchor, {val_name}")
                };
                format!("({params}) => {{}}")
            }
        } else {
            "null".to_string()
        };

        body.push_str(&format!("$.await({anchor_var}, () => {expr}, {pending_fn}, {then_fn});\n"));
        Some(())
    }

    /// Compile dynamic attributes and event handlers for an element.
    /// Returns true if any dynamic attributes/events were emitted.
    fn compile_element_attributes(
        &mut self,
        element: &RegularElement,
        el_var: &str,
        _source: &str,
        body: &mut String,
    ) -> bool {
        let is_svg = is_svg_element(&element.name);
        let is_custom = is_custom_element(&element.name);
        let mut emitted = false;
        for attr in element.attributes.iter() {
            match attr {
                Attribute::Attribute(attr) => {
                    // Event handlers: onclick={handler} → $.delegated('click', el, handler)
                    if let Some(event_name) = attr.name.strip_prefix("on") {
                        if let Some(handler) = render_attribute_value_dynamic(&attr.value) {
                            let handler = rewrite_state_accesses(&handler, &self.state_bindings);
                            let handler = add_blank_lines_in_arrow_body(&handler);
                            if is_delegatable_event(event_name) {
                                self.delegated_events.insert(event_name.to_string());
                                self.deferred_effects.push(format!(
                                    "$.delegated('{event_name}', {el_var}, {handler});\n"
                                ));
                            } else {
                                self.deferred_effects.push(format!(
                                    "{el_var}.{} = {};\n",
                                    attr.name, handler
                                ));
                            }
                            emitted = true;
                        }
                    }
                    // Dynamic attribute values
                    else if is_dynamic_attribute_value(&attr.value) {
                        if let Some(expr) = render_attribute_value_dynamic(&attr.value) {
                            // Check if expression is impure (contains function call)
                            let is_impure = expr.contains('(') && expr.contains(')');
                            let attr_name_resolved = if is_custom {
                                attr.name.to_string()
                            } else if is_svg {
                                attr.name.to_string()
                            } else {
                                attr.name.to_lowercase()
                            };

                            if is_impure {
                                // Extract dependency: for "y()" → "y", for "fn()" → "fn"
                                let dep = if expr.ends_with("()") {
                                    expr[..expr.len() - 2].to_string()
                                } else {
                                    expr.clone()
                                };
                                self.impure_attr_effects.push(ImpureAttrEffect {
                                    el_var: el_var.to_string(),
                                    attr_name: attr_name_resolved,
                                    dep,
                                    is_custom,
                                });
                            } else {
                                body.push('\n');
                                if is_custom {
                                    body.push_str(&format!(
                                        "$.set_custom_element_data({el_var}, '{}', {expr});\n",
                                        attr.name
                                    ));
                                } else {
                                    body.push_str(&format!(
                                        "$.set_attribute({el_var}, '{attr_name_resolved}', {expr});\n",
                                    ));
                                }
                            }
                            emitted = true;
                        }
                    }
                }
                Attribute::BindDirective(bind) => {
                    if let Some(expr) = bind.expression.render() {
                        // bind:value on input/select/textarea elements
                        if bind.name.as_ref() == "value" {
                            let tag = element.name.as_ref();
                            if tag == "input" || tag == "textarea" {
                                body.push('\n');
                                body.push_str(&format!("$.remove_input_defaults({el_var});\n"));
                            }
                            // Check if expr is a state binding
                            if self.state_bindings.contains::<str>(&expr) {
                                self.deferred_effects.push(format!(
                                    "$.bind_value({el_var}, () => $.get({expr}), ($$value) => $.set({expr}, $$value));\n"
                                ));
                            } else {
                                self.deferred_effects.push(format!(
                                    "$.bind_value({el_var}, () => {expr}, ($$value) => {expr} = $$value);\n"
                                ));
                            }
                            emitted = true;
                        } else if bind.name.as_ref() != "this" {
                            // Other bind directives (bind:checked, etc.)
                            self.deferred_effects.push(format!(
                                "$.bind_{el_var}_{name}();\n",
                                name = bind.name
                            ));
                            emitted = true;
                        }
                    }
                }
                _ => {}
            }
        }
        emitted
    }

    /// Compile `<svelte:element this={tag} />`
    fn compile_svelte_element(
        &mut self,
        el: &SvelteElement,
        anchor_var: &str,
        _source: &str,
        body: &mut String,
    ) -> Option<()> {
        if let Some(ref expr) = el.expression {
            if let Some(tag_expr) = expr.render() {
                body.push_str(&format!("$.element({anchor_var}, {tag_expr}, false);\n"));
            }
        }
        Some(())
    }
}

// ---------------------------------------------------------------------------
// Server template compilation
// ---------------------------------------------------------------------------

fn compile_server_fragment(
    fragment: &Fragment,
    source: &str,
    constant_bindings: &std::collections::HashMap<String, String>,
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
    let first_sig = fragment.nodes.iter().position(|n| is_sig_server(n));
    let last_sig = fragment.nodes.iter().rposition(|n| is_sig_server(n));
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
                    serialize_server_select_element(element, &mut parts, &mut output_lines, source, constant_bindings, &mut server_each_counter)?;
                    // Add closing tag to parts buffer for consolidation
                    // Add <!> anchor if select needs customizable_select pattern
                    if select_needs_fragment_anchor(&element.fragment.nodes) {
                        parts.push_static("<!>");
                    }
                    parts.push_static("</select>");
                } else {
                    serialize_server_element(element, &mut parts, source, constant_bindings)?;
                }
                last_was_comment = false;
                last_was_component = false;
            }
            Node::ExpressionTag(tag) => {
                // Try constant propagation first (known bindings)
                if let Some(value) = try_resolve_constant_binding(&tag.expression, constant_bindings) {
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
                let comp_code = compile_server_component(comp, source)?;
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
                let each_code = compile_server_each_block(each, source)?;
                output_lines.push(each_code);
            }
            Node::IfBlock(if_block) => {
                let flushed = parts.to_template_literal();
                if !flushed.is_empty() {
                    output_lines.push(format!("$$renderer.push(`{flushed}`);\n"));
                    parts = ServerTemplateParts::new();
                }
                let if_code = compile_server_if_block(if_block, source)?;
                output_lines.push(if_code);
            }
            Node::AwaitBlock(await_block) => {
                let flushed = parts.to_template_literal();
                if !flushed.is_empty() {
                    output_lines.push(format!("$$renderer.push(`{flushed}`);\n"));
                    parts = ServerTemplateParts::new();
                }
                let await_code = compile_server_await_block(await_block, source)?;
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
                if let Some(ref expr) = el.expression {
                    if let Some(tag_expr) = expr.render() {
                        output_lines.push(format!("$.element($$renderer, {tag_expr});\n"));
                    }
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

/// Normalize blank lines in server select output.
/// Ensures blank lines between top-level (zero-indent) statements.
fn normalize_server_select_blank_lines(code: &str) -> String {
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

/// Compile a server fragment when the script has async run pattern.
/// Template expressions that reference async-derived vars get wrapped in $$renderer.async().
fn compile_server_fragment_with_script_run(
    fragment: &Fragment,
    source: &str,
    constant_bindings: &std::collections::HashMap<String, String>,
    run_info: &ServerAsyncRunInfo,
) -> Option<String> {
    let mut output_lines: Vec<String> = Vec::new();
    let mut parts = ServerTemplateParts::new();

    let is_sig_server = |n: &Node| !is_whitespace_text(n) && !matches!(n, Node::SnippetBlock(_) | Node::Comment(_));
    let first_sig = fragment.nodes.iter().position(|n| is_sig_server(n));
    let last_sig = fragment.nodes.iter().rposition(|n| is_sig_server(n));
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
                    serialize_server_element(element, &mut parts, source, constant_bindings)?;
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
                    let if_code = compile_server_if_block_with_async_block(if_block, source, run_info)?;
                    output_lines.push(if_code);
                } else {
                    let mut if_code = compile_server_if_block(if_block, source)?;
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
                let each_code = compile_server_each_block(each, source)?;
                output_lines.push(each_code);
            }
            Node::Component(comp) => {
                let flushed = parts.to_template_literal();
                if !flushed.is_empty() {
                    output_lines.push(format!("$$renderer.push(`{flushed}`);\n"));
                    parts = ServerTemplateParts::new();
                }
                let comp_code = compile_server_component(comp, source)?;
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
                return compile_server_fragment(fragment, source, constant_bindings);
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

/// Check if a RegularElement's children reference any async vars
fn element_content_refs_async(element: &RegularElement, run_info: &ServerAsyncRunInfo) -> bool {
    for node in &element.fragment.nodes {
        if let Node::ExpressionTag(tag) = node {
            if let Some(expr_text) = tag.expression.render() {
                for var in &run_info.async_vars {
                    if expr_text.contains(var.as_str()) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Collects parts of a server template literal, separating static text
/// (which needs escaping) from interpolation expressions (which must not be escaped).
struct ServerTemplateParts {
    parts: Vec<ServerTemplatePart>,
}

enum ServerTemplatePart {
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

/// Compile a component call for server: `ComponentName($$renderer, { ...props })`
fn compile_server_component(comp: &ComponentNode, source: &str) -> Option<String> {
    let name = &comp.name;
    let mut props = Vec::new();

    for attr in comp.attributes.iter() {
        match attr {
            Attribute::Attribute(a) => {
                // Skip event handlers on server
                if a.name.starts_with("on") {
                    // Check for shorthand: {onmouseup} means name === value
                    if let AttributeValueList::ExpressionTag(tag) = &a.value {
                        if let Some(rendered) = tag.expression.render() {
                            if rendered == a.name.as_ref() {
                                // Shorthand property
                                props.push(a.name.to_string());
                                continue;
                            }
                        }
                    }
                    let value = render_attribute_value_js(&a.value, source);
                    props.push(format!("{}: {value}", a.name));
                } else {
                    let value = render_attribute_value_js(&a.value, source);
                    props.push(format!("{}: {value}", a.name));
                }
            }
            Attribute::BindDirective(bind) => {
                if bind.name.as_ref() != "this" {
                    if let Some(expr) = bind.expression.render() {
                        props.push(format!(
                            "get {}() {{\n\t\treturn {expr};\n\t}}", bind.name
                        ));
                        props.push(format!(
                            "set {}($$value) {{\n\t\t{expr} = $$value;\n\t\t$$settled = false;\n\t}}", bind.name
                        ));
                    }
                }
            }
            _ => {}
        }
    }

    // Check for child content (default slot)
    let has_children = comp.fragment.nodes.iter().any(|n| !is_whitespace_text(n));
    if has_children {
        // Build children function body — server slot children get `<!---->` marker prefix
        let children_body = compile_server_fragment(&comp.fragment, source, &std::collections::HashMap::new())?;
        if !children_body.is_empty() {
            // Insert `<!---->` marker at the start of the template content
            let trimmed = children_body.trim();
            // If it starts with $$renderer.push(`...`), inject <!---> at the start
            // and strip leading whitespace from the template content
            let body_with_marker = if let Some(rest) = trimmed.strip_prefix("$$renderer.push(`") {
                let rest_trimmed = rest.trim_start_matches(|c: char| c == '\n' || c == '\r' || c == '\t');
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
fn compile_server_each_block(each: &EachBlock, source: &str) -> Option<String> {
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
    let fallback_has_await = each.fallback.as_ref().map_or(false, |f| fragment_has_await(f));
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
            let body_code = compile_server_each_body_async(&each.body, source, body_has_await)?;
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
                let fallback_code = compile_server_each_body_async(fallback, source, fallback_has_await)?;
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
            let body_code = compile_server_each_body_async(&each.body, source, body_has_await)?;
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
                    serialize_server_element(element, &mut body_parts, source, &std::collections::HashMap::new())?;
                }
                _ => {
                    // For other node types, fall back to server fragment
                    let body_server = compile_server_fragment(&each.body, source, &std::collections::HashMap::new())?;
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
fn compile_server_each_body_async(fragment: &Fragment, source: &str, is_async: bool) -> Option<String> {
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
                serialize_server_element(element, &mut parts, source, &std::collections::HashMap::new())?;
            }
            _ => {
                // Fall back to full fragment compilation
                let flushed = parts.to_template_literal();
                if !flushed.is_empty() {
                    output_lines.push(format!("$$renderer.push(`{flushed}`);\n"));
                    parts = ServerTemplateParts::new();
                }
                let frag_code = compile_server_fragment(fragment, source, &std::collections::HashMap::new())?;
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
fn compile_server_await_block(await_block: &crate::ast::modern::AwaitBlock, source: &str) -> Option<String> {
    let expr = await_block.expression.render()?;
    let mut output = String::new();

    // pending callback
    let pending_fn = if let Some(ref pending_frag) = await_block.pending {
        let has_content = pending_frag.nodes.iter().any(|n| !is_whitespace_text(n));
        if has_content {
            let body = compile_server_fragment(pending_frag, source, &std::collections::HashMap::new())?;
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
            let body = compile_server_fragment(then_frag, source, &std::collections::HashMap::new())?;
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

/// Check if any test expression or body expression in an if chain contains `await`
fn has_await_in_if_chain(if_block: &IfBlock) -> bool {
    let mut current = if_block;
    loop {
        // Check test expression
        if let Some(test) = current.test.render() {
            if test.contains("await ") {
                return true;
            }
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
fn fragment_has_await(fragment: &Fragment) -> bool {
    for node in fragment.nodes.iter() {
        if let Node::ExpressionTag(tag) = node {
            if let Some(expr) = tag.expression.render() {
                if expr.contains("await ") {
                    return true;
                }
            }
        }
    }
    false
}

/// Check if a fragment contains any @const tag with `await` in its init
fn fragment_has_const_await(fragment: &Fragment) -> bool {
    fragment.nodes.iter().any(|n| {
        if let Node::ConstTag(ct) = n {
            ct.declaration.render().map_or(false, |s| s.contains("await "))
        } else {
            false
        }
    })
}

/// Transform `await expr` in an expression to `(await $.save(expr))()`
/// e.g. `await foo` → `(await $.save(foo))()`
/// e.g. `await foo > 10` → `(await $.save(foo))() > 10`
fn transform_await_in_expr(expr: &str) -> String {
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
fn find_await_expr_end(s: &str) -> usize {
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

/// Compile a server fragment with async expression handling.
/// When `is_async` is true, expression tags containing `await` are emitted as
/// `$$renderer.push(async () => $.escape(await expr))` instead of inline template interpolation.
fn compile_server_fragment_async(
    fragment: &Fragment,
    source: &str,
    is_async: bool,
) -> Option<String> {
    // Check if any @const tag has await — if so, use $renderer.run() pattern
    // This takes priority over the is_async flag since const-await uses run(), not child_block
    if fragment_has_const_await(fragment) {
        return compile_server_fragment_with_const_run(fragment, source);
    }

    if !is_async {
        return compile_server_fragment(fragment, source, &std::collections::HashMap::new());
    }

    // For async fragments, handle expression tags specially
    let mut output_lines: Vec<String> = Vec::new();
    let mut parts = ServerTemplateParts::new();

    let is_sig_server = |n: &Node| !is_whitespace_text(n) && !matches!(n, Node::SnippetBlock(_));
    let first_sig = fragment.nodes.iter().position(|n| is_sig_server(n));
    let last_sig = fragment.nodes.iter().rposition(|n| is_sig_server(n));
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
                let if_code = compile_server_if_block(if_block, source)?;
                output_lines.push(if_code);
            }
            Node::RegularElement(element) => {
                serialize_server_element(element, &mut parts, source, &std::collections::HashMap::new())?;
            }
            Node::Component(comp) => {
                let flushed = parts.to_template_literal();
                if !flushed.is_empty() {
                    output_lines.push(format!("$$renderer.push(`{flushed}`);\n"));
                    parts = ServerTemplateParts::new();
                }
                let comp_code = compile_server_component(comp, source)?;
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

/// Parse a const tag declaration text like "const a = await 1" into (name, init_expr).
fn parse_const_declaration(decl_text: &str) -> Option<(String, String)> {
    // Strip "const " prefix
    let rest = decl_text.strip_prefix("const ")?;
    // Find the "=" separator
    let eq_pos = rest.find('=')?;
    let name = rest[..eq_pos].trim().to_string();
    let init = rest[eq_pos + 1..].trim().to_string();
    Some((name, init))
}

/// Compile a server fragment that has @const tags with await, using $renderer.run() pattern.
/// Pattern: hoisted let declarations + $renderer.run([closures]) + $renderer.async() for template content.
fn compile_server_fragment_with_const_run(
    fragment: &Fragment,
    source: &str,
) -> Option<String> {
    let mut output_lines: Vec<String> = Vec::new();

    // First pass: collect all @const declarations
    let mut const_entries: Vec<(String, String, bool)> = Vec::new(); // (name, init, has_await)
    for node in &fragment.nodes {
        if let Node::ConstTag(ct) = node {
            if let Some(decl_text) = ct.declaration.render() {
                if let Some((name, init)) = parse_const_declaration(&decl_text) {
                    // Top-level await: `await expr` or `fn(await expr)`, but NOT `(async () => { ... await ... })()`
                    let has_top_level_await = init.contains("await ") && !init.trim_start().starts_with("(async ");
                    const_entries.push((name, init, has_top_level_await));
                }
            }
        }
    }

    if const_entries.is_empty() {
        return compile_server_fragment(fragment, source, &std::collections::HashMap::new());
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
                        serialize_server_element(element, &mut parts, source, &std::collections::HashMap::new())?;
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

/// Check if a RegularElement's content references any of the given variable names.
fn element_references_vars(element: &RegularElement, const_entries: &[(String, String, bool)]) -> bool {
    for node in &element.fragment.nodes {
        if let Node::ExpressionTag(tag) = node {
            if let Some(expr_text) = tag.expression.render() {
                for (name, _, _) in const_entries {
                    if expr_text.contains(name.as_str()) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Check if an if-chain's test expressions reference any reactive ($state/$derived) variables
fn if_chain_refs_reactive(if_block: &IfBlock, run_info: &ServerAsyncRunInfo) -> bool {
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

/// Compile an if block wrapped in $$renderer.async_block() for script-level async run context.
fn compile_server_if_block_with_async_block(
    if_block: &IfBlock,
    source: &str,
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
    let inner = compile_server_if_block_for_async_block(if_block, source, run_info)?;
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

/// Check if an if-chain has direct `await` in any test expression
fn if_chain_has_direct_await(if_block: &IfBlock) -> bool {
    let mut current = if_block;
    loop {
        if let Some(test) = current.test.render() {
            if test.contains("await ") {
                return true;
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

/// Compile an if block for use inside $$renderer.async_block()
fn compile_server_if_block_for_async_block(
    if_block: &IfBlock,
    source: &str,
    run_info: &ServerAsyncRunInfo,
) -> Option<String> {
    let test = if_block.test.render()?;

    // Transform test: await → $.save, async-derived vars → function calls
    let test = transform_test_for_async_block(&test, run_info);

    let mut output = String::new();
    output.push_str(&format!("if ({test}) {{\n"));
    output.push_str("\t$$renderer.push('<!--[0-->');\n");

    let consequent = compile_server_fragment(&if_block.consequent, source, &std::collections::HashMap::new())?;
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
    compile_server_if_alternate_for_async_block(&if_block.alternate, source, &mut output, &mut branch_idx, run_info)?;

    Some(output)
}

/// Transform a test expression for use in async_block:
/// - `await expr` → `(await $.save(expr))()`
/// - async-derived vars → called as functions: `blocking` → `blocking()`
fn transform_test_for_async_block(test: &str, run_info: &ServerAsyncRunInfo) -> String {
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

/// Replace a word (at word boundaries) in a string
fn replace_word_boundary(text: &str, word: &str, replacement: &str) -> String {
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

/// Handle alternates for if-blocks inside async_block.
/// When a later else-if has `await` in its test, nest it in a child_block/async_block.
fn compile_server_if_alternate_for_async_block(
    alternate: &Option<Box<crate::ast::modern::Alternate>>,
    source: &str,
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
                    emit_nested_await_branch(output, else_if, source, run_info, refs_reactive)?;
                } else {
                    let test = transform_test_for_async_block(&test, run_info);
                    output.push_str(&format!("}} else if ({test}) {{\n"));
                    output.push_str(&format!("\t$$renderer.push('<!--[{branch_idx}-->');\n"));
                    *branch_idx += 1;
                    emit_fragment_indented(output, &else_if.consequent, source)?;
                    compile_server_if_alternate_for_async_block(&else_if.alternate, source, output, branch_idx, run_info)?;
                }
            }
            crate::ast::modern::Alternate::Fragment(frag) => {
                if let Some(elseif_block) = extract_elseif_from_fragment(frag) {
                    let test = elseif_block.test.render()?;
                    let test_has_await = test.contains("await ");
                    if test_has_await {
                        let refs_reactive = run_info.state_vars.iter().chain(run_info.async_vars.iter())
                            .any(|v| test.contains(v.as_str()));
                        emit_nested_await_branch(output, elseif_block, source, run_info, refs_reactive)?;
                    } else {
                        let test = transform_test_for_async_block(&test, run_info);
                        output.push_str(&format!("}} else if ({test}) {{\n"));
                        output.push_str(&format!("\t$$renderer.push('<!--[{branch_idx}-->');\n"));
                        *branch_idx += 1;
                        emit_fragment_indented(output, &elseif_block.consequent, source)?;
                        compile_server_if_alternate_for_async_block(&elseif_block.alternate, source, output, branch_idx, run_info)?;
                    }
                } else {
                    output.push_str("} else {\n");
                    output.push_str("\t$$renderer.push('<!--[-1-->');\n");
                    emit_fragment_indented(output, frag, source)?;
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
fn emit_fragment_indented(output: &mut String, fragment: &Fragment, source: &str) -> Option<()> {
    let body = compile_server_fragment(fragment, source, &std::collections::HashMap::new())?;
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
fn emit_nested_await_branch(
    output: &mut String,
    if_block: &IfBlock,
    source: &str,
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

    let consequent = compile_server_fragment(&if_block.consequent, source, &std::collections::HashMap::new())?;
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
                        let inner_consequent = compile_server_fragment(&elseif_block.consequent, source, &std::collections::HashMap::new())?;
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
                                    let else_body = compile_server_fragment(f, source, &std::collections::HashMap::new())?;
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
                    let else_body = compile_server_fragment(frag, source, &std::collections::HashMap::new())?;
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

    if use_async_block {
        output.push_str("\t});\n\n");
    } else {
        output.push_str("\t});\n\n");
    }
    output.push_str("\t$$renderer.push(`<!--]-->`);\n");
    output.push_str("}\n");
    Some(())
}

/// Compile an if block for server
fn compile_server_if_block(if_block: &IfBlock, source: &str) -> Option<String> {
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

    let consequent = compile_server_fragment_async(&if_block.consequent, source, has_await)?;
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
    compile_server_if_alternate(&if_block.alternate, source, &mut output, &mut branch_idx, has_await, indent)?;

    if has_await {
        output.push_str("});\n");
    }

    output.push('\n');
    output.push_str("$$renderer.push(`<!--]-->`);\n");

    Some(output)
}

/// Extract an else-if IfBlock from a Fragment alternate, or None if it's a plain else.
/// The upstream Svelte parser wraps {:else if} in a Fragment containing a single IfBlock.
fn extract_elseif_from_fragment(frag: &Fragment) -> Option<&IfBlock> {
    if frag.nodes.len() == 1 {
        if let Some(Node::IfBlock(if_block)) = frag.nodes.first() {
            if if_block.elseif {
                return Some(if_block);
            }
        }
    }
    None
}

/// Handle the alternate of an if block (else/else-if/none)
fn compile_server_if_alternate(
    alternate: &Option<Box<crate::ast::modern::Alternate>>,
    source: &str,
    output: &mut String,
    branch_idx: &mut i32,
    is_async: bool,
    indent: &str,
) -> Option<()> {
    if let Some(alt) = alternate {
        match alt.as_ref() {
            crate::ast::modern::Alternate::IfBlock(else_if) => {
                output.push_str(&format!("{indent}}} else "));
                let else_code = compile_server_if_block_inner_async(else_if, source, branch_idx, is_async, indent)?;
                output.push_str(&else_code);
            }
            crate::ast::modern::Alternate::Fragment(frag) => {
                // Check if this Fragment wraps an {:else if} IfBlock
                if let Some(elseif_block) = extract_elseif_from_fragment(frag) {
                    output.push_str(&format!("{indent}}} else "));
                    let else_code = compile_server_if_block_inner_async(elseif_block, source, branch_idx, is_async, indent)?;
                    output.push_str(&else_code);
                } else {
                    output.push_str(&format!("{indent}}} else {{\n"));
                    output.push_str(&format!("{indent}\t$$renderer.push('<!--[-1-->');\n"));
                    let else_body = compile_server_fragment_async(frag, source, is_async)?;
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
fn compile_server_if_block_inner_async(
    if_block: &IfBlock,
    source: &str,
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

    let consequent = compile_server_fragment_async(&if_block.consequent, source, is_async)?;
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

    compile_server_if_alternate(&if_block.alternate, source, &mut output, branch_idx, is_async, indent)?;

    Some(output)
}

fn serialize_server_element(
    element: &RegularElement,
    parts: &mut ServerTemplateParts,
    source: &str,
    constant_bindings: &std::collections::HashMap<String, String>,
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
                    AttributeValueList::Boolean(true) => {
                        parts.push_static("=\"\"");
                    }
                    AttributeValueList::Boolean(false) => {}
                    _ => {
                        parts.push_static("=\"");
                        parts.push_static(&render_attribute_value_static(&attr.value, source));
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
                        let is_before_first = first_non_ws.map_or(true, |f| ci < f);
                        let is_after_last = last_non_ws.map_or(true, |l| ci > l);
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
                    serialize_server_element(el, parts, source, constant_bindings)?;
                }
                Node::ExpressionTag(tag) => {
                    // Try constant propagation first
                    if let Some(value) = try_resolve_constant_binding(&tag.expression, constant_bindings) {
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

/// Check if a <select> element needs special server-side rendering.
/// All <select> elements with any content go through the special path.
fn has_option_children(element: &RegularElement) -> bool {
    element.fragment.nodes.iter().any(|n| !is_whitespace_text(n) && !matches!(n, Node::Comment(_)))
}

/// Check if select children need a `<!>` fragment anchor (customizable_select pattern).
/// Only checks for components/snippets/html inside dynamic blocks (each/if),
/// since top-level ones already emit their own `<!>` markers.
fn select_needs_fragment_anchor(children: &[Node]) -> bool {
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

/// Serialize a <select> element with special $$renderer.option() handling.
/// Works with the caller's parts and output_lines to allow static content
/// consolidation across select boundaries.
fn serialize_server_select_element(
    element: &RegularElement,
    parts: &mut ServerTemplateParts,
    output_lines: &mut Vec<String>,
    source: &str,
    constant_bindings: &std::collections::HashMap<String, String>,
    each_counter: &mut usize,
) -> Option<()> {
    // Build opening <select> tag
    parts.push_static("<select");
    for attr in element.attributes.iter() {
        if let Attribute::Attribute(attr) = attr {
            if attr.name.starts_with("on") {
                continue;
            }
            parts.push_static(" ");
            parts.push_static(&attr.name.to_lowercase());
            match &attr.value {
                AttributeValueList::Boolean(true) => {
                    parts.push_static("=\"\"");
                }
                AttributeValueList::Boolean(false) => {}
                _ => {
                    parts.push_static("=\"");
                    parts.push_static(&render_attribute_value_static(&attr.value, source));
                    parts.push_static("\"");
                }
            }
        }
    }
    parts.push_static(">");

    compile_server_select_children(&element.fragment.nodes, parts, output_lines, source, constant_bindings, each_counter, false)?;
    Some(())
}

/// Compile children of a <select> or <optgroup> element for server rendering.
/// Puts markers and static content into `parts`, imperative code into `output_lines`.
/// `in_nested_block` suppresses `<!----><!>` markers for render/component calls inside each/if/key blocks.
fn compile_server_select_children(
    children: &[Node],
    parts: &mut ServerTemplateParts,
    output_lines: &mut Vec<String>,
    source: &str,
    constant_bindings: &std::collections::HashMap<String, String>,
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
                let option_call = serialize_server_option_element(el, source, constant_bindings)?;
                output_lines.push(option_call);
            }
            Node::RegularElement(el) if &*el.name == "optgroup" => {
                // Add optgroup opening to parts
                parts.push_static("<optgroup");
                for attr in el.attributes.iter() {
                    if let Attribute::Attribute(attr) = attr {
                        parts.push_static(&format!(" {}=\"", attr.name.to_lowercase()));
                        parts.push_static(&render_attribute_value_static(&attr.value, source));
                        parts.push_static("\"");
                    }
                }
                parts.push_static(">");
                // Process optgroup children recursively (same nesting level as parent)
                compile_server_select_children(&el.fragment.nodes, parts, output_lines, source, constant_bindings, each_counter, in_nested_block)?;
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
                let each_code = compile_server_each_in_select(each, source, constant_bindings, each_counter)?;
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
                let if_code = compile_server_if_in_select(if_block, source, constant_bindings, each_counter)?;
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
                let body = render_server_select_children_indented(&key.fragment.nodes, source, constant_bindings, each_counter, "\t")?;
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
                let comp_code = compile_server_component(comp, source)?;
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
                let body = render_server_select_children_indented(&boundary.fragment.nodes, source, constant_bindings, each_counter, "\t")?;
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
fn compile_server_each_in_select(
    each: &EachBlock,
    source: &str,
    constant_bindings: &std::collections::HashMap<String, String>,
    each_counter: &mut usize,
) -> Option<String> {
    let raw_expr = render_expression_from_source(&each.expression)
        .or_else(|| each.expression.render())?;

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

    let idx_var = if let Some(ref idx) = inferred_index {
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
                let option_call = serialize_server_option_element(el, source, constant_bindings)?;
                inner.push_str(&option_call);
            }
            Node::Component(comp) => {
                let comp_code = compile_server_component(comp, source)?;
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
fn render_server_select_children_indented(
    nodes: &[Node],
    source: &str,
    constant_bindings: &std::collections::HashMap<String, String>,
    each_counter: &mut usize,
    indent: &str,
) -> Option<String> {
    let mut inner_parts = ServerTemplateParts::new();
    let mut inner_lines = Vec::new();
    compile_server_select_children(nodes, &mut inner_parts, &mut inner_lines, source, constant_bindings, each_counter, true)?;
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
fn compile_server_if_in_select(
    if_block: &IfBlock,
    source: &str,
    constant_bindings: &std::collections::HashMap<String, String>,
    each_counter: &mut usize,
) -> Option<String> {
    let condition = if_block.test.render()?;

    let mut output = String::new();
    output.push_str(&format!("\nif ({condition}) {{\n"));
    output.push_str("\t$$renderer.push('<!--[0-->');\n");

    // Consequent body
    let body = render_server_select_children_indented(&if_block.consequent.nodes, source, constant_bindings, each_counter, "\t")?;
    output.push_str(&body);

    // Check for else/else-if
    if let Some(ref alternate) = if_block.alternate {
        match &**alternate {
            Alternate::IfBlock(nested_if) => {
                output.push_str("} else ");
                let nested = compile_server_if_in_select(nested_if, source, constant_bindings, each_counter)?;
                // Strip leading \n from nested
                output.push_str(nested.trim_start_matches('\n'));
            }
            Alternate::Fragment(frag) => {
                output.push_str("} else {\n");
                output.push_str("\t$$renderer.push('<!--[-1-->');\n");
                let body = render_server_select_children_indented(&frag.nodes, source, constant_bindings, each_counter, "\t")?;
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

/// Compile <svelte:boundary> inside a <select> for server rendering
fn compile_server_boundary_in_select(
    boundary: &SvelteBoundary,
    source: &str,
    constant_bindings: &std::collections::HashMap<String, String>,
    each_counter: &mut usize,
) -> Option<String> {
    let mut output = String::new();
    output.push_str("\n$$renderer.push(`<!--[-->`);\n\n{\n");
    let body = render_server_select_children_indented(&boundary.fragment.nodes, source, constant_bindings, each_counter, "\t")?;
    output.push_str(&body);
    output.push_str("}\n\n$$renderer.push(`<!--]-->`);\n");
    Some(output)
}

/// Parse a render tag expression like "opt()" into ("opt", "")
/// or "snippet(arg1, arg2)" into ("snippet", ", arg1, arg2")
fn parse_render_call_expr(expr: &str) -> (String, String) {
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

/// Serialize a server <option> element as $$renderer.option() call
fn serialize_server_option_element(
    element: &RegularElement,
    source: &str,
    _constant_bindings: &std::collections::HashMap<String, String>,
) -> Option<String> {
    // Build attributes object
    let attrs = build_server_option_attrs(element, source);

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
                    serialize_server_element(el, &mut content_parts, source, &std::collections::HashMap::new())?;
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
        if sig_children.len() == 1 {
            if let Node::ExpressionTag(tag) = sig_children[0] {
                if let Some(expr_text) = tag.expression.render() {
                    return Some(format!("\n$$renderer.option({attrs}, {expr_text});\n"));
                }
            }
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
fn build_server_option_attrs(element: &RegularElement, source: &str) -> String {
    let mut attrs = String::from("{");
    let mut first = true;

    for attr in element.attributes.iter() {
        if let Attribute::Attribute(attr) = attr {
            if !first { attrs.push_str(","); }
            first = false;
            attrs.push_str(&format!(" {}: ", attr.name));
            match &attr.value {
                AttributeValueList::Boolean(true) => attrs.push_str("true"),
                _ => {
                    let val = render_attribute_value_static(&attr.value, source);
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

/// Check if option children include rich content (HTML elements, @html, etc.)
fn server_option_has_rich_content(children: &[Node]) -> bool {
    children.iter().any(|n| matches!(n, Node::RegularElement(_) | Node::HtmlTag(_) | Node::Component(_)))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Collect variable bindings that are compile-time constants (never reassigned, no $state/$derived).
fn collect_constant_bindings(script: &Script) -> std::collections::HashMap<String, String> {
    use oxc_ast::ast::BindingPattern;
    let mut candidates = std::collections::HashMap::new();

    // First pass: collect candidates from variable declarations
    for statement in &script.oxc_program().body {
        let decl = match statement {
            OxcStatement::VariableDeclaration(d) => &**d,
            _ => continue,
        };
        for declarator in &decl.declarations {
            let Some(init) = declarator.init.as_ref() else { continue };
            // Skip $state/$derived/$props calls
            if let OxcExpression::CallExpression(call) = init.get_inner_expression() {
                if let OxcExpression::Identifier(id) = call.callee.get_inner_expression() {
                    if matches!(id.name.as_str(), "$state" | "$derived" | "$props" | "$effect") {
                        continue;
                    }
                }
            }
            // Only handle simple identifier bindings with const-evaluable initializers
            if let BindingPattern::BindingIdentifier(id) = &declarator.id {
                if let Some(value) = try_eval_constant(init) {
                    candidates.insert(id.name.to_string(), value);
                }
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
fn try_resolve_constant_binding(
    expr: &crate::ast::modern::Expression,
    constant_bindings: &std::collections::HashMap<String, String>,
) -> Option<String> {
    if constant_bindings.is_empty() {
        return None;
    }
    let oxc_expr = expr.oxc_expression()?;
    try_resolve_oxc_constant(oxc_expr, constant_bindings)
}

fn try_resolve_oxc_constant(
    expr: &OxcExpression<'_>,
    constant_bindings: &std::collections::HashMap<String, String>,
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

fn has_props_rune(root: &Root) -> bool {
    if let Some(instance) = root.instance.as_ref() {
        for statement in &instance.oxc_program().body {
            if contains_props_call(statement) {
                return true;
            }
        }
    }
    false
}

fn contains_props_call(statement: &OxcStatement<'_>) -> bool {
    match statement {
        OxcStatement::VariableDeclaration(decl) => decl.declarations.iter().any(|d| {
            d.init.as_ref().is_some_and(|init| {
                if let OxcExpression::CallExpression(call) = init.get_inner_expression() {
                    if let OxcExpression::Identifier(id) = call.callee.get_inner_expression() {
                        return id.name.as_str() == "$props";
                    }
                }
                false
            })
        }),
        OxcStatement::ExportNamedDeclaration(export) => {
            if let Some(Declaration::VariableDeclaration(decl)) = export.declaration.as_ref() {
                decl.declarations.iter().any(|d| {
                    d.init.as_ref().is_some_and(|init| {
                        if let OxcExpression::CallExpression(call) = init.get_inner_expression() {
                            if let OxcExpression::Identifier(id) =
                                call.callee.get_inner_expression()
                            {
                                return id.name.as_str() == "$props";
                            }
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

fn has_bindable_props(root: &Root) -> bool {
    // Components with bind: directives need $$props
    root.fragment.find_map(|entry| {
        let node = entry.as_node()?;
        match node {
            Node::RegularElement(el) => {
                if el.attributes.iter().any(|a| matches!(a, Attribute::BindDirective(_))) {
                    Some(())
                } else {
                    None
                }
            }
            _ => None,
        }
    }).is_some()
}

fn has_class_rune_fields(root: &Root) -> bool {
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
                    if let ClassElement::PropertyDefinition(prop) = element {
                        if let Some(init) = &prop.value {
                            if is_state_or_derived_call(init) {
                                return true;
                            }
                        }
                    }
                }
            }
        }
    }
    false
}

/// Add blank lines between top-level statements in multi-line arrow function bodies.
/// E.g., `(e) => {\n\tconst x = 1;\n\tconsole.log(x);\n}` →
///       `(e) => {\n\tconst x = 1;\n\n\tconsole.log(x);\n}`
/// Rewrite state variable accesses in rendered expressions:
/// - `name` (standalone identifier in expression context) → `$.get(name)`
/// - `name++` or `++name` → `$.update(name)`
/// - `name--` or `--name` → `$.update(name, -1)`
fn rewrite_state_accesses(text: &str, state_bindings: &BTreeSet<String>) -> String {
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
fn rewrite_state_assignments(text: &str, name: &str) -> String {
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
fn try_rewrite_assignment_line(line: &str, name: &str, name_bytes: &[u8], name_len: usize) -> Option<String> {
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
                    if after_trimmed.starts_with(op) {
                        let rhs = after_trimmed[op.len()..].trim_start();
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

/// Flush impure attribute effects as batched template_effects.
/// Custom element effects get individual template_effects.
/// Regular element effects are batched into one template_effect with numbered params and deps array.
fn flush_impure_attr_effects(effects: &[ImpureAttrEffect]) -> Vec<String> {
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

/// Collapse whitespace in template HTML: replace runs of whitespace (including newlines/tabs)
/// with a single space. Preserves content within the same text node.
/// Check if any component in a fragment has bind: directives.
fn fragment_has_component_bindings(fragment: &Fragment) -> bool {
    for node in &fragment.nodes {
        match node {
            Node::Component(comp) => {
                for attr in comp.attributes.iter() {
                    if let Attribute::BindDirective(bind) = attr {
                        // bind:this is client-only, doesn't need $$settled on server
                        if &*bind.name != "this" {
                            return true;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    false
}

/// Collect server-side snippet functions from a fragment.
fn collect_server_snippet_functions(
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
            let body_code = compile_server_snippet_body(&snippet.body, source);
            let mut snippet_fn = format!("function {name}($$renderer) {{\n");
            snippet_fn.push_str(&body_code);
            snippet_fn.push_str("}\n");
            snippets.push(snippet_fn);
        }
    }
    snippets
}

/// Compile the body of a server-side snippet.
fn compile_server_snippet_body(fragment: &Fragment, source: &str) -> String {
    // Check if body contains <option> elements
    let has_option = fragment.nodes.iter().any(|n| {
        matches!(n, Node::RegularElement(el) if &*el.name == "option")
    });

    if has_option {
        // Snippet body contains options → use $$renderer.option() calls
        let mut output = String::new();
        let empty_bindings = std::collections::HashMap::new();
        for node in &fragment.nodes {
            match node {
                Node::Text(text) if text.data.trim().is_empty() => {}
                Node::RegularElement(el) if &*el.name == "option" => {
                    if let Some(option_call) = serialize_server_option_element(el, source, &empty_bindings) {
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

/// Detect indices of nodes that are part of text+expression runs at fragment level.
/// A text+expression run is a consecutive sequence of Text and ExpressionTag nodes
/// where at least one ExpressionTag is present — the entire run shares a single text anchor.
fn detect_text_expr_runs(nodes: &[Node], first_sig: usize, last_sig: usize) -> std::collections::HashSet<usize> {
    let mut run_indices = std::collections::HashSet::new();

    // Find runs of consecutive Text/ExpressionTag nodes (skipping whitespace-only text and snippets)
    let mut i = first_sig;
    while i <= last_sig {
        // Check if this starts a text+expression run
        let mut run_start = i;
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
            for j in run_start..run_end {
                match &nodes[j] {
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

fn collapse_template_whitespace(text: &str) -> String {
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

/// Collect destructured prop names from a $props() destructuring pattern.
fn collect_destructured_prop_names(script: &Script) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let program = script.oxc_program();
    for stmt in &program.body {
        if let OxcStatement::VariableDeclaration(decl) = stmt {
            for declarator in &decl.declarations {
                if let Some(init) = declarator.init.as_ref() {
                    if let OxcExpression::CallExpression(call) = init.get_inner_expression() {
                        if let OxcExpression::Identifier(id) = call.callee.get_inner_expression() {
                            if id.name.as_str() == "$props" {
                                if let oxc_ast::ast::BindingPattern::ObjectPattern(obj) = &declarator.id {
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
                }
            }
        }
    }
    names
}

/// Replace whole-word occurrences of `word` with `replacement` in text.
fn replace_word_with(text: &str, word: &str, replacement: &str) -> String {
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

/// Collapse runs of multiple spaces into a single space.
fn collapse_spaces(s: &str) -> String {
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
fn is_simple_literal(expr: &str) -> bool {
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
fn strip_stmt_terminator(rhs: &str) -> (&str, &str) {
    if let Some(r) = rhs.strip_suffix(';') {
        (r.trim_end(), ";")
    } else if let Some(r) = rhs.strip_suffix(',') {
        (r.trim_end(), ",")
    } else {
        (rhs, "")
    }
}

fn add_blank_lines_in_arrow_body(text: &str) -> String {
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

/// Check if the component uses `await` expressions (in template or script).
fn has_async_content_with_source(root: &Root, source: &str) -> bool {
    if has_async_content(root) {
        return true;
    }
    // Check instance script source for await expressions
    if let Some(instance) = root.instance.as_ref() {
        if let Some(snippet) = source.get(instance.content_start..instance.content_end) {
            if snippet.contains("await ") {
                return true;
            }
        }
    }
    false
}

fn has_async_content(root: &Root) -> bool {
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
                if let Some(src) = if_block.test.render() {
                    if src.contains("await ") { return true; }
                }
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
                if let Some(src) = each.expression.render() {
                    if src.contains("await ") { return true; }
                }
                if check_fragment(&each.body) { return true; }
                if let Some(ref fallback) = each.fallback {
                    if check_fragment(fallback) { return true; }
                }
                false
            }
            Node::ConstTag(ct) => {
                ct.declaration.render().map_or(false, |s| s.contains("await "))
            }
            Node::ExpressionTag(tag) => {
                tag.expression.render().map_or(false, |s| s.contains("await "))
            }
            Node::RegularElement(el) => check_fragment(&el.fragment),
            Node::Component(comp) => check_fragment(&comp.fragment),
            _ => false,
        }
    }

    check_fragment(&root.fragment)
}

/// Check if an element needs JS-side traversal (not purely static in template).
/// Returns true if the element or any descendant has:
/// - Dynamic children (expressions, blocks, components)
/// - Dynamic/event/property attributes
/// - Is a custom element
/// - Has autofocus, muted attributes (handled as JS properties)
/// - Has value attribute on option (handled as JS property)
fn element_needs_js_traversal(el: &RegularElement) -> bool {
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

fn has_dynamic_content(fragment: &Fragment) -> bool {
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

/// Check if a string is a simple JS identifier (no dots, parens, spaces, etc.)
/// Replace standalone occurrences of `var_name` with `$.get(var_name)` in an expression.
/// Check if `text` contains `word` as a whole word (not part of a larger identifier).
fn contains_word(text: &str, word: &str) -> bool {
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

/// Transform `await expr OP rest` into `(await $.save(expr))() OP rest`.
/// E.g., `await foo > 10` → `(await $.save(foo))() > 10`
fn transform_await_with_save(test: &str) -> String {
    // Find `await ` and extract the target
    if let Some(pos) = test.find("await ") {
        let before = &test[..pos];
        let after = &test[pos + 6..];
        // Find end of the awaited expression (before operator/space)
        let end = after.find(|c: char| c == ' ' || c == '>' || c == '<' || c == '=' || c == '!' || c == '+' || c == '-' || c == '*' || c == '/' || c == '%' || c == '&' || c == '|').unwrap_or(after.len());
        let target = &after[..end];
        let rest = &after[end..];
        format!("{before}(await $.save({target}))(){rest}")
    } else {
        test.to_string()
    }
}

fn replace_var_with_get(text: &str, var_name: &str) -> String {
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
fn test_needs_derived(test: &str) -> bool {
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

/// Check if a string is a simple JS identifier (no dots, parens, spaces, etc.)
fn is_simple_identifier(s: &str) -> bool {
    let s = s.trim();
    !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '$')
        && !s.chars().next().unwrap().is_ascii_digit()
}

/// Normalize blank lines in client output to match upstream Svelte conventions.
/// Rules:
/// 1. Add blank line before `$.reset(...)` when preceded by `});` or `}`
/// 2. Add blank line before `$.append(...)` when preceded by a template/fragment assignment
/// 3. Collapse triple+ blank lines to double
/// 4. Add blank line before `$.customizable_select` when preceded by a var assignment
fn normalize_client_blank_lines(output: &str) -> String {
    let lines: Vec<&str> = output.lines().collect();
    let mut result = Vec::with_capacity(lines.len() + 20);

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        let prev_trimmed = if i > 0 { lines[i - 1].trim() } else { "" };
        let prev_is_blank = i > 0 && lines[i - 1].trim().is_empty();

        // Rule 1: blank line before $.reset() after }); or } or var assignment
        if trimmed.starts_with("$.reset(") && !prev_is_blank {
            if prev_trimmed.ends_with("});") || prev_trimmed.ends_with('}')
                || (prev_trimmed.starts_with("var ") && prev_trimmed.contains("= "))
            {
                result.push("");
            }
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
        if trimmed.starts_with("$.next()") && !prev_is_blank {
            if prev_trimmed.starts_with("var ") && prev_trimmed.contains("= ") {
                result.push("");
            }
        }

        // Rule 4: blank line before $.html() when preceded by var assignment
        if trimmed.starts_with("$.html(") && !prev_is_blank {
            if prev_trimmed.starts_with("var ") && prev_trimmed.contains("= ") {
                result.push("");
            }
        }

        // Rule 5: blank line before $.customizable_select when preceded by var assignment
        if trimmed.starts_with("$.customizable_select(") && !prev_is_blank {
            if prev_trimmed.starts_with("var ") && prev_trimmed.contains("= ") {
                result.push("");
            }
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

fn is_whitespace_text(node: &Node) -> bool {
    if let Node::Text(text) = node {
        text.data.trim().is_empty()
    } else {
        false
    }
}

/// Render an attribute value as a JS expression (for component props).
fn render_attribute_value_js(value: &AttributeValueList, _source: &str) -> String {
    match value {
        AttributeValueList::Boolean(true) => "true".to_string(),
        AttributeValueList::Boolean(false) => "false".to_string(),
        AttributeValueList::ExpressionTag(tag) => {
            tag.expression.render().unwrap_or_default()
        }
        AttributeValueList::Values(parts) => {
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
fn is_dynamic_attribute_value(value: &AttributeValueList) -> bool {
    match value {
        AttributeValueList::Boolean(_) => false,
        AttributeValueList::ExpressionTag(_) => true,
        AttributeValueList::Values(parts) => parts.iter().any(|p| matches!(p, AttributeValue::ExpressionTag(_))),
    }
}

/// Render a dynamic attribute value as an expression (for server $.attr() calls).
fn render_attribute_value_dynamic(value: &AttributeValueList) -> Option<String> {
    match value {
        AttributeValueList::ExpressionTag(tag) => tag.expression.render(),
        AttributeValueList::Values(parts) => {
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

fn render_attribute_value_static(value: &AttributeValueList, _source: &str) -> String {
    let mut result = String::new();
    match value {
        AttributeValueList::Boolean(_) => {}
        AttributeValueList::ExpressionTag(tag) => {
            if let Some(rendered) = tag.expression.render() {
                result.push_str(&rendered);
            }
        }
        AttributeValueList::Values(parts) => {
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

/// Re-indent a method body to use a single tab base indentation.
/// Strips common leading whitespace from non-first, non-empty lines,
/// then adds `base_indent` to all lines.
fn reindent_method(text: &str, base_indent: &str) -> String {
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

fn is_void_element(name: &str) -> bool {
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
fn is_svg_element(name: &str) -> bool {
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
fn is_custom_element(name: &str) -> bool {
    name.contains('-')
}

/// Sanitize an element name for use as a JS variable (replace hyphens with underscores).
fn sanitize_var_name(name: &str) -> String {
    name.replace('-', "_")
}

/// Events that can use event delegation (bubbling DOM events).
fn is_delegatable_event(name: &str) -> bool {
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

// ---------------------------------------------------------------------------
// $props() detection and rewriting
// ---------------------------------------------------------------------------

/// Detect `let <name> = $props()` in the instance script, returning the binding name.
fn detect_props_binding(script: &Script) -> Option<String> {
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
            if let OxcExpression::CallExpression(call) = init.get_inner_expression() {
                if let OxcExpression::Identifier(id) = call.callee.get_inner_expression() {
                    if id.name.as_str() == "$props" {
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
        }
    }
    None
}

/// Render `$props()` declarations on client.
/// - `let props = $props()` → `let props = $.rest_props($$props, [...])`
/// - `let { tag = 'hr' } = $props()` → `let tag = $.prop($$props, 'tag', 3, 'hr')`
fn render_props_declaration_client(
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
        if let OxcExpression::CallExpression(call) = init.get_inner_expression() {
            if let OxcExpression::Identifier(id) = call.callee.get_inner_expression() {
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
                        if let Some(ref rest) = obj_pat.rest {
                            if let BindingPattern::BindingIdentifier(id) = &rest.argument {
                                rest_name = Some(id.name.to_string());
                            }
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
    }
    None
}

/// Render `$props()` declarations on server.
/// - `let props = $props()` → `let { $$slots, $$events, ...props } = $$props`
/// - `let { tag = 'hr' } = $props()` → `let { tag = 'hr' } = $$props`
fn render_props_declaration_server(
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
        if let OxcExpression::CallExpression(call) = init.get_inner_expression() {
            if let OxcExpression::Identifier(id) = call.callee.get_inner_expression() {
                if id.name.as_str() == "$props" {
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
        }
    }
    None
}

/// Rewrite direct member access on the props binding to use $$props.
/// `props.a` → `$$props.a`, `props.a.b` → `$$props.a.b`
/// But NOT computed access: `props[a]` stays as `props[a]`.
/// And NOT direct assignment targets: `props.a = true` stays as `props.a = true`.
fn rewrite_props_member_access(text: &str, props_name: &str) -> String {
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
fn is_props_direct_assignment(text: &str, prop_start: usize) -> bool {
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
    if pos < bytes.len() && bytes[pos] == b'=' {
        if pos + 1 >= bytes.len() || bytes[pos + 1] != b'=' {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Statement formatting
// ---------------------------------------------------------------------------

/// Join rendered statements with blank lines between different "types" of
/// statements (variable declarations vs function declarations, etc.).
/// Extract leading line comments (// ...) from the gap between two statement spans.
fn extract_leading_comments(snippet: &str, start: usize, end: usize) -> Vec<String> {
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
fn prepend_comments(comments: &[String], rendered: &str) -> String {
    if comments.is_empty() {
        return rendered.to_string();
    }
    let mut result = comments.join("\n");
    result.push('\n');
    result.push_str(rendered);
    result
}

fn join_statements_with_blank_lines(statements: &[String]) -> String {
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
fn should_add_blank_line_between(prev: &str, next: &str) -> bool {
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
enum StatementKind {
    Declaration,
    Function,
    Class,
    Expression,
}

fn statement_kind(s: &str) -> StatementKind {
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

// ---------------------------------------------------------------------------
// Expression rendering
// ---------------------------------------------------------------------------

/// Render an expression preferring source text over OXC codegen to preserve formatting.
fn render_expression_from_source(expr: &crate::ast::modern::Expression) -> Option<String> {
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

// ---------------------------------------------------------------------------
// Template string building with constant folding
// ---------------------------------------------------------------------------

/// Build a template string using `$0`, `$1` params for dynamic expressions.
fn build_template_string_with_folding_params(children: &[&Node]) -> String {
    let mut parts = Vec::new();
    let mut param_idx = 0;
    for child in children {
        match child {
            Node::Text(t) => parts.push(t.data.to_string()),
            Node::ExpressionTag(tag) => {
                if let Some(folded) = try_fold_expression_to_string(&tag.expression) {
                    parts.push(folded);
                } else {
                    parts.push(format!("${{{} ?? ''}}", format!("${param_idx}")));
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
fn build_template_string_with_folding(children: &[&Node]) -> String {
    build_template_string_impl(children, true)
}

fn build_template_string_no_null_coalesce(children: &[&Node]) -> String {
    build_template_string_impl(children, false)
}

fn build_template_string_impl(children: &[&Node], null_coalesce: bool) -> String {
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

// ---------------------------------------------------------------------------
// Constant folding / expression analysis
// ---------------------------------------------------------------------------

/// Try to evaluate an ExpressionTag as a constant string.
/// Returns Some(string_value) for string/template literals, None for dynamic expressions.
fn try_fold_expression_to_string(expr: &crate::ast::modern::Expression) -> Option<String> {
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

/// Check if an ExpressionTag is a pure/static expression that can be constant-folded.
/// Returns true for: string literals, number literals, null, boolean,
/// and pure function calls like Math.max(), encodeURIComponent(), etc.
fn is_pure_expression(expr: &crate::ast::modern::Expression) -> bool {
    let Some(oxc_expr) = expr.oxc_expression() else {
        return false;
    };
    is_pure_oxc_expression(oxc_expr)
}

fn is_pure_oxc_expression(expr: &OxcExpression<'_>) -> bool {
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

fn is_pure_global_call(callee: &OxcExpression<'_>) -> bool {
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

fn is_global_member_access(mem: &oxc_ast::ast::StaticMemberExpression<'_>) -> bool {
    if let OxcExpression::Identifier(obj) = mem.object.get_inner_expression() {
        matches!(obj.name.as_str(), "location" | "navigator" | "document" | "window" | "globalThis" | "Math" | "JSON" | "Number" | "String")
    } else {
        false
    }
}

/// Try to evaluate a pure expression to a constant JS value string.
fn try_eval_constant(expr: &OxcExpression<'_>) -> Option<String> {
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
            if let OxcExpression::StaticMemberExpression(mem) = call.callee.get_inner_expression() {
                if let OxcExpression::Identifier(obj) = mem.object.get_inner_expression() {
                    if obj.name.as_str() == "Math" {
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
                }
            }
            None
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Select / Option rich content detection
// ---------------------------------------------------------------------------

/// Check if select/optgroup children need a full `$.customizable_select()` wrapper.
/// Returns true when direct children include Component/RenderTag/HtmlTag, or
/// when each/if blocks contain Component/RenderTag/HtmlTag in their body.
fn select_children_need_wrapper(children: &[&Node]) -> bool {
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
fn option_has_rich_content(el: &RegularElement) -> bool {
    el.fragment.nodes.iter().any(|child| matches!(child,
        Node::RegularElement(_) | Node::Component(_) | Node::HtmlTag(_) | Node::RenderTag(_)
    ))
}

/// Check if a select/optgroup's direct children contain "rich content" that requires
/// `$.customizable_select()`. Returns true for:
/// - Direct `<option>` with rich content (elements inside)
/// - Direct Component, {@render}, {@html} as children
/// - {#each} whose body contains Component or rich options
/// - {#if} whose consequent contains {@render} or Component
fn select_has_rich_content(fragment: &Fragment) -> bool {
    for node in &fragment.nodes {
        match node {
            Node::RegularElement(el) if &*el.name == "option" => {
                if option_has_rich_content(el) {
                    return true;
                }
            }
            Node::RegularElement(el) if &*el.name == "optgroup" => {
                if select_has_rich_content(&el.fragment) {
                    return true;
                }
            }
            Node::Component(_) | Node::HtmlTag(_) | Node::RenderTag(_) => {
                return true;
            }
            Node::EachBlock(each) => {
                if each_body_has_rich_content(&each.body) {
                    return true;
                }
            }
            Node::IfBlock(if_block) => {
                if if_body_has_rich_content(if_block) {
                    return true;
                }
            }
            Node::SvelteBoundary(boundary) => {
                if select_has_rich_content(&boundary.fragment) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

fn each_body_has_rich_content(fragment: &Fragment) -> bool {
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

fn if_body_has_rich_content(if_block: &IfBlock) -> bool {
    for node in &if_block.consequent.nodes {
        match node {
            Node::Component(_) | Node::RenderTag(_) | Node::HtmlTag(_) => return true,
            Node::RegularElement(el) if &*el.name == "option" && option_has_rich_content(el) => return true,
            _ => {}
        }
    }
    if let Some(alternate) = &if_block.alternate {
        use crate::ast::modern::Alternate;
        match alternate.as_ref() {
            Alternate::Fragment(fragment) => {
                for node in &fragment.nodes {
                    match node {
                        Node::Component(_) | Node::RenderTag(_) | Node::HtmlTag(_) => return true,
                        _ => {}
                    }
                }
            }
            Alternate::IfBlock(nested_if) => {
                if if_body_has_rich_content(nested_if) {
                    return true;
                }
            }
        }
    }
    false
}
