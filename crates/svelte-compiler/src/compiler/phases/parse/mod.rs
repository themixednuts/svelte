mod css;
mod cst;
mod oxc;
mod regions;

use std::sync::Arc;

use crate::api::ParseOptions;
use crate::ast::modern::{EstreeNode, Expression, Root};
use crate::ast::{CssAst, Document};
use crate::error::CompileError;

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

    #[allow(dead_code)]
    pub(crate) fn into_parts(self) -> (Arc<str>, Root) {
        (self.source, self.root)
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
    cst::SvelteParserCore::new(source, options).parse()
}

pub(crate) fn parse_component_for_compile(source: &str) -> Result<ParsedComponent, CompileError> {
    let normalized = source.replace('\r', "");
    let root = cst::parse_root_for_compile(normalized.as_str())?;

    Ok(ParsedComponent {
        source: Arc::from(normalized),
        root,
    })
}

pub(crate) fn parse_css(source: &str) -> Result<CssAst, CompileError> {
    css::parse_css_stylesheet(source)
}

pub(crate) fn parse_modern_css_nodes(
    source: &str,
    start: usize,
    end: usize,
) -> Vec<crate::ast::modern::CssNode> {
    css::parse_modern_css_nodes(source, start, end)
}

pub(crate) fn parse_js_import_ranges_for_compile(source: &str) -> Option<Vec<(usize, usize)>> {
    oxc::SvelteOxcParser::new(source).parse_import_ranges_for_compile()
}

pub(crate) fn can_parse_js_program(source: &str) -> bool {
    oxc::SvelteOxcParser::new(source).can_parse_program()
}

pub(crate) fn parse_js_program_for_compile(source: &str) -> Option<EstreeNode> {
    oxc::SvelteOxcParser::new(source).parse_program_for_compile()
}

pub(crate) fn non_module_script_content_ranges(root: &Root) -> Vec<(usize, usize)> {
    regions::non_module_script_content_ranges(root)
}

pub(crate) fn style_block_ranges(root: &Root) -> Vec<(usize, usize, usize, usize)> {
    regions::style_block_ranges(root)
}

pub(crate) fn parse_modern_program_content_with_offsets(
    snippet: &str,
    global_start: usize,
    start_line: usize,
    start_column: usize,
    end_line: usize,
    end_column: usize,
    is_ts: bool,
) -> Option<EstreeNode> {
    oxc::SvelteOxcParser::new(snippet)
        .with_offsets(oxc::OxcProgramOffsets {
            global_start,
            start_line,
            start_column,
            end_line,
            end_column,
        })
        .with_typescript(is_ts)
        .parse_program_for_compile()
}

pub(crate) fn parse_modern_expression_with_oxc(
    expression: &str,
    global_start: usize,
    base_line: usize,
    base_column: usize,
) -> Option<Expression> {
    oxc::SvelteOxcParser::new(expression)
        .with_offsets(oxc::OxcProgramOffsets {
            global_start,
            start_line: base_line,
            start_column: base_column,
            end_line: base_line,
            end_column: base_column + expression.len(),
        })
        .with_typescript(true)
        .parse_expression_for_template()
}

pub(crate) fn parse_modern_expression_error_with_oxc(
    expression: &str,
    global_start: usize,
    base_line: usize,
    base_column: usize,
) -> Option<Arc<str>> {
    oxc::SvelteOxcParser::new(expression)
        .with_offsets(oxc::OxcProgramOffsets {
            global_start,
            start_line: base_line,
            start_column: base_column,
            end_line: base_line,
            end_column: base_column + expression.len(),
        })
        .with_typescript(true)
        .parse_expression_error_for_template()
}

pub(crate) fn parse_modern_expression_error_detail_with_oxc(
    expression: &str,
    global_start: usize,
    base_line: usize,
    base_column: usize,
) -> Option<(usize, Arc<str>)> {
    oxc::SvelteOxcParser::new(expression)
        .with_offsets(oxc::OxcProgramOffsets {
            global_start,
            start_line: base_line,
            start_column: base_column,
            end_line: base_line,
            end_column: base_column + expression.len(),
        })
        .with_typescript(true)
        .parse_expression_error_detail_for_template()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::ast::common::{AttributeValueSyntax, ParseErrorKind};
    use crate::ast::modern::{
        Attribute, AttributeValue, AttributeValueList, DirectiveValueSyntax, Node, Root,
    };

    #[test]
    fn parsed_component_exposes_source_and_root_via_native_traits() {
        let parsed: super::ParsedComponent =
            super::parse_component_for_compile("<h1>Hello</h1>").expect("parse component");

        fn source<'a, T: AsRef<str>>(value: &'a T) -> &'a str {
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
        let AttributeValueList::ExpressionTag(tag) = &attribute.value else {
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
        let AttributeValueList::Values(values) = &attribute.value else {
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
        let AttributeValueList::Values(values) = &attribute.value else {
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
        let AttributeValueList::Values(values) = &attribute.value else {
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
        assert_eq!(
            crate::api::modern::estree_node_type(&tag.arguments[0].0),
            Some("MemberExpression")
        );
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
        assert_eq!(
            crate::api::modern::estree_node_type(&tag.arguments[0].0),
            Some("Identifier")
        );
        assert_eq!(
            crate::api::modern::estree_node_type(&tag.arguments[1].0),
            Some("MemberExpression")
        );
        assert_eq!(tag.identifiers.len(), 1);
        assert_eq!(tag.identifiers[0].name.as_ref(), "a");
    }

    #[test]
    fn parser_preserves_snippet_type_params_and_typed_parameters() {
        let snippet = first_snippet("{#snippet row<T>(item: Item, index: number)}{/snippet}");

        assert_eq!(snippet.type_params.as_deref(), Some("T"));
        assert_eq!(snippet.parameters.len(), 2);
        assert_eq!(
            crate::api::modern::estree_node_type(&snippet.parameters[0].0),
            Some("Identifier")
        );
        assert_eq!(
            crate::api::modern::estree_node_type(&snippet.parameters[1].0),
            Some("Identifier")
        );
    }

    #[test]
    fn parser_preserves_snippet_destructured_default_parameter_shape() {
        let snippet = first_snippet("{#snippet row({ name, value } = fallback)}{/snippet}");

        assert_eq!(snippet.parameters.len(), 1);
        assert_eq!(
            crate::api::modern::estree_node_type(&snippet.parameters[0].0),
            Some("AssignmentPattern")
        );
        let left = crate::api::modern::estree_node_field_object(
            &snippet.parameters[0].0,
            crate::api::modern::RawField::Left,
        )
        .expect("assignment pattern left");
        assert_eq!(
            crate::api::modern::estree_node_type(left),
            Some("ObjectPattern")
        );
    }

    #[test]
    fn parser_preserves_snippet_rest_parameter_shape() {
        let snippet = first_snippet("{#snippet row(...items)}{/snippet}");

        assert_eq!(snippet.parameters.len(), 1);
        assert_eq!(
            crate::api::modern::estree_node_type(&snippet.parameters[0].0),
            Some("RestElement")
        );
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

        assert_eq!(
            crate::api::modern::estree_node_type(&block.test.0),
            Some("Identifier")
        );
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
        let Some(crate::ast::modern::Node::ConstTag(tag)) = block.body.nodes.get(0) else {
            panic!("expected const tag");
        };

        assert_eq!(
            crate::api::modern::estree_node_type(&tag.declaration.0),
            Some("VariableDeclaration")
        );
    }
}
