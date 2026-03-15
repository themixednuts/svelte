use super::*;
use crate::api::validation::extend_name_set_with_oxc_pattern_bindings;
use crate::api::{
    ElementKind, classify_element_name, is_custom_element_name, is_valid_component_name,
    is_valid_element_name, is_void_element_name,
};
use crate::ast::common::{AttrErrorKind, AttributeValueSyntax, ParseErrorKind, Span};
use crate::ast::modern::{
    Alternate, Attribute, AttributeValue, AttributeValueKind, Component, ConstTag, DebugTag,
    DirectiveAttribute, DirectiveValueSyntax, EachBlock, Element, Entry, Expression, Fragment,
    IfBlock, NamedAttribute, Node, RegularElement, Script, ScriptContext, Search, SnippetBlock,
    SvelteElement, TransitionDirective,
};
use crate::{SourceId, SourceText};
use oxc_ast::ast::{
    ClassElement, Declaration, Expression as OxcExpression, FormalParameter,
    IdentifierReference, ImportDeclarationSpecifier, MethodDefinitionKind, Statement,
    VariableDeclarationKind,
};
use oxc_ast_visit::{Visit, walk};
use oxc_span::{GetSpan, Span as OxcSpan};
use oxc_syntax::scope::ScopeFlags;
use std::{
    collections::{HashMap, HashSet},
    ops::ControlFlow,
    sync::Arc,
};
use svelte_syntax::JsProgram;

impl ComponentValidator<'_> {
    pub(super) fn svelte_meta_structure_errors(&self) -> Option<CompileError> {
        detect_svelte_meta_structure_errors_from_root(self.source(), self.root())
    }

    pub(super) fn parse_error(
        &self,
        runes_mode: bool,
    ) -> Option<CompileError> {
        if let Some(error) = self.root().errors.iter().find(|error| {
            matches!(
                error.kind,
                ParseErrorKind::BlockInvalidContinuationPlacement
            )
        }) {
            return parse_error(self.source(), error, runes_mode);
        }

        // BlockUnexpectedCharacter (e.g. `{ #if}` with space) causes secondary structural
        // errors in tree-sitter. When present, it takes priority: report it in runes mode,
        // suppress all errors in legacy mode (matching JS compiler behavior).
        let has_block_whitespace_error = self.root()
            .errors
            .iter()
            .any(|error| matches!(error.kind, ParseErrorKind::BlockUnexpectedCharacter));

        if has_block_whitespace_error {
            if !runes_mode {
                return None;
            }
            return self.root()
                .errors
                .iter()
                .find(|error| matches!(error.kind, ParseErrorKind::BlockUnexpectedCharacter))
                .and_then(|error| parse_error(self.source(), error, true));
        }

        self.root().errors
            .iter()
            .find_map(|error| parse_error(self.source(), error, runes_mode))
    }

    pub(super) fn tag_invalid_name(&self) -> Option<CompileError> {
        let (start, end) = self.root().fragment.find_map(|entry| match entry.as_node()? {
            Node::RegularElement(element) if !is_valid_element_name(element.name.as_ref()) => {
                opening_tag_name_range(self.source(), element.start)
            }
            Node::Component(component) if !is_valid_component_name(component.name.as_ref()) => {
                opening_tag_name_range(self.source(), component.start)
            }
            _ => None,
        })?;

        Some(compile_error_with_range(
            self.source(),
            DiagnosticKind::TagInvalidName,
            start,
            end,
        ))
    }

    pub(super) fn svelte_self_invalid_placement(&self) -> Option<CompileError> {
        let (start, end) = find_invalid_svelte_self_in_fragment(&self.root().fragment, 0, false)?;
        Some(compile_error_with_range(
            self.source(),
            DiagnosticKind::SvelteSelfInvalidPlacement,
            start,
            end,
        ))
    }

    pub(super) fn each_key_without_as(&self) -> Option<CompileError> {
        let (start, end) = find_each_key_without_as_in_fragment(&self.root().fragment)?;
        Some(compile_error_with_range(
            self.source(),
            DiagnosticKind::EachKeyWithoutAs,
            start,
            end,
        ))
    }

    pub(super) fn each_context_error(&self) -> Option<CompileError> {
        let error = self.root().fragment.find_map(|entry| match entry.as_node()? {
            Node::EachBlock(block) => block.context_error.as_ref(),
            _ => None,
        })?;

        match &error.kind {
            ParseErrorKind::UnexpectedReservedWord { word } => Some(compile_error_custom(
                self.source(),
                "unexpected_reserved_word",
                format!("'{word}' is a reserved word in JavaScript and cannot be used here"),
                error.start,
                error.end,
            )),
            ParseErrorKind::JsParseError { message } => {
                if reserved_word_from_message(message.as_ref()).is_some() {
                    return Some(compile_error_custom(
                        self.source(),
                        "unexpected_reserved_word",
                        message.as_ref(),
                        error.start,
                        error.end,
                    ));
                }
                Some(compile_error_custom(
                    self.source(),
                    "js_parse_error",
                    message.as_ref(),
                    error.start,
                    error.end,
                ))
            }
            _ => None,
        }
    }

    pub(super) fn invalid_arguments_usage(&self) -> Option<CompileError> {
        for script in [&self.root().module, &self.root().instance] {
            let Some(script) = script.as_ref() else {
                continue;
            };
            let offset = script.content_start;
            struct ArgumentsVisitor {
                function_depth: usize,
                found: Option<(usize, usize)>,
            }
            impl<'a> Visit<'a> for ArgumentsVisitor {
                fn visit_function_body(&mut self, body: &oxc_ast::ast::FunctionBody<'a>) {
                    self.function_depth += 1;
                    walk::walk_function_body(self, body);
                    self.function_depth -= 1;
                }

                fn visit_identifier_reference(&mut self, ident: &IdentifierReference<'a>) {
                    if self.found.is_none() && self.function_depth == 0 && ident.name == "arguments" {
                        let span = ident.span();
                        self.found = Some((span.start as usize, span.end as usize));
                    }
                }
            }
            let mut visitor = ArgumentsVisitor {
                function_depth: 0,
                found: None,
            };
            visitor.visit_program(script.content.program());
            if let Some((start, end)) = visitor.found {
                return Some(compile_error_with_range(
                    self.source(),
                    DiagnosticKind::InvalidArgumentsUsage,
                    start + offset,
                    end + offset,
                ));
            }
        }
        None
    }

    pub(super) fn reactive_declaration_cycle(&self) -> Option<CompileError> {
        let script = self.root().instance.as_ref()?;
        let offset = script.content_start;
        let statements = collect_reactive_statements(&script.content);
        if statements.len() < 2 {
            return None;
        }

        let names = statements
            .iter()
            .flat_map(|statement| statement.assignments.iter().cloned())
            .fold(OrderedNames::default(), |mut names, name| {
                names.extend([name]);
                names
            });
        let (names, name_set) = names.into_parts();

        let mut graph = HashMap::<Arc<str>, OrderedNames>::new();
        for statement in &statements {
            for assignment in &statement.assignments {
                let edges = graph.entry(assignment.clone()).or_default();
                for dependency in &statement.dependencies {
                    if statement.assignment_set.contains(dependency.as_ref())
                        || !name_set.contains(dependency.as_ref())
                    {
                        continue;
                    }
                    edges.extend([dependency.clone()]);
                }
            }
        }

        let graph = freeze_name_graph(graph);
        let mut stack = Vec::<Arc<str>>::new();
        let mut active = NameSet::default();
        let mut visited = NameSet::default();
        for name in &names {
            if let Some(cycle) =
                find_reactive_cycle(name.as_ref(), &graph, &mut visited, &mut active, &mut stack)
            {
                let statement = statements
                    .iter()
                    .find(|statement| statement.assignment_set.contains(cycle[0].as_ref()))?;
                return Some(compile_error_custom(
                    self.source(),
                    "reactive_declaration_cycle",
                    format!(
                        "Cyclical dependency detected: {}",
                        cycle
                            .iter()
                            .map(Arc::as_ref)
                            .collect::<Vec<_>>()
                            .join(" \u{2192} ")
                    ),
                    statement.start + offset,
                    statement.end + offset,
                ));
            }
        }

        None
    }

    pub(super) fn missing_directive_name(&self) -> Option<CompileError> {
        detect_missing_directive_name_in_fragment(self.source(), &self.root().fragment)
    }

    pub(super) fn directive_invalid_value(&self) -> Option<CompileError> {
        detect_invalid_directive_value_in_fragment(self.source(), &self.root().fragment)
    }

    pub(super) fn empty_attribute_shorthand(&self) -> Option<CompileError> {
        let start = self.root().fragment.find_map(|entry| {
            let el = entry.as_node()?.as_element()?;
            el.attributes()
                .iter()
                .find_map(empty_attribute_shorthand_start)
        })?;

        Some(compile_error_with_range(
            self.source(),
            DiagnosticKind::AttributeEmptyShorthand,
            start,
            start,
        ))
    }

    pub(super) fn duplicate_attributes(&self) -> Option<CompileError> {
        self.root().fragment.find_map(|entry| {
            let element = entry.as_node()?.as_element()?;
            duplicate_attribute_error(self.source(), element.attributes())
        })
    }

    pub(super) fn debug_tag_invalid_arguments(&self) -> Option<CompileError> {
        let (start, end) = find_debug_tag_invalid_argument_in_fragment(&self.root().fragment)?;
        Some(compile_error_with_range(
            self.source(),
            DiagnosticKind::DebugTagInvalidArguments,
            start,
            end,
        ))
    }

    pub(super) fn template_directive_errors(
        &self,
        runes_mode: bool,
    ) -> Option<CompileError> {
        let mut context = ValidationContext {
            imports: collect_imported_bindings(self.root()),
            immutable: NameStack::from_items(collect_script_constant_bindings(self.root())),
            snippets: NameStack::default(),
            each: NameStack::default(),
            runes: runes_mode,
        };
        detect_template_directive_errors_in_fragment(self.source(), &self.root().fragment, None, &mut context)
    }

    pub(super) fn slot_attribute_errors(&self) -> Option<CompileError> {
        detect_slot_attribute_errors_in_fragment(self.source(), &self.root().fragment, &mut Vec::new())
    }

    pub(super) fn const_tag_errors(
        &self,
        async_mode: bool,
    ) -> Option<CompileError> {
        detect_const_tag_errors_in_fragment(
            self.source(),
            &self.root().fragment,
            ConstOwner::Root,
            &ConstScope::default(),
            async_mode,
        )
    }

    pub(super) fn bind_invalid_value(
        &self,
        runes_mode: bool,
    ) -> Option<CompileError> {
        let mut bindable = NameStack::from_items(collect_bindable_bindings(self.root(), runes_mode));
        detect_bind_invalid_value_in_fragment(self.source(), &self.root().fragment, &mut bindable)
    }

    pub(super) fn bind_invalid_value_warn_mode(
        &self,
        runes_mode: bool,
    ) -> Option<CompileError> {
        let mut bindable = NameStack::from_items(collect_bindable_bindings(self.root(), runes_mode));
        bindable.extend(collect_script_constant_bindings(self.root()));
        detect_bind_invalid_value_in_fragment(self.source(), &self.root().fragment, &mut bindable)
    }

    pub(super) fn constant_binding(&self) -> Option<CompileError> {
        let immutable = NameStack::from_items(collect_script_constant_bindings(self.root()));
        let mut scope = NameStack::default();
        detect_constant_binding_in_fragment(self.source(), &self.root().fragment, &immutable, &mut scope)
    }

    pub(super) fn script_duplicate(&self) -> Option<CompileError> {
        let mut saw_default = false;
        let mut saw_module = false;

        for script in scripts(self.root()) {
            let context = match script_kind(self.source(), script) {
                Ok(context) => context,
                Err(error) => return Some(error),
            };
            match context {
                ScriptContext::Default => {
                    if saw_default {
                        return Some(compile_error_custom(
                            self.source(),
                            "script_duplicate",
                            "A component can have a single top-level `<script>` element and/or a single top-level `<script module>` element",
                            script.start,
                            script.start,
                        ));
                    }
                    saw_default = true;
                }
                ScriptContext::Module => {
                    if saw_module {
                        return Some(compile_error_custom(
                            self.source(),
                            "script_duplicate",
                            "A component can have a single top-level `<script>` element and/or a single top-level `<script module>` element",
                            script.start,
                            script.start,
                        ));
                    }
                    saw_module = true;
                }
            }
        }

        None
    }

    pub(super) fn typescript_invalid_features(&self) -> Option<CompileError> {
        for script in scripts(self.root()) {
            if !script_has_typescript_lang(script) {
                continue;
            }
            let offset = script.content_start;
            if let Some(issue) = find_typescript_invalid_feature(&script.content) {
                return Some(compile_error_custom(
                    self.source(),
                    "typescript_invalid_feature",
                    format!(
                        "TypeScript language features like {} are not natively supported, and their use is generally discouraged. Outside of `<script>` tags, these features are not supported. For use within `<script>` tags, you will need to use a preprocessor to convert it to JavaScript before it gets passed to the Svelte compiler. If you are using `vitePreprocess`, make sure to specifically enable preprocessing script tags (`vitePreprocess({{ script: true }})`)",
                        issue.feature.description()
                    ),
                    issue.start + offset,
                    issue.end + offset,
                ));
            }
        }

        None
    }

    pub(super) fn svelte_options_invalid_namespace(&self) -> Option<CompileError> {
        let options = self.root().options.as_ref()?;

        for attribute in options.attributes.iter() {
            let Attribute::Attribute(attribute) = attribute else {
                continue;
            };
            if attribute.name.as_ref() != "namespace" {
                continue;
            }

            let valid = matches!(
                static_attribute_text(attribute),
                Some(
                    "html"
                        | "mathml"
                        | "svg"
                        | "http://www.w3.org/1998/Math/MathML"
                        | "http://www.w3.org/2000/svg"
                )
            );
            if !valid {
                return Some(compile_error_custom(
                    self.source(),
                    "svelte_options_invalid_attribute_value",
                    "Value must be \"html\", \"mathml\" or \"svg\", if specified",
                    attribute.start,
                    attribute.end,
                ));
            }
        }

        None
    }

    pub(super) fn svelte_options_invalid_custom_element(&self) -> Option<CompileError> {
        let options = self.root().options.as_ref()?;

        for attribute in options.attributes.iter() {
            let Attribute::Attribute(attribute) = attribute else {
                continue;
            };
            if attribute.name.as_ref() != "customElement" {
                continue;
            }

            let tag = match &attribute.value {
                AttributeValueKind::Values(values) => {
                    if values.len() == 1
                        && let Some(AttributeValue::Text(text)) = values.first()
                    {
                        text.data.as_ref()
                    } else {
                        return Some(compile_error_custom(
                            self.source(),
                            "svelte_options_invalid_customelement",
                            "\"customElement\" must be a string literal defining a valid custom element name or an object of the form { tag?: string; shadow?: \"open\" | \"none\" | `ShadowRootInit`; props?: { [key: string]: { attribute?: string; reflect?: boolean; type: .. } } }",
                            attribute.start,
                            attribute.end,
                        ));
                    }
                }
                AttributeValueKind::ExpressionTag(tag) => {
                    if expression_kind(&tag.expression) == Some("ObjectExpression") {
                        continue;
                    }
                    return Some(compile_error_custom(
                        self.source(),
                        "svelte_options_invalid_customelement",
                        "\"customElement\" must be a string literal defining a valid custom element name or an object of the form { tag?: string; shadow?: \"open\" | \"none\" | `ShadowRootInit`; props?: { [key: string]: { attribute?: string; reflect?: boolean; type: .. } } }",
                        attribute.start,
                        attribute.end,
                    ));
                }
                AttributeValueKind::Boolean(_) => {
                    return Some(compile_error_custom(
                        self.source(),
                        "svelte_options_invalid_customelement",
                        "\"customElement\" must be a string literal defining a valid custom element name or an object of the form { tag?: string; shadow?: \"open\" | \"none\" | `ShadowRootInit`; props?: { [key: string]: { attribute?: string; reflect?: boolean; type: .. } } }",
                        attribute.start,
                        attribute.end,
                    ));
                }
            };

            if !is_valid_custom_element_tag_name(tag) {
                return Some(compile_error_custom(
                    self.source(),
                    "svelte_options_invalid_tagname",
                    "Tag name must be lowercase and hyphenated",
                    attribute.start,
                    attribute.end,
                ));
            }
            if is_reserved_custom_element_tag_name(tag) {
                return Some(compile_error_custom(
                    self.source(),
                    "svelte_options_reserved_tagname",
                    "Tag name is reserved",
                    attribute.start,
                    attribute.end,
                ));
            }
        }

        None
    }

    pub(super) fn let_directive_invalid_placement(&self) -> Option<CompileError> {
        detect_let_directive_invalid_placement_in_fragment(self.source(), &self.root().fragment)
    }

    pub(super) fn style_directive_invalid_modifier(&self) -> Option<CompileError> {
        detect_style_directive_invalid_modifier_in_fragment(self.source(), &self.root().fragment)
    }

    pub(super) fn svelte_fragment_invalid_placement(&self) -> Option<CompileError> {
        detect_svelte_fragment_invalid_placement_in_fragment(self.source(), &self.root().fragment, false)
    }

    pub(super) fn svelte_head_illegal_attribute(&self) -> Option<CompileError> {
        detect_svelte_head_illegal_attribute_in_fragment(self.source(), &self.root().fragment)
    }

    pub(super) fn text_content_model_errors(&self) -> Option<CompileError> {
        detect_text_content_model_errors_in_fragment(self.source(), &self.root().fragment)
    }

    pub(super) fn mixed_event_handler_syntax(
        &self,
        runes_mode: bool,
    ) -> Option<CompileError> {
        if !runes_mode {
            return None;
        }
        if !fragment_has_modern_dom_event_syntax(&self.root().fragment) {
            return None;
        }
        detect_mixed_event_handler_syntax_in_fragment(self.source(), &self.root().fragment)
    }

    pub(super) fn snippet_shadowing_prop(&self) -> Option<CompileError> {
        for node in self.root().fragment.nodes.iter() {
            let Node::Component(component) = node else {
                continue;
            };

            let prop_names = component
                .attributes
                .iter()
                .filter_map(component_prop_attribute_name)
                .collect::<NameSet>();
            if prop_names.is_empty() {
                continue;
            }

            if let Some((snippet_name, start, end)) =
                find_component_scope_snippet_with_name(&component.fragment, &prop_names)
            {
                return Some(compile_error_custom(
                    self.source(),
                    "snippet_shadowing_prop",
                    format!("This snippet is shadowing the prop `{snippet_name}` with the same name"),
                    start,
                    end,
                ));
            }
        }

        None
    }

    pub(super) fn additional_template_structure_errors(&self) -> Option<CompileError> {
        detect_additional_template_structure_errors_in_fragment(
            self.source(),
            &self.root().fragment,
            StructureContext::default(),
        )
    }

    pub(super) fn attribute_invalid_name(&self) -> Option<CompileError> {
        detect_attribute_invalid_name_in_fragment(self.source(), &self.root().fragment)
    }

    pub(super) fn attribute_syntax(&self) -> Option<CompileError> {
        detect_attribute_syntax_in_fragment(self.source(), &self.root().fragment)
    }
}


fn parse_error(
    source: &str,
    error: &crate::ast::common::ParseError,
    runes_mode: bool,
) -> Option<CompileError> {
    let kind = match error.kind {
        ParseErrorKind::BlockInvalidContinuationPlacement => {
            DiagnosticKind::BlockInvalidContinuationPlacement
        }
        ParseErrorKind::ExpectedTokenElse => DiagnosticKind::ExpectedTokenElse,
        ParseErrorKind::ExpectedTokenAwaitBranch => {
            DiagnosticKind::ExpectedTokenAwaitBranch
        }
        ParseErrorKind::ExpectedTokenCommentClose => {
            DiagnosticKind::ExpectedTokenCommentClose
        }
        ParseErrorKind::ExpectedTokenStyleClose => DiagnosticKind::ExpectedTokenStyleClose,
        ParseErrorKind::ExpectedTokenRightBrace => DiagnosticKind::ExpectedTokenRightBrace,
        ParseErrorKind::ExpectedWhitespace => DiagnosticKind::ExpectedWhitespace,
        ParseErrorKind::BlockUnexpectedCharacter => {
            if !runes_mode {
                return None;
            }

            return Some(compile_error_custom(
                source,
                "block_unexpected_character",
                "Expected a `#` character immediately following the opening bracket",
                error.start,
                error.end,
            ));
        }
        ParseErrorKind::UnexpectedReservedWord { ref word } => {
            return Some(compile_error_custom(
                source,
                "unexpected_reserved_word",
                format!("'{word}' is a reserved word in JavaScript and cannot be used here"),
                error.start,
                error.end,
            ));
        }
        ParseErrorKind::JsParseError { ref message } => {
            return Some(compile_error_custom(
                source,
                "js_parse_error",
                message.as_ref(),
                error.start,
                error.end,
            ));
        }
        ParseErrorKind::CssExpectedIdentifier => DiagnosticKind::CssExpectedIdentifier,
        ParseErrorKind::UnexpectedEof => DiagnosticKind::UnexpectedEof,
        ParseErrorKind::BlockUnclosed => DiagnosticKind::BlockUnclosed,
        ParseErrorKind::ElementUnclosed { ref name } => {
            return Some(compile_error_custom(
                source,
                "element_unclosed",
                format!("`<{name}>` was left open"),
                error.start,
                error.end,
            ));
        }
        ParseErrorKind::ElementInvalidClosingTag { ref name } => {
            if is_void_element_name(name.as_ref()) {
                return Some(compile_error_with_range(
                    source,
                    DiagnosticKind::VoidElementInvalidContent,
                    error.start,
                    error.end,
                ));
            }
            return Some(compile_error_custom(
                source,
                "element_invalid_closing_tag",
                format!("`</{name}>` attempted to close an element that was not open"),
                error.start,
                error.end,
            ));
        }
        ParseErrorKind::ElementInvalidClosingTagAutoclosed {
            ref name,
            ref reason,
        } => {
            return Some(compile_error_custom(
                source,
                "element_invalid_closing_tag_autoclosed",
                format!(
                    "`</{name}>` attempted to close element that was already automatically closed by `<{reason}>` (cannot nest `<{reason}>` inside `<{name}>`)"
                ),
                error.start,
                error.end,
            ));
        }
    };

    Some(compile_error_with_range(
        source,
        kind,
        error.start,
        error.end,
    ))
}

#[derive(Default)]
struct SvelteMetaScanState {
    head_count: usize,
    window_count: usize,
    document_count: usize,
    body_count: usize,
}

fn name_range(name: &crate::ast::common::SourceRange) -> (usize, usize) {
    (name.start.character, name.end.character)
}

fn opening_tag_name_range(source: &str, tag_start: usize) -> Option<(usize, usize)> {
    let start = tag_start.checked_add(1)?;
    let rest = source.get(start..)?;
    let name_end = rest
        .char_indices()
        .find_map(|(idx, ch)| {
            (ch.is_whitespace() || matches!(ch, '/' | '>')).then_some(start + idx)
        })
        .unwrap_or(source.len());
    Some((start, name_end))
}

#[derive(Clone, Copy)]
enum SvelteMetaKind {
    Head,
    Window,
    Document,
    Body,
}

impl SvelteMetaKind {
    fn invalid_content(self) -> DiagnosticKind {
        match self {
            Self::Head => DiagnosticKind::SvelteMetaInvalidContent,
            Self::Window => DiagnosticKind::SvelteWindowInvalidContent,
            Self::Document | Self::Body => DiagnosticKind::SvelteMetaInvalidContent,
        }
    }
}

fn detect_svelte_meta_structure_errors_from_root(
    source: &str,
    root: &Root,
) -> Option<CompileError> {
    if let Some(options) = root.options.as_ref()
        && let Some((start, end)) = first_non_whitespace_fragment_range(&options.fragment)
    {
        return Some(compile_error_with_range(
            source,
            DiagnosticKind::SvelteMetaInvalidContent,
            start,
            end,
        ));
    }

    let mut state = SvelteMetaScanState::default();
    scan_modern_fragment_for_svelte_meta(source, &root.fragment, 0, 0, &mut state)
}

fn scan_root_meta(
    source: &str,
    kind: SvelteMetaKind,
    start: usize,
    fragment: &Fragment,
    count: &mut usize,
    depth: MetaDepth,
    allow_children: bool,
) -> Result<(), CompileError> {
    let MetaDepth {
        element: element_depth,
        block: block_depth,
    } = depth;
    if element_depth > 0 || block_depth > 0 {
        return Err(compile_error_with_range(
            source,
            DiagnosticKind::SvelteMetaInvalidPlacement,
            start,
            start,
        ));
    }

    *count += 1;
    if *count > 1 {
        return Err(compile_error_with_range(
            source,
            DiagnosticKind::SvelteMetaDuplicate,
            start,
            start,
        ));
    }

    if !allow_children && let Some((start, end)) = first_non_whitespace_fragment_range(fragment) {
        return Err(compile_error_with_range(
            source,
            kind.invalid_content(),
            start,
            end,
        ));
    }

    Ok(())
}

#[derive(Clone, Copy)]
struct MetaDepth {
    element: usize,
    block: usize,
}

fn scan_modern_fragment_for_svelte_meta(
    source: &str,
    fragment: &Fragment,
    element_depth: usize,
    block_depth: usize,
    state: &mut SvelteMetaScanState,
) -> Option<CompileError> {
    for node in fragment.nodes.iter() {
        if let Some(error) =
            scan_modern_node_for_svelte_meta(source, node, element_depth, block_depth, state)
        {
            return Some(error);
        }
    }
    None
}

