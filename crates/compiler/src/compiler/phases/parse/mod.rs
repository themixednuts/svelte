mod oxc;
mod regions;

use std::rc::Rc;
use std::sync::Arc;

use camino::Utf8Path;
use oxc_ast::ast::Statement;
use oxc_ast_visit::{Visit, walk};
use svelte_syntax::JsProgram;

use crate::api::ParseOptions;
use crate::ast::modern::Root;
use crate::ast::{CssAst, Document};
use crate::error::CompileError;
use crate::{SourceId, SourceText};

#[derive(Debug, Clone)]
pub(crate) struct ParsedComponent {
    pub source: Arc<str>,
    pub root: Root,
}

impl ParsedComponent {
    pub(crate) fn source(&self) -> &str {
        self.source.as_ref()
    }

    pub(crate) fn root(&self) -> &Root {
        &self.root
    }

    #[cfg(test)]
    pub(crate) fn into_parts(self) -> (Arc<str>, Root) {
        (self.source, self.root)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ParsedModuleProgram<'src> {
    source: SourceText<'src>,
    language: ModuleProgramLanguage,
    program: Rc<JsProgram>,
}

impl<'src> ParsedModuleProgram<'src> {
    pub(crate) const fn source_text(&self) -> SourceText<'src> {
        self.source
    }

    pub(crate) const fn language(&self) -> ModuleProgramLanguage {
        self.language
    }

    pub(crate) fn program(&self) -> &JsProgram {
        &self.program
    }
}

impl AsRef<str> for ParsedComponent {
    fn as_ref(&self) -> &str {
        self.source()
    }
}

impl AsRef<Root> for ParsedComponent {
    fn as_ref(&self) -> &Root {
        self.root()
    }
}

pub(crate) fn parse_component(
    source: &str,
    options: ParseOptions,
) -> Result<Document, CompileError> {
    let syntax_options = syntax_parse_options(options);
    let parsed = svelte_syntax::parse(source, syntax_options)?;
    let source_arc = Arc::<str>::from(parsed.source());

    Ok(Document {
        root: parsed.root,
        source: source_arc,
    })
}

pub(crate) fn parse_component_for_compile(source: &str) -> Result<ParsedComponent, CompileError> {
    let source_text = SourceText::new(SourceId::new(0), source, None);
    parse_component_for_compile_source(source_text)
}

pub(crate) fn parse_component_for_compile_source(
    source_text: SourceText<'_>,
) -> Result<ParsedComponent, CompileError> {
    let root = svelte_syntax::parse_modern_root(source_text.text)
        .map_err(|error| error.with_source_text(source_text))?;

    Ok(ParsedComponent {
        source: Arc::from(source_text.text),
        root,
    })
}

pub(crate) fn parse_css(source: &str) -> Result<CssAst, CompileError> {
    svelte_syntax::parse_css(source)
}

pub(crate) fn parse_modern_css_nodes(
    source: &str,
    start: usize,
    end: usize,
) -> Vec<crate::ast::modern::CssNode> {
    svelte_syntax::parse_modern_css_nodes(source, start, end)
}

pub(crate) fn parse_js_import_ranges_for_compile(source: &str) -> Option<Vec<(usize, usize)>> {
    oxc::SvelteOxcParser::new(source).parse_import_ranges_for_compile()
}

pub(crate) fn parse_module_program_for_compile_source(
    source: SourceText<'_>,
) -> Result<ParsedModuleProgram<'_>, CompileError> {
    let (language, program) = detect_module_program(source)
        .ok_or_else(|| CompileError::internal("failed to parse module source with oxc parser"))?;

    Ok(ParsedModuleProgram {
        source,
        language,
        program,
    })
}

pub(crate) fn non_module_script_content_ranges(root: &Root) -> Vec<(usize, usize)> {
    regions::non_module_script_content_ranges(root)
}

pub(crate) fn style_block_ranges(root: &Root) -> Vec<(usize, usize, usize, usize)> {
    regions::style_block_ranges(root)
}

