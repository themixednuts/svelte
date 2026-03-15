use super::*;

// ---------------------------------------------------------------------------
// Client codegen
// ---------------------------------------------------------------------------

pub(super) fn compile_client(
    source: &str,
    root: &Root,
    runes_mode: bool,
    component_name: &str,
) -> Option<String> {
    let mut ctx = ClientContext::from_root(root, runes_mode);

    // Determine if component needs $$props parameter
    let has_props = has_props_rune(root) || has_class_rune_fields(root);

    // Check if props come only from destructured $props() pattern with no defaults
    let mut props_are_destructured_only = runes_mode
        && has_props
        && !has_class_rune_fields(root)
        && root.instance.as_ref().is_some_and(|inst| {
            detect_props_binding(inst) == Some("$$destructured_props".to_string())
        });

    // Collect destructured prop names for direct $$props access
    if props_are_destructured_only
        && let Some(instance) = root.instance.as_ref() {
            let names = collect_destructured_prop_names(instance);
            if names.is_empty() {
                // Has defaults or rest — can't use direct access
                props_are_destructured_only = false;
            } else {
                ctx.destructured_props = names;
            }
    }

    // Build instance script body FIRST (to get async_run_info before fragment compilation)
    let InstanceScriptResult { body: script_body, async_run_info: client_async_run_info } =
        if let Some(instance) = root.instance.as_ref() {
            compile_instance_script_client(source, instance, runes_mode, root)?
        } else {
            InstanceScriptResult { body: String::new(), async_run_info: None }
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
    let has_props = has_props || client_async_run_info.as_ref().is_some_and(|info| info.has_sync_derived);

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
        .chain(root_template)
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
        ", $$props"
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
        && root.instance.as_ref().is_some_and(|inst| {
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

    output.push('}');

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
// Client template + DOM traversal
// ---------------------------------------------------------------------------

struct ClientContext {
    templates: Vec<HoistedTemplate>,
    template_counter: usize,
    /// Counters for named template prefixes (option_content, select_content, etc.)
    named_template_counters: HashMap<String, usize>,
    var_counter: VarCounter,
    delegated_events: BTreeSet<String>,
    runes_mode: bool,
    /// Names of local function declarations (for getter optimization)
    local_functions: BTreeSet<String>,
    /// Compile-time constant bindings (variable name → string value)
    constant_bindings: HashMap<String, String>,
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

pub(super) struct ImpureAttrEffect {
    pub(super) el_var: String,
    pub(super) attr_name: String,
    pub(super) dep: String,      // dependency function ref (e.g., "y")
    pub(super) is_custom: bool,  // custom element → separate template_effect
}

struct HoistedTemplate {
    name: String,
    html: String,
    flags: u32,
}

struct VarCounter {
    counts: HashMap<String, usize>,
}

impl VarCounter {
    fn new() -> Self {
        Self {
            counts: HashMap::new(),
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

impl std::fmt::Display for VarCounter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "VarCounter({} bases)", self.counts.len())
    }
}

impl std::fmt::Debug for ClientContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientContext")
            .field("template_count", &self.templates.len())
            .field("runes_mode", &self.runes_mode)
            .field("state_bindings", &self.state_bindings.len())
            .field("delegated_events", &self.delegated_events.len())
            .finish()
    }
}

impl ClientContext {
    fn new(runes_mode: bool) -> Self {
        Self {
            templates: Vec::new(),
            template_counter: 0,
            named_template_counters: HashMap::new(),
            var_counter: VarCounter::new(),
            delegated_events: BTreeSet::new(),
            runes_mode,
            local_functions: BTreeSet::new(),
            constant_bindings: HashMap::new(),
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

    /// Construct a `ClientContext` pre-populated with state analysis from the component root.
    fn from_root(root: &Root, runes_mode: bool) -> Self {
        let mut ctx = Self::new(runes_mode);

        if let Some(instance) = root.instance.as_ref() {
            ctx.local_functions = collect_local_function_names(instance);
            ctx.constant_bindings = collect_constant_bindings(instance);

            let all_state = collect_instance_state_bindings(instance);
            let mutated = collect_mutated_state_bindings(instance, &all_state, Some(root));
            let proxy = collect_proxy_bindings(instance, &mutated);
            ctx.proxy_bindings = proxy.clone();
            ctx.state_bindings = mutated.difference(&proxy).cloned().collect();
            ctx.derived_bindings = collect_derived_bindings(instance);
        }

        ctx
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
        let first_sig = fragment.nodes.iter().position(&is_sig);
        let last_sig = fragment.nodes.iter().rposition(is_sig);
        let (first_sig, last_sig) = match (first_sig, last_sig) {
            (Some(f), Some(l)) => (f, l),
            _ => return Some(String::new()),
        };

        // Special case: single Component node — call directly with $$anchor (no template needed)
        if significant_nodes.len() == 1
            && let Node::Component(_) = significant_nodes[0] {
                let mut body = String::new();
                self.compile_single_dynamic_node(significant_nodes[0], "$$anchor", source, &mut body)?;
                return Some(body);
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
                            || el.fragment.nodes.iter().any(check_custom_elements)
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
                        AttributeValueKind::Boolean(true) => {
                            // Boolean attribute: <input disabled>
                        }
                        AttributeValueKind::Boolean(false) => {}
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
                    if let (Some(f), Some(l)) = (child_first_sig, child_last_sig)
                        && (ci < f || ci > l) {
                            continue;
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
            if let Some(OxcExpression::CallExpression(call)) = tag.expression.oxc_expression()
                && call.arguments.is_empty()
                && let OxcExpression::Identifier(id) = &call.callee
                && self.local_functions.contains(id.name.as_str()) {
                    expr_texts.push(id.name.to_string());
                    continue;
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
                if let Some(ref info) = self.async_run_info
                    && info.promise_var == "promises" {
                        // @const run: vars are signals/deriveds, need $.get()
                        for v in &info.async_vars {
                            expr = replace_var_with_get(&expr, v);
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
        let mut _first_child_var = String::new();
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
                        _first_child_var = child_var.clone();
                    } else {
                        let sep = if body.trim_end_matches('\n').lines().last().unwrap_or("").starts_with("var ") { "" } else { "\n" };
                        body.push_str(&format!("{sep}var {child_var} = $.sibling({prev_child_var}, {skip});\n"));
                    }

                    // Recursively handle this child's content
                    self.compile_element_deep(child_el, &child_var, source, body)?;

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
                        _first_child_var = node_var.clone();
                    } else {
                        body.push_str(&format!("\nvar {node_var} = $.sibling({prev_child_var}, {skip});\n"));
                    }

                    // Emit $.html() call
                    if let Some(expr) = html_tag.expression.render() {
                        let expr = self.maybe_rewrite_state_expr(&expr);
                        body.push_str(&format!("\n$.html({node_var}, () => {expr});\n"));
                    }

                    // Count remaining siblings after this node for $.next()
                    let _remaining = sig_children.len() - si - 1;
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
                        _first_child_var = node_var.clone();
                    } else {
                        body.push_str(&format!("\nvar {node_var} = $.sibling({prev_child_var}, {skip});\n"));
                    }
                    body.push('\n');
                    self.compile_single_dynamic_node(child, &node_var, source, body)?;
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
            body.push_str("\t$.next();\n");
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
            if let Node::IfBlock(if_block) = child
                && self.if_branch_has_single_render_tag(&if_block.consequent) {
                    self.var_counter.next("fragment");
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
                    body.push('\n');
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
                if let Node::RegularElement(opt_el) = child
                    && &*opt_el.name == "option" && option_has_rich_content(opt_el) {
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
                        if let Node::ConstTag(const_tag) = child
                            && let Some(decl_text) = const_tag.declaration.render() {
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

                // Create option template with space placeholder
                let template_name = self.add_template("<option> </option>".to_string(), 0);
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
                let template_str = template_str.trim_start_matches(['\n', '\r']);
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
                    self.compile_element_deep(element, &el_var, source, body)?;
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
        if sig_children.len() == 1
            && let Node::RegularElement(el) = sig_children[0]
            && &*el.name == "option" && option_has_rich_content(el) {
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
                        "$.template_effect(() => $.set_text(text, `${{{expr_text} ?? ''}}`);\n"
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
                    if let Node::ConstTag(const_tag) = child
                        && let Some(decl_text) = const_tag.declaration.render() {
                            // render() may already include 'const' prefix
                            if decl_text.starts_with("const ") || decl_text.starts_with("let ") || decl_text.starts_with("var ") {
                                each_body.push_str(&format!("{decl_text};\n"));
                            } else {
                                each_body.push_str(&format!("const {decl_text};\n"));
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
            body.push_str("\t$.each(\n");
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

        if body_children.len() == 1
            && let Node::ExpressionTag(tag) = body_children[0]
            && let Some(expr_text) = tag.expression.render() {
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
            let all_vars: Vec<&str> = info.state_vars.iter().chain(info.async_vars.iter()).map(String::as_str).collect();
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

        for (_test, fragment) in branches.iter() {
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
        for (_test, fragment, _) in &branches[..this_level_count] {
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
            body.push_str("\t$.if(\n");
            body.push_str(&format!("\t\t{anchor_var},\n"));
            body.push_str("\t\t($$render) => {\n");
            body.push_str(&format!("\t\t\t{render_line}\n"));
            body.push_str("\t\t},\n");
            body.push_str("\t\ttrue\n");
            body.push_str("\t);\n");
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
            let end = rest.find([' ', '>', '<', '=', '!', '+', '-', '*', '/', '%', '&', '|']).unwrap_or(rest.len());
            let target = rest[..end].trim().to_string();
            let is_compound = end < rest.len();
            (target, is_compound)
        } else {
            // Contains `await` somewhere inside
            let pos = test.find("await ").unwrap_or(0);
            let after = &test[pos + 6..];
            let end = after.find([' ', '>', '<']).unwrap_or(after.len());
            (after[..end].trim().to_string(), true)
        };

        // Compute promise deps: check if the expression references state/async vars
        let promise_deps = if let Some(ref info) = self.async_run_info {
            let all_vars: Vec<&str> = info.state_vars.iter().chain(info.async_vars.iter()).map(String::as_str).collect();
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
                    let _text_var_name = if i > 0 { format!("text{suffix}") } else { "text".to_string() };
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
        if self.in_select_context && significant.len() == 1
            && let Node::RenderTag(render) = significant[0] {
                let mut body = String::new();
                self.compile_render_tag(render, "$$anchor", source, &mut body);
                return body;
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
        if significant.len() == 1
            && let Node::Text(text) = significant[0] {
                let data = text.data.trim();
                if !data.is_empty() {
                    let text_var = self.var_counter.next("text");
                    let mut result = format!("var {text_var} = $.text('{data}');\n\n");
                    result.push_str(&format!("$.append($$anchor, {text_var});\n"));
                    return result;
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
            if let Node::ConstTag(ct) = node
                && let Some(decl_text) = ct.declaration.render()
                && let Some((name, init)) = parse_const_declaration(&decl_text) {
                    let has_top_level_await = init.contains("await ") && !init.trim_start().starts_with("(async ");
                    const_entries.push((name, init, has_top_level_await));
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
                    else if is_dynamic_attribute_value(&attr.value)
                        && let Some(expr) = render_attribute_value_dynamic(&attr.value) {
                            // Check if expression is impure (contains function call)
                            let is_impure = expr.contains('(') && expr.contains(')');
                            let attr_name_resolved = if is_custom || is_svg {
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
        if let Some(ref expr) = el.expression
            && let Some(tag_expr) = expr.render() {
                body.push_str(&format!("$.element({anchor_var}, {tag_expr}, false);\n"));
        }
        Some(())
    }
}









// ---------------------------------------------------------------------------
// Statement formatting
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Expression rendering
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Template string building with constant folding
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Constant folding / expression analysis
// ---------------------------------------------------------------------------