fn scan_modern_node_for_svelte_meta(
    source: &str,
    node: &Node,
    element_depth: usize,
    block_depth: usize,
    state: &mut SvelteMetaScanState,
) -> Option<CompileError> {
    match node {
        Node::RegularElement(element) => {
            let name = element.name.as_ref();
            if let ElementKind::Svelte(kind) = classify_element_name(name)
                && !kind.is_known()
            {
                let (start, end) = opening_tag_name_range(source, element.start)
                    .unwrap_or_else(|| name_range(&element.name_loc));
                return Some(compile_error_with_range(
                    source,
                    DiagnosticKind::SvelteMetaInvalidTag,
                    start,
                    end,
                ));
            }

            scan_modern_fragment_for_svelte_meta(
                source,
                &element.fragment,
                element_depth + 1,
                block_depth,
                state,
            )
        }
        Node::SvelteHead(element) => {
            if let Err(error) = scan_root_meta(
                source,
                SvelteMetaKind::Head,
                element.start,
                &element.fragment,
                &mut state.head_count,
                MetaDepth {
                    element: element_depth,
                    block: block_depth,
                },
                true,
            ) {
                return Some(error);
            }
            scan_modern_fragment_for_svelte_meta(
                source,
                &element.fragment,
                element_depth + 1,
                block_depth,
                state,
            )
        }
        Node::SvelteWindow(element) => {
            if let Err(error) = scan_root_meta(
                source,
                SvelteMetaKind::Window,
                element.start,
                &element.fragment,
                &mut state.window_count,
                MetaDepth {
                    element: element_depth,
                    block: block_depth,
                },
                false,
            ) {
                return Some(error);
            }
            scan_modern_fragment_for_svelte_meta(
                source,
                &element.fragment,
                element_depth + 1,
                block_depth,
                state,
            )
        }
        Node::SvelteDocument(element) => {
            if let Err(error) = scan_root_meta(
                source,
                SvelteMetaKind::Document,
                element.start,
                &element.fragment,
                &mut state.document_count,
                MetaDepth {
                    element: element_depth,
                    block: block_depth,
                },
                false,
            ) {
                return Some(error);
            }
            scan_modern_fragment_for_svelte_meta(
                source,
                &element.fragment,
                element_depth + 1,
                block_depth,
                state,
            )
        }
        Node::SvelteBody(element) => {
            if let Err(error) = scan_root_meta(
                source,
                SvelteMetaKind::Body,
                element.start,
                &element.fragment,
                &mut state.body_count,
                MetaDepth {
                    element: element_depth,
                    block: block_depth,
                },
                false,
            ) {
                return Some(error);
            }
            scan_modern_fragment_for_svelte_meta(
                source,
                &element.fragment,
                element_depth + 1,
                block_depth,
                state,
            )
        }
        Node::Component(component) => scan_modern_fragment_for_svelte_meta(
            source,
            &component.fragment,
            element_depth + 1,
            block_depth,
            state,
        ),
        Node::SlotElement(slot) => scan_modern_fragment_for_svelte_meta(
            source,
            &slot.fragment,
            element_depth + 1,
            block_depth,
            state,
        ),
        Node::IfBlock(block) => {
            scan_modern_if_block_for_svelte_meta(source, block, element_depth, block_depth, state)
        }
        Node::EachBlock(block) => {
            if let Some(error) = scan_modern_fragment_for_svelte_meta(
                source,
                &block.body,
                element_depth,
                block_depth + 1,
                state,
            ) {
                return Some(error);
            }
            if let Some(fallback) = block.fallback.as_ref() {
                return scan_modern_fragment_for_svelte_meta(
                    source,
                    fallback,
                    element_depth,
                    block_depth + 1,
                    state,
                );
            }
            None
        }
        Node::AwaitBlock(block) => {
            for branch in [
                block.pending.as_ref(),
                block.then.as_ref(),
                block.catch.as_ref(),
            ] {
                if let Some(fragment) = branch
                    && let Some(error) = scan_modern_fragment_for_svelte_meta(
                        source,
                        fragment,
                        element_depth,
                        block_depth + 1,
                        state,
                    )
                {
                    return Some(error);
                }
            }
            None
        }
        Node::SnippetBlock(block) => scan_modern_fragment_for_svelte_meta(
            source,
            &block.body,
            element_depth,
            block_depth + 1,
            state,
        ),
        Node::KeyBlock(block) => scan_modern_fragment_for_svelte_meta(
            source,
            &block.fragment,
            element_depth,
            block_depth + 1,
            state,
        ),
        _ => None,
    }
}

fn scan_modern_alternate_for_svelte_meta(
    source: &str,
    alternate: &Alternate,
    element_depth: usize,
    block_depth: usize,
    state: &mut SvelteMetaScanState,
) -> Option<CompileError> {
    match alternate {
        Alternate::Fragment(fragment) => scan_modern_fragment_for_svelte_meta(
            source,
            fragment,
            element_depth,
            block_depth,
            state,
        ),
        Alternate::IfBlock(block) => {
            scan_modern_if_block_for_svelte_meta(source, block, element_depth, block_depth, state)
        }
    }
}

fn scan_modern_if_block_for_svelte_meta(
    source: &str,
    block: &IfBlock,
    element_depth: usize,
    block_depth: usize,
    state: &mut SvelteMetaScanState,
) -> Option<CompileError> {
    if let Some(error) = scan_modern_fragment_for_svelte_meta(
        source,
        &block.consequent,
        element_depth,
        block_depth + 1,
        state,
    ) {
        return Some(error);
    }
    if let Some(alternate) = block.alternate.as_ref() {
        return scan_modern_alternate_for_svelte_meta(
            source,
            alternate,
            element_depth,
            block_depth + 1,
            state,
        );
    }
    None
}

fn first_non_whitespace_fragment_range(fragment: &Fragment) -> Option<(usize, usize)> {
    for node in fragment.nodes.iter() {
        if let Node::Text(text) = node {
            if text.data.chars().all(char::is_whitespace) {
                continue;
            }
            return Some((text.start, text.end));
        }
        return Some((node.start(), node.end()));
    }
    None
}

fn reserved_word_from_message(message: &str) -> Option<&str> {
    let prefix = "'";
    let middle = "' is a reserved word in JavaScript and cannot be used here";
    let word = message.strip_prefix(prefix)?.strip_suffix(middle)?;
    (!word.is_empty()).then_some(word)
}

struct ReactiveStatement {
    assignments: Box<[Arc<str>]>,
    assignment_set: NameSet,
    dependencies: Box<[Arc<str>]>,
    start: usize,
    end: usize,
}

type NameGraph = HashMap<Arc<str>, Box<[Arc<str>]>>;

fn collect_reactive_statements(program: &JsProgram) -> Vec<ReactiveStatement> {
    struct AssignmentCollector {
        names: OrderedNames,
        function_depth: usize,
    }

    impl<'a> Visit<'a> for AssignmentCollector {
        fn visit_function(&mut self, func: &oxc_ast::ast::Function<'a>, flags: ScopeFlags) {
            self.function_depth += 1;
            walk::walk_function(self, func, flags);
            self.function_depth -= 1;
        }

        fn visit_arrow_function_expression(
            &mut self,
            expr: &oxc_ast::ast::ArrowFunctionExpression<'a>,
        ) {
            self.function_depth += 1;
            walk::walk_arrow_function_expression(self, expr);
            self.function_depth -= 1;
        }

        fn visit_assignment_target(&mut self, target: &oxc_ast::ast::AssignmentTarget<'a>) {
            if self.function_depth == 0 {
                let mut set = NameSet::default();
                collect_assignment_target_names(target, &mut set);
                for name in set {
                    self.names.extend([name]);
                }
            }
            walk::walk_assignment_target(self, target);
        }

        fn visit_update_expression(&mut self, expr: &oxc_ast::ast::UpdateExpression<'a>) {
            if self.function_depth == 0
                && let Some(name) = expr.argument.get_identifier_name()
            {
                self.names.extend([Arc::from(name)]);
            }
            walk::walk_update_expression(self, expr);
        }
    }

    struct DependencyCollector {
        names: OrderedNames,
        assigned: NameSet,
        function_depth: usize,
    }

    impl<'a> Visit<'a> for DependencyCollector {
        fn visit_function(&mut self, func: &oxc_ast::ast::Function<'a>, flags: ScopeFlags) {
            self.function_depth += 1;
            walk::walk_function(self, func, flags);
            self.function_depth -= 1;
        }

        fn visit_arrow_function_expression(
            &mut self,
            expr: &oxc_ast::ast::ArrowFunctionExpression<'a>,
        ) {
            self.function_depth += 1;
            walk::walk_arrow_function_expression(self, expr);
            self.function_depth -= 1;
        }

        fn visit_identifier_reference(&mut self, ident: &IdentifierReference<'a>) {
            if self.function_depth == 0 && !self.assigned.contains(ident.name.as_str()) {
                self.names.extend([Arc::from(ident.name.as_str())]);
            }
        }
    }

    let mut out = Vec::new();

    for statement in &program.program().body {
        let Statement::LabeledStatement(labeled) = statement else {
            continue;
        };
        if labeled.label.name.as_str() != "$" {
            continue;
        }

        let mut assignment_collector = AssignmentCollector {
            names: OrderedNames::default(),
            function_depth: 0,
        };
        assignment_collector.visit_statement(&labeled.body);

        let mut assignment_set = NameSet::default();
        for name in assignment_collector.names.as_slice() {
            assignment_set.insert(name.clone());
        }

        let mut dependency_collector = DependencyCollector {
            names: OrderedNames::default(),
            assigned: assignment_set.clone(),
            function_depth: 0,
        };
        dependency_collector.visit_statement(&labeled.body);

        out.push(ReactiveStatement {
            assignments: assignment_collector.names.into_boxed_slice(),
            assignment_set,
            dependencies: dependency_collector.names.into_boxed_slice(),
            start: labeled.span.start as usize,
            end: labeled.span.end as usize,
        });
    }

    out
}

fn collect_assignment_target_names(
    target: &oxc_ast::ast::AssignmentTarget<'_>,
    out: &mut NameSet,
) {
    match target {
        oxc_ast::ast::AssignmentTarget::AssignmentTargetIdentifier(identifier) => {
            out.insert(Arc::from(identifier.name.as_str()));
        }
        oxc_ast::ast::AssignmentTarget::ComputedMemberExpression(_)
        | oxc_ast::ast::AssignmentTarget::StaticMemberExpression(_)
        | oxc_ast::ast::AssignmentTarget::PrivateFieldExpression(_)
        | oxc_ast::ast::AssignmentTarget::TSAsExpression(_)
        | oxc_ast::ast::AssignmentTarget::TSSatisfiesExpression(_)
        | oxc_ast::ast::AssignmentTarget::TSNonNullExpression(_)
        | oxc_ast::ast::AssignmentTarget::TSTypeAssertion(_) => {}
        oxc_ast::ast::AssignmentTarget::ArrayAssignmentTarget(pattern) => {
            for element in &pattern.elements {
                if let Some(target) = element.as_ref() {
                    collect_assignment_target_maybe_default(target, out);
                }
            }
            if let Some(rest) = pattern.rest.as_ref() {
                collect_assignment_target_names(&rest.target, out);
            }
        }
        oxc_ast::ast::AssignmentTarget::ObjectAssignmentTarget(pattern) => {
            for property in &pattern.properties {
                match property {
                    oxc_ast::ast::AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(
                        property,
                    ) => {
                        out.insert(Arc::from(property.binding.name.as_str()));
                    }
                    oxc_ast::ast::AssignmentTargetProperty::AssignmentTargetPropertyProperty(
                        property,
                    ) => {
                        collect_assignment_target_maybe_default(&property.binding, out);
                    }
                }
            }
            if let Some(rest) = pattern.rest.as_ref() {
                collect_assignment_target_names(&rest.target, out);
            }
        }
    }
}

fn collect_assignment_target_maybe_default(
    target: &oxc_ast::ast::AssignmentTargetMaybeDefault<'_>,
    out: &mut NameSet,
) {
    match target {
        oxc_ast::ast::AssignmentTargetMaybeDefault::AssignmentTargetIdentifier(identifier) => {
            out.insert(Arc::from(identifier.name.as_str()));
        }
        oxc_ast::ast::AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(target) => {
            collect_assignment_target_names(&target.binding, out);
        }
        oxc_ast::ast::AssignmentTargetMaybeDefault::ArrayAssignmentTarget(pattern) => {
            for element in &pattern.elements {
                if let Some(target) = element.as_ref() {
                    collect_assignment_target_maybe_default(target, out);
                }
            }
            if let Some(rest) = pattern.rest.as_ref() {
                collect_assignment_target_names(&rest.target, out);
            }
        }
        oxc_ast::ast::AssignmentTargetMaybeDefault::ObjectAssignmentTarget(pattern) => {
            for property in &pattern.properties {
                match property {
                    oxc_ast::ast::AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(
                        property,
                    ) => {
                        out.insert(Arc::from(property.binding.name.as_str()));
                    }
                    oxc_ast::ast::AssignmentTargetProperty::AssignmentTargetPropertyProperty(
                        property,
                    ) => {
                        collect_assignment_target_maybe_default(&property.binding, out);
                    }
                }
            }
            if let Some(rest) = pattern.rest.as_ref() {
                collect_assignment_target_names(&rest.target, out);
            }
        }
        _ => {}
    }
}

fn find_reactive_cycle(
    name: &str,
    graph: &NameGraph,
    visited: &mut NameSet,
    active: &mut NameSet,
    stack: &mut Vec<Arc<str>>,
) -> Option<Vec<Arc<str>>> {
    if let Some(index) = stack.iter().position(|entry| entry.as_ref() == name) {
        let mut cycle = stack[index..].to_vec();
        cycle.push(name.into());
        return Some(cycle);
    }
    if active.contains(name) || visited.contains(name) {
        return None;
    }

    active.insert(name.into());
    stack.push(name.into());

    if let Some(dependencies) = graph.get(name) {
        for dependency in dependencies {
            if let Some(cycle) = find_reactive_cycle(dependency, graph, visited, active, stack) {
                return Some(cycle);
            }
        }
    }

    stack.pop();
    active.remove(name);
    visited.insert(name.into());
    None
}

fn freeze_name_graph(graph: HashMap<Arc<str>, OrderedNames>) -> NameGraph {
    graph
        .into_iter()
        .map(|(name, dependencies)| (name, dependencies.into_boxed_slice()))
        .collect()
}

fn find_invalid_svelte_self_in_fragment(
    fragment: &Fragment,
    block_depth: usize,
    inside_component: bool,
) -> Option<(usize, usize)> {
    for node in fragment.nodes.iter() {
        let Some(span) = find_invalid_svelte_self_in_node(node, block_depth, inside_component)
        else {
            continue;
        };
        return Some(span);
    }
    None
}

fn find_invalid_svelte_self_in_node(
    node: &Node,
    block_depth: usize,
    inside_component: bool,
) -> Option<(usize, usize)> {
    match node {
        Node::SvelteSelf(el) => {
            if block_depth == 0 && !inside_component && !element_has_slot_attribute(&el.attributes)
            {
                return Some((el.start, el.end));
            }
            find_invalid_svelte_self_in_fragment(&el.fragment, block_depth, true)
        }
        Node::RegularElement(element) => {
            find_invalid_svelte_self_in_fragment(&element.fragment, block_depth, inside_component)
        }
        Node::Component(component) => {
            find_invalid_svelte_self_in_fragment(&component.fragment, block_depth, true)
        }
        Node::SlotElement(slot) => {
            find_invalid_svelte_self_in_fragment(&slot.fragment, block_depth, inside_component)
        }
        Node::IfBlock(block) => {
            find_invalid_svelte_self_in_if_block(block, block_depth, inside_component)
        }
        Node::EachBlock(block) => {
            if let Some(span) =
                find_invalid_svelte_self_in_fragment(&block.body, block_depth + 1, inside_component)
            {
                return Some(span);
            }
            if let Some(fallback) = block.fallback.as_ref() {
                return find_invalid_svelte_self_in_fragment(
                    fallback,
                    block_depth + 1,
                    inside_component,
                );
            }
            None
        }
        Node::AwaitBlock(block) => {
            for fragment in [
                block.pending.as_ref(),
                block.then.as_ref(),
                block.catch.as_ref(),
            ] {
                if let Some(fragment) = fragment
                    && let Some(span) = find_invalid_svelte_self_in_fragment(
                        fragment,
                        block_depth,
                        inside_component,
                    )
                {
                    return Some(span);
                }
            }
            None
        }
        Node::SnippetBlock(block) => {
            find_invalid_svelte_self_in_fragment(&block.body, block_depth + 1, inside_component)
        }
        Node::KeyBlock(block) => {
            find_invalid_svelte_self_in_fragment(&block.fragment, block_depth, inside_component)
        }
        Node::SvelteHead(_)
        | Node::SvelteBody(_)
        | Node::SvelteWindow(_)
        | Node::SvelteDocument(_)
        | Node::SvelteComponent(_)
        | Node::SvelteElement(_)
        | Node::SvelteFragment(_)
        | Node::SvelteBoundary(_)
        | Node::TitleElement(_) => {
            let fragment = node.as_element().unwrap().fragment();
            let inside_component = inside_component || matches!(node, Node::SvelteComponent(_));
            find_invalid_svelte_self_in_fragment(fragment, block_depth, inside_component)
        }
        _ => None,
    }
}

fn find_invalid_svelte_self_in_if_block(
    block: &IfBlock,
    block_depth: usize,
    inside_component: bool,
) -> Option<(usize, usize)> {
    if let Some(span) =
        find_invalid_svelte_self_in_fragment(&block.consequent, block_depth + 1, inside_component)
    {
        return Some(span);
    }
    match block.alternate.as_deref() {
        Some(Alternate::Fragment(fragment)) => {
            find_invalid_svelte_self_in_fragment(fragment, block_depth + 1, inside_component)
        }
        Some(Alternate::IfBlock(block)) => {
            find_invalid_svelte_self_in_if_block(block, block_depth, inside_component)
        }
        None => None,
    }
}

fn element_has_slot_attribute(attributes: &[Attribute]) -> bool {
    attributes.iter().any(|attribute| match attribute {
        Attribute::Attribute(attribute) => attribute.name.as_ref() == "slot",
        _ => false,
    })
}

fn find_each_key_without_as_in_fragment(fragment: &Fragment) -> Option<(usize, usize)> {
    fragment.find_map(|entry| match entry.as_node()? {
        Node::EachBlock(block) if block.key.is_some() && block.context.is_none() => {
            Some((block.start, block.end))
        }
        _ => None,
    })
}

fn detect_missing_directive_name_in_fragment(
    source: &str,
    fragment: &Fragment,
) -> Option<CompileError> {
    fragment.find_map(|entry| {
        let el = entry.as_node()?.as_element()?;
        detect_missing_directive_name_in_attributes(source, el.attributes())
    })
}

fn detect_missing_directive_name_in_attributes(
    source: &str,
    attributes: &[Attribute],
) -> Option<CompileError> {
    for attribute in attributes {
        let (directive, start, end, is_missing) = match attribute {
            Attribute::BindDirective(attribute) => (
                "bind",
                attribute.start,
                attribute.name_loc.end.character,
                attribute.name.is_empty(),
            ),
            Attribute::OnDirective(attribute) => (
                "on",
                attribute.start,
                attribute.name_loc.end.character,
                attribute.name.is_empty(),
            ),
            Attribute::ClassDirective(attribute) => (
                "class",
                attribute.start,
                attribute.name_loc.end.character,
                attribute.name.is_empty(),
            ),
            Attribute::LetDirective(attribute) => (
                "let",
                attribute.start,
                attribute.name_loc.end.character,
                attribute.name.is_empty(),
            ),
            Attribute::StyleDirective(attribute) => (
                "style",
                attribute.start,
                attribute.name_loc.end.character,
                attribute.name.is_empty(),
            ),
            Attribute::TransitionDirective(attribute) => (
                transition_directive_prefix(attribute),
                attribute.start,
                attribute.name_loc.end.character,
                attribute.name.is_empty(),
            ),
            Attribute::AnimateDirective(attribute) => (
                "animate",
                attribute.start,
                attribute.name_loc.end.character,
                attribute.name.is_empty(),
            ),
            Attribute::UseDirective(attribute) => (
                "use",
                attribute.start,
                attribute.name_loc.end.character,
                attribute.name.is_empty(),
            ),
            Attribute::Attribute(_) | Attribute::SpreadAttribute(_) | Attribute::AttachTag(_) => {
                continue;
            }
        };

        if is_missing {
            return Some(compile_error_with_range(
                source,
                DiagnosticKind::DirectiveMissingName {
                    directive: Arc::from(directive),
                },
                start,
                end,
            ));
        }
    }
    None
}

fn detect_invalid_directive_value_in_fragment(
    source: &str,
    fragment: &Fragment,
) -> Option<CompileError> {
    fragment.find_map(|entry| {
        let el = entry.as_node()?.as_element()?;
        detect_invalid_directive_value_in_attributes(source, el.attributes())
    })
}

fn detect_invalid_directive_value_in_attributes(
    source: &str,
    attributes: &[Attribute],
) -> Option<CompileError> {
    for attribute in attributes {
        let invalid_start = match attribute {
            Attribute::BindDirective(attribute)
            | Attribute::OnDirective(attribute)
            | Attribute::ClassDirective(attribute)
            | Attribute::AnimateDirective(attribute)
            | Attribute::UseDirective(attribute)
                if matches!(attribute.value_syntax, DirectiveValueSyntax::Invalid) =>
            {
                Some(attribute.value_start)
            }
            Attribute::TransitionDirective(attribute)
                if matches!(attribute.value_syntax, DirectiveValueSyntax::Invalid) =>
            {
                Some(attribute.value_start)
            }
            _ => None,
        };

        if let Some(position) = invalid_start {
            return Some(compile_error_custom(
                source,
                "directive_invalid_value",
                "Directive value must be a JavaScript expression enclosed in curly braces",
                position,
                position,
            ));
        }
    }
    None
}

fn transition_directive_prefix(attribute: &TransitionDirective) -> &'static str {
    match (attribute.intro, attribute.outro) {
        (true, true) => "transition",
        (true, false) => "in",
        (false, true) => "out",
        (false, false) => "transition",
    }
}

fn empty_attribute_shorthand_start(attribute: &Attribute) -> Option<usize> {
    let Attribute::Attribute(attribute) = attribute else {
        return None;
    };
    if !attribute.name.is_empty() {
        return None;
    }

    let AttributeValueKind::ExpressionTag(tag) = &attribute.value else {
        return None;
    };
    if tag
        .expression
        .identifier_name()
        .is_some_and(|name| !name.is_empty())
    {
        return None;
    }
    // `{this}` shorthand is handled as `unexpected_reserved_word` by svelte:element validation
    if expression_kind(&tag.expression) == Some("ThisExpression") {
        return None;
    }

    Some(attribute.start)
}

fn find_debug_tag_invalid_argument_in_fragment(fragment: &Fragment) -> Option<(usize, usize)> {
    for node in &fragment.nodes {
        let Some(span) = find_debug_tag_invalid_argument_in_node(node) else {
            continue;
        };
        return Some(span);
    }
    None
}

fn find_debug_tag_invalid_argument_in_node(node: &Node) -> Option<(usize, usize)> {
    match node {
        Node::DebugTag(tag) => debug_tag_invalid_argument_span(tag),
        _ => match node.try_for_each_child_fragment(|fragment| {
            match find_debug_tag_invalid_argument_in_fragment(fragment) {
                Some(span) => std::ops::ControlFlow::Break(span),
                None => std::ops::ControlFlow::Continue(()),
            }
        }) {
            std::ops::ControlFlow::Break(span) => Some(span),
            std::ops::ControlFlow::Continue(()) => None,
        },
    }
}

fn debug_tag_invalid_argument_span(tag: &DebugTag) -> Option<(usize, usize)> {
    for argument in &tag.arguments {
        if argument.identifier_name().is_some() {
            continue;
        }
        let start = argument.start;
        return Some((start, start));
    }
    None
}

fn find_error_in_child_fragments(
    node: &Node,
    mut visit: impl FnMut(&Fragment) -> Option<CompileError>,
) -> Option<CompileError> {
    match node.try_for_each_child_fragment(|fragment| match visit(fragment) {
        Some(error) => std::ops::ControlFlow::Break(error),
        None => std::ops::ControlFlow::Continue(()),
    }) {
        std::ops::ControlFlow::Break(error) => Some(error),
        std::ops::ControlFlow::Continue(()) => None,
    }
}

fn find_error_in_child_fragments_with_scope(
    node: &Node,
    scope: &mut NameStack,
    mut visit: impl FnMut(&Fragment, &mut NameStack) -> Option<CompileError>,
) -> Option<CompileError> {
    match node.try_for_each_child_fragment(|fragment| match visit(fragment, scope) {
        Some(error) => std::ops::ControlFlow::Break(error),
        None => std::ops::ControlFlow::Continue(()),
    }) {
        std::ops::ControlFlow::Break(error) => Some(error),
        std::ops::ControlFlow::Continue(()) => None,
    }
}

struct ConstCycle<'a> {
    tag: &'a ConstTag,
    names: Box<[Arc<str>]>,
}

fn compile_error_custom(
    source: &str,
    code: &'static str,
    message: impl Into<Arc<str>>,
    start: usize,
    end: usize,
) -> CompileError {
    let source_text = SourceText::new(SourceId::new(0), source, None);
    let start_location = source_text.location_at_offset(start);
    let end_location = source_text.location_at_offset(end);

    CompileError {
        code: Arc::from(code),
        message: message.into(),
        position: Some(Box::new(SourcePosition {
            start: start_location.character,
            end: end_location.character,
        })),
        start: Some(Box::new(start_location)),
        end: Some(Box::new(end_location)),
        filename: None,
    }
}

fn scripts(root: &Root) -> Vec<&Script> {
    if !root.scripts.is_empty() {
        return root.scripts.iter().collect();
    }
    if !root.js.is_empty() {
        return root.js.iter().collect();
    }

    let mut out = Vec::with_capacity(2);
    if let Some(module) = root.module.as_ref() {
        out.push(module);
    }
    if let Some(instance) = root.instance.as_ref() {
        out.push(instance);
    }
    out
}

fn script_kind(source: &str, script: &Script) -> Result<ScriptContext, CompileError> {
    let mut context = ScriptContext::Default;

    for attribute in &script.attributes {
        let Attribute::Attribute(attribute) = attribute else {
            continue;
        };

        match attribute.name.as_ref() {
            "module" => {
                context = ScriptContext::Module;
            }
            "context" => {
                if static_attribute_text(attribute) != Some("module") {
                    return Err(compile_error_custom(
                        source,
                        "script_invalid_context",
                        "If the context attribute is supplied, its value must be \"module\"",
                        attribute.start,
                        attribute.end,
                    ));
                }
                context = ScriptContext::Module;
            }
            _ => {}
        }
    }

    Ok(context)
}