fn syntax_parse_options(options: ParseOptions) -> svelte_syntax::ParseOptions {
    svelte_syntax::ParseOptions {
        filename: options.filename,
        root_dir: options.root_dir,
        modern: options.modern,
        mode: match options.mode {
            crate::api::ParseMode::Legacy => svelte_syntax::ParseMode::Legacy,
            crate::api::ParseMode::Modern => svelte_syntax::ParseMode::Modern,
        },
        loose: options.loose,
    }
}

fn parse_program_for_compile_with_language(
    source: &str,
    is_ts: bool,
) -> Option<Rc<JsProgram>> {
    oxc::SvelteOxcParser::new(source)
        .with_typescript(is_ts)
        .parse_program_for_compile()
}

fn detect_module_program(
    source: SourceText<'_>,
) -> Option<(ModuleProgramLanguage, Rc<JsProgram>)> {
    if module_filename_is_typescript(source.filename) {
        let program = parse_program_for_compile_with_language(source.text, true)?;
        return Some((ModuleProgramLanguage::TypeScript, program));
    }
    if module_filename_is_javascript(source.filename) {
        let program = parse_program_for_compile_with_language(source.text, false)?;
        return Some((ModuleProgramLanguage::JavaScript, program));
    }
    let javascript = parse_program_for_compile_with_language(source.text, false);
    let typescript = parse_program_for_compile_with_language(source.text, true);

    match (javascript, typescript) {
        (Some(_), Some(program)) if program_contains_typescript_syntax(&program) => {
            Some((ModuleProgramLanguage::TypeScript, program))
        }
        (Some(program), Some(_)) | (Some(program), None) => {
            Some((ModuleProgramLanguage::JavaScript, program))
        }
        (None, Some(program)) => Some((ModuleProgramLanguage::TypeScript, program)),
        (None, None) => None,
    }
}

fn module_filename_is_typescript(filename: Option<&Utf8Path>) -> bool {
    matches!(
        filename.and_then(Utf8Path::extension),
        Some("ts" | "mts" | "cts")
    )
}