fn script_has_typescript_lang(script: &Script) -> bool {
    script.attributes.iter().any(|attribute| {
        matches!(
            attribute,
            Attribute::Attribute(attribute) if attribute.name.as_ref() == "lang"
                && matches!(static_attribute_text(attribute), Some("ts" | "typescript"))
        )
    })
}

#[derive(Clone, Copy)]
enum TsFeature {
    AccessorFields,
    Decorators,
    Enums,
    NamespaceValues,
    ConstructorParameterModifiers,
}

impl TsFeature {
    fn description(self) -> &'static str {
        match self {
            Self::AccessorFields => "accessor fields (related TSC proposal is not stage 4 yet)",
            Self::Decorators => "decorators (related TSC proposal is not stage 4 yet)",
            Self::Enums => "enums",
            Self::NamespaceValues => "namespaces with non-type nodes",
            Self::ConstructorParameterModifiers => {
                "accessibility modifiers on constructor parameters"
            }
        }
    }
}

struct TsIssue {
    feature: TsFeature,
    start: usize,
    end: usize,
}

fn find_typescript_invalid_feature(node: &JsProgram) -> Option<TsIssue> {
    fn namespace_body_has_runtime(body: &oxc_ast::ast::TSModuleDeclarationBody<'_>) -> bool {
        match body {
            oxc_ast::ast::TSModuleDeclarationBody::TSModuleBlock(block) => {
                block.body.iter().any(statement_has_runtime)
            }
            oxc_ast::ast::TSModuleDeclarationBody::TSModuleDeclaration(declaration) => {
                declaration
                    .body
                    .as_ref()
                    .is_some_and(namespace_body_has_runtime)
            }
        }
    }

    fn declaration_has_runtime(declaration: &oxc_ast::ast::Declaration<'_>) -> bool {
        !matches!(
            declaration,
            oxc_ast::ast::Declaration::TSTypeAliasDeclaration(_)
                | oxc_ast::ast::Declaration::TSInterfaceDeclaration(_)
        )
    }

    fn statement_has_runtime(statement: &Statement<'_>) -> bool {
        match statement {
            Statement::TSInterfaceDeclaration(_) | Statement::TSTypeAliasDeclaration(_) => false,
            Statement::ImportDeclaration(declaration) => {
                if declaration.import_kind.is_type() {
                    return false;
                }
                !declaration.specifiers.as_ref().is_some_and(|specifiers| {
                    !specifiers.is_empty()
                        && specifiers.iter().all(|specifier| match specifier {
                            ImportDeclarationSpecifier::ImportSpecifier(specifier) => {
                                specifier.import_kind.is_type()
                            }
                            ImportDeclarationSpecifier::ImportDefaultSpecifier(_)
                            | ImportDeclarationSpecifier::ImportNamespaceSpecifier(_) => false,
                        })
                })
            }
            Statement::ExportNamedDeclaration(declaration) => declaration
                .declaration
                .as_ref()
                .is_some_and(declaration_has_runtime),
            Statement::TSModuleDeclaration(declaration) => declaration
                .body
                .as_ref()
                .is_some_and(namespace_body_has_runtime),
            _ => true,
        }
    }

    struct Visitor {
        found: Option<TsIssue>,
        in_constructor: usize,
    }

    impl Visitor {
        fn issue(&mut self, feature: TsFeature, span: OxcSpan) {
            if self.found.is_none() {
                self.found = Some(TsIssue {
                    feature,
                    start: span.start as usize,
                    end: span.end as usize,
                });
            }
        }
    }

    impl<'a> Visit<'a> for Visitor {
        fn visit_decorator(&mut self, decorator: &oxc_ast::ast::Decorator<'a>) {
            self.issue(TsFeature::Decorators, decorator.span);
        }

        fn visit_statement(&mut self, statement: &Statement<'a>) {
            if self.found.is_some() {
                return;
            }

            match statement {
                Statement::TSEnumDeclaration(declaration) => {
                    self.issue(TsFeature::Enums, declaration.span);
                    return;
                }
                Statement::TSModuleDeclaration(declaration) => {
                    if declaration
                        .body
                        .as_ref()
                        .is_some_and(namespace_body_has_runtime)
                    {
                        self.issue(TsFeature::NamespaceValues, declaration.span);
                        return;
                    }
                }
                _ => {}
            }

            walk::walk_statement(self, statement);
        }

        fn visit_class_element(&mut self, element: &ClassElement<'a>) {
            if self.found.is_some() {
                return;
            }

            if matches!(element, ClassElement::AccessorProperty(_))
            {
                let span = match element {
                    ClassElement::AccessorProperty(property) => property.span,
                    _ => unreachable!(),
                };
                self.issue(TsFeature::AccessorFields, span);
                return;
            }

            walk::walk_class_element(self, element);
        }

        fn visit_method_definition(&mut self, definition: &oxc_ast::ast::MethodDefinition<'a>) {
            let was_constructor = matches!(definition.kind, MethodDefinitionKind::Constructor);
            if was_constructor {
                self.in_constructor += 1;
            }
            walk::walk_method_definition(self, definition);
            if was_constructor {
                self.in_constructor -= 1;
            }
        }

        fn visit_formal_parameter(&mut self, parameter: &FormalParameter<'a>) {
            if self.found.is_none()
                && self.in_constructor != 0
                && (parameter.accessibility.is_some() || parameter.readonly)
            {
                self.issue(TsFeature::ConstructorParameterModifiers, parameter.span);
                return;
            }
            walk::walk_formal_parameter(self, parameter);
        }
    }

    let mut visitor = Visitor {
        found: None,
        in_constructor: 0,
    };
    visitor.visit_program(node.program());
    visitor.found
}

fn detect_let_directive_invalid_placement_in_fragment(
    source: &str,
    fragment: &Fragment,
) -> Option<CompileError> {
    fragment.find_map(|entry| {
        let node = entry.as_node()?;
        let el = match node {
            Node::SvelteWindow(_)
            | Node::SvelteDocument(_)
            | Node::SvelteBody(_)
            | Node::SvelteHead(_)
            | Node::SvelteElement(_)
            | Node::SvelteBoundary(_) => node.as_element().unwrap(),
            _ => return None,
        };

        for attribute in el.attributes().iter() {
            let Attribute::LetDirective(directive) = attribute else {
                continue;
            };
            return Some(compile_error_custom(
                source,
                "let_directive_invalid_placement",
                "`let:` directive at invalid position",
                directive.start,
                directive.end,
            ));
        }

        None
    })
}

fn detect_style_directive_invalid_modifier_in_fragment(
    source: &str,
    fragment: &Fragment,
) -> Option<CompileError> {
    fragment.find_map(|entry| match entry.as_node()? {
        Node::RegularElement(element) => {
            detect_style_directive_invalid_modifier_in_attributes(source, &element.attributes)
        }
        Node::Component(component) => {
            detect_style_directive_invalid_modifier_in_attributes(source, &component.attributes)
        }
        Node::SlotElement(slot) => {
            detect_style_directive_invalid_modifier_in_attributes(source, &slot.attributes)
        }
        _ => None,
    })
}

fn detect_style_directive_invalid_modifier_in_attributes(
    source: &str,
    attributes: &[Attribute],
) -> Option<CompileError> {
    for attribute in attributes.iter() {
        let Attribute::StyleDirective(directive) = attribute else {
            continue;
        };
        if directive
            .modifiers
            .iter()
            .all(|modifier| modifier.as_ref() == "important")
        {
            continue;
        }
        return Some(compile_error_custom(
            source,
            "style_directive_invalid_modifier",
            "`style:` directive can only use the `important` modifier",
            directive.start,
            directive.end,
        ));
    }
    None
}

fn detect_svelte_fragment_invalid_placement_in_fragment(
    source: &str,
    fragment: &Fragment,
    direct_component_child: bool,
) -> Option<CompileError> {
    for node in fragment.nodes.iter() {
        match node {
            Node::RegularElement(element) => {
                if let Some(error) = detect_svelte_fragment_invalid_placement_in_fragment(
                    source,
                    &element.fragment,
                    false,
                ) {
                    return Some(error);
                }
            }
            Node::SvelteFragment(el) => {
                if !direct_component_child {
                    return Some(compile_error_custom(
                        source,
                        "svelte_fragment_invalid_placement",
                        "`<svelte:fragment>` must be the direct child of a component",
                        el.start,
                        el.end,
                    ));
                }
                if let Some(error) = detect_svelte_fragment_invalid_placement_in_fragment(
                    source,
                    &el.fragment,
                    false,
                ) {
                    return Some(error);
                }
            }
            Node::Component(_) | Node::SvelteComponent(_) | Node::SvelteElement(_) => {
                let fragment = node.as_element().unwrap().fragment();
                if let Some(error) =
                    detect_svelte_fragment_invalid_placement_in_fragment(source, fragment, true)
                {
                    return Some(error);
                }
            }
            Node::SlotElement(_)
            | Node::SvelteHead(_)
            | Node::SvelteBody(_)
            | Node::SvelteWindow(_)
            | Node::SvelteDocument(_)
            | Node::SvelteSelf(_)
            | Node::SvelteBoundary(_)
            | Node::TitleElement(_) => {
                let fragment = node.as_element().unwrap().fragment();
                if let Some(error) =
                    detect_svelte_fragment_invalid_placement_in_fragment(source, fragment, false)
                {
                    return Some(error);
                }
            }
            Node::IfBlock(block) => {
                if let Some(error) = detect_svelte_fragment_invalid_placement_in_fragment(
                    source,
                    &block.consequent,
                    false,
                ) {
                    return Some(error);
                }
                if let Some(alternate) = block.alternate.as_deref() {
                    let result = match alternate {
                        Alternate::Fragment(fragment) => {
                            detect_svelte_fragment_invalid_placement_in_fragment(
                                source, fragment, false,
                            )
                        }
                        Alternate::IfBlock(block) => {
                            detect_svelte_fragment_invalid_placement_in_fragment(
                                source,
                                &block.consequent,
                                false,
                            )
                        }
                    };
                    if result.is_some() {
                        return result;
                    }
                }
            }
            Node::EachBlock(block) => {
                if let Some(error) =
                    detect_svelte_fragment_invalid_placement_in_fragment(source, &block.body, false)
                {
                    return Some(error);
                }
                if let Some(fallback) = block.fallback.as_ref()
                    && let Some(error) = detect_svelte_fragment_invalid_placement_in_fragment(
                        source, fallback, false,
                    )
                {
                    return Some(error);
                }
            }
            Node::AwaitBlock(block) => {
                for branch in [
                    block.pending.as_ref(),
                    block.then.as_ref(),
                    block.catch.as_ref(),
                ] {
                    if let Some(fragment) = branch
                        && let Some(error) = detect_svelte_fragment_invalid_placement_in_fragment(
                            source, fragment, false,
                        )
                    {
                        return Some(error);
                    }
                }
            }
            Node::SnippetBlock(block) => {
                if let Some(error) =
                    detect_svelte_fragment_invalid_placement_in_fragment(source, &block.body, false)
                {
                    return Some(error);
                }
            }
            Node::KeyBlock(block) => {
                if let Some(error) = detect_svelte_fragment_invalid_placement_in_fragment(
                    source,
                    &block.fragment,
                    false,
                ) {
                    return Some(error);
                }
            }
            _ => {}
        }
    }
    None
}

fn detect_svelte_head_illegal_attribute_in_fragment(
    source: &str,
    fragment: &Fragment,
) -> Option<CompileError> {
    for node in fragment.nodes.iter() {
        match node {
            Node::SvelteHead(el) => {
                if let Some(attribute) = el.attributes.first() {
                    let (start, end) = attribute_span(attribute);
                    return Some(compile_error_custom(
                        source,
                        "svelte_head_illegal_attribute",
                        "`<svelte:head>` cannot have attributes nor directives",
                        start,
                        end,
                    ));
                }
                if let Some(error) =
                    detect_svelte_head_illegal_attribute_in_fragment(source, &el.fragment)
                {
                    return Some(error);
                }
            }
            _ => {
                if let Some(error) = find_error_in_child_fragments(node, |fragment| {
                    detect_svelte_head_illegal_attribute_in_fragment(source, fragment)
                }) {
                    return Some(error);
                }
            }
        }
    }
    None
}

fn detect_text_content_model_errors_in_fragment(
    source: &str,
    fragment: &Fragment,
) -> Option<CompileError> {
    for node in fragment.nodes.iter() {
        match node {
            Node::RegularElement(element) => {
                if element.name.as_ref() == "textarea" {
                    let has_value = element.attributes.iter().any(|attribute| {
                        matches!(attribute, Attribute::Attribute(attribute) if attribute.name.as_ref() == "value")
                    });
                    if has_value
                        && let Some((_, end)) =
                            first_non_whitespace_fragment_range(&element.fragment)
                    {
                        return Some(compile_error_custom(
                            source,
                            "textarea_invalid_content",
                            "A `<textarea>` can have either a value attribute or (equivalently) child content, but not both",
                            element.start,
                            element.end.max(end),
                        ));
                    }
                }

                if let Some(error) =
                    detect_text_content_model_errors_in_fragment(source, &element.fragment)
                {
                    return Some(error);
                }
            }
            Node::TitleElement(el) => {
                if let Some(attribute) = el.attributes.first() {
                    let (start, end) = attribute_span(attribute);
                    return Some(compile_error_custom(
                        source,
                        "title_illegal_attribute",
                        "`<title>` cannot have attributes nor directives",
                        start,
                        end,
                    ));
                }

                for child in el.fragment.nodes.iter() {
                    if matches!(child, Node::Text(_) | Node::ExpressionTag(_)) {
                        continue;
                    }
                    let (start, end) = (child.start(), child.end());
                    return Some(compile_error_custom(
                        source,
                        "title_invalid_content",
                        "`<title>` can only contain text and {tags}",
                        start,
                        end,
                    ));
                }
            }
            _ => {
                if let Some(error) = find_error_in_child_fragments(node, |child| {
                    detect_text_content_model_errors_in_fragment(source, child)
                }) {
                    return Some(error);
                }
            }
        }
    }

    None
}

fn detect_mixed_event_handler_syntax_in_fragment(
    source: &str,
    fragment: &Fragment,
) -> Option<CompileError> {
    fragment.find_map(|entry| {
        let node = entry.as_node()?;
        let Node::RegularElement(element) = node else {
            return None;
        };
        if element.name.starts_with("svelte:") {
            return None;
        }
        let directive = element.attributes.iter().find_map(|attribute| match attribute {
            Attribute::OnDirective(directive) => Some(directive),
            _ => None,
        })?;
        Some(compile_error_custom(
            source,
            "mixed_event_handler_syntaxes",
            format!(
                "Mixing old (on:{name}) and new syntaxes for event handling is not allowed. Use only the on{name} syntax",
                name = directive.name
            ),
            directive.start,
            directive.end,
        ))
    })
}

fn fragment_has_modern_dom_event_syntax(fragment: &Fragment) -> bool {
    fragment
        .find_map(|entry| {
            let node = entry.as_node()?;
            let Node::RegularElement(element) = node else {
                return None;
            };
            if element.name.starts_with("svelte:") {
                return None;
            }
            element
                .attributes
                .iter()
                .any(|attribute| {
                    matches!(
                        attribute,
                        Attribute::Attribute(attribute)
                            if attribute.name.starts_with("on") && attribute.name.len() > 2
                    )
                })
                .then_some(())
        })
        .is_some()
}

fn component_prop_attribute_name(attribute: &Attribute) -> Option<Arc<str>> {
    match attribute {
        Attribute::Attribute(attribute) => Some(attribute.name.clone()),
        Attribute::BindDirective(attribute) => Some(attribute.name.clone()),
        Attribute::ClassDirective(attribute) => Some(attribute.name.clone()),
        Attribute::StyleDirective(attribute) => Some(attribute.name.clone()),
        Attribute::LetDirective(attribute) => Some(attribute.name.clone()),
        Attribute::OnDirective(attribute) => Some(attribute.name.clone()),
        Attribute::AnimateDirective(attribute) => Some(attribute.name.clone()),
        Attribute::UseDirective(attribute) => Some(attribute.name.clone()),
        Attribute::TransitionDirective(attribute) => Some(attribute.name.clone()),
        Attribute::SpreadAttribute(_) | Attribute::AttachTag(_) => None,
    }
}

fn find_component_scope_snippet_with_name(
    fragment: &Fragment,
    names: &NameSet,
) -> Option<(Arc<str>, usize, usize)> {
    fragment.search(|entry, _| match entry {
        Entry::Node(Node::Component(_)) => Search::Skip,
        Entry::Node(Node::SnippetBlock(block)) => {
            let Some(name) = block.expression.identifier_name() else {
                return Search::Continue;
            };
            if names.contains(name.as_ref()) {
                Search::Found((name, block.start, block.end))
            } else {
                Search::Continue
            }
        }
        _ => Search::Continue,
    })
}

fn attribute_span(attribute: &Attribute) -> (usize, usize) {
    match attribute {
        Attribute::Attribute(attribute) => (attribute.start, attribute.end),
        Attribute::SpreadAttribute(attribute) => (attribute.start, attribute.end),
        Attribute::BindDirective(attribute) => (attribute.start, attribute.end),
        Attribute::ClassDirective(attribute) => (attribute.start, attribute.end),
        Attribute::StyleDirective(attribute) => (attribute.start, attribute.end),
        Attribute::LetDirective(attribute) => (attribute.start, attribute.end),
        Attribute::OnDirective(attribute) => (attribute.start, attribute.end),
        Attribute::AnimateDirective(attribute) => (attribute.start, attribute.end),
        Attribute::UseDirective(attribute) => (attribute.start, attribute.end),
        Attribute::TransitionDirective(attribute) => (attribute.start, attribute.end),
        Attribute::AttachTag(attribute) => (attribute.start, attribute.end),
    }
}

#[allow(dead_code)]
fn node_span(node: &Node) -> (usize, usize) {
    (node.start(), node.end())
}

fn is_valid_custom_element_tag_name(name: &str) -> bool {
    name.contains('-')
        && !name
            .chars()
            .any(|ch| ch.is_whitespace() || ch.is_uppercase())
}

fn is_reserved_custom_element_tag_name(name: &str) -> bool {
    matches!(
        name,
        "annotation-xml"
            | "color-profile"
            | "font-face"
            | "font-face-src"
            | "font-face-uri"
            | "font-face-format"
            | "font-face-name"
            | "missing-glyph"
    )
}

#[derive(Clone, Default)]
struct StructureContext {
    inside_anchor: bool,
    inside_paragraph: bool,
    inside_textarea: bool,
    inside_form: bool,
    inside_dd: bool,
    direct_parent_name: Option<Arc<str>>,
    direct_parent_is_custom_element: bool,
}

impl StructureContext {
    fn with_element(&self, element: &RegularElement, is_custom_element: bool) -> Self {
        Self {
            inside_anchor: self.inside_anchor || element.name.as_ref() == "a",
            inside_paragraph: self.inside_paragraph || element.name.as_ref() == "p",
            inside_textarea: self.inside_textarea || element.name.as_ref() == "textarea",
            inside_form: self.inside_form || element.name.as_ref() == "form",
            inside_dd: if element.name.as_ref() == "dl" || is_custom_element {
                false
            } else if element.name.as_ref() == "dd" {
                true
            } else {
                self.inside_dd
            },
            direct_parent_name: Some(element.name.clone()),
            direct_parent_is_custom_element: is_custom_element,
        }
    }

    fn inside_control_branch(&self) -> Self {
        Self {
            inside_form: false,
            ..self.clone()
        }
    }
}

fn detect_additional_template_structure_errors_in_fragment(
    source: &str,
    fragment: &Fragment,
    context: StructureContext,
) -> Option<CompileError> {
    for node in &fragment.nodes {
        match node {
            Node::Text(text)
                if context.direct_parent_name.as_deref() == Some("tbody")
                    && text.data.chars().any(|ch| !ch.is_whitespace()) =>
            {
                return Some(tbody_child_invalid_placement_error(
                    source, "#text", text.start, text.end,
                ));
            }
            Node::ExpressionTag(tag) if context.direct_parent_name.as_deref() == Some("tbody") => {
                return Some(tbody_child_invalid_placement_error(
                    source, "#text", tag.start, tag.end,
                ));
            }
            Node::RegularElement(element) => {
                let is_custom_element = is_custom_element_name(element.name.as_ref());
                if context.inside_anchor && element.name.as_ref() == "a" {
                    return Some(node_invalid_placement_error(
                        source,
                        "a",
                        "a",
                        element.start,
                        element.end,
                    ));
                }
                if context.inside_paragraph
                    && is_paragraph_forbidden_descendant(element.name.as_ref())
                {
                    return Some(node_invalid_placement_error(
                        source,
                        element.name.as_ref(),
                        "p",
                        element.start,
                        element.end,
                    ));
                }
                if context.inside_form && element.name.as_ref() == "form" {
                    return Some(node_invalid_placement_error(
                        source,
                        "form",
                        "form",
                        element.start,
                        element.end,
                    ));
                }
                if context.inside_dd && element.name.as_ref() == "dt" {
                    return Some(node_invalid_placement_error(
                        source,
                        "dt",
                        "dd",
                        element.start,
                        element.end,
                    ));
                }
                if context.direct_parent_name.as_deref() == Some("tbody")
                    && !is_custom_element
                    && !matches!(
                        element.name.as_ref(),
                        "tr" | "style" | "script" | "template"
                    )
                {
                    return Some(tbody_child_invalid_placement_error(
                        source,
                        element.name.as_ref(),
                        element.start,
                        element.end,
                    ));
                }
                if let Some(parent) = context.direct_parent_name.as_deref()
                    && element.name.as_ref() == "tbody"
                    && !context.direct_parent_is_custom_element
                    && parent != "table"
                {
                    return Some(tbody_parent_invalid_placement_error(
                        source,
                        parent,
                        element.start,
                        element.end,
                    ));
                }
                if let Some(error) =
                    detect_additional_template_structure_errors_in_element(source, element)
                {
                    return Some(error);
                }

                if let Some(error) = detect_additional_template_structure_errors_in_fragment(
                    source,
                    &element.fragment,
                    context.with_element(element, is_custom_element),
                ) {
                    return Some(error);
                }
            }
            Node::Component(component) => {
                if let Some(error) = detect_additional_template_structure_errors_in_fragment(
                    source,
                    &component.fragment,
                    context.clone(),
                ) {
                    return Some(error);
                }
            }
            Node::SlotElement(slot) => {
                if let Some(error) = detect_additional_template_structure_errors_in_fragment(
                    source,
                    &slot.fragment,
                    context.clone(),
                ) {
                    return Some(error);
                }
            }
            Node::SvelteElement(el) => {
                if let Some(error) = detect_svelte_element_this_errors(source, el) {
                    return Some(error);
                }
                if let Some(error) = detect_additional_template_structure_errors_in_fragment(
                    source,
                    &el.fragment,
                    context.clone(),
                ) {
                    return Some(error);
                }
            }
            Node::SvelteWindow(_) | Node::SvelteDocument(_) => {
                let el = node.as_element().unwrap();
                if let Some(error) =
                    detect_svelte_meta_spread_errors(source, el.name(), el.attributes())
                {
                    return Some(error);
                }
                if let Some(error) = detect_additional_template_structure_errors_in_fragment(
                    source,
                    el.fragment(),
                    context.clone(),
                ) {
                    return Some(error);
                }
            }
            Node::SvelteHead(_)
            | Node::SvelteBody(_)
            | Node::SvelteComponent(_)
            | Node::SvelteSelf(_)
            | Node::SvelteFragment(_)
            | Node::SvelteBoundary(_)
            | Node::TitleElement(_) => {
                let fragment = node.as_element().unwrap().fragment();
                if let Some(error) = detect_additional_template_structure_errors_in_fragment(
                    source,
                    fragment,
                    context.clone(),
                ) {
                    return Some(error);
                }
            }
            Node::IfBlock(block) => {
                if context.inside_textarea {
                    return Some(textarea_block_error(source, "if", block.start));
                }
                let branch_context = context.inside_control_branch();
                if let Some(error) = detect_additional_template_structure_errors_in_fragment(
                    source,
                    &block.consequent,
                    branch_context.clone(),
                ) {
                    return Some(error);
                }
                if let Some(alternate) = block.alternate.as_deref() {
                    let result = match alternate {
                        Alternate::Fragment(fragment) => {
                            detect_additional_template_structure_errors_in_fragment(
                                source,
                                fragment,
                                branch_context.clone(),
                            )
                        }
                        Alternate::IfBlock(block) => {
                            detect_additional_template_structure_errors_in_fragment(
                                source,
                                &block.consequent,
                                branch_context.clone(),
                            )
                        }
                    };
                    if result.is_some() {
                        return result;
                    }
                }
            }
            Node::EachBlock(block) => {
                if context.inside_textarea {
                    return Some(textarea_block_error(source, "each", block.start));
                }
                let branch_context = context.inside_control_branch();
                if let Some(error) = detect_additional_template_structure_errors_in_fragment(
                    source,
                    &block.body,
                    branch_context.clone(),
                ) {
                    return Some(error);
                }
                if let Some(fallback) = block.fallback.as_ref()
                    && let Some(error) = detect_additional_template_structure_errors_in_fragment(
                        source,
                        fallback,
                        branch_context.clone(),
                    )
                {
                    return Some(error);
                }
            }
            Node::AwaitBlock(block) => {
                if context.inside_textarea {
                    return Some(textarea_block_error(source, "await", block.start));
                }
                let branch_context = context.inside_control_branch();
                for branch in [
                    block.pending.as_ref(),
                    block.then.as_ref(),
                    block.catch.as_ref(),
                ] {
                    if let Some(fragment) = branch
                        && let Some(error) = detect_additional_template_structure_errors_in_fragment(
                            source,
                            fragment,
                            branch_context.clone(),
                        )
                    {
                        return Some(error);
                    }
                }
            }
            Node::SnippetBlock(block) => {
                if context.inside_textarea {
                    return Some(textarea_block_error(source, "snippet", block.start));
                }
                if let Some(error) = detect_additional_template_structure_errors_in_fragment(
                    source,
                    &block.body,
                    context.inside_control_branch(),
                ) {
                    return Some(error);
                }
            }
            Node::KeyBlock(block) => {
                if context.inside_textarea {
                    return Some(textarea_block_error(source, "key", block.start));
                }
                if let Some(error) = detect_additional_template_structure_errors_in_fragment(
                    source,
                    &block.fragment,
                    context.inside_control_branch(),
                ) {
                    return Some(error);
                }
            }
            Node::HtmlTag(tag) if context.inside_textarea => {
                return Some(compile_error_custom(
                    source,
                    "tag_invalid_placement",
                    "{@html ...} tag cannot be inside <textarea>",
                    tag.start,
                    tag.start,
                ));
            }
            _ => {}
        }
    }

    None
}

fn textarea_block_error(source: &str, kind: &str, start: usize) -> CompileError {
    compile_error_custom(
        source,
        "block_invalid_placement",
        format!("{{#{kind} ...}} block cannot be inside <textarea>"),
        start,
        start,
    )
}

fn detect_additional_template_structure_errors_in_element(
    source: &str,
    element: &RegularElement,
) -> Option<CompileError> {
    if is_void_element_name(element.name.as_ref())
        && (element.has_end_tag || first_non_whitespace_fragment_range(&element.fragment).is_some())
    {
        let start = element
            .fragment
            .nodes
            .first()
            .map(Node::start)
            .unwrap_or(element.end.saturating_sub(1));
        return Some(compile_error_with_range(
            source,
            DiagnosticKind::VoidElementInvalidContent,
            start,
            start,
        ));
    }

    if element.name.as_ref() == "svelte:element" {
        let mut has_this_attribute = false;
        for attribute in &element.attributes {
            let Attribute::Attribute(attribute) = attribute else {
                continue;
            };
            if attribute.name.is_empty()
                && matches!(
                    &attribute.value,
                    AttributeValueKind::ExpressionTag(tag)
                        if expression_kind(&tag.expression) == Some("ThisExpression")
                )
            {
                return Some(compile_error_custom(
                    source,
                    "unexpected_reserved_word",
                    "'this' is a reserved word in JavaScript and cannot be used here",
                    attribute.name_loc.start.character,
                    attribute.name_loc.start.character,
                ));
            }
            if attribute.name.as_ref() != "this" {
                continue;
            }

            has_this_attribute = true;
            if matches!(&attribute.value, AttributeValueKind::Boolean(true)) {
                return Some(compile_error_custom(
                    source,
                    "svelte_element_missing_this",
                    "`<svelte:element>` must have a 'this' attribute with a value",
                    attribute.name_loc.start.character,
                    attribute.name_loc.end.character,
                ));
            }
            if let AttributeValueKind::ExpressionTag(tag) = &attribute.value
                && matches!(
                    tag.expression.identifier_name().as_deref(),
                    Some(name) if name == "this"
                )
            {
                return Some(compile_error_custom(
                    source,
                    "unexpected_reserved_word",
                    "'this' is a reserved word in JavaScript and cannot be used here",
                    attribute.name_loc.start.character,
                    attribute.name_loc.start.character,
                ));
            }
        }

        if !has_this_attribute {
            return Some(compile_error_custom(
                source,
                "svelte_element_missing_this",
                "`<svelte:element>` must have a 'this' attribute with a value",
                element.start,
                element.start,
            ));
        }
    }

    if matches!(element.name.as_ref(), "svelte:window" | "svelte:document") {
        for attribute in &element.attributes {
            let Attribute::SpreadAttribute(spread) = attribute else {
                continue;
            };
            return Some(compile_error_custom(
                source,
                "illegal_element_attribute",
                format!(
                    "`<{}>` does not support non-event attributes or spread attributes",
                    element.name
                ),
                spread.start,
                spread.end,
            ));
        }
    }

    for attribute in &element.attributes {
        match attribute {
            Attribute::Attribute(attribute)
                if !element.name.starts_with("svelte:")
                    && attribute.name.len() > 2
                    && attribute.name.starts_with("on")
                    && !matches!(&attribute.value, AttributeValueKind::ExpressionTag(_)) =>
            {
                return Some(compile_error_custom(
                    source,
                    "attribute_invalid_event_handler",
                    "Event attribute must be a JavaScript expression, not a string",
                    attribute.start,
                    attribute.end,
                ));
            }
            Attribute::OnDirective(directive) => {
                let mut passive = false;
                let mut nonpassive = false;
                let mut prevent_default = false;

                for modifier in directive.modifiers.iter() {
                    match modifier.as_ref() {
                        "preventDefault" => prevent_default = true,
                        "stopPropagation"
                        | "stopImmediatePropagation"
                        | "capture"
                        | "once"
                        | "self"
                        | "trusted" => {}
                        "passive" => passive = true,
                        "nonpassive" => nonpassive = true,
                        _ => {
                            return Some(compile_error_custom(
                                source,
                                "event_handler_invalid_modifier",
                                "Valid event modifiers are preventDefault, stopPropagation, stopImmediatePropagation, capture, once, passive, nonpassive, self or trusted",
                                directive.start,
                                directive.end,
                            ));
                        }
                    }
                }

                if passive && nonpassive {
                    return Some(compile_error_custom(
                        source,
                        "event_handler_invalid_modifier_combination",
                        "The 'passive' and 'nonpassive' modifiers cannot be used together",
                        directive.start,
                        directive.end,
                    ));
                }
                if passive && prevent_default {
                    return Some(compile_error_custom(
                        source,
                        "event_handler_invalid_modifier_combination",
                        "The 'passive' and 'preventDefault' modifiers cannot be used together",
                        directive.start,
                        directive.end,
                    ));
                }
            }
            _ => {}
        }
    }

    None
}

fn detect_svelte_element_this_errors(source: &str, el: &SvelteElement) -> Option<CompileError> {
    // Check remaining attributes for a boolean `this` (no value)
    for attribute in el.attributes.iter() {
        let Attribute::Attribute(attribute) = attribute else {
            continue;
        };
        if attribute.name.is_empty()
            && matches!(
                &attribute.value,
                AttributeValueKind::ExpressionTag(tag)
                    if expression_kind(&tag.expression) == Some("ThisExpression")
            )
        {
            return Some(compile_error_custom(
                source,
                "unexpected_reserved_word",
                "'this' is a reserved word in JavaScript and cannot be used here",
                attribute.name_loc.start.character,
                attribute.name_loc.start.character,
            ));
        }
        if attribute.name.as_ref() != "this" {
            continue;
        }
        if matches!(&attribute.value, AttributeValueKind::Boolean(true)) {
            return Some(compile_error_custom(
                source,
                "svelte_element_missing_this",
                "`<svelte:element>` must have a 'this' attribute with a value",
                attribute.name_loc.start.character,
                attribute.name_loc.end.character,
            ));
        }
    }

    // Check extracted expression
    if let Some(ref expression) = el.expression {
        if matches!(
            expression.identifier_name().as_deref(),
            Some("this")
        ) {
            return Some(compile_error_custom(
                source,
                "unexpected_reserved_word",
                "'this' is a reserved word in JavaScript and cannot be used here",
                el.start,
                el.start,
            ));
        }
    } else {
        // No expression extracted and no `this` in remaining attributes
        let has_this_attr = el
            .attributes
            .iter()
            .any(|a| matches!(a, Attribute::Attribute(a) if a.name.as_ref() == "this"));
        if !has_this_attr {
            return Some(compile_error_custom(
                source,
                "svelte_element_missing_this",
                "`<svelte:element>` must have a 'this' attribute with a value",
                el.start,
                el.start,
            ));
        }
    }

    None
}

fn detect_svelte_meta_spread_errors(
    source: &str,
    name: &str,
    attributes: &[Attribute],
) -> Option<CompileError> {
    for attribute in attributes {
        let Attribute::SpreadAttribute(spread) = attribute else {
            continue;
        };
        return Some(compile_error_custom(
            source,
            "illegal_element_attribute",
            format!(
                "`<{}>` does not support non-event attributes or spread attributes",
                name
            ),
            spread.start,
            spread.end,
        ));
    }
    None
}

fn is_paragraph_forbidden_descendant(name: &str) -> bool {
    matches!(
        name,
        "address"
            | "article"
            | "aside"
            | "blockquote"
            | "div"
            | "dl"
            | "fieldset"
            | "footer"
            | "form"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "header"
            | "hgroup"
            | "hr"
            | "main"
            | "nav"
            | "ol"
            | "p"
            | "pre"
            | "section"
            | "table"
            | "ul"
    )
}

fn node_invalid_placement_error(
    source: &str,
    child: &str,
    ancestor: &str,
    start: usize,
    end: usize,
) -> CompileError {
    compile_error_custom(
        source,
        "node_invalid_placement",
        format!(
            "`<{child}>` cannot be a descendant of `<{ancestor}>`. The browser will 'repair' the HTML (by moving, removing, or inserting elements) which breaks Svelte's assumptions about the structure of your components."
        ),
        start,
        end,
    )
}

fn tbody_parent_invalid_placement_error(
    source: &str,
    parent: &str,
    start: usize,
    end: usize,
) -> CompileError {
    compile_error_custom(
        source,
        "node_invalid_placement",
        format!(
            "`<tbody>` must be the child of a `<table>`, not a `<{parent}>`. The browser will 'repair' the HTML (by moving, removing, or inserting elements) which breaks Svelte's assumptions about the structure of your components."
        ),
        start,
        end,
    )
}

fn tbody_child_invalid_placement_error(
    source: &str,
    child: &str,
    start: usize,
    end: usize,
) -> CompileError {
    compile_error_custom(
        source,
        "node_invalid_placement",
        format!(
            "`<{child}>` cannot be a child of `<tbody>`. `<tbody>` only allows these children: `<tr>`, `<style>`, `<script>`, `<template>`. The browser will 'repair' the HTML (by moving, removing, or inserting elements) which breaks Svelte's assumptions about the structure of your components."
        ),
        start,
        end,
    )
}

#[derive(Clone, Copy)]
struct EachContext {
    keyed: bool,
    animation_relevant_children: usize,
}

impl NameStack {
    fn extend_expression_bindings(&mut self, expression: &Expression) {
        self.extend(expression_binding_names(expression));
    }

    fn extend_optional_expression_bindings(&mut self, expression: Option<&Expression>) {
        if let Some(expression) = expression {
            self.extend_expression_bindings(expression);
        }
    }

    fn extend_optional_name(&mut self, name: Option<&Arc<str>>) {
        if let Some(name) = name {
            self.push(name.clone());
        }
    }

    fn extend_each_block_bindings(&mut self, block: &EachBlock) {
        if let Some(context) = block.context.as_ref() {
            self.extend_expression_bindings(context);
        }
        self.extend_optional_name(block.index.as_ref());
    }

    fn extend_snippet_parameters(&mut self, block: &SnippetBlock) {
        for parameter in &block.parameters {
            self.extend_expression_bindings(parameter);
        }
    }

    fn extend_const_tag_identifiers(&mut self, tag: &ConstTag) {
        self.extend(const_tag_declared_identifiers(tag));
    }
}

#[derive(Clone, Copy)]
struct ContextMark {
    immutable: NameMark,
    snippets: NameMark,
    each: NameMark,
}

struct ValidationContext {
    imports: NameSet,
    immutable: NameStack,
    snippets: NameStack,
    each: NameStack,
    runes: bool,
}

impl ValidationContext {
    fn mark(&self) -> ContextMark {
        ContextMark {
            immutable: self.immutable.mark(),
            snippets: self.snippets.mark(),
            each: self.each.mark(),
        }
    }

    fn reset(&mut self, mark: ContextMark) {
        self.immutable.reset(mark.immutable);
        self.snippets.reset(mark.snippets);
        self.each.reset(mark.each);
    }

    fn with_frame<T>(
        &mut self,
        extend: impl FnOnce(&mut Self),
        visit: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let mark = self.mark();
        extend(self);
        let result = visit(self);
        self.reset(mark);
        result
    }

    fn push_component_lets(&mut self, component: &Component) {
        for attribute in &component.attributes {
            if let Attribute::LetDirective(directive) = attribute {
                self.immutable
                    .extend(let_directive_binding_names(directive));
            }
        }
    }

    fn push_snippet_params(&mut self, block: &SnippetBlock) {
        for parameter in &block.parameters {
            self.immutable.extend_expression_bindings(parameter);
            self.snippets.extend_expression_bindings(parameter);
        }
    }

    fn push_each_bindings(&mut self, block: &EachBlock) {
        self.each.extend_each_block_bindings(block);
    }

    fn push_const(&mut self, tag: &ConstTag) {
        self.immutable.extend_const_tag_identifiers(tag);
    }

    fn with_component_lets<T>(
        &mut self,
        component: &Component,
        visit: impl FnOnce(&mut Self) -> T,
    ) -> T {
        self.with_frame(|context| context.push_component_lets(component), visit)
    }

    fn with_snippet_params<T>(
        &mut self,
        block: &SnippetBlock,
        visit: impl FnOnce(&mut Self) -> T,
    ) -> T {
        self.with_frame(|context| context.push_snippet_params(block), visit)
    }

    fn with_each_bindings<T>(
        &mut self,
        block: &EachBlock,
        visit: impl FnOnce(&mut Self) -> T,
    ) -> T {
        self.with_frame(|context| context.push_each_bindings(block), visit)
    }

    fn with_await_binding<T>(
        &mut self,
        binding: Option<&Expression>,
        visit: impl FnOnce(&mut Self) -> T,
    ) -> T {
        self.with_frame(
            |context| {
                context
                    .immutable
                    .extend_optional_expression_bindings(binding)
            },
            visit,
        )
    }
}

#[derive(Clone, Copy)]
enum AssignmentKind {
    Constant,
    SnippetParameter,
    EachItemInvalid,
}

#[derive(Clone, Copy)]
struct AssignmentViolation {
    kind: AssignmentKind,
    start: usize,
    end: usize,
}

fn detect_template_directive_errors_in_fragment(
    source: &str,
    fragment: &Fragment,
    each: Option<EachContext>,
    context: &mut ValidationContext,
) -> Option<CompileError> {
    context.with_frame(
        |_| {},
        |context| {
            for node in &fragment.nodes {
                match node {
                    Node::RegularElement(element) => {
                        if let Some(error) =
                            detect_element_directive_errors(source, element, each, context)
                        {
                            return Some(error);
                        }
                        if let Some(error) = detect_template_directive_errors_in_fragment(
                            source,
                            &element.fragment,
                            each,
                            context,
                        ) {
                            return Some(error);
                        }
                    }
                    Node::Component(component) => {
                        if let Some(error) = context.with_component_lets(component, |context| {
                            if let Some(error) =
                                detect_component_directive_errors(source, component, context)
                            {
                                return Some(error);
                            }
                            detect_template_directive_errors_in_fragment(
                                source,
                                &component.fragment,
                                None,
                                context,
                            )
                        }) {
                            return Some(error);
                        }
                    }
                    Node::SlotElement(slot) => {
                        for attribute in &slot.attributes {
                            let Attribute::Attribute(attribute) = attribute else {
                                continue;
                            };
                            if attribute.name.as_ref() != "name" {
                                continue;
                            }
                            let Some(name) = static_attribute_text(attribute) else {
                                return Some(compile_error_custom(
                                    source,
                                    "slot_element_invalid_name",
                                    "slot attribute must be a static value",
                                    attribute.start,
                                    attribute.end,
                                ));
                            };
                            if name == "default" {
                                return Some(compile_error_custom(
                                    source,
                                    "slot_element_invalid_name_default",
                                    "`default` is a reserved word — it cannot be used as a slot name",
                                    attribute.start,
                                    attribute.end,
                                ));
                            }
                        }
                        if let Some(error) = detect_template_directive_errors_in_fragment(
                            source,
                            &slot.fragment,
                            None,
                            context,
                        ) {
                            return Some(error);
                        }
                    }
                    Node::SvelteWindow(_) | Node::SvelteDocument(_) => {
                        let el = node.as_element().unwrap();
                        for attribute in el.attributes() {
                            if let Attribute::BindDirective(directive) = attribute {
                                if let Some(error) =
                                    detect_bind_target_error_for_name(source, el.name(), directive)
                                {
                                    return Some(error);
                                }
                                if let Some(error) =
                                    detect_bind_directive_error(source, directive, context, true)
                                {
                                    return Some(error);
                                }
                            }
                        }
                        if let Some(error) = detect_template_directive_errors_in_fragment(
                            source,
                            el.fragment(),
                            None,
                            context,
                        ) {
                            return Some(error);
                        }
                    }
                    Node::SvelteElement(element) => {
                        if let Some(error) =
                            detect_element_directive_errors(source, element, each, context)
                        {
                            return Some(error);
                        }
                        if let Some(error) = detect_template_directive_errors_in_fragment(
                            source,
                            &element.fragment,
                            None,
                            context,
                        ) {
                            return Some(error);
                        }
                    }
                    Node::SvelteHead(_)
                    | Node::SvelteBody(_)
                    | Node::SvelteComponent(_)
                    | Node::SvelteSelf(_)
                    | Node::SvelteFragment(_)
                    | Node::SvelteBoundary(_)
                    | Node::TitleElement(_) => {
                        let fragment = node.as_element().unwrap().fragment();
                        if let Some(error) = detect_template_directive_errors_in_fragment(
                            source, fragment, None, context,
                        ) {
                            return Some(error);
                        }
                    }
                    Node::IfBlock(block) => {
                        if let Some(error) = detect_template_directive_errors_in_fragment(
                            source,
                            &block.consequent,
                            None,
                            context,
                        ) {
                            return Some(error);
                        }
                        if let Some(alternate) = &block.alternate {
                            match alternate.as_ref() {
                                Alternate::Fragment(fragment) => {
                                    if let Some(error) = detect_template_directive_errors_in_fragment(
                                        source, fragment, None, context,
                                    ) {
                                        return Some(error);
                                    }
                                }
                                Alternate::IfBlock(elseif) => {
                                    if let Some(error) = detect_template_directive_errors_in_fragment(
                                        source,
                                        &elseif.consequent,
                                        None,
                                        context,
                                    ) {
                                        return Some(error);
                                    }
                                }
                            }
                        }
                    }
                    Node::KeyBlock(block) => {
                        if let Some(error) = detect_template_directive_errors_in_fragment(
                            source,
                            &block.fragment,
                            None,
                            context,
                        ) {
                            return Some(error);
                        }
                    }
                    Node::AwaitBlock(block) => {
                        if let Some(pending) = &block.pending
                            && let Some(error) = detect_template_directive_errors_in_fragment(
                                source, pending, None, context,
                            )
                        {
                            return Some(error);
                        }
                        if let Some(then) = &block.then
                            && let Some(error) = context.with_await_binding(
                                block.value.as_ref(),
                                |context| {
                                    detect_template_directive_errors_in_fragment(
                                        source, then, None, context,
                                    )
                                },
                            )
                        {
                            return Some(error);
                        }
                        if let Some(catch) = &block.catch
                            && let Some(error) = context.with_await_binding(
                                block.error.as_ref(),
                                |context| {
                                    detect_template_directive_errors_in_fragment(
                                        source, catch, None, context,
                                    )
                                },
                            )
                        {
                            return Some(error);
                        }
                    }
                    Node::SnippetBlock(block) => {
                        if let Some(error) = context.with_snippet_params(block, |context| {
                            detect_template_directive_errors_in_fragment(
                                source,
                                &block.body,
                                None,
                                context,
                            )
                        }) {
                            return Some(error);
                        }
                    }
                    Node::EachBlock(block) => {
                        let body_each = EachContext {
                            keyed: block.key.is_some(),
                            animation_relevant_children: count_animation_relevant_nodes(&block.body),
                        };

                        if let Some(error) = context.with_each_bindings(block, |context| {
                            detect_template_directive_errors_in_fragment(
                                source,
                                &block.body,
                                Some(body_each),
                                context,
                            )
                        }) {
                            return Some(error);
                        }

                        if let Some(fallback) = &block.fallback
                            && let Some(error) = detect_template_directive_errors_in_fragment(
                                source, fallback, None, context,
                            )
                        {
                            return Some(error);
                        }
                    }
                    Node::ConstTag(tag) => {
                        context.push_const(tag);
                    }
                    Node::ExpressionTag(tag) => {
                        if let Some(violation) =
                            find_assignment_violation_in_template_expression(&tag.expression, context)
                        {
                            let kind = match violation.kind {
                                AssignmentKind::Constant => DiagnosticKind::ConstantAssignment,
                                AssignmentKind::SnippetParameter => {
                                    DiagnosticKind::SnippetParameterAssignment
                                }
                                AssignmentKind::EachItemInvalid => {
                                    DiagnosticKind::EachItemInvalidAssignment
                                }
                            };
                            return Some(compile_error_with_range(
                                source,
                                kind,
                                violation.start,
                                violation.end,
                            ));
                        }
                    }
                    _ => {}
                }
            }

            None
        },
    )
}

fn detect_attribute_syntax_in_fragment(source: &str, fragment: &Fragment) -> Option<CompileError> {
    for node in &fragment.nodes {
        if let Some(error) = detect_attribute_syntax_in_node(source, node) {
            return Some(error);
        }
    }
    None
}

fn detect_attribute_syntax_in_node(source: &str, node: &Node) -> Option<CompileError> {
    match node {
        Node::RegularElement(element) => {
            detect_attribute_syntax_in_element(source, &element.attributes, &element.fragment)
        }
        Node::Component(component) => {
            detect_attribute_syntax_in_element(source, &component.attributes, &component.fragment)
        }
        Node::SlotElement(slot) => {
            detect_attribute_syntax_in_element(source, &slot.attributes, &slot.fragment)
        }
        _ => find_error_in_child_fragments(node, |fragment| {
            detect_attribute_syntax_in_fragment(source, fragment)
        }),
    }
}

fn detect_attribute_syntax_in_element(
    source: &str,
    attributes: &[Attribute],
    fragment: &Fragment,
) -> Option<CompileError> {
    if let Some(error) = detect_attribute_syntax_in_attributes(source, attributes) {
        return Some(error);
    }
    detect_attribute_syntax_in_fragment(source, fragment)
}

fn detect_attribute_syntax_in_attributes(
    source: &str,
    attributes: &[Attribute],
) -> Option<CompileError> {
    for attribute in attributes {
        let Attribute::Attribute(attribute) = attribute else {
            continue;
        };
        let Some(error) = attribute.error.as_ref() else {
            continue;
        };

        return Some(match &error.kind {
            AttrErrorKind::InvalidName => compile_error_custom(
                source,
                "attribute_invalid_name",
                format!("'{}' is not a valid attribute name", attribute.name),
                error.start,
                error.end,
            ),
            AttrErrorKind::ExpectedEquals => compile_error_custom(
                source,
                "expected_token",
                "Expected token =",
                error.start,
                error.end,
            ),
            AttrErrorKind::ExpectedValue => compile_error_with_range(
                source,
                DiagnosticKind::ExpectedAttributeValue,
                error.start,
                error.end,
            ),
            AttrErrorKind::HtmlTag => compile_error_custom(
                source,
                "tag_invalid_placement",
                "{@html ...} tag cannot be in attribute value",
                error.start,
                error.end,
            ),
            AttrErrorKind::Block(kind) => compile_error_custom(
                source,
                "block_invalid_placement",
                format!("{{#{kind} ...}} block cannot be in attribute value"),
                error.start,
                error.end,
            ),
        });
    }
    None
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum AttrKey {
    Named(Arc<str>),
    Class(Arc<str>),
    Style(Arc<str>),
}

fn duplicate_attribute_error(source: &str, attributes: &[Attribute]) -> Option<CompileError> {
    let mut seen = HashSet::new();

    for attribute in attributes {
        let Some(key) = attr_key(attribute) else {
            continue;
        };

        if !seen.insert(key) {
            let (start, end) = attribute_span(attribute);
            return Some(compile_error_with_range(
                source,
                DiagnosticKind::AttributeDuplicate,
                start,
                end,
            ));
        }
    }

    None
}

fn attr_key(attribute: &Attribute) -> Option<AttrKey> {
    match attribute {
        Attribute::Attribute(attribute) => Some(AttrKey::Named(attribute.name.clone())),
        Attribute::BindDirective(attribute) => Some(AttrKey::Named(attribute.name.clone())),
        Attribute::ClassDirective(attribute) => Some(AttrKey::Class(attribute.name.clone())),
        Attribute::StyleDirective(attribute) => Some(AttrKey::Style(attribute.name.clone())),
        _ => None,
    }
}

fn detect_attribute_invalid_name_in_fragment(
    source: &str,
    fragment: &Fragment,
) -> Option<CompileError> {
    for node in fragment.nodes.iter() {
        if let Some(error) = detect_attribute_invalid_name_in_node(source, node) {
            return Some(error);
        }
    }
    None
}

fn detect_attribute_invalid_name_in_node(source: &str, node: &Node) -> Option<CompileError> {
    match node {
        Node::RegularElement(element) => {
            if let Some(error) =
                detect_attribute_invalid_name_in_attributes(source, &element.attributes, false)
            {
                return Some(error);
            }
            find_error_in_child_fragments(node, |fragment| {
                detect_attribute_invalid_name_in_fragment(source, fragment)
            })
        }
        Node::SlotElement(slot) => {
            if let Some(error) =
                detect_attribute_invalid_name_in_attributes(source, &slot.attributes, false)
            {
                return Some(error);
            }
            find_error_in_child_fragments(node, |fragment| {
                detect_attribute_invalid_name_in_fragment(source, fragment)
            })
        }
        Node::IfBlock(block) => {
            detect_attribute_invalid_name_in_fragment(source, &block.consequent).or_else(|| {
                block
                    .alternate
                    .as_deref()
                    .and_then(|alternate| match alternate {
                        Alternate::Fragment(fragment) => {
                            detect_attribute_invalid_name_in_fragment(source, fragment)
                        }
                        Alternate::IfBlock(block) => {
                            detect_attribute_invalid_name_in_if_block(source, block)
                        }
                    })
            })
        }
        _ => find_error_in_child_fragments(node, |fragment| {
            detect_attribute_invalid_name_in_fragment(source, fragment)
        }),
    }
}

fn detect_attribute_invalid_name_in_attributes(
    source: &str,
    attributes: &[Attribute],
    allow_custom_css_properties: bool,
) -> Option<CompileError> {
    for attribute in attributes.iter() {
        let Attribute::Attribute(attribute) = attribute else {
            continue;
        };
        if attribute.error.is_some() {
            continue;
        }

        let name = attribute.name.as_ref();
        if name.is_empty() {
            // `{foo}` shorthand attributes are represented as empty-name attributes with an
            // expression payload in this AST shape; they are validated elsewhere.
            if matches!(attribute.value, AttributeValueKind::ExpressionTag(_)) {
                continue;
            }
        }
        if allow_custom_css_properties && name.starts_with("--") {
            continue;
        }
        if !is_valid_attribute_name(name) {
            let start = attribute.start;
            let end = attribute.end;
            return Some(compile_error_custom(
                source,
                "attribute_invalid_name",
                format!("'{name}' is not a valid attribute name"),
                start,
                end,
            ));
        }
    }
    None
}

fn detect_attribute_invalid_name_in_if_block(
    source: &str,
    block: &IfBlock,
) -> Option<CompileError> {
    if let Some(error) = detect_attribute_invalid_name_in_fragment(source, &block.consequent) {
        return Some(error);
    }

    match &block.alternate {
        Some(alternate) => match alternate.as_ref() {
            Alternate::Fragment(fragment) => {
                detect_attribute_invalid_name_in_fragment(source, fragment)
            }
            Alternate::IfBlock(block) => detect_attribute_invalid_name_in_if_block(source, block),
        },
        None => None,
    }
}

fn is_valid_attribute_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }

    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };

    if first.is_ascii_digit() || matches!(first, '-' | '.') {
        return false;
    }

    !name.chars().any(|ch| {
        ch.is_whitespace()
            || matches!(
                ch,
                '<' | '>'
                    | '='
                    | '/'
                    | '"'
                    | '\''
                    | '^'
                    | '$'
                    | '@'
                    | '%'
                    | '&'
                    | '#'
                    | '?'
                    | '!'
                    | '|'
                    | '('
                    | ')'
                    | '['
                    | ']'
                    | '{'
                    | '}'
                    | '*'
                    | '+'
                    | '~'
                    | ';'
            )
    })
}