fn module_filename_is_javascript(filename: Option<&Utf8Path>) -> bool {
    matches!(
        filename.and_then(Utf8Path::extension),
        Some("js" | "mjs" | "cjs")
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModuleProgramLanguage {
    JavaScript,
    TypeScript,
}

impl ModuleProgramLanguage {
    pub(crate) const fn is_typescript(self) -> bool {
        matches!(self, Self::TypeScript)
    }
}

fn program_contains_typescript_syntax(program: &JsProgram) -> bool {
    struct TypescriptSyntaxVisitor {
        found: bool,
    }

    impl<'a> Visit<'a> for TypescriptSyntaxVisitor {
        fn visit_statement(&mut self, statement: &Statement<'a>) {
            if self.found {
                return;
            }

            if matches!(
                statement,
                Statement::TSImportEqualsDeclaration(_)
                    | Statement::TSExportAssignment(_)
                    | Statement::TSNamespaceExportDeclaration(_)
                    | Statement::TSEnumDeclaration(_)
                    | Statement::TSInterfaceDeclaration(_)
                    | Statement::TSModuleDeclaration(_)
                    | Statement::TSTypeAliasDeclaration(_)
            ) {
                self.found = true;
                return;
            }

            walk::walk_statement(self, statement);
        }

        fn visit_ts_type_annotation(&mut self, _annotation: &oxc_ast::ast::TSTypeAnnotation<'a>) {
            self.found = true;
        }

        fn visit_ts_type_parameter_declaration(
            &mut self,
            _declaration: &oxc_ast::ast::TSTypeParameterDeclaration<'a>,
        ) {
            self.found = true;
        }

        fn visit_ts_type_parameter_instantiation(
            &mut self,
            _instantiation: &oxc_ast::ast::TSTypeParameterInstantiation<'a>,
        ) {
            self.found = true;
        }

        fn visit_ts_satisfies_expression(
            &mut self,
            _expression: &oxc_ast::ast::TSSatisfiesExpression<'a>,
        ) {
            self.found = true;
        }

        fn visit_ts_as_expression(&mut self, _expression: &oxc_ast::ast::TSAsExpression<'a>) {
            self.found = true;
        }
    }

    let mut visitor = TypescriptSyntaxVisitor { found: false };
    visitor.visit_program(program.program());
    visitor.found
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::ast::common::{AttributeValueSyntax, ParseErrorKind};
    use crate::ast::modern::{
        Attribute, AttributeValue, AttributeValueKind, DirectiveValueSyntax, Node, Root,
    };

    #[test]
    fn parsed_component_exposes_source_and_root_via_native_traits() {
        let parsed: super::ParsedComponent =
            super::parse_component_for_compile("<h1>Hello</h1>").expect("parse component");

        fn source<T: AsRef<str>>(value: &T) -> &str {
            value.as_ref()
        }

        fn root<T: AsRef<Root>>(value: &T) -> &Root {
            value.as_ref()
        }

        assert_eq!(source(&parsed), "<h1>Hello</h1>");
        assert_eq!(root(&parsed).start, 0);

        let (source, root) = parsed.clone().into_parts();
        assert_eq!(source, Arc::<str>::from("<h1>Hello</h1>"));
        assert_eq!(root.start, 0);
    }

    #[test]
    fn node_child_fragment_helpers_visit_direct_child_fragments() {
        let parsed = super::parse_component_for_compile(
            "{#if foo}<p>one</p>{:else if bar}<p>two</p>{:else}<p>three</p>{/if}",
        )
        .expect("parse component");

        let Some(node) = parsed.root.fragment.nodes.first() else {
            panic!("expected a top-level node");
        };
        let Node::IfBlock(_) = node else {
            panic!("expected an if block");
        };

        let mut fragment_lengths = Vec::new();
        node.for_each_child_fragment(|fragment| fragment_lengths.push(fragment.nodes.len()));

        assert_eq!(fragment_lengths, vec![1, 1]);

        let stopped = node.try_for_each_child_fragment(|fragment| {
            std::ops::ControlFlow::Break(fragment.nodes.len())
        });

        assert_eq!(stopped, std::ops::ControlFlow::Break(1));
    }

    fn first_attribute(source: &str) -> Attribute {
        let parsed = super::parse_component_for_compile(source).expect("parse component");
        let Some(node) = parsed.root.fragment.nodes.into_vec().into_iter().next() else {
            panic!("expected a top-level node");
        };
        let Node::RegularElement(element) = node else {
            panic!("expected a regular element");
        };
        element
            .attributes
            .into_vec()
            .into_iter()
            .next()
            .expect("expected an attribute")
    }

    fn first_snippet(source: &str) -> crate::ast::modern::SnippetBlock {
        let parsed = super::parse_component_for_compile(source).expect("parse component");
        let Some(node) = parsed.root.fragment.nodes.into_vec().into_iter().next() else {
            panic!("expected a top-level node");
        };
        let Node::SnippetBlock(block) = node else {
            panic!("expected a snippet block");
        };
        block
    }

    #[test]
    fn parser_tracks_named_attribute_value_syntax() {
        let boolean = first_attribute("<div disabled></div>");
        let quoted = first_attribute("<div class=\"foo\"></div>");
        let unquoted = first_attribute("<div class=foo></div>");
        let expression = first_attribute("<div class={foo}></div>");

        let Attribute::Attribute(boolean) = boolean else {
            panic!("expected named attribute");
        };
        let Attribute::Attribute(quoted) = quoted else {
            panic!("expected named attribute");
        };
        let Attribute::Attribute(unquoted) = unquoted else {
            panic!("expected named attribute");
        };
        let Attribute::Attribute(expression) = expression else {
            panic!("expected named attribute");
        };

        assert_eq!(boolean.value_syntax, AttributeValueSyntax::Boolean);
        assert_eq!(quoted.value_syntax, AttributeValueSyntax::Quoted);
        assert_eq!(unquoted.value_syntax, AttributeValueSyntax::Unquoted);
        assert_eq!(expression.value_syntax, AttributeValueSyntax::Expression);
    }

    #[test]
    fn parser_tracks_style_directive_value_syntax() {
        let quoted = first_attribute("<div style:color=\"red\"></div>");
        let expression = first_attribute("<div style:color={color}></div>");

        let Attribute::StyleDirective(quoted) = quoted else {
            panic!("expected style directive");
        };
        let Attribute::StyleDirective(expression) = expression else {
            panic!("expected style directive");
        };

        assert_eq!(quoted.value_syntax, AttributeValueSyntax::Quoted);
        assert_eq!(expression.value_syntax, AttributeValueSyntax::Expression);
    }

    #[test]
    fn parser_preserves_colons_in_class_directive_names() {
        let attribute = first_attribute("<div class:foo:bar={enabled}></div>");

        let Attribute::ClassDirective(attribute) = attribute else {
            panic!("expected class directive");
        };

        assert_eq!(attribute.name.as_ref(), "foo:bar");
        assert_eq!(attribute.value_syntax, DirectiveValueSyntax::Expression);
    }

    #[test]
    fn parser_tracks_parenthesized_attribute_expression_shape() {
        let attribute = first_attribute("<div foo={(a, b)}></div>");

        let Attribute::Attribute(attribute) = attribute else {
            panic!("expected named attribute");
        };
        let AttributeValueKind::ExpressionTag(tag) = &attribute.value else {
            panic!("expected single expression tag");
        };

        assert_eq!(tag.expression.parens(), 1);
    }

    #[test]
    fn parser_lowers_textarea_raw_text_into_text_nodes() {
        let parsed = super::parse_component_for_compile(
            "<textarea value='{foo}'>some illegal text</textarea>",
        )
        .expect("parse component");

        let Some(crate::ast::modern::Node::RegularElement(element)) =
            parsed.root.fragment.nodes.first()
        else {
            panic!("expected textarea element");
        };

        assert_eq!(element.name.as_ref(), "textarea");
        assert!(matches!(
            element.fragment.nodes.first(),
            Some(crate::ast::modern::Node::Text(_))
        ));
    }

    #[test]
    fn parser_preserves_unquoted_attribute_sequences() {
        let attribute = first_attribute("<div class=foo{bar}></div>");

        let Attribute::Attribute(attribute) = attribute else {
            panic!("expected named attribute");
        };

        assert_eq!(attribute.value_syntax, AttributeValueSyntax::Unquoted);
        let AttributeValueKind::Values(values) = &attribute.value else {
            panic!("expected split value parts");
        };
        assert_eq!(values.len(), 2);
        assert!(matches!(&values[0], AttributeValue::Text(_)));
        assert!(matches!(&values[1], AttributeValue::ExpressionTag(_)));
    }

    #[test]
    fn parser_merges_trailing_brace_into_unquoted_attribute_sequence() {
        let attribute = first_attribute("<div onclick={true}}></div>");

        let Attribute::Attribute(attribute) = attribute else {
            panic!("expected named attribute");
        };

        assert_eq!(attribute.value_syntax, AttributeValueSyntax::Unquoted);
        let AttributeValueKind::Values(values) = &attribute.value else {
            panic!("expected split value parts");
        };
        assert_eq!(values.len(), 2);
        assert!(matches!(&values[0], AttributeValue::ExpressionTag(_)));
        assert!(matches!(&values[1], AttributeValue::Text(_)));
    }

    #[test]
    fn parser_preserves_unquoted_style_directive_sequences() {
        let attribute = first_attribute("<div style:color=foo{bar}></div>");

        let Attribute::StyleDirective(attribute) = attribute else {
            panic!("expected style directive");
        };

        assert_eq!(attribute.value_syntax, AttributeValueSyntax::Unquoted);
        let AttributeValueKind::Values(values) = &attribute.value else {
            panic!("expected split value parts");
        };
        assert_eq!(values.len(), 2);
        assert!(matches!(&values[0], AttributeValue::Text(_)));
        assert!(matches!(&values[1], AttributeValue::ExpressionTag(_)));
    }

    #[test]
    fn parser_preserves_debug_tag_member_expression_arguments() {
        let parsed =
            super::parse_component_for_compile("{@debug user.name}").expect("parse component");
        let Some(crate::ast::modern::Node::DebugTag(tag)) = parsed.root.fragment.nodes.first()
        else {
            panic!("expected debug tag");
        };

        assert_eq!(tag.arguments.len(), 1);
        assert!(matches!(
            tag.arguments[0].oxc_expression(),
            Some(oxc_ast::ast::Expression::StaticMemberExpression(_))
                | Some(oxc_ast::ast::Expression::ComputedMemberExpression(_))
                | Some(oxc_ast::ast::Expression::PrivateFieldExpression(_))
        ));
        assert!(tag.identifiers.is_empty());
    }

    #[test]
    fn parser_preserves_debug_tag_sequence_arguments() {
        let parsed =
            super::parse_component_for_compile("{@debug a, foo.bar}").expect("parse component");
        let Some(crate::ast::modern::Node::DebugTag(tag)) = parsed.root.fragment.nodes.first()
        else {
            panic!("expected debug tag");
        };

        assert_eq!(tag.arguments.len(), 2);
        assert!(matches!(
            tag.arguments[0].oxc_expression(),
            Some(oxc_ast::ast::Expression::Identifier(_))
        ));
        assert!(matches!(
            tag.arguments[1].oxc_expression(),
            Some(oxc_ast::ast::Expression::StaticMemberExpression(_))
                | Some(oxc_ast::ast::Expression::ComputedMemberExpression(_))
                | Some(oxc_ast::ast::Expression::PrivateFieldExpression(_))
        ));
        assert_eq!(tag.identifiers.len(), 1);
        assert_eq!(tag.identifiers[0].name.as_ref(), "a");
    }

    #[test]
    fn parser_preserves_snippet_type_params_and_typed_parameters() {
        let snippet = first_snippet("{#snippet row<T>(item: Item, index: number)}{/snippet}");

        assert_eq!(snippet.type_params.as_deref(), Some("T"));
        assert_eq!(snippet.parameters.len(), 2);
        assert!(matches!(
            snippet.parameters[0].oxc_pattern(),
            Some(oxc_ast::ast::BindingPattern::BindingIdentifier(_))
        ));
        assert!(matches!(
            snippet.parameters[1].oxc_pattern(),
            Some(oxc_ast::ast::BindingPattern::BindingIdentifier(_))
        ));
    }

    #[test]
    fn parser_preserves_snippet_destructured_default_parameter_shape() {
        let snippet = first_snippet("{#snippet row({ name, value } = fallback)}{/snippet}");

        assert_eq!(snippet.parameters.len(), 1);
        let parameter = snippet.parameters[0]
            .oxc_parameter()
            .expect("formal parameter");
        assert!(matches!(
            &parameter.pattern,
            oxc_ast::ast::BindingPattern::ObjectPattern(_)
        ));
        assert!(matches!(
            parameter.initializer.as_deref(),
            Some(oxc_ast::ast::Expression::Identifier(_))
        ));
    }

    #[test]
    fn parser_preserves_snippet_rest_parameter_shape() {
        let snippet = first_snippet("{#snippet row(...items)}{/snippet}");

        assert_eq!(snippet.parameters.len(), 1);
        assert!(matches!(
            snippet.parameters[0].oxc_pattern(),
            Some(oxc_ast::ast::BindingPattern::ArrayPattern(_))
                | Some(oxc_ast::ast::BindingPattern::ObjectPattern(_))
                | Some(oxc_ast::ast::BindingPattern::AssignmentPattern(_))
                | Some(oxc_ast::ast::BindingPattern::BindingIdentifier(_))
        ));
        let pattern = snippet.parameters[0]
            .oxc_pattern()
            .expect("rest parameter pattern");
        assert!(matches!(
            pattern,
            oxc_ast::ast::BindingPattern::ArrayPattern(array) if array.rest.is_some()
        ) || matches!(
            pattern,
            oxc_ast::ast::BindingPattern::ObjectPattern(object) if object.rest.is_some()
        ) || matches!(
            pattern,
            oxc_ast::ast::BindingPattern::AssignmentPattern(_)
                | oxc_ast::ast::BindingPattern::BindingIdentifier(_)
        ));
    }

    #[test]
    fn parser_classifies_svelte_window_nodes() {
        let parsed =
            super::parse_component_for_compile("<svelte:window />").expect("parse component");
        let Some(node) = parsed.root.fragment.nodes.first() else {
            panic!("expected top-level node");
        };
        assert!(matches!(node, crate::ast::modern::Node::SvelteWindow(_)));
    }

    #[test]
    fn parser_preserves_multiple_svelte_window_nodes() {
        let parsed = super::parse_component_for_compile("<svelte:window /><svelte:window />")
            .expect("parse component");
        assert_eq!(parsed.root.fragment.nodes.len(), 2);
        assert!(matches!(
            parsed.root.fragment.nodes[0],
            crate::ast::modern::Node::SvelteWindow(_)
        ));
        assert!(matches!(
            parsed.root.fragment.nodes[1],
            crate::ast::modern::Node::SvelteWindow(_)
        ));
    }

    #[test]
    fn parser_preserves_svelte_window_children() {
        let parsed = super::parse_component_for_compile("<svelte:window>content</svelte:window>")
            .expect("parse component");
        let Some(crate::ast::modern::Node::SvelteWindow(node)) = parsed.root.fragment.nodes.first()
        else {
            panic!("expected svelte:window");
        };
        assert_eq!(node.fragment.nodes.len(), 1);
        assert!(matches!(
            node.fragment.nodes[0],
            crate::ast::modern::Node::Text(_)
        ));
    }

    #[test]
    fn parser_recovers_if_block_missing_right_brace_from_cst() {
        let parsed = super::parse_component_for_compile("{#if visible <p>ok</p>{/if}")
            .expect("parse component");

        let Some(crate::ast::modern::Node::IfBlock(block)) = parsed.root.fragment.nodes.first()
        else {
            panic!("expected if block");
        };

        assert!(matches!(
            block.test.oxc_expression(),
            Some(oxc_ast::ast::Expression::Identifier(_))
        ));
        assert_eq!(block.consequent.nodes.len(), 1);
        assert!(matches!(
            block.consequent.nodes[0],
            crate::ast::modern::Node::RegularElement(_)
        ));
    }

    #[test]
    fn parser_records_block_invalid_continuation_placement() {
        let parsed = super::parse_component_for_compile("{#if true}\n\t<li>\n{:else}\n{/if}")
            .expect("parse component");

        assert_eq!(parsed.root.errors.len(), 1);
        assert_eq!(
            parsed.root.errors[0].kind,
            ParseErrorKind::BlockInvalidContinuationPlacement
        );
        assert_eq!(parsed.root.errors[0].start, 18);
    }

    #[test]
    fn parser_allows_continuation_after_self_closing_element() {
        let parsed = super::parse_component_for_compile("{#if true}\n\t<input />\n{:else}\n{/if}")
            .expect("parse component");

        assert!(parsed.root.errors.is_empty());
    }

    #[test]
    fn parser_allows_capitalized_component_names_that_overlap_html_tags() {
        let parsed = super::parse_component_for_compile(
            "<script>import Link from './Link.svelte';</script><Link>Hello</Link>",
        )
        .expect("parse component");

        assert!(parsed.root.errors.is_empty(), "{:?}", parsed.root.errors);
    }

    #[test]
    fn parser_allows_capitalized_component_names_in_slots_and_void_like_positions() {
        let parsed = super::parse_component_for_compile(
            "<script>import Input from './Input.svelte'; import Display from './Display.svelte';</script><Input let:val={foo}>{#if foo}<Display>{foo}</Display>{/if}</Input>",
        )
        .expect("parse component");

        assert!(parsed.root.errors.is_empty(), "{:?}", parsed.root.errors);
    }

    #[test]
    fn parser_allows_capitalized_self_closing_components_inside_html_elements() {
        let parsed = super::parse_component_for_compile(
            "<script>import H1 from './h1.svelte';</script><p><H1 /></p>",
        )
        .expect("parse component");

        assert!(parsed.root.errors.is_empty(), "{:?}", parsed.root.errors);
    }

    #[test]
    fn parser_allows_multiline_attach_tags_inside_start_tags() {
        let parsed = super::parse_component_for_compile(
            "{#if await true}\n\t<div\n\t\t{@attach (node) => {\n\t\t\tnode.textContent = 'attachment ran';\n\t\t}}\n\t>\n\t\tattachment did not run\n\t</div>\n{/if}",
        )
        .expect("parse component");

        assert!(parsed.root.errors.is_empty(), "{:?}", parsed.root.errors);
    }

    #[test]
    fn parser_records_expected_await_branch_error() {
        let parsed = super::parse_component_for_compile("{#if true}\n\t{#await p}\n{:else}\n{/if}")
            .expect("parse component");

        assert_eq!(parsed.root.errors.len(), 1);
        assert_eq!(
            parsed.root.errors[0].kind,
            ParseErrorKind::ExpectedTokenAwaitBranch
        );
        assert_eq!(parsed.root.errors[0].start, 24);
    }

    #[test]
    fn parser_records_top_level_continuation_error() {
        let parsed = super::parse_component_for_compile("{:then foo}").expect("parse component");

        assert_eq!(parsed.root.errors.len(), 1);
        assert_eq!(
            parsed.root.errors[0].kind,
            ParseErrorKind::BlockInvalidContinuationPlacement
        );
        assert_eq!(parsed.root.errors[0].start, 1);
    }

    #[test]
    fn parser_records_expected_whitespace_for_html_tag() {
        let parsed = super::parse_component_for_compile("{@htmlfoo}").expect("parse component");

        assert_eq!(parsed.root.errors.len(), 1);
        assert_eq!(
            parsed.root.errors[0].kind,
            ParseErrorKind::ExpectedWhitespace
        );
        assert_eq!(parsed.root.errors[0].start, 6);
    }

    #[test]
    fn parser_records_expected_whitespace_for_const_tag() {
        let parsed = super::parse_component_for_compile("{#if true}{@constfoo = bar}{/if}")
            .expect("parse component");

        assert_eq!(parsed.root.errors.len(), 1);
        assert_eq!(
            parsed.root.errors[0].kind,
            ParseErrorKind::ExpectedWhitespace
        );
        assert_eq!(parsed.root.errors[0].start, 17);
    }

    #[test]
    fn parser_preserves_typescript_const_tag_declarations() {
        let parsed = super::parse_component_for_compile(
            "<script lang=\"ts\">const boxes = [{ width: 10, height: 10 }];</script>{#each boxes as box}{@const area: number = box.width * box.height}{area}{/each}",
        )
        .expect("parse component");

        let Some(block) = parsed
            .root
            .fragment
            .nodes
            .iter()
            .find_map(|node| match node {
                crate::ast::modern::Node::EachBlock(block) => Some(block),
                _ => None,
            })
        else {
            panic!("expected each block");
        };
        let Some(crate::ast::modern::Node::ConstTag(tag)) = block.body.nodes.first() else {
            panic!("expected const tag");
        };

        assert!(tag.declaration.oxc_variable_declaration().is_some());
    }
}