fn detect_component_directive_errors(
    source: &str,
    component: &Component,
    context: &ValidationContext,
) -> Option<CompileError> {
    for attribute in &component.attributes {
        if let Some(error) =
            detect_attribute_sequence_or_syntax_error(source, attribute, context.runes)
        {
            return Some(error);
        }

        match attribute {
            Attribute::BindDirective(directive) => {
                if let Some(error) = detect_bind_directive_error(source, directive, context, false)
                {
                    return Some(error);
                }
            }
            Attribute::OnDirective(directive) => {
                if !directive.modifiers.is_empty()
                    && directive
                        .modifiers
                        .iter()
                        .any(|modifier| modifier.as_ref() != "once")
                {
                    return Some(compile_error_custom(
                        source,
                        "event_handler_invalid_component_modifier",
                        "Event modifiers other than 'once' can only be used on DOM elements",
                        directive.start,
                        directive.end,
                    ));
                }
            }
            Attribute::UseDirective(directive) | Attribute::AnimateDirective(directive) => {
                return Some(compile_error_custom(
                    source,
                    "component_invalid_directive",
                    "This type of directive is not valid on components",
                    directive.start,
                    directive.end,
                ));
            }
            Attribute::TransitionDirective(directive) => {
                return Some(compile_error_custom(
                    source,
                    "component_invalid_directive",
                    "This type of directive is not valid on components",
                    directive.start,
                    directive.end,
                ));
            }
            Attribute::StyleDirective(directive) => {
                return Some(compile_error_custom(
                    source,
                    "component_invalid_directive",
                    "This type of directive is not valid on components",
                    directive.start,
                    directive.end,
                ));
            }
            Attribute::Attribute(_)
            | Attribute::SpreadAttribute(_)
            | Attribute::ClassDirective(_)
            | Attribute::LetDirective(_)
            | Attribute::AttachTag(_) => {}
        }
    }
    None
}

fn detect_element_directive_errors<E: Element>(
    source: &str,
    element: &E,
    each: Option<EachContext>,
    context: &ValidationContext,
) -> Option<CompileError> {
    let mut animate_spans = Vec::new();
    let mut has_in_transition = false;
    let mut has_out_transition = false;
    let mut has_bi_transition = false;
    for attribute in element.attributes() {
        if let Some(error) =
            detect_attribute_sequence_or_syntax_error(source, attribute, context.runes)
        {
            return Some(error);
        }

        if let Attribute::BindDirective(directive) = attribute {
            if let Some(error) = detect_bind_target_error_for_element(source, element, directive) {
                return Some(error);
            }
            if let Some(error) = detect_bind_directive_error(source, directive, context, true) {
                return Some(error);
            }
        }

        match attribute {
            Attribute::OnDirective(directive) => {
                if let Some(violation) =
                    find_assignment_violation_in_template_expression(&directive.expression, context)
                {
                    let kind = match violation.kind {
                        AssignmentKind::Constant => DiagnosticKind::ConstantAssignment,
                        AssignmentKind::SnippetParameter => {
                            DiagnosticKind::SnippetParameterAssignment
                        }
                        AssignmentKind::EachItemInvalid => {
                            DiagnosticKind::EachItemInvalidAssignment
                        }
                    };
                    return Some(compile_error_with_range(
                        source,
                        kind,
                        violation.start,
                        violation.end,
                    ));
                }
            }
            Attribute::AnimateDirective(directive) => {
                animate_spans.push((directive.start, directive.end));
            }
            Attribute::Attribute(attribute) if attribute.name.as_ref() == "animate" => {
                animate_spans.push((attribute.start, attribute.end));
            }
            Attribute::TransitionDirective(directive) => {
                if directive.intro && directive.outro {
                    if has_bi_transition {
                        return Some(compile_error_custom(
                            source,
                            "transition_duplicate",
                            "Cannot use multiple `transition:` directives on a single element",
                            directive.start,
                            directive.end,
                        ));
                    }
                    if has_in_transition {
                        return Some(compile_error_custom(
                            source,
                            "transition_conflict",
                            "Cannot use `in:` alongside existing `transition:` directive",
                            directive.start,
                            directive.end,
                        ));
                    }
                    if has_out_transition {
                        return Some(compile_error_custom(
                            source,
                            "transition_conflict",
                            "Cannot use `out:` alongside existing `transition:` directive",
                            directive.start,
                            directive.end,
                        ));
                    }
                    has_bi_transition = true;
                } else if directive.intro {
                    if has_in_transition {
                        return Some(compile_error_custom(
                            source,
                            "transition_duplicate",
                            "Cannot use multiple `in:` directives on a single element",
                            directive.start,
                            directive.end,
                        ));
                    }
                    if has_bi_transition {
                        return Some(compile_error_custom(
                            source,
                            "transition_conflict",
                            "Cannot use `transition:` alongside existing `in:` directive",
                            directive.start,
                            directive.end,
                        ));
                    }
                    has_in_transition = true;
                } else if directive.outro {
                    if has_out_transition {
                        return Some(compile_error_custom(
                            source,
                            "transition_duplicate",
                            "Cannot use multiple `out:` directives on a single element",
                            directive.start,
                            directive.end,
                        ));
                    }
                    if has_bi_transition {
                        return Some(compile_error_custom(
                            source,
                            "transition_conflict",
                            "Cannot use `transition:` alongside existing `out:` directive",
                            directive.start,
                            directive.end,
                        ));
                    }
                    has_out_transition = true;
                }
            }
            _ => {}
        }
    }

    if animate_spans.len() > 1 {
        let (start, end) = animate_spans[1];
        return Some(compile_error_custom(
            source,
            "animation_duplicate",
            "An element can only have one 'animate' directive",
            start,
            end,
        ));
    }

    if let Some((start, end)) = animate_spans.first().copied() {
        match each {
            None => {
                return Some(compile_error_custom(
                    source,
                    "animation_invalid_placement",
                    "An element that uses the `animate:` directive must be the only child of a keyed `{#each ...}` block",
                    start,
                    end,
                ));
            }
            Some(context) if !context.keyed => {
                return Some(compile_error_custom(
                    source,
                    "animation_missing_key",
                    "An element that uses the `animate:` directive must be the only child of a keyed `{#each ...}` block. Did you forget to add a key to your each block?",
                    start,
                    end,
                ));
            }
            Some(context) if context.animation_relevant_children != 1 => {
                return Some(compile_error_custom(
                    source,
                    "animation_invalid_placement",
                    "An element that uses the `animate:` directive must be the only child of a keyed `{#each ...}` block",
                    start,
                    end,
                ));
            }
            _ => {}
        }
    }

    None
}

fn detect_attribute_sequence_or_syntax_error(
    source: &str,
    attribute: &Attribute,
    runes_mode: bool,
) -> Option<CompileError> {
    match attribute {
        Attribute::Attribute(attribute) => {
            if runes_mode {
                if let Some(error) = detect_unquoted_attribute_sequence_from_ast(source, attribute)
                {
                    return Some(error);
                }
                if let Some(expression) = single_attribute_expression(attribute) {
                    return detect_unparenthesized_attribute_sequence_expression(
                        source, expression,
                    );
                }
            }
            None
        }
        Attribute::BindDirective(_) => None,
        Attribute::OnDirective(attribute)
        | Attribute::ClassDirective(attribute)
        | Attribute::LetDirective(attribute)
        | Attribute::AnimateDirective(attribute)
        | Attribute::UseDirective(attribute) => runes_mode
            .then(|| {
                detect_unparenthesized_attribute_sequence_expression(source, &attribute.expression)
            })
            .flatten(),
        Attribute::TransitionDirective(attribute) => runes_mode
            .then(|| {
                detect_unparenthesized_attribute_sequence_expression(source, &attribute.expression)
            })
            .flatten(),
        Attribute::StyleDirective(attribute) => runes_mode
            .then(|| {
                style_directive_expression(attribute).and_then(|expression| {
                    detect_unparenthesized_attribute_sequence_expression(source, expression)
                })
            })
            .flatten(),
        Attribute::AttachTag(attribute) => {
            detect_unparenthesized_attribute_sequence_expression(source, &attribute.expression)
        }
        Attribute::SpreadAttribute(_) => None,
    }
}

fn detect_unquoted_attribute_sequence_from_ast(
    source: &str,
    attribute: &NamedAttribute,
) -> Option<CompileError> {
    if attribute.value_syntax != AttributeValueSyntax::Unquoted {
        return None;
    }

    let AttributeValueKind::Values(values) = &attribute.value else {
        return None;
    };
    if values.len() <= 1 {
        return None;
    }

    Some(compile_error_with_range(
        source,
        DiagnosticKind::AttributeUnquotedSequence,
        attribute.start,
        attribute.end,
    ))
}

fn single_attribute_expression(attribute: &NamedAttribute) -> Option<&Expression> {
    match &attribute.value {
        AttributeValueKind::ExpressionTag(tag) => Some(&tag.expression),
        AttributeValueKind::Values(values) => {
            let [AttributeValue::ExpressionTag(tag)] = &values[..] else {
                return None;
            };
            Some(&tag.expression)
        }
        AttributeValueKind::Boolean(_) => None,
    }
}

fn style_directive_expression(
    attribute: &crate::ast::modern::StyleDirective,
) -> Option<&Expression> {
    match &attribute.value {
        AttributeValueKind::ExpressionTag(tag) => Some(&tag.expression),
        AttributeValueKind::Values(values) => {
            let [AttributeValue::ExpressionTag(tag)] = &values[..] else {
                return None;
            };
            Some(&tag.expression)
        }
        AttributeValueKind::Boolean(_) => None,
    }
}

fn detect_unparenthesized_attribute_sequence_expression(
    source: &str,
    expression: &Expression,
) -> Option<CompileError> {
    if expression_kind(expression) != Some("SequenceExpression") || expression.is_parenthesized() {
        return None;
    }

    let (start, end) = expression_span(expression)?;
    Some(compile_error_with_range(
        source,
        DiagnosticKind::AttributeInvalidSequenceExpression,
        start,
        end,
    ))
}

#[derive(Clone, Copy)]
enum BindExpr<'a> {
    Target,
    Pair(&'a Expression, usize),
    Invalid(&'a Expression),
}

fn bind_expr(directive: &DirectiveAttribute) -> BindExpr<'_> {
    let Some(target) = unwrap_expression_typescript(&directive.expression) else {
        return BindExpr::Invalid(&directive.expression);
    };

    if let OxcExpression::SequenceExpression(sequence) = target {
        let len = sequence.expressions.len();
        return BindExpr::Pair(&directive.expression, len);
    }

    if expression_is_identifier_or_member(target) {
        return BindExpr::Target;
    }

    BindExpr::Invalid(&directive.expression)
}

fn detect_bind_directive_error(
    source: &str,
    directive: &DirectiveAttribute,
    context: &ValidationContext,
    allow_group_specific_checks: bool,
) -> Option<CompileError> {
    if allow_group_specific_checks && directive.name.as_ref() == "group" {
        if !matches!(bind_expr(directive), BindExpr::Target) {
            return Some(compile_error_custom(
                source,
                "bind_group_invalid_expression",
                "`bind:group` can only bind to an Identifier or MemberExpression",
                directive.start,
                directive.end,
            ));
        }

        if let Some(base_identifier) = binding_base_identifier_name(&directive.expression)
            && context.snippets.contains(base_identifier.as_ref())
        {
            return Some(compile_error_custom(
                source,
                "bind_group_invalid_snippet_parameter",
                "Cannot `bind:group` to a snippet parameter",
                directive.start,
                directive.end,
            ));
        }
    }

    match bind_expr(directive) {
        BindExpr::Pair(expression, 2) => {
            if expression.is_parenthesized() {
                return Some(compile_error_custom(
                    source,
                    "bind_invalid_parens",
                    format!(
                        "`bind:{}={{get, set}}` must not have surrounding parentheses",
                        directive.name
                    ),
                    directive.start,
                    directive.end,
                ));
            }
            return None;
        }
        BindExpr::Pair(expression, _) | BindExpr::Invalid(expression) => {
            let (start, end) =
                expression_span(expression).unwrap_or((directive.start, directive.end));
            return Some(compile_error_custom(
                source,
                "bind_invalid_expression",
                "Can only bind to an Identifier or MemberExpression or a `{get, set}` pair",
                start,
                end,
            ));
        }
        BindExpr::Target => {}
    }

    let base_identifier = binding_base_identifier_name(&directive.expression)?;
    let unwrapped_expression = unwrap_expression_typescript(&directive.expression)?;
    let is_identifier_target = matches!(unwrapped_expression, OxcExpression::Identifier(_));

    if is_identifier_target && context.imports.contains(base_identifier.as_ref()) {
        return Some(compile_error_custom(
            source,
            "constant_binding",
            "Cannot bind to import",
            directive.start,
            directive.end,
        ));
    }

    if context.runes
        && context.each.contains(base_identifier.as_ref())
        && matches!(
            unwrap_expression_typescript(&directive.expression),
            Some(OxcExpression::Identifier(_))
        )
    {
        return Some(compile_error_with_range(
            source,
            DiagnosticKind::EachItemInvalidAssignment,
            directive.start,
            directive.end,
        ));
    }

    if context.snippets.contains(base_identifier.as_ref()) {
        return Some(compile_error_with_range(
            source,
            DiagnosticKind::SnippetParameterAssignment,
            directive.start,
            directive.end,
        ));
    }

    if is_identifier_target && context.immutable.contains(base_identifier.as_ref()) {
        return Some(compile_error_custom(
            source,
            "constant_binding",
            "Cannot bind to constant",
            directive.start,
            directive.end,
        ));
    }

    None
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SlotKind {
    Component,
    Element,
    Custom,
    Snippet,
    Other,
}

struct SlotFrame {
    kind: SlotKind,
    name: Option<Arc<str>>,
    slots: NameSet,
}

impl SlotFrame {
    fn new(kind: SlotKind) -> Self {
        Self {
            kind,
            name: None,
            slots: NameSet::default(),
        }
    }

    fn component(name: &Arc<str>) -> Self {
        Self {
            kind: SlotKind::Component,
            name: Some(name.clone()),
            slots: NameSet::default(),
        }
    }
}

fn detect_slot_attribute_errors_in_fragment(
    source: &str,
    fragment: &Fragment,
    stack: &mut Vec<SlotFrame>,
) -> Option<CompileError> {
    for node in &fragment.nodes {
        if let Some(error) = detect_slot_attribute_error_for_node(source, node, fragment, stack) {
            return Some(error);
        }
        if let Some(error) = detect_slot_attribute_errors_in_child_fragments(source, node, stack) {
            return Some(error);
        }
    }

    None
}

fn detect_slot_attribute_errors_in_child_fragments(
    source: &str,
    node: &Node,
    stack: &mut Vec<SlotFrame>,
) -> Option<CompileError> {
    stack.push(slot_frame(node));
    let result = node.try_for_each_child_fragment(|fragment| {
        if let Some(error) = detect_slot_attribute_errors_in_fragment(source, fragment, stack) {
            ControlFlow::Break(error)
        } else {
            ControlFlow::Continue(())
        }
    });
    stack.pop();
    match result {
        ControlFlow::Break(error) => Some(error),
        ControlFlow::Continue(()) => None,
    }
}

fn detect_slot_attribute_error_for_node(
    source: &str,
    node: &Node,
    fragment: &Fragment,
    stack: &mut [SlotFrame],
) -> Option<CompileError> {
    let attributes = node.as_element()?.attributes();
    let attribute = attributes.iter().find_map(|attribute| match attribute {
        Attribute::Attribute(attribute) if attribute.name.as_ref() == "slot" => Some(attribute),
        _ => None,
    })?;

    let kind = slot_kind(node);
    let is_component_attribute = matches!(kind, SlotKind::Component | SlotKind::Element);
    let parent_kind = stack.last().map(|frame| frame.kind);

    if parent_kind == Some(SlotKind::Snippet) {
        if static_attribute_text(attribute).is_none() {
            return Some(compile_error_custom(
                source,
                "slot_attribute_invalid",
                "slot attribute must be a static value",
                attribute.start,
                attribute.end,
            ));
        }
        return None;
    }

    let owner = stack.iter_mut().enumerate().rev().find(|(_, frame)| {
        matches!(
            frame.kind,
            SlotKind::Component | SlotKind::Element | SlotKind::Custom
        )
    });

    match owner {
        Some((_, frame)) if frame.kind == SlotKind::Component => {
            let direct_child_of_component = parent_kind == Some(SlotKind::Component);
            if !direct_child_of_component && !is_component_attribute {
                return Some(compile_error_with_range(
                    source,
                    DiagnosticKind::SlotAttributeInvalidPlacement,
                    attribute.start,
                    attribute.end,
                ));
            }

            if !direct_child_of_component {
                return None;
            }

            let Some(name) = static_attribute_text(attribute).map(Arc::<str>::from) else {
                return Some(compile_error_custom(
                    source,
                    "slot_attribute_invalid",
                    "slot attribute must be a static value",
                    attribute.start,
                    attribute.end,
                ));
            };

            let component = frame.name.clone().unwrap_or_else(|| Arc::from("Component"));
            if !frame.slots.insert(name.clone()) {
                return Some(compile_error_with_range(
                    source,
                    DiagnosticKind::SlotAttributeDuplicate {
                        slot: name,
                        component,
                    },
                    attribute.start,
                    attribute.end,
                ));
            }

            if name.as_ref() == "default"
                && let Some((start, end)) = default_slot_conflict(fragment)
            {
                return Some(compile_error_with_range(
                    source,
                    DiagnosticKind::SlotDefaultDuplicate,
                    start,
                    end,
                ));
            }
        }
        Some((_, frame)) if matches!(frame.kind, SlotKind::Element | SlotKind::Custom) => {}
        Some((_, frame)) if matches!(frame.kind, SlotKind::Snippet | SlotKind::Other) => {}
        Some((_, _)) => {}
        None if !is_component_attribute => {
            return Some(compile_error_with_range(
                source,
                DiagnosticKind::SlotAttributeInvalidPlacement,
                attribute.start,
                attribute.end,
            ));
        }
        None => {}
    }

    None
}

fn slot_frame(node: &Node) -> SlotFrame {
    match node {
        Node::Component(component) => SlotFrame::component(&component.name),
        Node::SvelteComponent(component) => SlotFrame::component(&component.name),
        Node::SvelteSelf(component) => SlotFrame::component(&component.name),
        _ => SlotFrame::new(slot_kind(node)),
    }
}

fn slot_kind(node: &Node) -> SlotKind {
    match node {
        Node::Component(_) | Node::SvelteComponent(_) | Node::SvelteSelf(_) => SlotKind::Component,
        Node::SvelteElement(_) => SlotKind::Element,
        Node::SnippetBlock(_) => SlotKind::Snippet,
        Node::RegularElement(RegularElement { name, .. }) if is_custom_element_name(name) => {
            SlotKind::Custom
        }
        _ => SlotKind::Other,
    }
}

fn default_slot_conflict(fragment: &Fragment) -> Option<(usize, usize)> {
    for node in fragment.nodes.iter() {
        if let Node::Text(text) = node
            && text.data.chars().all(char::is_whitespace)
        {
            continue;
        }

        if let Some(element) = node.as_element()
            && matches!(node, Node::RegularElement(_) | Node::SvelteFragment(_))
            && element_has_slot_attribute(element.attributes())
        {
            continue;
        }

        return Some(node_span(node));
    }

    None
}

#[derive(Clone, Copy)]
enum ConstOwner {
    Root,
    Element { slot_parent: bool },
    Component,
    Fragment,
    Boundary,
    If,
    Each,
    Await,
    Snippet,
    Key,
}

#[derive(Clone, Default)]
struct ConstScope {
    inherited: NameSet,
    current: NameSet,
}

impl ConstScope {
    fn visible(&self, local: &NameSet) -> NameSet {
        let mut visible = self.inherited.clone();
        visible.extend(self.current.iter().cloned());
        visible.extend(local.iter().cloned());
        visible
    }

    fn current_visible(&self, local: &NameSet) -> NameSet {
        let mut visible = self.current.clone();
        visible.extend(local.iter().cloned());
        visible
    }

    fn child(&self, local: &NameSet) -> Self {
        Self {
            inherited: self.visible(local),
            current: NameSet::default(),
        }
    }

    fn with_expression_bindings(mut self, expression: &Expression) -> Self {
        insert_expression_binding_names(expression, &mut self.current);
        self
    }

    fn with_optional_expression_bindings(self, expression: Option<&Expression>) -> Self {
        match expression {
            Some(expression) => self.with_expression_bindings(expression),
            None => self,
        }
    }

    fn with_optional_name(mut self, name: Option<&Arc<str>>) -> Self {
        if let Some(name) = name {
            self.current.insert(name.clone());
        }
        self
    }

    fn with_each_block_bindings(self, block: &EachBlock) -> Self {
        self.with_optional_expression_bindings(block.context.as_ref())
            .with_optional_name(block.index.as_ref())
    }

    fn snippet(&self, local: &NameSet, parameters: &[Expression], owner: ConstOwner) -> Self {
        let mut inherited = self.inherited.clone();
        if !matches!(owner, ConstOwner::Component | ConstOwner::Boundary) {
            inherited.extend(self.current.iter().cloned());
            inherited.extend(local.iter().cloned());
        }
        let mut current = NameSet::default();
        for parameter in parameters {
            insert_expression_binding_names(parameter, &mut current);
        }
        Self { inherited, current }
    }
}

fn detect_const_tag_errors_in_fragment(
    source: &str,
    fragment: &Fragment,
    owner: ConstOwner,
    scope: &ConstScope,
    async_mode: bool,
) -> Option<CompileError> {
    if let Some(cycle) = find_const_cycle(fragment) {
        return Some(compile_error_custom(
            source,
            "const_tag_cycle",
            format!(
                "Cyclical dependency detected: {}",
                cycle
                    .names
                    .iter()
                    .map(Arc::as_ref)
                    .collect::<Vec<_>>()
                    .join(" → ")
            ),
            cycle.tag.start,
            cycle.tag.end,
        ));
    }

    if let Some(error) = detect_const_tag_invalid_reference(source, fragment, owner, async_mode) {
        return Some(error);
    }

    let mut local = NameSet::default();

    for node in &fragment.nodes {
        match node {
            Node::ConstTag(tag) => {
                let visible = scope.current_visible(&local);
                if !const_owner_allows_declaration(owner) {
                    return Some(compile_error_custom(
                        source,
                        "const_tag_invalid_placement",
                        "`{@const}` must be the immediate child of `{#snippet}`, `{#if}`, `{:else if}`, `{:else}`, `{#each}`, `{:then}`, `{:catch}`, `<svelte:fragment>`, `<svelte:boundary>` or `<Component>`",
                        tag.start,
                        tag.end,
                    ));
                }

                if let Some((start, end)) = const_tag_invalid_expression_span(tag) {
                    return Some(compile_error_with_range(
                        source,
                        DiagnosticKind::ConstTagInvalidExpression,
                        start,
                        end,
                    ));
                }

                let bindings = const_tag_declared_bindings(tag);
                for (name, start, end) in &bindings {
                    if visible.contains(name.as_ref()) {
                        return Some(compile_error_custom(
                            source,
                            "declaration_duplicate",
                            format!("`{name}` has already been declared"),
                            *start,
                            *end,
                        ));
                    }
                }

                if let Some((kind, start, end)) = find_const_tag_invalid_rune_usage(tag) {
                    return Some(compile_error_with_range(source, kind, start, end));
                }

                for (name, _, _) in bindings {
                    local.insert(name);
                }
            }
            Node::RegularElement(_)
            | Node::Component(_)
            | Node::SlotElement(_)
            | Node::SvelteHead(_)
            | Node::SvelteBody(_)
            | Node::SvelteWindow(_)
            | Node::SvelteDocument(_)
            | Node::SvelteComponent(_)
            | Node::SvelteElement(_)
            | Node::SvelteSelf(_)
            | Node::SvelteFragment(_)
            | Node::SvelteBoundary(_)
            | Node::TitleElement(_) => {
                let Some(element) = node.as_element() else {
                    continue;
                };
                if let Some(error) = detect_const_tag_errors_in_fragment(
                    source,
                    element.fragment(),
                    const_owner(node, element),
                    &scope.child(&local),
                    async_mode,
                ) {
                    return Some(error);
                }
            }
            Node::IfBlock(block) => {
                let child = scope.child(&local);
                if let Some(error) = detect_const_tag_errors_in_fragment(
                    source,
                    &block.consequent,
                    ConstOwner::If,
                    &child,
                    async_mode,
                ) {
                    return Some(error);
                }
                if let Some(alternate) = block.alternate.as_deref() {
                    let result = match alternate {
                        Alternate::Fragment(fragment) => detect_const_tag_errors_in_fragment(
                            source,
                            fragment,
                            ConstOwner::If,
                            &child,
                            async_mode,
                        ),
                        Alternate::IfBlock(block) => detect_const_tag_errors_in_fragment(
                            source,
                            &block.consequent,
                            ConstOwner::If,
                            &child,
                            async_mode,
                        ),
                    };
                    if result.is_some() {
                        return result;
                    }
                }
            }
            Node::EachBlock(block) => {
                let child = scope.child(&local).with_each_block_bindings(block);
                if let Some(error) = detect_const_tag_errors_in_fragment(
                    source,
                    &block.body,
                    ConstOwner::Each,
                    &child,
                    async_mode,
                ) {
                    return Some(error);
                }
                if let Some(fallback) = block.fallback.as_ref()
                    && let Some(error) = detect_const_tag_errors_in_fragment(
                        source,
                        fallback,
                        ConstOwner::Each,
                        &scope.child(&local),
                        async_mode,
                    )
                {
                    return Some(error);
                }
            }
            Node::AwaitBlock(block) => {
                let child = scope.child(&local);
                if let Some(pending) = block.pending.as_ref()
                    && let Some(error) = detect_const_tag_errors_in_fragment(
                        source,
                        pending,
                        ConstOwner::Await,
                        &child,
                        async_mode,
                    )
                {
                    return Some(error);
                }

                if let Some(then) = block.then.as_ref() {
                    let then_scope = child
                        .clone()
                        .with_optional_expression_bindings(block.value.as_ref());
                    if let Some(error) = detect_const_tag_errors_in_fragment(
                        source,
                        then,
                        ConstOwner::Await,
                        &then_scope,
                        async_mode,
                    ) {
                        return Some(error);
                    }
                }

                if let Some(catch) = block.catch.as_ref() {
                    let catch_scope = child
                        .clone()
                        .with_optional_expression_bindings(block.error.as_ref());
                    if let Some(error) = detect_const_tag_errors_in_fragment(
                        source,
                        catch,
                        ConstOwner::Await,
                        &catch_scope,
                        async_mode,
                    ) {
                        return Some(error);
                    }
                }
            }
            Node::SnippetBlock(block) => {
                let child = scope.snippet(&local, &block.parameters, owner);
                if let Some(error) = detect_const_tag_errors_in_fragment(
                    source,
                    &block.body,
                    ConstOwner::Snippet,
                    &child,
                    async_mode,
                ) {
                    return Some(error);
                }
            }
            Node::KeyBlock(block) => {
                if let Some(error) = detect_const_tag_errors_in_fragment(
                    source,
                    &block.fragment,
                    ConstOwner::Key,
                    &scope.child(&local),
                    async_mode,
                ) {
                    return Some(error);
                }
            }
            _ => {}
        }
    }

    None
}

fn detect_const_tag_invalid_reference(
    source: &str,
    fragment: &Fragment,
    owner: ConstOwner,
    async_mode: bool,
) -> Option<CompileError> {
    if !matches!(owner, ConstOwner::Component | ConstOwner::Boundary) {
        return None;
    }

    let unavailable = unavailable_const_names(fragment);
    if unavailable.is_empty() {
        return None;
    }

    for node in &fragment.nodes {
        let Node::SnippetBlock(block) = node else {
            continue;
        };

        if !async_mode
            && matches!(owner, ConstOwner::Boundary)
            && matches!(
                block.expression.identifier_name().as_deref(),
                Some("failed" | "pending")
            )
        {
            continue;
        }

        let mut available = NameSet::default();
        for parameter in &block.parameters {
            insert_expression_binding_names(parameter, &mut available);
        }

        if let Some((name, start, end)) =
            find_unavailable_const_reference_in_fragment(&block.body, &unavailable, &available)
        {
            return Some(compile_error_custom(
                source,
                "const_tag_invalid_reference",
                format!(
                    "The `{{@const {name} = ...}}` declaration is not available in this snippet"
                ),
                start,
                end,
            ));
        }
    }

    None
}

fn unavailable_const_names(fragment: &Fragment) -> NameSet {
    let mut local = NameSet::default();
    for node in &fragment.nodes {
        let Node::ConstTag(tag) = node else {
            continue;
        };
        for (name, _, _) in const_tag_declared_bindings(tag) {
            local.insert(name);
        }
    }
    if local.is_empty() {
        return local;
    }

    let mut used = NameSet::default();
    for node in &fragment.nodes {
        if matches!(node, Node::SnippetBlock(_)) {
            continue;
        }
        collect_const_references_in_node(node, &local, &mut used);
    }

    local.retain(|name| !used.contains(name));
    local
}

fn find_unavailable_const_reference_in_fragment(
    fragment: &Fragment,
    unavailable: &NameSet,
    inherited: &NameSet,
) -> Option<(Arc<str>, usize, usize)> {
    let mut visible = inherited.clone();

    for node in &fragment.nodes {
        if let Some(found) = find_unavailable_const_reference_in_node(node, unavailable, &visible) {
            return Some(found);
        }

        match node {
            Node::ConstTag(tag) => {
                for (name, _, _) in const_tag_declared_bindings(tag) {
                    visible.insert(name);
                }
            }
            Node::IfBlock(block) => {
                if let Some(found) = find_unavailable_const_reference_in_fragment(
                    &block.consequent,
                    unavailable,
                    &visible,
                ) {
                    return Some(found);
                }
                if let Some(alternate) = block.alternate.as_deref() {
                    let found = match alternate {
                        Alternate::Fragment(fragment) => {
                            find_unavailable_const_reference_in_fragment(
                                fragment,
                                unavailable,
                                &visible,
                            )
                        }
                        Alternate::IfBlock(block) => find_unavailable_const_reference_in_fragment(
                            &block.consequent,
                            unavailable,
                            &visible,
                        ),
                    };
                    if found.is_some() {
                        return found;
                    }
                }
            }
            Node::EachBlock(block) => {
                let child = visible_with_each_block_bindings(&visible, block);
                if let Some(found) =
                    find_unavailable_const_reference_in_fragment(&block.body, unavailable, &child)
                {
                    return Some(found);
                }
                if let Some(fallback) = block.fallback.as_ref()
                    && let Some(found) = find_unavailable_const_reference_in_fragment(
                        fallback,
                        unavailable,
                        &visible,
                    )
                {
                    return Some(found);
                }
            }
            Node::AwaitBlock(block) => {
                if let Some(pending) = block.pending.as_ref()
                    && let Some(found) =
                        find_unavailable_const_reference_in_fragment(pending, unavailable, &visible)
                {
                    return Some(found);
                }
                if let Some(then) = block.then.as_ref() {
                    let child =
                        visible_with_optional_expression_bindings(&visible, block.value.as_ref());
                    if let Some(found) =
                        find_unavailable_const_reference_in_fragment(then, unavailable, &child)
                    {
                        return Some(found);
                    }
                }
                if let Some(catch) = block.catch.as_ref() {
                    let child =
                        visible_with_optional_expression_bindings(&visible, block.error.as_ref());
                    if let Some(found) =
                        find_unavailable_const_reference_in_fragment(catch, unavailable, &child)
                    {
                        return Some(found);
                    }
                }
            }
            Node::SnippetBlock(block) => {
                let child = visible_with_snippet_parameters(&visible, block);
                if let Some(found) =
                    find_unavailable_const_reference_in_fragment(&block.body, unavailable, &child)
                {
                    return Some(found);
                }
            }
            Node::KeyBlock(block) => {
                if let Some(found) = find_unavailable_const_reference_in_fragment(
                    &block.fragment,
                    unavailable,
                    &visible,
                ) {
                    return Some(found);
                }
            }
            _ => {
                if let Some(element) = node.as_element()
                    && let Some(found) = find_unavailable_const_reference_in_fragment(
                        element.fragment(),
                        unavailable,
                        &visible,
                    )
                {
                    return Some(found);
                }
            }
        }
    }

    None
}

fn visible_with_expression_bindings(visible: &NameSet, expression: &Expression) -> NameSet {
    let mut child = visible.clone();
    insert_expression_binding_names(expression, &mut child);
    child
}

fn visible_with_optional_expression_bindings(
    visible: &NameSet,
    expression: Option<&Expression>,
) -> NameSet {
    match expression {
        Some(expression) => visible_with_expression_bindings(visible, expression),
        None => visible.clone(),
    }
}

fn visible_with_each_block_bindings(visible: &NameSet, block: &EachBlock) -> NameSet {
    let mut child = visible.clone();
    if let Some(context) = block.context.as_ref() {
        insert_expression_binding_names(context, &mut child);
    }
    extend_name_set_with_optional_name(&mut child, block.index.as_ref());
    child
}

fn visible_with_snippet_parameters(visible: &NameSet, block: &SnippetBlock) -> NameSet {
    let mut child = visible.clone();
    for parameter in &block.parameters {
        insert_expression_binding_names(parameter, &mut child);
    }
    child
}

fn collect_const_references_in_node(node: &Node, names: &NameSet, out: &mut NameSet) {
    match node {
        Node::ConstTag(tag) => {
            for dependency in const_tag_dependencies(tag).as_slice() {
                if names.contains(dependency.as_ref()) {
                    out.insert(dependency.clone());
                }
            }
        }
        _ => {
            let _ = find_unavailable_const_reference_in_node(node, names, &NameSet::default())
                .map(|(name, _, _)| {
                    out.insert(name);
                });
        }
    }

    match node {
        Node::SnippetBlock(_) => {}
        _ => node.for_each_child_fragment(|fragment| {
            collect_const_references_in_fragment(fragment, names, out);
        }),
    }
}

fn collect_const_references_in_fragment(fragment: &Fragment, names: &NameSet, out: &mut NameSet) {
    for node in &fragment.nodes {
        collect_const_references_in_node(node, names, out);
    }
}

fn detect_const_tag_invalid_reference_in_attrs(
    attrs: &[Attribute],
    visible: &NameSet,
    unavailable: &NameSet,
) -> Option<(Arc<str>, usize, usize)> {
    for attr in attrs {
        let found = match attr {
            Attribute::Attribute(attr) => match &attr.value {
                AttributeValueKind::Boolean(_) => None,
                AttributeValueKind::ExpressionTag(tag) => {
                    find_unavailable_const_reference(&tag.expression, visible, unavailable)
                }
                AttributeValueKind::Values(values) => values.iter().find_map(|value| match value {
                    AttributeValue::Text(_) => None,
                    AttributeValue::ExpressionTag(tag) => {
                        find_unavailable_const_reference(&tag.expression, visible, unavailable)
                    }
                }),
            },
            Attribute::StyleDirective(style) => match &style.value {
                AttributeValueKind::Boolean(_) => None,
                AttributeValueKind::ExpressionTag(tag) => {
                    find_unavailable_const_reference(&tag.expression, visible, unavailable)
                }
                AttributeValueKind::Values(values) => values.iter().find_map(|value| match value {
                    AttributeValue::Text(_) => None,
                    AttributeValue::ExpressionTag(tag) => {
                        find_unavailable_const_reference(&tag.expression, visible, unavailable)
                    }
                }),
            },
            Attribute::BindDirective(attr)
            | Attribute::OnDirective(attr)
            | Attribute::ClassDirective(attr)
            | Attribute::LetDirective(attr)
            | Attribute::AnimateDirective(attr)
            | Attribute::UseDirective(attr) => {
                find_unavailable_const_reference(&attr.expression, visible, unavailable)
            }
            Attribute::TransitionDirective(attr) => {
                find_unavailable_const_reference(&attr.expression, visible, unavailable)
            }
            Attribute::SpreadAttribute(attr) => {
                find_unavailable_const_reference(&attr.expression, visible, unavailable)
            }
            Attribute::AttachTag(tag) => {
                find_unavailable_const_reference(&tag.expression, visible, unavailable)
            }
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

fn find_unavailable_const_reference_in_node(
    node: &Node,
    unavailable: &NameSet,
    visible: &NameSet,
) -> Option<(Arc<str>, usize, usize)> {
    let expression = match node {
        Node::ExpressionTag(tag) => {
            find_unavailable_const_reference(&tag.expression, visible, unavailable)
        }
        Node::RenderTag(tag) => {
            find_unavailable_const_reference(&tag.expression, visible, unavailable)
        }
        Node::HtmlTag(tag) => {
            find_unavailable_const_reference(&tag.expression, visible, unavailable)
        }
        Node::ConstTag(tag) => {
            find_unavailable_const_reference(&tag.declaration, visible, unavailable)
        }
        Node::IfBlock(block) => find_unavailable_const_reference(&block.test, visible, unavailable),
        Node::EachBlock(block) => {
            find_unavailable_const_reference(&block.expression, visible, unavailable).or_else(
                || {
                    block
                        .key
                        .as_ref()
                        .and_then(|key| find_unavailable_const_reference(key, visible, unavailable))
                },
            )
        }
        Node::KeyBlock(block) => {
            find_unavailable_const_reference(&block.expression, visible, unavailable)
        }
        Node::AwaitBlock(block) => {
            find_unavailable_const_reference(&block.expression, visible, unavailable)
        }
        Node::DebugTag(tag) => tag
            .arguments
            .iter()
            .find_map(|argument| find_unavailable_const_reference(argument, visible, unavailable)),
        _ => None,
    };
    if expression.is_some() {
        return expression;
    }

    let element = node.as_element()?;
    detect_const_tag_invalid_reference_in_attrs(element.attributes(), visible, unavailable).or_else(
        || {
            element.expression().and_then(|expression| {
                find_unavailable_const_reference(expression, visible, unavailable)
            })
        },
    )
}

fn find_unavailable_const_reference(
    expression: &Expression,
    visible: &NameSet,
    unavailable: &NameSet,
) -> Option<(Arc<str>, usize, usize)> {
    struct Visitor<'a> {
        visible: &'a NameSet,
        unavailable: &'a NameSet,
        found: Option<(Arc<str>, usize, usize)>,
    }

    impl<'a> Visit<'a> for Visitor<'_> {
        fn visit_identifier_reference(&mut self, it: &IdentifierReference<'a>) {
            if self.found.is_some() {
                return;
            }

            let name = it.name.as_str();
            if self.visible.contains(name) || !self.unavailable.contains(name) {
                return;
            }

            let (start, end) = span_range(it.span);
            self.found = Some((Arc::from(name), start, end));
        }
    }

    let offset = expression.start;

    if let Some(node) = expression.oxc_expression() {
        let mut visitor = Visitor {
            visible,
            unavailable,
            found: None,
        };
        visitor.visit_expression(node);
        if let Some((name, start, end)) = visitor.found {
            return Some((name, start + offset, end + offset));
        }
    }

    if let Some(declaration) = expression.oxc_variable_declaration() {
        for declarator in &declaration.declarations {
            if let Some(init) = declarator.init.as_ref() {
                let mut visitor = Visitor {
                    visible,
                    unavailable,
                    found: None,
                };
                visitor.visit_expression(init);
                if let Some((name, start, end)) = visitor.found {
                    return Some((name, start + offset, end + offset));
                }
            }
        }
    }

    for name in expression_binding_names(expression).iter() {
        if visible.contains(name.as_ref()) || !unavailable.contains(name.as_ref()) {
            continue;
        }
        return Some((name.clone(), expression.start, expression.end));
    }

    None
}

fn find_const_cycle(fragment: &Fragment) -> Option<ConstCycle<'_>> {
    let mut tags = HashMap::<Arc<str>, &ConstTag>::new();
    let mut graph = HashMap::<Arc<str>, OrderedNames>::new();
    let mut order = Vec::<Arc<str>>::new();

    for node in &fragment.nodes {
        let Node::ConstTag(tag) = node else {
            continue;
        };

        let bindings = const_tag_declared_bindings(tag);
        if bindings.is_empty() {
            continue;
        }

        let deps = const_tag_dependencies(tag);
        for (name, _, _) in bindings {
            tags.insert(name.clone(), tag);
            graph.insert(name.clone(), deps.clone());
            order.push(name);
        }
    }

    if tags.len() < 2 {
        return None;
    }

    for deps in graph.values_mut() {
        deps.retain(|dep| tags.contains_key(dep));
    }

    let graph = freeze_name_graph(graph);
    let mut stack = Vec::<Arc<str>>::new();
    let mut active = NameSet::default();
    let mut visited = NameSet::default();

    for name in &order {
        let Some(cycle) = find_reactive_cycle(name, &graph, &mut visited, &mut active, &mut stack)
        else {
            continue;
        };
        let tag = tags.get(&cycle[0])?;
        return Some(ConstCycle {
            tag,
            names: cycle.into_boxed_slice(),
        });
    }

    None
}

fn const_tag_dependencies(tag: &ConstTag) -> OrderedNames {
    let Some(declaration) = tag.declaration.oxc_variable_declaration() else {
        return OrderedNames::default();
    };

    struct Visitor<'a> {
        names: &'a mut OrderedNames,
    }

    impl<'a> Visit<'a> for Visitor<'_> {
        fn visit_identifier_reference(&mut self, it: &IdentifierReference<'a>) {
            self.names.extend([Arc::from(it.name.as_str())]);
        }
    }

    let mut names = OrderedNames::default();
    for declarator in &declaration.declarations {
        if let Some(init) = declarator.init.as_ref() {
            let mut visitor = Visitor { names: &mut names };
            visitor.visit_expression(init);
        }
    }
    names
}

fn const_tag_invalid_expression_span(tag: &ConstTag) -> Option<(usize, usize)> {
    let declaration = tag.declaration.oxc_variable_declaration()?;
    let declaration_offset = tag
        .declaration
        .start
        .saturating_sub(declaration.span.start as usize);
    if declaration.kind != VariableDeclarationKind::Const {
        return expression_span(&tag.declaration).or(Some((tag.start, tag.end.saturating_sub(1))));
    }
    if declaration.declarations.len() > 1 {
        let first = declaration.declarations.first()?;
        let start = first
            .init
            .as_ref()
            .map(|init| declaration_offset + init.span().start as usize)
            .unwrap_or(declaration_offset + first.span.end as usize);
        let end = declaration_offset + declaration.span.end as usize;
        return Some((start, end));
    }
    if declaration.declarations.len() != 1 {
        return expression_span(&tag.declaration).or(Some((tag.start, tag.end.saturating_sub(1))));
    }
    let declarator = declaration.declarations.first()?;
    let init = declarator.init.as_ref()?;
    if let OxcExpression::SequenceExpression(sequence) = init.get_inner_expression()
        && !matches!(init, OxcExpression::ParenthesizedExpression(_))
    {
        return Some(span_range(sequence.span));
    }
    None
}

fn const_owner(node: &Node, element: &dyn Element) -> ConstOwner {
    match node {
        Node::Component(_) | Node::SvelteComponent(_) | Node::SvelteSelf(_) => {
            ConstOwner::Component
        }
        Node::SvelteFragment(_) => ConstOwner::Fragment,
        Node::SvelteBoundary(_) => ConstOwner::Boundary,
        _ => ConstOwner::Element {
            slot_parent: element_has_slot_attribute(element.attributes()),
        },
    }
}

fn const_owner_allows_declaration(owner: ConstOwner) -> bool {
    match owner {
        ConstOwner::Root => false,
        ConstOwner::Element { slot_parent } => slot_parent,
        ConstOwner::Component
        | ConstOwner::Fragment
        | ConstOwner::Boundary
        | ConstOwner::If
        | ConstOwner::Each
        | ConstOwner::Await
        | ConstOwner::Snippet
        | ConstOwner::Key => true,
    }
}

fn find_const_tag_invalid_rune_usage(
    tag: &ConstTag,
) -> Option<(DiagnosticKind, usize, usize)> {
    let offset = tag.declaration.start;
    for name in ["$state", "$state.raw"] {
        if let Some((start, end)) = find_first_call_named_in_expression(&tag.declaration, name) {
            return Some((
                DiagnosticKind::StateInvalidPlacement,
                start + offset,
                end + offset,
            ));
        }
    }
    for name in ["$derived", "$derived.by"] {
        if let Some((start, end)) = find_first_call_named_in_expression(&tag.declaration, name) {
            return Some((
                DiagnosticKind::StateInvalidPlacementDerived,
                start + offset,
                end + offset,
            ));
        }
    }

    None
}

fn find_first_call_named_in_expression(
    expression: &Expression,
    expected_name: &str,
) -> Option<(usize, usize)> {
    struct Visitor<'n> {
        expected_name: &'n str,
        found: Option<(usize, usize)>,
    }

    impl<'a> Visit<'a> for Visitor<'_> {
        fn visit_call_expression(&mut self, it: &oxc_ast::ast::CallExpression<'a>) {
            if self.found.is_some() {
                return;
            }
            if oxc_callee_name(&it.callee).as_deref() == Some(self.expected_name) {
                self.found = Some(span_range(it.span));
                return;
            }
            walk::walk_call_expression(self, it);
        }
    }

    if let Some(node) = expression.oxc_expression() {
        let mut visitor = Visitor {
            expected_name,
            found: None,
        };
        visitor.visit_expression(node);
        if visitor.found.is_some() {
            return visitor.found;
        }
    }

    let declaration = expression.oxc_variable_declaration()?;
    for declarator in &declaration.declarations {
        if let Some(init) = declarator.init.as_ref() {
            let mut visitor = Visitor {
                expected_name,
                found: None,
            };
            visitor.visit_expression(init);
            if visitor.found.is_some() {
                return visitor.found;
            }
        }
    }

    None
}

fn detect_bind_invalid_value_in_fragment(
    source: &str,
    fragment: &Fragment,
    scope: &mut NameStack,
) -> Option<CompileError> {
    scope.with_frame(
        |_| {},
        |scope| {
            for node in &fragment.nodes {
                match node {
                    Node::RegularElement(element) => {
                        if let Some(error) = detect_bind_invalid_value_in_attributes(
                            source,
                            &element.attributes,
                            scope,
                        ) {
                            return Some(error);
                        }
                        if let Some(error) = find_error_in_child_fragments_with_scope(
                            node,
                            scope,
                            |fragment, scope| {
                                detect_bind_invalid_value_in_fragment(source, fragment, scope)
                            },
                        ) {
                            return Some(error);
                        }
                    }
                    Node::Component(component) => {
                        if let Some(error) = detect_bind_invalid_value_in_attributes(
                            source,
                            &component.attributes,
                            scope,
                        ) {
                            return Some(error);
                        }
                        if let Some(error) = find_error_in_child_fragments_with_scope(
                            node,
                            scope,
                            |fragment, scope| {
                                detect_bind_invalid_value_in_fragment(source, fragment, scope)
                            },
                        ) {
                            return Some(error);
                        }
                    }
                    Node::SlotElement(_) => {
                        if let Some(error) = find_error_in_child_fragments_with_scope(
                            node,
                            scope,
                            |fragment, scope| {
                                detect_bind_invalid_value_in_fragment(source, fragment, scope)
                            },
                        ) {
                            return Some(error);
                        }
                    }
                    Node::IfBlock(_) => {
                        if let Some(error) = find_error_in_child_fragments_with_scope(
                            node,
                            scope,
                            |fragment, scope| {
                                detect_bind_invalid_value_in_fragment(source, fragment, scope)
                            },
                        ) {
                            return Some(error);
                        }
                    }
                    Node::EachBlock(block) => {
                        if let Some(error) = scope.with_frame(
                            |scope| scope.extend_each_block_bindings(block),
                            |scope| {
                                detect_bind_invalid_value_in_fragment(source, &block.body, scope)
                            },
                        ) {
                            return Some(error);
                        }
                        if let Some(fallback) = block.fallback.as_ref()
                            && let Some(error) =
                                detect_bind_invalid_value_in_fragment(source, fallback, scope)
                        {
                            return Some(error);
                        }
                    }
                    Node::AwaitBlock(block) => {
                        if let Some(pending) = block.pending.as_ref()
                            && let Some(error) =
                                detect_bind_invalid_value_in_fragment(source, pending, scope)
                        {
                            return Some(error);
                        }
                        if let Some(then) = block.then.as_ref()
                            && let Some(error) = scope.with_frame(
                                |scope| {
                                    scope.extend_optional_expression_bindings(block.value.as_ref());
                                },
                                |scope| detect_bind_invalid_value_in_fragment(source, then, scope),
                            )
                        {
                            return Some(error);
                        }
                        if let Some(catch) = block.catch.as_ref()
                            && let Some(error) = scope.with_frame(
                                |scope| {
                                    scope.extend_optional_expression_bindings(block.error.as_ref());
                                },
                                |scope| detect_bind_invalid_value_in_fragment(source, catch, scope),
                            )
                        {
                            return Some(error);
                        }
                    }
                    Node::SnippetBlock(block) => {
                        if let Some(error) = scope.with_frame(
                            |scope| scope.extend_snippet_parameters(block),
                            |scope| {
                                detect_bind_invalid_value_in_fragment(source, &block.body, scope)
                            },
                        ) {
                            return Some(error);
                        }
                    }
                    Node::KeyBlock(block) => {
                        if let Some(error) =
                            detect_bind_invalid_value_in_fragment(source, &block.fragment, scope)
                        {
                            return Some(error);
                        }
                    }
                    Node::ConstTag(tag) => {
                        scope.extend_const_tag_identifiers(tag);
                    }
                    _ => {}
                }
            }

            None
        },
    )
}

fn detect_constant_binding_in_fragment(
    source: &str,
    fragment: &Fragment,
    immutable: &NameStack,
    scope: &mut NameStack,
) -> Option<CompileError> {
    scope.with_frame(
        |_| {},
        |scope| {
            for node in &fragment.nodes {
                match node {
                    Node::RegularElement(element) => {
                        if let Some(error) = detect_constant_binding_in_attributes(
                            source,
                            &element.attributes,
                            immutable,
                            scope,
                        ) {
                            return Some(error);
                        }
                        if let Some(error) = find_error_in_child_fragments_with_scope(
                            node,
                            scope,
                            |fragment, scope| {
                                detect_constant_binding_in_fragment(
                                    source, fragment, immutable, scope,
                                )
                            },
                        ) {
                            return Some(error);
                        }
                    }
                    Node::Component(component) => {
                        if let Some(error) = detect_constant_binding_in_attributes(
                            source,
                            &component.attributes,
                            immutable,
                            scope,
                        ) {
                            return Some(error);
                        }
                        if let Some(error) = find_error_in_child_fragments_with_scope(
                            node,
                            scope,
                            |fragment, scope| {
                                detect_constant_binding_in_fragment(
                                    source, fragment, immutable, scope,
                                )
                            },
                        ) {
                            return Some(error);
                        }
                    }
                    Node::SlotElement(_) => {
                        if let Some(error) = find_error_in_child_fragments_with_scope(
                            node,
                            scope,
                            |fragment, scope| {
                                detect_constant_binding_in_fragment(
                                    source, fragment, immutable, scope,
                                )
                            },
                        ) {
                            return Some(error);
                        }
                    }
                    Node::IfBlock(_) => {
                        if let Some(error) = find_error_in_child_fragments_with_scope(
                            node,
                            scope,
                            |fragment, scope| {
                                detect_constant_binding_in_fragment(
                                    source, fragment, immutable, scope,
                                )
                            },
                        ) {
                            return Some(error);
                        }
                    }
                    Node::EachBlock(block) => {
                        if let Some(error) = scope.with_frame(
                            |scope| scope.extend_each_block_bindings(block),
                            |scope| {
                                detect_constant_binding_in_fragment(
                                    source,
                                    &block.body,
                                    immutable,
                                    scope,
                                )
                            },
                        ) {
                            return Some(error);
                        }
                        if let Some(fallback) = block.fallback.as_ref()
                            && let Some(error) = detect_constant_binding_in_fragment(
                                source, fallback, immutable, scope,
                            )
                        {
                            return Some(error);
                        }
                    }
                    Node::AwaitBlock(block) => {
                        if let Some(pending) = block.pending.as_ref()
                            && let Some(error) = detect_constant_binding_in_fragment(
                                source, pending, immutable, scope,
                            )
                        {
                            return Some(error);
                        }
                        if let Some(then) = block.then.as_ref()
                            && let Some(error) = scope.with_frame(
                                |scope| {
                                    scope.extend_optional_expression_bindings(block.value.as_ref());
                                },
                                |scope| {
                                    detect_constant_binding_in_fragment(
                                        source, then, immutable, scope,
                                    )
                                },
                            )
                        {
                            return Some(error);
                        }
                        if let Some(catch) = block.catch.as_ref()
                            && let Some(error) = scope.with_frame(
                                |scope| {
                                    scope.extend_optional_expression_bindings(block.error.as_ref());
                                },
                                |scope| {
                                    detect_constant_binding_in_fragment(
                                        source, catch, immutable, scope,
                                    )
                                },
                            )
                        {
                            return Some(error);
                        }
                    }
                    Node::SnippetBlock(block) => {
                        if let Some(error) = scope.with_frame(
                            |scope| scope.extend_snippet_parameters(block),
                            |scope| {
                                detect_constant_binding_in_fragment(
                                    source,
                                    &block.body,
                                    immutable,
                                    scope,
                                )
                            },
                        ) {
                            return Some(error);
                        }
                    }
                    Node::KeyBlock(block) => {
                        if let Some(error) = detect_constant_binding_in_fragment(
                            source,
                            &block.fragment,
                            immutable,
                            scope,
                        ) {
                            return Some(error);
                        }
                    }
                    Node::ConstTag(tag) => {
                        scope.extend_const_tag_identifiers(tag);
                    }
                    _ => {}
                }
            }

            None
        },
    )
}

fn detect_constant_binding_in_attributes(
    source: &str,
    attributes: &[Attribute],
    immutable: &NameStack,
    scope: &NameStack,
) -> Option<CompileError> {
    for attribute in attributes {
        let Attribute::BindDirective(directive) = attribute else {
            continue;
        };
        if directive.name.as_ref() == "this"
            || !matches!(
                unwrap_expression_typescript(&directive.expression),
                Some(OxcExpression::Identifier(_))
            )
        {
            continue;
        }

        let Some(name) = binding_base_identifier_name(&directive.expression) else {
            continue;
        };
        if scope.contains(name.as_ref()) || !immutable.contains(name.as_ref()) {
            continue;
        }

        let (start, end) =
            expression_span(&directive.expression).unwrap_or((directive.start, directive.end));
        return Some(compile_error_custom(
            source,
            "constant_binding",
            "Cannot bind to constant",
            start,
            end,
        ));
    }

    None
}

fn detect_bind_invalid_value_in_attributes(
    source: &str,
    attributes: &[Attribute],
    scope: &NameStack,
) -> Option<CompileError> {
    for attribute in attributes {
        let Attribute::BindDirective(directive) = attribute else {
            continue;
        };
        if directive.name.as_ref() == "this"
            || expression_kind(&directive.expression) != Some("Identifier")
        {
            continue;
        }

        let Some(name) = binding_base_identifier_name(&directive.expression) else {
            continue;
        };
        if scope.contains(name.as_ref()) || is_store_subscription_binding_name(name.as_ref()) {
            continue;
        }

        let (start, end) =
            expression_span(&directive.expression).unwrap_or((directive.start, directive.end));
        return Some(compile_error_custom(
            source,
            "bind_invalid_value",
            "Can only bind to state or props",
            start,
            end,
        ));
    }

    None
}

fn is_store_subscription_binding_name(name: &str) -> bool {
    name.len() > 1 && name.starts_with('$') && !name.starts_with("$$")
}

fn detect_bind_target_error_for_name(
    source: &str,
    name: &str,
    directive: &DirectiveAttribute,
) -> Option<CompileError> {
    if name == "svelte:window" {
        let allowed = [
            "devicePixelRatio",
            "focused",
            "innerHeight",
            "innerWidth",
            "online",
            "outerHeight",
            "outerWidth",
            "scrollX",
            "scrollY",
            "this",
        ];
        if !allowed.contains(&directive.name.as_ref()) {
            let message = if directive.name.as_ref() == "innerwidth" {
                "`bind:innerwidth` is not a valid binding. Did you mean 'innerWidth'?".to_string()
            } else if looks_like_window_dimension_binding(directive.name.as_ref()) {
                format!(
                    "`bind:{}` is not a valid binding. Possible bindings for <svelte:window> are devicePixelRatio, focused, innerHeight, innerWidth, online, outerHeight, outerWidth, scrollX, scrollY, this",
                    directive.name
                )
            } else {
                format!("`bind:{}` is not a valid binding", directive.name)
            };
            return Some(compile_error_custom(
                source,
                "bind_invalid_name",
                message,
                directive.start,
                directive.end,
            ));
        }
    }

    if name == "svelte:document" {
        let allowed = [
            "activeElement",
            "focused",
            "fullscreenElement",
            "pointerLockElement",
            "this",
            "visibilityState",
        ];
        if !allowed.contains(&directive.name.as_ref()) {
            return Some(compile_error_custom(
                source,
                "bind_invalid_name",
                format!(
                    "`bind:{}` is not a valid binding. Possible bindings for <svelte:document> are activeElement, focused, fullscreenElement, pointerLockElement, this, visibilityState",
                    directive.name
                ),
                directive.start,
                directive.end,
            ));
        }
    }

    None
}

fn detect_bind_target_error_for_element<E: Element>(
    source: &str,
    element: &E,
    directive: &DirectiveAttribute,
) -> Option<CompileError> {
    if let Some(error) = detect_bind_target_error_for_name(source, element.name(), directive) {
        return Some(error);
    }

    match directive.name.as_ref() {
        "value" => {
            if !matches!(element.name(), "input" | "textarea" | "select") {
                return Some(compile_error_custom(
                    source,
                    "bind_invalid_target",
                    "`bind:value` can only be used with `<input>`, `<textarea>`, `<select>`",
                    directive.start,
                    directive.end,
                ));
            }

            if element.name() == "input"
                && let Some((start, end)) = invalid_input_type_attribute_span(element.attributes())
            {
                return Some(compile_error_custom(
                    source,
                    "attribute_invalid_type",
                    "'type' attribute must be a static text value if input uses two-way binding",
                    start,
                    end,
                ));
            }

            if element.name() == "select"
                && let Some((start, end)) =
                    invalid_select_multiple_attribute_span(element.attributes())
            {
                return Some(compile_error_custom(
                    source,
                    "attribute_invalid_multiple",
                    "'multiple' attribute must be static if select uses two-way binding",
                    start,
                    end,
                ));
            }
        }
        "checked" => {
            if element.name() != "input" {
                return Some(compile_error_custom(
                    source,
                    "bind_invalid_target",
                    "`bind:checked` can only be used with `<input type=\"checkbox\">`",
                    directive.start,
                    directive.end,
                ));
            }
            match input_type_attribute(element.attributes()) {
                Some(InputTypeAttribute::Static("checkbox")) => {}
                Some(InputTypeAttribute::Static("radio")) => {
                    return Some(compile_error_custom(
                        source,
                        "bind_invalid_target",
                        "`bind:checked` can only be used with `<input type=\"checkbox\">` — for `<input type=\"radio\">`, use `bind:group`",
                        directive.start,
                        directive.end,
                    ));
                }
                _ => {
                    return Some(compile_error_custom(
                        source,
                        "bind_invalid_target",
                        "`bind:checked` can only be used with `<input type=\"checkbox\">`",
                        directive.start,
                        directive.end,
                    ));
                }
            }
        }
        "open" if element.name() != "details" => {
            return Some(compile_error_custom(
                source,
                "bind_invalid_target",
                "`bind:open` can only be used with `<details>`",
                directive.start,
                directive.end,
            ));
        }
        "offsetWidth" if element.name() == "svg" => {
            return Some(compile_error_custom(
                source,
                "bind_invalid_target",
                "`bind:offsetWidth` can only be used with non-`<svg>` elements. Use `bind:clientWidth` for `<svg>` instead",
                directive.start,
                directive.end,
            ));
        }
        "textContent" | "innerHTML" | "innerText" => {
            let contenteditable =
                element
                    .attributes()
                    .iter()
                    .find_map(|attribute| match attribute {
                        Attribute::Attribute(attribute)
                            if attribute.name.as_ref() == "contenteditable" =>
                        {
                            Some(attribute)
                        }
                        _ => None,
                    });

            let Some(contenteditable) = contenteditable else {
                return Some(compile_error_custom(
                    source,
                    "attribute_contenteditable_missing",
                    "'contenteditable' attribute is required for textContent, innerHTML and innerText two-way bindings",
                    directive.start,
                    directive.end,
                ));
            };

            let is_dynamic = !matches!(contenteditable.value, AttributeValueKind::Boolean(true))
                && static_attribute_text(contenteditable).is_none();
            if is_dynamic {
                return Some(compile_error_custom(
                    source,
                    "attribute_contenteditable_dynamic",
                    "'contenteditable' attribute cannot be dynamic if element uses two-way binding",
                    contenteditable.start,
                    contenteditable.end,
                ));
            }
        }
        "whatever" => {
            return Some(compile_error_custom(
                source,
                "bind_invalid_name",
                "`bind:whatever` is not a valid binding",
                directive.start,
                directive.end,
            ));
        }
        _ => {}
    }

    None
}

fn looks_like_window_dimension_binding(name: &str) -> bool {
    matches!(
        name,
        "clientWidth" | "clientHeight" | "offsetWidth" | "offsetHeight"
    ) || name.ends_with("Width")
        || name.ends_with("Height")
}

enum InputTypeAttribute<'a> {
    Static(&'a str),
    Dynamic,
}

fn input_type_attribute(attributes: &[Attribute]) -> Option<InputTypeAttribute<'_>> {
    let attribute = attributes.iter().find_map(|attribute| match attribute {
        Attribute::Attribute(attribute) if attribute.name.as_ref() == "type" => Some(attribute),
        _ => None,
    })?;

    match &attribute.value {
        AttributeValueKind::Values(values) => {
            if values.len() == 1
                && let Some(AttributeValue::Text(text)) = values.first()
            {
                return Some(InputTypeAttribute::Static(text.data.as_ref()));
            }
            Some(InputTypeAttribute::Dynamic)
        }
        AttributeValueKind::Boolean(_) | AttributeValueKind::ExpressionTag(_) => {
            Some(InputTypeAttribute::Dynamic)
        }
    }
}

fn invalid_input_type_attribute_span(attributes: &[Attribute]) -> Option<(usize, usize)> {
    let attribute = attributes.iter().find_map(|attribute| match attribute {
        Attribute::Attribute(attribute) if attribute.name.as_ref() == "type" => Some(attribute),
        _ => None,
    })?;

    match &attribute.value {
        AttributeValueKind::Boolean(_) => Some((attribute.start, attribute.end)),
        AttributeValueKind::Values(_) | AttributeValueKind::ExpressionTag(_) => None,
    }
}

fn invalid_select_multiple_attribute_span(attributes: &[Attribute]) -> Option<(usize, usize)> {
    let attribute = attributes.iter().find_map(|attribute| match attribute {
        Attribute::Attribute(attribute) if attribute.name.as_ref() == "multiple" => Some(attribute),
        _ => None,
    })?;

    match &attribute.value {
        AttributeValueKind::Boolean(_) => None,
        _ => Some((attribute.start, attribute.end)),
    }
}

fn static_attribute_text(attribute: &NamedAttribute) -> Option<&str> {
    match &attribute.value {
        AttributeValueKind::Values(values) => {
            if values.len() == 1
                && let Some(AttributeValue::Text(text)) = values.first()
            {
                return Some(text.data.as_ref());
            }
            None
        }
        AttributeValueKind::Boolean(_) | AttributeValueKind::ExpressionTag(_) => None,
    }
}

fn collect_imported_bindings(root: &Root) -> NameSet {
    let mut bindings = NameSet::default();

    if let Some(script) = root.module.as_ref() {
        collect_imported_bindings_in_program(&script.content, &mut bindings);
    }
    if let Some(script) = root.instance.as_ref() {
        collect_imported_bindings_in_program(&script.content, &mut bindings);
    }

    bindings
}

fn collect_bindable_bindings(root: &Root, runes_mode: bool) -> NameSet {
    let mut bindings = NameSet::default();

    if let Some(script) = root.instance.as_ref() {
        collect_bindable_bindings_in_program(&script.content, runes_mode, &mut bindings);
    }

    bindings
}

fn collect_script_constant_bindings(root: &Root) -> Box<[Arc<str>]> {
    let mut bindings = OrderedNames::default();
    if let Some(script) = root.module.as_ref() {
        collect_script_constant_bindings_in_program(&script.content, &mut bindings);
    }
    if let Some(script) = root.instance.as_ref() {
        collect_script_constant_bindings_in_program(&script.content, &mut bindings);
    }
    bindings.into_boxed_slice()
}

fn collect_script_constant_bindings_in_program(program: &JsProgram, out: &mut OrderedNames) {
    for statement in &program.program().body {
        match statement {
            Statement::ImportDeclaration(declaration) => {
                for specifier in declaration.specifiers.iter().flatten() {
                    if let Some(name) = import_specifier_local_name(specifier) {
                        out.extend([Arc::from(name)]);
                    }
                }
            }
            Statement::VariableDeclaration(declaration)
                if declaration.kind == VariableDeclarationKind::Const =>
            {
                collect_bindings_from_oxc_variable_declaration(declaration, out);
            }
            Statement::FunctionDeclaration(declaration) => {
                if let Some(id) = declaration.id.as_ref() {
                    out.extend([Arc::from(id.name.as_str())]);
                }
            }
            Statement::ClassDeclaration(declaration) => {
                if let Some(id) = declaration.id.as_ref() {
                    out.extend([Arc::from(id.name.as_str())]);
                }
            }
            Statement::ExportNamedDeclaration(declaration) => {
                if let Some(declaration) = declaration.declaration.as_ref() {
                    collect_constant_bindings_from_declaration(declaration, out);
                }
            }
            _ => {}
        }
    }
}

fn collect_bindable_bindings_in_program(
    program: &JsProgram,
    runes_mode: bool,
    out: &mut NameSet,
) {
    for statement in &program.program().body {
        match statement {
            Statement::VariableDeclaration(declaration) => {
                collect_bindable_bindings_from_oxc_variable_declaration(
                    declaration,
                    runes_mode,
                    out,
                );
            }
            Statement::ExportNamedDeclaration(declaration) => {
                let Some(Declaration::VariableDeclaration(declaration)) =
                    declaration.declaration.as_ref()
                else {
                    continue;
                };
                collect_bindable_bindings_from_oxc_variable_declaration(
                    declaration,
                    runes_mode,
                    out,
                );
            }
            _ => {}
        }
    }
}

fn find_assignment_violation_in_template_expression(
    expression: &Expression,
    context: &ValidationContext,
) -> Option<AssignmentViolation> {
    let mut immutable = NameSet::default();
    for name in context.immutable.as_slice() {
        immutable.insert(name.clone());
    }
    if context.runes {
        for name in context.each.as_slice() {
            immutable.insert(name.clone());
        }
    }
    let span =
        super::runes::find_constant_assignment_in_expression(expression, &immutable)?;
    let offset = expression.start;
    let kind = assignment_kind_for_expression_span(expression, (span.start + offset, span.end + offset), context);
    Some(AssignmentViolation {
        kind,
        start: span.start + offset,
        end: span.end + offset,
    })
}

fn assignment_kind_for_expression_span(
    root: &Expression,
    span: (usize, usize),
    context: &ValidationContext,
) -> AssignmentKind {
    let _ = span;
    let mut kind = AssignmentKind::Constant;
    let Some(name) = binding_base_identifier_name(root) else {
        return kind;
    };
    if context.runes && context.each.contains(name.as_ref()) {
        return AssignmentKind::EachItemInvalid;
    }
    if context.snippets.contains(name.as_ref()) {
        kind = AssignmentKind::SnippetParameter;
    }

    kind
}

fn unwrap_expression_typescript(expression: &Expression) -> Option<&OxcExpression<'_>> {
    let mut current = expression.oxc_expression()?;
    loop {
        current = match current {
            OxcExpression::TSAsExpression(inner) => &inner.expression,
            OxcExpression::TSSatisfiesExpression(inner) => &inner.expression,
            OxcExpression::TSTypeAssertion(inner) => &inner.expression,
            OxcExpression::TSNonNullExpression(inner) => &inner.expression,
            OxcExpression::ParenthesizedExpression(inner) => &inner.expression,
            _ => return Some(current),
        };
    }
}

fn expression_kind(expression: &Expression) -> Option<&'static str> {
    Some(match unwrap_expression_typescript(expression)? {
        OxcExpression::Identifier(_) => "Identifier",
        OxcExpression::ThisExpression(_) => "ThisExpression",
        OxcExpression::ObjectExpression(_) => "ObjectExpression",
        OxcExpression::SequenceExpression(_) => "SequenceExpression",
        OxcExpression::CallExpression(_) => "CallExpression",
        OxcExpression::StaticMemberExpression(_)
        | OxcExpression::ComputedMemberExpression(_)
        | OxcExpression::PrivateFieldExpression(_) => "MemberExpression",
        _ => return None,
    })
}

fn expression_span(expression: &Expression) -> Option<(usize, usize)> {
    Some((expression.start, expression.end))
}

fn expression_is_identifier_or_member(expression: &OxcExpression<'_>) -> bool {
    matches!(
        expression,
        OxcExpression::Identifier(_)
            | OxcExpression::StaticMemberExpression(_)
            | OxcExpression::ComputedMemberExpression(_)
            | OxcExpression::PrivateFieldExpression(_)
    )
}

fn collect_imported_bindings_in_program(program: &JsProgram, bindings: &mut NameSet) {
    for statement in &program.program().body {
        let Statement::ImportDeclaration(declaration) = statement else {
            continue;
        };
        for specifier in declaration.specifiers.iter().flatten() {
            if let Some(name) = import_specifier_local_name(specifier) {
                bindings.insert(Arc::from(name));
            }
        }
    }
}

fn binding_base_identifier_name(expression: &Expression) -> Option<Arc<str>> {
    let mut current = unwrap_expression_typescript(expression)?;
    loop {
        match current {
            OxcExpression::Identifier(identifier) => {
                return Some(Arc::from(identifier.name.as_str()));
            }
            OxcExpression::StaticMemberExpression(member) => {
                current = &member.object;
            }
            OxcExpression::ComputedMemberExpression(member) => {
                current = &member.object;
            }
            OxcExpression::PrivateFieldExpression(member) => {
                current = &member.object;
            }
            _ => return None,
        }
    }
}

fn const_tag_declared_identifiers(tag: &ConstTag) -> Box<[Arc<str>]> {
    let mut names = NameSet::default();
    insert_expression_binding_names(&tag.declaration, &mut names);
    let mut ordered = OrderedNames::default();
    for name in names {
        ordered.extend([name]);
    }
    ordered.into_boxed_slice()
}

fn const_tag_declared_bindings(tag: &ConstTag) -> Vec<(Arc<str>, usize, usize)> {
    let offset = tag.declaration.start;
    let mut bindings = Vec::new();
    if let Some(identifier) = tag.declaration.binding_identifier() {
        let span = identifier.span();
        bindings.push((
            Arc::from(identifier.name.as_str()),
            span.start as usize + offset,
            span.end as usize + offset,
        ));
    }
    bindings
}

fn expression_binding_names(expression: &Expression) -> Box<[Arc<str>]> {
    let mut names = NameSet::default();
    insert_expression_binding_names(expression, &mut names);
    let mut ordered = OrderedNames::default();
    for name in names {
        ordered.extend([name]);
    }
    ordered.into_boxed_slice()
}

fn insert_expression_binding_names(expression: &Expression, out: &mut NameSet) {
    if let Some(declaration) = expression.oxc_variable_declaration() {
        for declarator in &declaration.declarations {
            extend_name_set_with_oxc_pattern_bindings(out, &declarator.id);
        }
        return;
    }
    if let Some(pattern) = expression.oxc_pattern() {
        extend_name_set_with_oxc_pattern_bindings(out, pattern);
    }
}

fn collect_constant_bindings_from_declaration(
    declaration: &Declaration<'_>,
    out: &mut OrderedNames,
) {
    match declaration {
        Declaration::VariableDeclaration(declaration)
            if declaration.kind == VariableDeclarationKind::Const =>
        {
            collect_bindings_from_oxc_variable_declaration(declaration, out);
        }
        Declaration::FunctionDeclaration(declaration) => {
            if let Some(id) = declaration.id.as_ref() {
                out.extend([Arc::from(id.name.as_str())]);
            }
        }
        Declaration::ClassDeclaration(declaration) => {
            if let Some(id) = declaration.id.as_ref() {
                out.extend([Arc::from(id.name.as_str())]);
            }
        }
        _ => {}
    }
}

fn collect_bindable_bindings_from_oxc_variable_declaration(
    declaration: &oxc_ast::ast::VariableDeclaration<'_>,
    runes_mode: bool,
    out: &mut NameSet,
) {
    for declarator in &declaration.declarations {
        let is_bindable = matches!(
            declaration.kind,
            VariableDeclarationKind::Let | VariableDeclarationKind::Var
        ) || (runes_mode
            && declarator
                .init
                .as_ref()
                .and_then(oxc_callee_name)
                .is_some_and(|name| matches!(name.as_str(), "$state" | "$state.raw")));

        if is_bindable {
            extend_name_set_with_oxc_pattern_bindings(out, &declarator.id);
        }
    }
}

fn collect_bindings_from_oxc_variable_declaration(
    declaration: &oxc_ast::ast::VariableDeclaration<'_>,
    out: &mut OrderedNames,
) {
    let mut names = NameSet::default();
    for declarator in &declaration.declarations {
        extend_name_set_with_oxc_pattern_bindings(&mut names, &declarator.id);
    }
    for name in names {
        out.extend([name]);
    }
}

fn import_specifier_local_name<'a>(
    specifier: &'a ImportDeclarationSpecifier<'a>,
) -> Option<&'a str> {
    match specifier {
        ImportDeclarationSpecifier::ImportSpecifier(specifier) => {
            Some(specifier.local.name.as_str())
        }
        ImportDeclarationSpecifier::ImportDefaultSpecifier(specifier) => {
            Some(specifier.local.name.as_str())
        }
        ImportDeclarationSpecifier::ImportNamespaceSpecifier(specifier) => {
            Some(specifier.local.name.as_str())
        }
    }
}

fn oxc_callee_name(callee: &OxcExpression<'_>) -> Option<String> {
    match callee.get_inner_expression() {
        OxcExpression::Identifier(reference) => Some(reference.name.to_string()),
        OxcExpression::StaticMemberExpression(member) => {
            let object = member.object.get_inner_expression();
            let OxcExpression::Identifier(object) = object else {
                return None;
            };
            Some(format!("{}.{}", object.name, member.property.name))
        }
        _ => None,
    }
}

fn span_range(span: OxcSpan) -> (usize, usize) {
    (span.start as usize, span.end as usize)
}

fn let_directive_binding_names(directive: &DirectiveAttribute) -> Box<[Arc<str>]> {
    if let Some(name) = directive.expression.identifier_name() {
        if !name.is_empty() {
            return Box::new([Arc::from(name)]);
        }
        return Box::new([directive.name.clone()]);
    }
    let names = expression_binding_names(&directive.expression);
    if names.is_empty() {
        // Shorthand `let:bar` — the name itself is the binding
        return Box::new([directive.name.clone()]);
    }
    names
}

fn count_animation_relevant_nodes(fragment: &Fragment) -> usize {
    fragment
        .nodes
        .iter()
        .filter(|node| match node {
            Node::Text(text) => !text.data.trim().is_empty(),
            Node::Comment(_) => false,
            Node::ConstTag(_) => false,
            Node::DebugTag(_) => false,
            _ => true,
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::super::validate_component_template;
    use super::detect_svelte_meta_structure_errors_from_root;
    use crate::api::CompileOptions;
    use crate::compiler::phases::parse::parse_component_for_compile;

    fn validate(source: &str) -> Option<crate::error::CompileError> {
        let parsed = parse_component_for_compile(source).expect("parse component");
        let options = CompileOptions {
            runes: Some(true),
            ..CompileOptions::default()
        };
        validate_component_template(source, &options, parsed.root())
    }

    fn validate_legacy(source: &str) -> Option<crate::error::CompileError> {
        let parsed = parse_component_for_compile(source).expect("parse component");
        let options = CompileOptions {
            runes: Some(false),
            ..CompileOptions::default()
        };
        validate_component_template(source, &options, parsed.root())
    }

    fn error_range(error: &crate::error::CompileError) -> Option<(usize, usize)> {
        error
            .position
            .as_deref()
            .map(|position| (position.start, position.end))
    }

    #[test]
    fn runes_reject_unquoted_attribute_sequences() {
        let error = validate("<div foo=bar{baz}></div>").expect("expected validation error");
        assert_eq!(error.code.as_ref(), "attribute_unquoted_sequence");
    }

    #[test]
    fn runes_reject_unparenthesized_sequence_attribute_expressions() {
        let error = validate("<div foo={a, b}></div>").expect("expected validation error");
        assert_eq!(error.code.as_ref(), "attribute_invalid_sequence_expression");
    }

    #[test]
    fn runes_allow_parenthesized_sequence_attribute_expressions() {
        let error = validate("<div foo={(a, b)}></div>");
        assert!(error.is_none(), "unexpected validation error: {error:?}");
    }

    #[test]
    fn runes_allow_bind_getter_setter_pairs() {
        let error = validate(
            "<script>let value = $state(0);</script><input bind:value={() => value, v => value = v} />",
        );
        assert!(error.is_none(), "unexpected validation error: {error:?}");
    }

    #[test]
    fn runes_reject_parenthesized_bind_getter_setter_pairs() {
        let error = validate(
            "<script>let value = $state(0);</script><input bind:value={(() => value, v => value = v)} />",
        )
        .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "bind_invalid_parens");
    }

    #[test]
    fn runes_reject_bind_sequences_with_wrong_arity() {
        let error = validate(
            "<script>let value = $state(0);</script><input bind:value={() => value, v => value = v, extra} />",
        )
        .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "bind_invalid_expression");
    }

    #[test]
    fn rejects_unparenthesized_const_sequence_expressions() {
        let error =
            validate("{#if ok}{@const value = a, b}{/if}").expect("expected validation error");
        assert_eq!(error.code.as_ref(), "const_tag_invalid_expression");
    }

    #[test]
    fn allows_parenthesized_const_sequence_expressions() {
        let error = validate("{#if ok}{@const value = (a, b)}{/if}");
        assert!(error.is_none(), "unexpected validation error: {error:?}");
    }

    #[test]
    fn rejects_const_cycles_from_ast() {
        let error = validate("{#if true}{@const a = b}{@const b = a}{/if}")
            .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "const_tag_cycle");
        assert!(error.message.contains("a") && error.message.contains("b"));
    }

    #[test]
    fn allows_const_tag_reference_from_boundary_failed_snippet() {
        let error = validate(
            "<svelte:boundary>{@const foo = 'bar'}{#snippet failed()}{foo}{/snippet}</svelte:boundary>",
        );
        assert!(error.is_none(), "unexpected validation error: {error:?}");
    }

    #[test]
    fn allows_const_tag_reference_when_boundary_uses_it_outside_snippet() {
        let error = validate(
            "<svelte:boundary>{@const foo = 'bar'}{foo}{#snippet other()}{foo}{/snippet}</svelte:boundary>",
        );
        assert!(error.is_none(), "unexpected validation error: {error:?}");
    }

    #[test]
    fn rejects_const_tag_reference_from_same_component_snippet() {
        let error =
            validate("<Widget>{@const foo = 'bar'}{#snippet failed()}{foo}{/snippet}</Widget>")
                .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "const_tag_invalid_reference");
        assert_eq!(
            error.message.as_ref(),
            "The `{@const foo = ...}` declaration is not available in this snippet"
        );
    }

    #[test]
    fn reports_const_tag_sequence_from_initializer() {
        let error = validate("{#if true}{@const foo = 'foo', bar = 'bar'}{/if}")
            .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "const_tag_invalid_expression");
        assert_eq!(error_range(&error), Some((24, 42)));
    }

    #[test]
    fn rejects_non_call_render_tag_expressions() {
        let error = validate("{@render foo}").expect("expected validation error");
        assert_eq!(error.code.as_ref(), "render_tag_invalid_expression");
    }

    #[test]
    fn rejects_render_tag_spread_arguments() {
        let error = validate("{@render foo(...args)}").expect("expected validation error");
        assert_eq!(error.code.as_ref(), "render_tag_invalid_spread_argument");
    }

    #[test]
    fn rejects_render_tag_bind_calls() {
        let error = validate("{@render foo.bind(bar)}").expect("expected validation error");
        assert_eq!(error.code.as_ref(), "render_tag_invalid_call_expression");
    }

    #[test]
    fn allows_optional_call_render_tags() {
        let error = validate("{@render foo?.()}");
        assert!(error.is_none(), "unexpected validation error: {error:?}");
    }

    #[test]
    fn rejects_non_identifier_debug_tag_arguments() {
        let error = validate("{@debug foo.bar}").expect("expected validation error");
        assert_eq!(error.code.as_ref(), "debug_tag_invalid_arguments");
    }

    #[test]
    fn allows_identifier_debug_tag_arguments() {
        let error = validate("{@debug foo, bar}");
        assert!(error.is_none(), "unexpected validation error: {error:?}");
    }

    #[test]
    fn mixed_event_handler_error_uses_actual_directive_name() {
        let error = validate("<div onkeyup={handler} on:keydown={handler}></div>")
            .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "mixed_event_handler_syntaxes");
        assert!(
            error.message.contains("on:keydown") && error.message.contains("onkeydown"),
            "unexpected message: {}",
            error.message
        );
    }

    #[test]
    fn snippet_shadowing_prop_ignores_nested_component_scope() {
        let error =
            validate("<Widget foo={bar}><Inner>{#snippet foo()}{/snippet}</Inner></Widget>");
        assert!(error.is_none(), "unexpected validation error: {error:?}");
    }

    #[test]
    fn snippet_shadowing_prop_detects_current_component_scope() {
        let error = validate("<Widget foo={bar}><div>{#snippet foo()}{/snippet}</div></Widget>")
            .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "snippet_shadowing_prop");
    }

    #[test]
    fn allows_title_expression_content() {
        let error = validate("<svelte:head><title>{pageTitle}</title></svelte:head>");
        assert!(error.is_none(), "unexpected validation error: {error:?}");
    }

    #[test]
    fn rejects_title_nested_elements_from_ast() {
        let error = validate("<svelte:head><title><span>bad</span></title></svelte:head>")
            .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "title_invalid_content");
    }

    #[test]
    fn rejects_empty_attribute_shorthand_from_ast() {
        let error = validate("<div {}></div>").expect("expected validation error");
        assert_eq!(error.code.as_ref(), "attribute_empty_shorthand");
    }

    #[test]
    fn rejects_expected_attribute_value_from_ast() {
        let error = validate("<div class= ></div>").expect("expected validation error");
        assert_eq!(error.code.as_ref(), "expected_attribute_value");
    }

    #[test]
    fn rejects_duplicate_named_attributes_from_ast() {
        let error =
            validate("<div class='foo' class='bar'></div>").expect("expected validation error");
        assert_eq!(error.code.as_ref(), "attribute_duplicate");
    }

    #[test]
    fn rejects_duplicate_bind_attributes_from_ast() {
        let error = validate("<Widget foo={42} bind:foo/>").expect("expected validation error");
        assert_eq!(error.code.as_ref(), "attribute_duplicate");
    }

    #[test]
    fn rejects_duplicate_class_directives_from_ast() {
        let error = validate("<div class:cool={true} class:cool={true}></div>")
            .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "attribute_duplicate");
    }

    #[test]
    fn rejects_duplicate_slot_names_from_ast() {
        let error = validate("<Widget><div slot='header'></div><div slot='header'></div></Widget>")
            .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "slot_attribute_duplicate");
        assert_eq!(
            error.message.as_ref(),
            "Duplicate slot name 'header' in <Widget>"
        );
    }

    #[test]
    fn rejects_explicit_default_slot_with_implicit_content_from_ast() {
        let error = validate("<Widget><div slot='default'></div><p>implicit</p></Widget>")
            .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "slot_default_duplicate");
    }

    #[test]
    fn rejects_explicit_default_slot_with_component_child_from_ast() {
        let error = validate("<Widget><Inner slot='default' /></Widget>")
            .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "slot_default_duplicate");
    }

    #[test]
    fn rejects_duplicate_style_directives_from_ast() {
        let error = validate("<div style:color=\"red\" style:color=\"green\"></div>")
            .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "attribute_duplicate");
    }

    #[test]
    fn rejects_duplicate_shorthand_attributes_from_ast() {
        let error = validate("<div title='foo' {title}></div>").expect("expected validation error");
        assert_eq!(error.code.as_ref(), "attribute_duplicate");
    }

    #[test]
    fn runes_reject_unbalanced_curly_element_attributes() {
        let error =
            validate("<button onclick={true}}></button>").expect("expected validation error");
        assert_eq!(error.code.as_ref(), "attribute_unquoted_sequence");
    }

    #[test]
    fn runes_reject_unbalanced_curly_component_attributes() {
        let error = validate("<Widget onclick={true}} />").expect("expected validation error");
        assert_eq!(error.code.as_ref(), "attribute_unquoted_sequence");
    }

    #[test]
    fn rejects_attribute_expected_equals_from_ast() {
        let error = validate("<h1 class\"=foo\">Hello</h1>").expect("expected validation error");
        assert_eq!(error.code.as_ref(), "expected_token");
    }

    #[test]
    fn rejects_html_tag_in_attribute_from_ast() {
        let error =
            validate("<div style=\"{@html text}\"></div>").expect("expected validation error");
        assert_eq!(error.code.as_ref(), "tag_invalid_placement");
        assert_eq!(
            error.message.as_ref(),
            "{@html ...} tag cannot be in attribute value"
        );
    }

    #[test]
    fn rejects_logic_block_in_attribute_from_ast() {
        let error =
            validate("<div foo=\"{#if ok}x{/if}\"></div>").expect("expected validation error");
        assert_eq!(error.code.as_ref(), "block_invalid_placement");
    }

    #[test]
    fn rejects_textarea_value_and_children_from_ast() {
        let error = validate("<textarea value='{foo}'>some illegal text</textarea>")
            .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "textarea_invalid_content");
    }

    #[test]
    fn rejects_logic_block_in_textarea_from_ast() {
        let error = validate("<textarea>{#each items as item}{item}{/each}</textarea>")
            .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "block_invalid_placement");
        assert_eq!(
            error.message.as_ref(),
            "{#each ...} block cannot be inside <textarea>"
        );
    }

    #[test]
    fn rejects_invalid_attribute_names_from_ast() {
        for source in [
            "<p 3aa=\"abc\">Test</p>",
            "<p a*a>Test</p>",
            "<p -a>Test</p>",
            "<p a;=\"abc\">Test</p>",
        ] {
            let error = validate(source).expect("expected validation error");
            assert_eq!(error.code.as_ref(), "attribute_invalid_name");
        }
    }

    #[test]
    fn rejects_unknown_svelte_meta_tags_from_ast() {
        let error = validate("<svelte:unknown />").expect("expected validation error");
        assert_eq!(error.code.as_ref(), "svelte_meta_invalid_tag");
        assert_eq!(error_range(&error), Some((1, 15)));
    }

    #[test]
    fn rejects_duplicate_svelte_window_from_ast() {
        let error =
            validate("<svelte:window /><svelte:window />").expect("expected validation error");
        assert_eq!(error.code.as_ref(), "svelte_meta_duplicate");
    }

    #[test]
    fn rejects_nested_svelte_window_from_ast() {
        let error = validate("<div><svelte:window /></div>").expect("expected validation error");
        assert_eq!(error.code.as_ref(), "svelte_meta_invalid_placement");
    }

    #[test]
    fn rejects_svelte_window_content_from_ast() {
        let error =
            validate("<svelte:window>content</svelte:window>").expect("expected validation error");
        assert_eq!(error.code.as_ref(), "svelte_meta_invalid_content");
        assert!(
            error
                .message
                .contains("<svelte:window> cannot have children")
        );
    }

    #[test]
    fn rejects_svelte_options_content_from_ast() {
        let error = validate("<svelte:options>content</svelte:options>")
            .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "svelte_meta_invalid_content");
        assert!(
            error
                .message
                .contains("<svelte:options> cannot have children")
        );
    }

    #[test]
    fn rejects_reactive_declaration_cycle_from_ast() {
        let error = validate_legacy("<script>let a; let b; $: a = b; $: b = a;</script>")
            .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "reactive_declaration_cycle");
        assert!(error.message.contains("a") && error.message.contains("b"));
    }

    #[test]
    fn rejects_duplicate_default_scripts_from_ast() {
        let error = validate("<script>let a = 1;</script><script>let b = 2;</script>")
            .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "script_duplicate");
    }

    #[test]
    fn rejects_duplicate_module_scripts_from_ast() {
        let error = validate(
            "<script module>export const a = 1;</script><script context=\"module\">export const b = 2;</script>",
        )
        .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "script_duplicate");
    }

    #[test]
    fn rejects_invalid_script_context_from_ast() {
        let error = validate("<script context=\"client\">let a = 1;</script>")
            .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "script_invalid_context");
    }

    #[test]
    fn rejects_typescript_enum_from_ast() {
        let error = validate("<script lang=\"ts\">enum Color { Red, Blue }</script>")
            .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "typescript_invalid_feature");
        assert!(error.message.contains("enums"));
    }

    #[test]
    fn rejects_typescript_accessor_fields_from_ast() {
        let error = validate("<script lang=\"ts\">class Foo { accessor y = 1; }</script>")
            .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "typescript_invalid_feature");
        assert!(error.message.contains("accessor fields"));
    }

    #[test]
    fn rejects_typescript_namespace_values_from_ast() {
        let error =
            validate("<script lang=\"ts\">namespace Foo { export const value = 1; }</script>")
                .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "typescript_invalid_feature");
        assert!(error.message.contains("namespaces with non-type nodes"));
    }

    #[test]
    fn rejects_typescript_constructor_parameter_modifiers_from_ast() {
        let error = validate(
            "<script lang=\"ts\">class Foo { constructor(private value: number) {} }</script>",
        )
        .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "typescript_invalid_feature");
        assert!(
            error
                .message
                .contains("accessibility modifiers on constructor parameters")
        );
    }

    #[test]
    fn allows_type_only_typescript_namespace_from_ast() {
        let error = validate(
            "<script lang=\"ts\">namespace Foo { export interface Bar { value: string } }</script>",
        );
        assert!(error.is_none(), "unexpected validation error: {error:?}");
    }

    #[test]
    fn rejects_block_reactive_declaration_cycle_from_ast() {
        let error = validate_legacy("<script>let a; let b; $: { a = b; } $: b = a;</script>")
            .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "reactive_declaration_cycle");
        assert!(error.message.contains("a") && error.message.contains("b"));
    }

    #[test]
    fn direct_meta_scan_rejects_duplicate_svelte_window() {
        let source = "<svelte:window /><svelte:window />";
        let parsed = parse_component_for_compile(source).expect("parse component");
        assert_eq!(parsed.root().fragment.nodes.len(), 2);
        assert!(matches!(
            parsed.root().fragment.nodes[0],
            crate::ast::modern::Node::SvelteWindow(_)
        ));
        assert!(matches!(
            parsed.root().fragment.nodes[1],
            crate::ast::modern::Node::SvelteWindow(_)
        ));

        let mut state = super::SvelteMetaScanState::default();
        let first = super::scan_modern_node_for_svelte_meta(
            source,
            &parsed.root().fragment.nodes[0],
            0,
            0,
            &mut state,
        );
        assert!(first.is_none(), "unexpected first-node error: {first:?}");
        assert_eq!(state.window_count, 1);

        let second = super::scan_modern_node_for_svelte_meta(
            source,
            &parsed.root().fragment.nodes[1],
            0,
            0,
            &mut state,
        );
        assert_eq!(state.window_count, 2);
        assert!(second.is_some(), "expected second-node error");

        let error = detect_svelte_meta_structure_errors_from_root(source, parsed.root())
            .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "svelte_meta_duplicate");
    }

    #[test]
    fn rejects_else_before_closing_from_parse_errors() {
        let error =
            validate("{#if true}\n\t<li>\n{:else}\n{/if}").expect("expected validation error");
        assert_eq!(error.code.as_ref(), "block_invalid_continuation_placement");
    }

    #[test]
    fn rejects_else_before_closing_await_from_parse_errors() {
        let error = validate("{#if true}\n\t{#await p}\n{:else}\n{/if}")
            .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "expected_token");
        assert_eq!(
            error.message.as_ref(),
            "Expected token {:then ...} or {:catch ...}"
        );
    }

    #[test]
    fn rejects_top_level_then_from_parse_errors() {
        let error = validate("{:then foo}").expect("expected validation error");
        assert_eq!(error.code.as_ref(), "block_invalid_continuation_placement");
    }

    #[test]
    fn rejects_unclosed_comment_from_parse_errors() {
        let error = validate("<!-- an unclosed comment").expect("expected validation error");
        assert_eq!(error.code.as_ref(), "expected_token");
        assert_eq!(error.message.as_ref(), "Expected token -->");
    }

    #[test]
    fn rejects_unclosed_script_from_parse_errors() {
        let error =
            validate("<script>\n\n<h1>Hello {name}!</h1>").expect("expected validation error");
        assert_eq!(error.code.as_ref(), "element_unclosed");
        assert_eq!(error.message.as_ref(), "`<script>` was left open");
    }

    #[test]
    fn rejects_unclosed_if_block_from_parse_errors() {
        let error = validate("{#if foo}\n\t<p>foo</p>").expect("expected validation error");
        assert_eq!(error.code.as_ref(), "block_unclosed");
    }

    #[test]
    fn rejects_unclosed_div_from_parse_errors() {
        let error = validate("<div>").expect("expected validation error");
        assert_eq!(error.code.as_ref(), "element_unclosed");
        assert_eq!(error.message.as_ref(), "`<div>` was left open");
    }

    #[test]
    fn rejects_const_tag_reference_from_non_failed_boundary_snippet() {
        let error = validate(
            "<svelte:boundary>{@const foo = 'bar'}{#snippet other()}{foo}{/snippet}</svelte:boundary>",
        )
        .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "const_tag_invalid_reference");
    }

    #[test]
    fn allows_const_tag_shadowing_in_nested_each_blocks() {
        let error = validate(
            "{#each items as { a, b, children }}{@const ab = a + b}{#each children as { a, b }}{@const ab = a + b}{ab}{/each}{ab}{/each}",
        );
        assert!(error.is_none(), "unexpected validation error: {error:?}");
    }

    #[test]
    fn allows_css_custom_property_attributes_on_component_like_nodes() {
        assert!(validate("<Widget --prop=\"red\" />").is_none());
        assert!(validate("<svelte:component this={Widget} --prop=\"red\" />").is_none());
        assert!(validate("{#if ok}<svelte:self --prop=\"red\" />{/if}").is_none());
    }

    #[test]
    fn allows_arguments_inside_function_declarations() {
        let error = validate(
            "<script>function increment() { return arguments.length; }</script><button onclick={increment}></button>",
        );
        assert!(error.is_none(), "unexpected validation error: {error:?}");
    }

    #[test]
    fn allows_svelte_self_inside_component_slot_content() {
        let error = validate(
            "<script>import Countdown from './Countdown.svelte'; export let count = 5;</script><Countdown {count} let:count><svelte:self {count} /></Countdown>",
        );
        assert!(error.is_none(), "unexpected validation error: {error:?}");
    }

    #[test]
    fn allows_uri_namespace_values_in_svelte_options() {
        let error =
            validate("<svelte:options namespace=\"http://www.w3.org/2000/svg\"/><rect></rect>");
        assert!(error.is_none(), "unexpected validation error: {error:?}");
    }

    #[test]
    fn allows_weird_prop_names_on_components() {
        let error = validate(
            "<script>import Child from './Child.svelte';</script><Child 0={0} ysc%%gibberish={1} />",
        );
        assert!(error.is_none(), "unexpected validation error: {error:?}");
    }

    #[test]
    fn allows_bindings_to_imported_object_members() {
        let error = validate(
            "<script>import Child from './child.svelte'; import { global } from './state.svelte.js';</script><Child bind:a={global.value} />",
        );
        assert!(error.is_none(), "unexpected validation error: {error:?}");
    }

    #[test]
    fn legacy_allows_bindings_to_store_subscriptions() {
        let error = validate_legacy(
            "<script>import { writable } from 'svelte/store'; export const name = writable('world');</script><input bind:value={$name}><textarea bind:value={$name} />",
        );
        assert!(error.is_none(), "unexpected validation error: {error:?}");
    }

    #[test]
    fn runes_allows_bindings_to_state_variables() {
        let error =
            validate("<script>let playbackRate = $state(0.5);</script><audio bind:playbackRate />");
        assert!(error.is_none(), "unexpected validation error: {error:?}");
    }

    #[test]
    fn rejects_unexpected_eof_from_missing_self_closing_tag() {
        let error = validate("<d").expect("expected validation error");
        assert_eq!(error.code.as_ref(), "unexpected_eof");
    }

    #[test]
    fn rejects_missing_attribute_expression_right_brace_from_parse_errors() {
        let error = validate("<Component test={ />").expect("expected validation error");
        assert_eq!(error.code.as_ref(), "expected_token");
        assert_eq!(error.message.as_ref(), "Expected token }");
    }

    #[test]
    fn rejects_illegal_expression_from_parse_errors() {
        let error = validate("{42 = nope}").expect("expected validation error");
        assert_eq!(error.code.as_ref(), "js_parse_error");
        assert_eq!(error.message.as_ref(), "Assigning to rvalue");
    }

    #[test]
    fn rejects_unmatched_closing_tag_from_parse_errors() {
        let error = validate("</div>").expect("expected validation error");
        assert_eq!(error.code.as_ref(), "element_invalid_closing_tag");
        assert_eq!(
            error.message.as_ref(),
            "`</div>` attempted to close an element that was not open"
        );
    }

    #[test]
    fn rejects_unclosed_style_with_markup_from_parse_errors() {
        let error =
            validate("<style>\n\n<h1>Hello {name}!</h1>").expect("expected validation error");
        assert_eq!(error.code.as_ref(), "css_expected_identifier");
    }

    #[test]
    fn rejects_invalid_component_names_from_ast() {
        let error = validate("<Components[1] />").expect("expected validation error");
        assert_eq!(error.code.as_ref(), "tag_invalid_name");
        assert_eq!(error_range(&error), Some((1, 14)));
    }

    #[test]
    fn rejects_invalid_svelte_meta_tag_full_name_range_from_ast() {
        let error = validate("{#if x}\n\t<svelte:selfdestructive x=\"{x - 1}\"/>\n{/if}")
            .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "svelte_meta_invalid_tag");
        assert_eq!(error_range(&error), Some((10, 32)));
    }

    #[test]
    fn rejects_invalid_element_names_from_ast() {
        let error = validate("<yes[no]></yes[no]>").expect("expected validation error");
        assert_eq!(error.code.as_ref(), "tag_invalid_name");
    }

    #[test]
    fn allows_unicode_component_names_from_ast() {
        assert!(validate("<Wunderschön />").is_none());
        assert!(validate("<Namespace.Schön />").is_none());
    }

    #[test]
    fn allows_unicode_custom_element_names_from_ast() {
        assert!(validate("<math-α></math-α>").is_none());
    }

    #[test]
    fn rejects_invalid_bind_targets_on_svelte_element() {
        let error = validate(
            "<script>const tag = 'div'; let value;</script><svelte:element this={tag} bind:value />",
        )
        .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "bind_invalid_target");
    }

    #[test]
    fn allows_bind_this_on_svelte_element() {
        assert!(
            validate("<script>const tag = 'div'; let node;</script><svelte:element this={tag} bind:this={node} />")
                .is_none()
        );
    }

    #[test]
    fn allows_animate_on_svelte_element_inside_keyed_each() {
        assert!(
            validate(
                "<script>const tag = 'div'; const items = [{ id: 1 }];</script>{#each items as item (item.id)}<svelte:element this={tag} animate:flip />{/each}"
            )
            .is_none()
        );
    }

    #[test]
    fn runes_allows_dynamic_input_type_for_bind_value() {
        let error = validate(
            "<script>let type = $state('text'); let value = $state('');</script><input type={type} bind:value={value} />",
        );
        assert!(error.is_none(), "unexpected validation error: {error:?}");
    }

    #[test]
    fn allows_component_attributes_with_non_html_prop_names() {
        let error = validate(
            "<script>import Child from './Child.svelte';</script><Child 0={0} ysc%%gibberish={1} />",
        );
        assert!(error.is_none(), "unexpected validation error: {error:?}");
    }

    #[test]
    fn allows_bindings_to_member_expressions_rooted_at_imports() {
        let error = validate(
            "<script>import { global } from './state.svelte.js';</script><Child bind:a={global.value} />",
        );
        assert!(error.is_none(), "unexpected validation error: {error:?}");
    }

    #[test]
    fn rejects_void_element_closing_tag_from_ast() {
        let error = validate("<input>this is illegal!</input>").expect("expected validation error");
        assert_eq!(error.code.as_ref(), "void_element_invalid_content");
    }
}
