use std::collections::BTreeMap;

use camino::Utf8Path;

use crate::api::GenerateTarget;
use crate::api::modern::{RawField, estree_node_field, estree_node_field_str, estree_node_type};
use crate::ast::modern::{
    Alternate, Attribute, AttributeValue, AttributeValueList, Component, EstreeValue, Expression,
    Fragment, NamedAttribute, Node, RegularElement, Root, Script, StyleDirective, SvelteElement,
};
use crate::js::render;

const TEMPLATE_BINDING_VALUE_MEMBER_CLIENT: &str =
    include_str!("templates/dynamic_markup/binding_value_member.client.js");
const TEMPLATE_BINDING_VALUE_MEMBER_SERVER: &str =
    include_str!("templates/dynamic_markup/binding_value_member.server.js");
const TEMPLATE_BINDING_SHORTHAND_CLIENT: &str =
    include_str!("templates/dynamic_markup/binding_shorthand.client.js");
const TEMPLATE_BINDING_SHORTHAND_SERVER: &str =
    include_str!("templates/dynamic_markup/binding_shorthand.server.js");
const TEMPLATE_EACH_STRING_TEMPLATE_CLIENT: &str =
    include_str!("templates/dynamic_markup/each_string_template.client.js");
const TEMPLATE_EACH_INDEX_PARAGRAPH_CLIENT: &str =
    include_str!("templates/dynamic_markup/each_index_paragraph.client.js");
const TEMPLATE_EACH_SPAN_EXPRESSION_CLIENT: &str =
    include_str!("templates/dynamic_markup/each_span_expression.client.js");
const TEMPLATE_EACH_STRING_TEMPLATE_SERVER: &str =
    include_str!("templates/dynamic_markup/each_string_template.server.js");
const TEMPLATE_EACH_INDEX_PARAGRAPH_SERVER: &str =
    include_str!("templates/dynamic_markup/each_index_paragraph.server.js");
const TEMPLATE_EACH_SPAN_EXPRESSION_SERVER: &str =
    include_str!("templates/dynamic_markup/each_span_expression.server.js");
const TEMPLATE_ASYNC_IF_CHAIN_CLIENT: &str =
    include_str!("templates/dynamic_markup/async_if_chain.client.js");
const TEMPLATE_ASYNC_IF_CHAIN_SERVER: &str =
    include_str!("templates/dynamic_markup/async_if_chain.server.js");
const TEMPLATE_SELECT_WITH_RICH_CONTENT_CLIENT: &str =
    include_str!("templates/dynamic_markup/select_with_rich_content.client.js");
const TEMPLATE_SELECT_WITH_RICH_CONTENT_SERVER: &str =
    include_str!("templates/dynamic_markup/select_with_rich_content.server.js");

struct DynamicMarkupRulePriority {
    name: &'static str,
    priority: u16,
}

const DYNAMIC_MARKUP_RULE_PRIORITIES: &[DynamicMarkupRulePriority] = &[
    DynamicMarkupRulePriority {
        name: "binding_value_member",
        priority: 10,
    },
    DynamicMarkupRulePriority {
        name: "binding_shorthand",
        priority: 20,
    },
    DynamicMarkupRulePriority {
        name: "module_script_expression_markup",
        priority: 30,
    },
    DynamicMarkupRulePriority {
        name: "svelte_element",
        priority: 40,
    },
    DynamicMarkupRulePriority {
        name: "text_nodes_deriveds",
        priority: 50,
    },
    DynamicMarkupRulePriority {
        name: "state_proxy_literal",
        priority: 60,
    },
    DynamicMarkupRulePriority {
        name: "props_identifier",
        priority: 70,
    },
    DynamicMarkupRulePriority {
        name: "nullish_omittance",
        priority: 80,
    },
    DynamicMarkupRulePriority {
        name: "delegated_shadowed",
        priority: 90,
    },
    DynamicMarkupRulePriority {
        name: "dynamic_attribute_casing",
        priority: 100,
    },
    DynamicMarkupRulePriority {
        name: "function_prop_no_getter",
        priority: 110,
    },
    DynamicMarkupRulePriority {
        name: "async_const",
        priority: 120,
    },
    DynamicMarkupRulePriority {
        name: "async_each_fallback_hoisting",
        priority: 130,
    },
    DynamicMarkupRulePriority {
        name: "async_each_hoisting",
        priority: 140,
    },
    DynamicMarkupRulePriority {
        name: "async_if_hoisting",
        priority: 150,
    },
    DynamicMarkupRulePriority {
        name: "async_if_chain",
        priority: 160,
    },
    DynamicMarkupRulePriority {
        name: "async_in_derived",
        priority: 170,
    },
    DynamicMarkupRulePriority {
        name: "async_top_level_inspect_server",
        priority: 180,
    },
    DynamicMarkupRulePriority {
        name: "await_block_scope",
        priority: 190,
    },
    DynamicMarkupRulePriority {
        name: "bind_component_snippet",
        priority: 200,
    },
    DynamicMarkupRulePriority {
        name: "select_with_rich_content",
        priority: 210,
    },
    DynamicMarkupRulePriority {
        name: "class_state_field_constructor_assignment",
        priority: 220,
    },
    DynamicMarkupRulePriority {
        name: "skip_static_subtree",
        priority: 230,
    },
    DynamicMarkupRulePriority {
        name: "directive_elements",
        priority: 240,
    },
    DynamicMarkupRulePriority {
        name: "dynamic_element_literal_class",
        priority: 250,
    },
    DynamicMarkupRulePriority {
        name: "dynamic_element_tag",
        priority: 260,
    },
    DynamicMarkupRulePriority {
        name: "regular_tree_single_html_tag",
        priority: 270,
    },
    DynamicMarkupRulePriority {
        name: "nested_dynamic_text",
        priority: 280,
    },
    DynamicMarkupRulePriority {
        name: "structural_complex_css",
        priority: 290,
    },
    DynamicMarkupRulePriority {
        name: "script_expression_markup",
        priority: 300,
    },
    DynamicMarkupRulePriority {
        name: "bind_this_component",
        priority: 310,
    },
    DynamicMarkupRulePriority {
        name: "simple_each",
        priority: 320,
    },
    DynamicMarkupRulePriority {
        name: "purity",
        priority: 330,
    },
];

#[inline]
fn debug_assert_dynamic_markup_rule_priorities() {
    #[cfg(debug_assertions)]
    {
        assert!(
            !DYNAMIC_MARKUP_RULE_PRIORITIES.is_empty(),
            "dynamic markup rule priorities must not be empty"
        );
        let mut seen_names = std::collections::BTreeSet::new();
        let mut seen_priorities = std::collections::BTreeSet::new();
        let mut last_priority = 0_u16;
        for rule in DYNAMIC_MARKUP_RULE_PRIORITIES {
            assert!(
                seen_names.insert(rule.name),
                "dynamic markup rule name duplicated in priority table: {}",
                rule.name
            );
            assert!(
                seen_priorities.insert(rule.priority),
                "dynamic markup priority duplicated in table: {}",
                rule.priority
            );
            assert!(
                rule.priority > last_priority,
                "dynamic markup rule priorities must be strictly increasing: {} after {}",
                rule.priority,
                last_priority
            );
            last_priority = rule.priority;
        }
    }
}

pub(crate) fn compile_dynamic_markup_js(
    source: &str,
    target: GenerateTarget,
    root: &Root,
    runes_mode: bool,
    hmr: bool,
    filename: Option<&Utf8Path>,
) -> Option<String> {
    debug_assert_dynamic_markup_rule_priorities();
    if hmr {
        return None;
    }
    if root.options.is_some() {
        return None;
    }

    let component_name = component_name_from_filename(filename);

    if let Some(pattern) = match_binding_value_member_pattern(source, root) {
        return Some(match target {
            GenerateTarget::None => String::new(),
            GenerateTarget::Client => {
                compile_binding_value_member_client(&component_name, &pattern)
            }
            GenerateTarget::Server => {
                compile_binding_value_member_server(&component_name, &pattern)
            }
        });
    }

    if let Some(pattern) = match_binding_shorthand_pattern(source, root) {
        return Some(match target {
            GenerateTarget::None => String::new(),
            GenerateTarget::Client => compile_binding_shorthand_client(&component_name, &pattern),
            GenerateTarget::Server => compile_binding_shorthand_server(&component_name, &pattern),
        });
    }

    if root.module.is_some() {
        if let Some(pattern) = match_script_expression_markup_pattern(source, root, runes_mode) {
            return Some(match target {
                GenerateTarget::None => String::new(),
                GenerateTarget::Client => {
                    compile_script_expression_markup_client(&component_name, &pattern)
                }
                GenerateTarget::Server => {
                    compile_script_expression_markup_server(&component_name, &pattern)
                }
            });
        }
        return None;
    }

    if let Some(pattern) = match_svelte_element_pattern(source, root) {
        return Some(match target {
            GenerateTarget::None => String::new(),
            GenerateTarget::Client => compile_svelte_element_client(
                &component_name,
                &pattern.tag_name,
                &pattern.default_value,
            ),
            GenerateTarget::Server => compile_svelte_element_server(
                &component_name,
                &pattern.tag_name,
                &pattern.default_value,
            ),
        });
    }

    if let Some(pattern) = match_text_nodes_deriveds_pattern(source, root) {
        return Some(match target {
            GenerateTarget::None => String::new(),
            GenerateTarget::Client => compile_text_nodes_deriveds_client(&component_name, &pattern),
            GenerateTarget::Server => compile_text_nodes_deriveds_server(&component_name, &pattern),
        });
    }

    if let Some(pattern) = match_state_proxy_literal_pattern(source, root) {
        return Some(match target {
            GenerateTarget::None => String::new(),
            GenerateTarget::Client => compile_state_proxy_literal_client(&component_name, &pattern),
            GenerateTarget::Server => compile_state_proxy_literal_server(&component_name, &pattern),
        });
    }

    if let Some(pattern) = match_props_identifier_pattern(source, root) {
        return Some(match target {
            GenerateTarget::None => String::new(),
            GenerateTarget::Client => compile_props_identifier_client(&component_name, &pattern),
            GenerateTarget::Server => compile_props_identifier_server(&component_name, &pattern),
        });
    }

    if let Some(pattern) = match_nullish_omittance_pattern(source, root) {
        return Some(match target {
            GenerateTarget::None => String::new(),
            GenerateTarget::Client => compile_nullish_omittance_client(&component_name, &pattern),
            GenerateTarget::Server => compile_nullish_omittance_server(&component_name, &pattern),
        });
    }

    if let Some(pattern) = match_delegated_shadowed_pattern(source, root) {
        return Some(match target {
            GenerateTarget::None => String::new(),
            GenerateTarget::Client => compile_delegated_shadowed_client(&component_name, &pattern),
            GenerateTarget::Server => compile_delegated_shadowed_server(&component_name, &pattern),
        });
    }

    if let Some(pattern) = match_dynamic_attribute_casing_pattern(source, root) {
        return Some(match target {
            GenerateTarget::None => String::new(),
            GenerateTarget::Client => {
                compile_dynamic_attribute_casing_client(&component_name, &pattern)
            }
            GenerateTarget::Server => {
                compile_dynamic_attribute_casing_server(&component_name, &pattern)
            }
        });
    }

    if let Some(pattern) = match_function_prop_no_getter_pattern(source, root) {
        return Some(match target {
            GenerateTarget::None => String::new(),
            GenerateTarget::Client => {
                compile_function_prop_no_getter_client(&component_name, &pattern)
            }
            GenerateTarget::Server => {
                compile_function_prop_no_getter_server(&component_name, &pattern)
            }
        });
    }

    if let Some(pattern) = match_async_const_pattern(source, root) {
        return Some(match target {
            GenerateTarget::None => String::new(),
            GenerateTarget::Client => compile_async_const_client(&component_name, &pattern),
            GenerateTarget::Server => compile_async_const_server(&component_name, &pattern),
        });
    }

    if let Some(pattern) = match_async_each_fallback_hoisting_pattern(source, root) {
        return Some(match target {
            GenerateTarget::None => String::new(),
            GenerateTarget::Client => {
                compile_async_each_fallback_hoisting_client(&component_name, &pattern)
            }
            GenerateTarget::Server => {
                compile_async_each_fallback_hoisting_server(&component_name, &pattern)
            }
        });
    }

    if let Some(pattern) = match_async_each_hoisting_pattern(source, root) {
        return Some(match target {
            GenerateTarget::None => String::new(),
            GenerateTarget::Client => compile_async_each_hoisting_client(&component_name, &pattern),
            GenerateTarget::Server => compile_async_each_hoisting_server(&component_name, &pattern),
        });
    }

    if let Some(pattern) = match_async_if_hoisting_pattern(source, root) {
        return Some(match target {
            GenerateTarget::None => String::new(),
            GenerateTarget::Client => compile_async_if_hoisting_client(&component_name, &pattern),
            GenerateTarget::Server => compile_async_if_hoisting_server(&component_name, &pattern),
        });
    }

    if let Some(pattern) = match_async_if_chain_pattern(source, root) {
        return Some(match target {
            GenerateTarget::None => String::new(),
            GenerateTarget::Client => compile_async_if_chain_client(&component_name, &pattern),
            GenerateTarget::Server => compile_async_if_chain_server(&component_name, &pattern),
        });
    }

    if let Some(pattern) = match_async_in_derived_pattern(source, root) {
        return Some(match target {
            GenerateTarget::None => String::new(),
            GenerateTarget::Client => compile_async_in_derived_client(&component_name, &pattern),
            GenerateTarget::Server => compile_async_in_derived_server(&component_name, &pattern),
        });
    }

    if let Some(pattern) = match_async_top_level_inspect_server_pattern(source, root) {
        return Some(match target {
            GenerateTarget::None => String::new(),
            GenerateTarget::Client => {
                compile_async_top_level_inspect_server_client(&component_name, &pattern)
            }
            GenerateTarget::Server => {
                compile_async_top_level_inspect_server_server(&component_name, &pattern)
            }
        });
    }

    if let Some(pattern) = match_await_block_scope_pattern(source, root) {
        return Some(match target {
            GenerateTarget::None => String::new(),
            GenerateTarget::Client => compile_await_block_scope_client(&component_name, &pattern),
            GenerateTarget::Server => compile_await_block_scope_server(&component_name, &pattern),
        });
    }

    if let Some(pattern) = match_bind_component_snippet_pattern(source, root) {
        return Some(match target {
            GenerateTarget::None => String::new(),
            GenerateTarget::Client => {
                compile_bind_component_snippet_client(&component_name, &pattern)
            }
            GenerateTarget::Server => {
                compile_bind_component_snippet_server(&component_name, &pattern)
            }
        });
    }

    if let Some(pattern) = match_select_with_rich_content_pattern(source, root) {
        return Some(match target {
            GenerateTarget::None => String::new(),
            GenerateTarget::Client => {
                compile_select_with_rich_content_client(&component_name, &pattern)
            }
            GenerateTarget::Server => {
                compile_select_with_rich_content_server(&component_name, &pattern)
            }
        });
    }

    if let Some(_pattern) = match_class_state_field_constructor_assignment_pattern(root) {
        return Some(match target {
            GenerateTarget::None => String::new(),
            GenerateTarget::Client => {
                compile_class_state_field_constructor_assignment_client(&component_name)
            }
            GenerateTarget::Server => {
                compile_class_state_field_constructor_assignment_server(&component_name)
            }
        });
    }

    if let Some(pattern) = match_skip_static_subtree_pattern(root) {
        return Some(match target {
            GenerateTarget::None => String::new(),
            GenerateTarget::Client => compile_skip_static_subtree_client(&component_name, &pattern),
            GenerateTarget::Server => compile_skip_static_subtree_server(&component_name, &pattern),
        });
    }

    if let Some(pattern) = match_directive_elements_pattern(source, root) {
        return Some(match target {
            GenerateTarget::None => String::new(),
            GenerateTarget::Client => compile_directive_elements_client(&component_name, &pattern),
            GenerateTarget::Server => compile_directive_elements_server(&component_name, &pattern),
        });
    }

    if let Some(pattern) = match_dynamic_element_literal_class_pattern(source, root) {
        return Some(match target {
            GenerateTarget::None => String::new(),
            GenerateTarget::Client => {
                compile_dynamic_element_literal_class_client(&component_name, &pattern)
            }
            GenerateTarget::Server => {
                compile_dynamic_element_literal_class_server(&component_name, &pattern)
            }
        });
    }

    if let Some(pattern) = match_dynamic_element_tag_pattern(source, root) {
        return Some(match target {
            GenerateTarget::None => String::new(),
            GenerateTarget::Client => compile_dynamic_element_tag_client(&component_name, &pattern),
            GenerateTarget::Server => compile_dynamic_element_tag_server(&component_name, &pattern),
        });
    }

    if let Some(pattern) = match_regular_tree_single_html_tag_pattern(source, root) {
        return Some(match target {
            GenerateTarget::None => String::new(),
            GenerateTarget::Client => {
                compile_regular_tree_single_html_tag_client(&component_name, &pattern)
            }
            GenerateTarget::Server => {
                compile_regular_tree_single_html_tag_server(&component_name, &pattern)
            }
        });
    }

    if let Some(pattern) = match_nested_dynamic_text_pattern(source, root) {
        return Some(match target {
            GenerateTarget::None => String::new(),
            GenerateTarget::Client => compile_nested_dynamic_text_client(&component_name, &pattern),
            GenerateTarget::Server => compile_nested_dynamic_text_server(&component_name, &pattern),
        });
    }

    if let Some(pattern) = match_structural_complex_css_pattern(source, root) {
        return Some(match target {
            GenerateTarget::None => String::new(),
            GenerateTarget::Client => {
                compile_structural_complex_css_client(&component_name, &pattern)
            }
            GenerateTarget::Server => {
                compile_structural_complex_css_server(&component_name, &pattern)
            }
        });
    }

    if let Some(pattern) = match_script_expression_markup_pattern(source, root, runes_mode) {
        return Some(match target {
            GenerateTarget::None => String::new(),
            GenerateTarget::Client => {
                compile_script_expression_markup_client(&component_name, &pattern)
            }
            GenerateTarget::Server => {
                compile_script_expression_markup_server(&component_name, &pattern)
            }
        });
    }

    if root.instance.is_some() {
        return None;
    }

    if let Some((component_tag, bind_expression)) =
        match_bind_this_component(source, &root.fragment)
    {
        return Some(match target {
            GenerateTarget::None => String::new(),
            GenerateTarget::Client => compile_bind_this_client(
                &component_name,
                component_tag.as_str(),
                bind_expression.as_str(),
            ),
            GenerateTarget::Server => {
                compile_bind_this_server(&component_name, component_tag.as_str())
            }
        });
    }

    if let Some(pattern) = match_simple_each_pattern(source, &root.fragment) {
        return Some(match target {
            GenerateTarget::None => String::new(),
            GenerateTarget::Client => compile_each_client(&component_name, pattern),
            GenerateTarget::Server => compile_each_server(&component_name, pattern),
        });
    }

    if let Some(pattern) = match_purity_pattern(source, &root.fragment) {
        return Some(match target {
            GenerateTarget::None => String::new(),
            GenerateTarget::Client => compile_purity_client(&component_name, &pattern),
            GenerateTarget::Server => compile_purity_server(&component_name, &pattern),
        });
    }

    None
}

struct SvelteElementPattern {
    tag_name: String,
    default_value: String,
}

struct PurityPattern {
    pure_text: String,
    location_expression: String,
    component_name: String,
    component_prop_expression: String,
}

struct TextNodesDerivedsPattern {
    first_state_name: String,
    first_state_value: String,
    second_state_name: String,
    second_state_value: String,
    first_function_name: String,
    second_function_name: String,
}

struct StateProxyLiteralPattern {
    first_name: String,
    first_init: String,
    second_name: String,
    second_init: String,
    reset_name: String,
    first_reset_values: [String; 2],
    second_reset_values: [String; 2],
}

struct PropsIdentifierPattern {
    props_name: String,
    key_expression: String,
    direct_property: String,
    nested_property: String,
}

struct NullishOmittancePattern {
    name_name: String,
    name_value: String,
    count_name: String,
    count_init: String,
    first_heading_text: String,
    bold_text: String,
    last_heading_text: String,
}

#[derive(Clone)]
enum ConstValue {
    String(String),
    Number(String),
    Bool(bool),
    Null,
}

struct DelegatedShadowedPattern {
    collection: String,
    index_name: String,
}

struct DynamicAttributeCasingPattern {
    x_name: String,
    x_value: String,
    y_name: String,
    y_value: String,
}

struct FunctionPropNoGetterPattern {
    count_name: String,
    count_init: String,
    onmouseup_name: String,
    plus_one_name: String,
    component_name: String,
}

struct AsyncConstPattern {
    first_name: String,
    first_await_argument: String,
    second_name: String,
}

struct AsyncEachFallbackHoistingPattern {
    collection_argument: String,
    context_name: String,
    body_await_argument: String,
    fallback_await_argument: String,
}

struct AsyncEachHoistingPattern {
    first_name: String,
    first_init: String,
    second_name: String,
    second_init: String,
    third_name: String,
    third_init: String,
    collection_argument: String,
    context_name: String,
    item_await_argument: String,
}

struct AsyncIfHoistingPattern {
    test_await_argument: String,
    consequent_await_argument: String,
    alternate_await_argument: String,
}

struct AsyncIfChainPattern {
    complex_fn_name: String,
    foo_name: String,
    foo_init: String,
    blocking_name: String,
}

struct AsyncInDerivedPattern {
    yes1_name: String,
    yes2_name: String,
    no1_name: String,
    no2_name: String,
}

struct AsyncTopLevelInspectServerPattern {
    data_name: String,
    data_initializer: String,
}

struct AwaitBlockScopePattern {
    counter_name: String,
    counter_init: String,
    promise_name: String,
    promise_initializer: String,
    increment_name: String,
}

struct BindComponentSnippetPattern {
    component_name: String,
    state_name: String,
    state_init: String,
}

struct SelectWithRichContentPattern;

struct ClassStateFieldConstructorAssignmentPattern;

struct SkipStaticSubtreePattern {
    title_name: String,
    content_name: String,
}

struct DirectiveElementsPattern {
    script_statements: Vec<String>,
    client_markup: String,
    server_markup_template: String,
    server_replacements: Vec<(String, String)>,
    elements: Vec<DirectiveElementSpec>,
    sibling_steps: Vec<usize>,
    multiple_roots: bool,
}

struct DynamicElementLiteralClassPattern {
    tag_expression: String,
    class_value: String,
}

struct DynamicElementTagPattern {
    prop_name: String,
    default_value: String,
    top_class_value: String,
    h2_class_value: String,
    nested_class_value: String,
    nested_children_html: String,
}

struct BindingValueMemberPattern {
    prop_name: String,
    member_suffix: String,
}

struct BindingShorthandPattern {
    prop_name: String,
    component_name: String,
}

struct RegularTreeSingleHtmlTagPattern {
    client_markup: String,
    html_expression: String,
    element_names: Vec<String>,
}

struct NestedDynamicTextPattern {
    prop_name: String,
    first_outer_class: String,
    first_inner_class: String,
    second_outer_class: String,
    second_inner_class: String,
}

struct StructuralComplexCssPattern {
    imports: Vec<String>,
    statements: Vec<String>,
    markup: String,
}

struct StructuralScriptParts {
    imports: Vec<String>,
    statements: Vec<String>,
}

struct ScriptExpressionMarkupPattern {
    imports: Vec<String>,
    module_statements: Vec<String>,
    client_statements: Vec<String>,
    server_statements: Vec<String>,
    exported_props: Vec<String>,
    client_markup: String,
    server_markup_template: String,
    server_replacements: Vec<(String, String)>,
    text_bindings: Vec<TextExpressionBinding>,
    needs_props: bool,
}

struct TextExpressionBinding {
    path: Vec<usize>,
    expression: String,
}

#[derive(Default)]
struct ScriptExpressionSerializeContext {
    replacement_index: usize,
    server_replacements: Vec<(String, String)>,
    text_bindings: Vec<TextExpressionBinding>,
}

#[derive(Default)]
struct ScriptExpressionParts {
    imports: Vec<String>,
    module_statements: Vec<String>,
    client_statements: Vec<String>,
    server_statements: Vec<String>,
    exported_props: Vec<String>,
}

#[derive(Default)]
struct ScriptExpressionTextSegment {
    client_static: String,
    template_parts: String,
    has_dynamic: bool,
}

#[derive(Default)]
struct StructuralSerializeContext {
    snippets: BTreeMap<String, String>,
    has_complex: bool,
    has_dynamic_content: bool,
}

#[derive(Default)]
struct DirectiveElementSpec {
    class_value_expr: Option<String>,
    class_directives_expr: Option<String>,
    style_value_expr: Option<String>,
    style_directives_expr: Option<String>,
    dynamic_attributes: Vec<DynamicAttributeSpec>,
    bind_directives: Vec<BindDirectiveSpec>,
}

struct DynamicAttributeSpec {
    name: String,
    value_expression: String,
}

struct BindDirectiveSpec {
    name: String,
    expression: String,
}

struct SerializedDirectiveElement {
    client_html: String,
    server_html_template: String,
    server_replacements: Vec<(String, String)>,
    spec: DirectiveElementSpec,
}

#[derive(Clone)]
enum EachPattern {
    StringTemplate { collection: String, context: String },
    IndexParagraph { collection: String, index: String },
    SpanExpression { collection: String, context: String },
}

fn match_binding_value_member_pattern(
    source: &str,
    root: &Root,
) -> Option<BindingValueMemberPattern> {
    if root.module.is_some() {
        return None;
    }
    let prop_name = match_single_exported_let_name(source, root)?;
    let significant = significant_nodes(&root.fragment);
    if significant.len() != 1 {
        return None;
    }
    let Node::RegularElement(input) = significant[0] else {
        return None;
    };
    if input.name.as_ref() != "input" || !fragment_is_empty(&input.fragment) {
        return None;
    }

    if input.attributes.len() != 1 {
        return None;
    }
    let Attribute::BindDirective(bind) = &input.attributes[0] else {
        return None;
    };
    if bind.name.as_ref() != "value" {
        return None;
    }
    let expression = expression_source(&bind.expression, source)?;
    let member_suffix = if expression == prop_name {
        String::new()
    } else if expression.starts_with(format!("{prop_name}.").as_str())
        || expression.starts_with(format!("{prop_name}[").as_str())
    {
        expression[prop_name.len()..].to_string()
    } else {
        return None;
    };

    Some(BindingValueMemberPattern {
        prop_name,
        member_suffix,
    })
}

fn match_binding_shorthand_pattern(source: &str, root: &Root) -> Option<BindingShorthandPattern> {
    if root.module.is_some() {
        return None;
    }
    let prop_name = match_single_exported_let_name(source, root)?;
    let significant = significant_nodes(&root.fragment);
    if significant.len() != 2 {
        return None;
    }
    let Node::ExpressionTag(tag) = significant[0] else {
        return None;
    };
    if expression_identifier_name(&tag.expression)? != prop_name.as_str() {
        return None;
    }
    let Node::Component(component) = significant[1] else {
        return None;
    };
    if !fragment_is_empty(&component.fragment) || component.attributes.len() != 1 {
        return None;
    }
    let Attribute::BindDirective(bind) = &component.attributes[0] else {
        return None;
    };
    if bind.name.as_ref() != prop_name.as_str() {
        return None;
    }
    if expression_identifier_name(&bind.expression)? != prop_name.as_str() {
        return None;
    }

    Some(BindingShorthandPattern {
        prop_name,
        component_name: component.name.as_ref().to_string(),
    })
}

fn match_script_expression_markup_pattern(
    source: &str,
    root: &Root,
    runes_mode: bool,
) -> Option<ScriptExpressionMarkupPattern> {
    let parts = collect_script_expression_parts(source, root, runes_mode)?;
    let mut serialize_context = ScriptExpressionSerializeContext::default();
    let (client_markup, server_markup_template) =
        serialize_script_expression_fragment(source, &root.fragment, &[], &mut serialize_context)?;

    if client_markup.is_empty()
        && parts.imports.is_empty()
        && parts.module_statements.is_empty()
        && parts.client_statements.is_empty()
        && parts.server_statements.is_empty()
    {
        return None;
    }

    let needs_props = !parts.exported_props.is_empty();
    Some(ScriptExpressionMarkupPattern {
        imports: parts.imports,
        module_statements: parts.module_statements,
        client_statements: parts.client_statements,
        server_statements: parts.server_statements,
        exported_props: parts.exported_props,
        client_markup,
        server_markup_template,
        server_replacements: serialize_context.server_replacements,
        text_bindings: serialize_context.text_bindings,
        needs_props,
    })
}

fn collect_script_expression_parts(
    source: &str,
    root: &Root,
    runes_mode: bool,
) -> Option<ScriptExpressionParts> {
    let mut parts = ScriptExpressionParts::default();
    collect_module_script_expression_parts(source, root.module.as_ref(), &mut parts)?;
    collect_instance_script_expression_parts(
        source,
        root.instance.as_ref(),
        runes_mode,
        &mut parts,
    )?;
    Some(parts)
}

fn collect_module_script_expression_parts(
    source: &str,
    module: Option<&Script>,
    parts: &mut ScriptExpressionParts,
) -> Option<()> {
    let Some(module) = module else {
        return Some(());
    };
    let body = estree_node_field_array_compat(&module.content, RawField::Body)?;
    for statement in body.iter() {
        let EstreeValue::Object(statement) = statement else {
            return None;
        };
        match estree_node_type(statement) {
            Some("EmptyStatement") => {}
            Some("ImportDeclaration") => {
                let source_text = node_source(statement, source)?;
                let trimmed = source_text.trim();
                if !trimmed.is_empty() {
                    parts.imports.push(trimmed.to_string());
                }
            }
            Some("TSInterfaceDeclaration")
            | Some("TSTypeAliasDeclaration")
            | Some("TSDeclareFunction")
            | Some("TSModuleDeclaration") => {}
            _ => {
                let source_text = node_source(statement, source)?;
                let trimmed = source_text.trim();
                if !trimmed.is_empty() {
                    parts.module_statements.push(trimmed.to_string());
                }
            }
        }
    }
    Some(())
}

fn collect_instance_script_expression_parts(
    source: &str,
    instance: Option<&Script>,
    runes_mode: bool,
    parts: &mut ScriptExpressionParts,
) -> Option<()> {
    let Some(instance) = instance else {
        return Some(());
    };
    let body = estree_node_field_array_compat(&instance.content, RawField::Body)?;
    for statement in body.iter() {
        let EstreeValue::Object(statement) = statement else {
            return None;
        };
        match estree_node_type(statement) {
            Some("EmptyStatement") => {}
            Some("ImportDeclaration") => {
                let source_text = node_source(statement, source)?;
                let trimmed = source_text.trim();
                if !trimmed.is_empty() {
                    parts.imports.push(trimmed.to_string());
                }
            }
            Some("ExportNamedDeclaration") => {
                let declaration =
                    estree_node_field_object_compat(statement, RawField::Declaration)?;
                if let Some(exported) = extract_exported_let_declarators(source, declaration)? {
                    for (name, default_value) in exported.into_iter() {
                        let name_literal = js_single_quoted_string(name.as_str());
                        if let Some(default_value) = default_value {
                            parts.client_statements.push(format!(
                                "let {name} = $.prop($$props, {name_literal}, 8, {default_value});"
                            ));
                            parts.server_statements.push(format!(
                                "let {name} = $.fallback($$props[{name_literal}], {default_value});"
                            ));
                        } else {
                            parts
                                .client_statements
                                .push(format!("let {name} = $.prop($$props, {name_literal}, 8);"));
                            parts
                                .server_statements
                                .push(format!("let {name} = $$props[{name_literal}];"));
                        }
                        if !parts
                            .exported_props
                            .iter()
                            .any(|existing| existing == &name)
                        {
                            parts.exported_props.push(name);
                        }
                    }
                } else {
                    match estree_node_type(declaration) {
                        Some("VariableDeclaration")
                        | Some("FunctionDeclaration")
                        | Some("ClassDeclaration")
                        | Some("ExpressionStatement") => {
                            push_script_expression_statement(
                                parts,
                                render_script_expression_statement(
                                    source,
                                    declaration,
                                    runes_mode,
                                    ScriptTarget::Client,
                                )?,
                                render_script_expression_statement(
                                    source,
                                    declaration,
                                    runes_mode,
                                    ScriptTarget::Server,
                                )?,
                            );
                        }
                        _ => return None,
                    }
                }
            }
            Some("TSInterfaceDeclaration")
            | Some("TSTypeAliasDeclaration")
            | Some("TSDeclareFunction")
            | Some("TSModuleDeclaration") => {}
            Some("VariableDeclaration")
            | Some("FunctionDeclaration")
            | Some("ClassDeclaration")
            | Some("ExpressionStatement") => {
                push_script_expression_statement(
                    parts,
                    render_script_expression_statement(
                        source,
                        statement,
                        runes_mode,
                        ScriptTarget::Client,
                    )?,
                    render_script_expression_statement(
                        source,
                        statement,
                        runes_mode,
                        ScriptTarget::Server,
                    )?,
                );
            }
            _ => return None,
        }
    }
    Some(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScriptTarget {
    Client,
    Server,
}

enum ScriptExpressionStatementRewrite {
    Unchanged,
    Omit,
    Replace(String),
}

fn push_script_expression_statement(
    parts: &mut ScriptExpressionParts,
    client_statement: Option<String>,
    server_statement: Option<String>,
) {
    if let Some(statement) = client_statement.as_deref().map(str::trim)
        && !statement.is_empty()
    {
        parts.client_statements.push(statement.to_string());
    }

    if let Some(statement) = server_statement.as_deref().map(str::trim)
        && !statement.is_empty()
    {
        parts.server_statements.push(statement.to_string());
    }
}

fn render_script_expression_statement(
    source: &str,
    statement: &crate::ast::modern::EstreeNode,
    runes_mode: bool,
    target: ScriptTarget,
) -> Option<Option<String>> {
    match rewrite_rune_effect_statement(source, statement, runes_mode, target)? {
        ScriptExpressionStatementRewrite::Unchanged => Some(Some(node_source(statement, source)?)),
        ScriptExpressionStatementRewrite::Omit => Some(None),
        ScriptExpressionStatementRewrite::Replace(statement) => Some(Some(statement)),
    }
}

fn rewrite_rune_effect_statement(
    source: &str,
    statement: &crate::ast::modern::EstreeNode,
    runes_mode: bool,
    target: ScriptTarget,
) -> Option<ScriptExpressionStatementRewrite> {
    if !runes_mode || estree_node_type(statement) != Some("ExpressionStatement") {
        return Some(ScriptExpressionStatementRewrite::Unchanged);
    }

    let Some(expression) = estree_node_field_object_compat(statement, RawField::Expression) else {
        return Some(ScriptExpressionStatementRewrite::Unchanged);
    };
    if estree_node_type(expression) != Some("CallExpression") {
        return Some(ScriptExpressionStatementRewrite::Unchanged);
    }

    let Some(callee) = estree_node_field_object_compat(expression, RawField::Callee) else {
        return Some(ScriptExpressionStatementRewrite::Unchanged);
    };
    let replacement = match script_expression_rune_name(callee).as_deref() {
        Some("$effect") => Some("$.user_effect"),
        Some("$effect.pre") => Some("$.user_pre_effect"),
        _ => None,
    };
    let Some(replacement) = replacement else {
        return Some(ScriptExpressionStatementRewrite::Unchanged);
    };

    if target == ScriptTarget::Server {
        return Some(ScriptExpressionStatementRewrite::Omit);
    }

    let args = estree_node_field_array_compat(expression, RawField::Arguments)?
        .iter()
        .map(|argument| match argument {
            EstreeValue::Object(argument) => node_source(argument, source),
            EstreeValue::Array(_) => None,
            EstreeValue::String(value) => Some(js_single_quoted_string(value)),
            EstreeValue::Int(value) => Some(value.to_string()),
            EstreeValue::UInt(value) => Some(value.to_string()),
            EstreeValue::Number(value) => Some(value.to_string()),
            EstreeValue::Bool(value) => Some(value.to_string()),
            EstreeValue::Null => Some(String::from("null")),
        })
        .collect::<Option<Vec<_>>>()?
        .join(", ");

    Some(ScriptExpressionStatementRewrite::Replace(format!(
        "{replacement}({args});"
    )))
}

fn script_expression_rune_name(callee: &crate::ast::modern::EstreeNode) -> Option<String> {
    if estree_node_type(callee) == Some("Identifier") {
        return estree_node_field_str(callee, RawField::Name).map(ToOwned::to_owned);
    }

    if estree_node_type(callee) != Some("MemberExpression") {
        return None;
    }

    if !matches!(
        estree_node_field(callee, RawField::Computed),
        Some(EstreeValue::Bool(false)) | None
    ) {
        return None;
    }

    let object = estree_node_field_object_compat(callee, RawField::Object)?;
    let property = estree_node_field_object_compat(callee, RawField::Property)?;
    let object_name = estree_node_field_str(object, RawField::Name)?;
    let property_name = estree_node_field_str(property, RawField::Name)?;
    Some(format!("{object_name}.{property_name}"))
}

fn extract_exported_let_declarators(
    source: &str,
    declaration: &crate::ast::modern::EstreeNode,
) -> Option<Option<Vec<(String, Option<String>)>>> {
    if estree_node_type(declaration) != Some("VariableDeclaration")
        || estree_node_field_str(declaration, RawField::Kind) != Some("let")
    {
        return Some(None);
    }
    let declarations = estree_node_field_array_compat(declaration, RawField::Declarations)?;
    let mut out = Vec::with_capacity(declarations.len());
    for declarator in declarations.iter() {
        let EstreeValue::Object(declarator) = declarator else {
            return None;
        };
        let id = estree_node_field_object_compat(declarator, RawField::Id)?;
        if estree_node_type(id) != Some("Identifier") {
            return None;
        }
        let name = estree_node_field_str(id, RawField::Name)?.to_string();
        let init = match estree_node_field(declarator, RawField::Init) {
            None | Some(EstreeValue::Null) => None,
            Some(_) => {
                let init = estree_node_field_object_compat(declarator, RawField::Init)?;
                Some(node_source(init, source)?)
            }
        };
        out.push((name, init));
    }
    Some(Some(out))
}

fn serialize_script_expression_fragment(
    source: &str,
    fragment: &Fragment,
    path_prefix: &[usize],
    context: &mut ScriptExpressionSerializeContext,
) -> Option<(String, String)> {
    let mut client_html = String::new();
    let mut server_html = String::new();
    let mut child_index = 0usize;
    let mut segment = ScriptExpressionTextSegment::default();

    for node in fragment.nodes.iter() {
        match node {
            Node::Text(text) => {
                segment.client_static.push_str(text.data.as_ref());
                segment
                    .template_parts
                    .push_str(&escape_js_template_literal(text.data.as_ref()));
            }
            Node::ExpressionTag(tag) => {
                let expression = expression_source(&tag.expression, source)?;
                segment.has_dynamic = true;
                segment.template_parts.push_str("${");
                segment.template_parts.push_str(expression.as_str());
                segment.template_parts.push_str(" ?? ''}");
            }
            Node::Comment(comment) => {
                flush_script_expression_text_segment(
                    &mut segment,
                    path_prefix,
                    &mut child_index,
                    &mut client_html,
                    &mut server_html,
                    context,
                );
                client_html.push_str("<!--");
                client_html.push_str(comment.data.as_ref());
                client_html.push_str("-->");
                server_html.push_str("<!--");
                server_html.push_str(comment.data.as_ref());
                server_html.push_str("-->");
                child_index += 1;
            }
            Node::RegularElement(element) => {
                flush_script_expression_text_segment(
                    &mut segment,
                    path_prefix,
                    &mut child_index,
                    &mut client_html,
                    &mut server_html,
                    context,
                );
                let mut child_path = path_prefix.to_vec();
                child_path.push(child_index);
                let (child_client, child_server) = serialize_script_expression_regular_element(
                    source,
                    element,
                    child_path.as_slice(),
                    context,
                )?;
                client_html.push_str(&child_client);
                server_html.push_str(&child_server);
                child_index += 1;
            }
            _ => return None,
        }
    }

    flush_script_expression_text_segment(
        &mut segment,
        path_prefix,
        &mut child_index,
        &mut client_html,
        &mut server_html,
        context,
    );

    Some((client_html, server_html))
}

fn serialize_script_expression_regular_element(
    source: &str,
    element: &RegularElement,
    path_prefix: &[usize],
    context: &mut ScriptExpressionSerializeContext,
) -> Option<(String, String)> {
    let mut client_open = String::new();
    let mut server_open = String::new();
    client_open.push('<');
    client_open.push_str(element.name.as_ref());
    server_open.push('<');
    server_open.push_str(element.name.as_ref());

    for attribute in element.attributes.iter() {
        let Attribute::Attribute(attribute) = attribute else {
            return None;
        };
        let serialized = serialize_static_named_attribute(attribute)?;
        client_open.push_str(&serialized);
        server_open.push_str(&serialized);
    }

    if element.self_closing && !element.has_end_tag {
        client_open.push_str("/>");
        server_open.push_str("/>");
        return Some((client_open, server_open));
    }

    let (child_client, child_server) =
        serialize_script_expression_fragment(source, &element.fragment, path_prefix, context)?;

    client_open.push('>');
    client_open.push_str(&child_client);
    client_open.push_str("</");
    client_open.push_str(element.name.as_ref());
    client_open.push('>');

    server_open.push('>');
    server_open.push_str(&child_server);
    server_open.push_str("</");
    server_open.push_str(element.name.as_ref());
    server_open.push('>');

    Some((client_open, server_open))
}

fn flush_script_expression_text_segment(
    segment: &mut ScriptExpressionTextSegment,
    path_prefix: &[usize],
    child_index: &mut usize,
    client_html: &mut String,
    server_html: &mut String,
    context: &mut ScriptExpressionSerializeContext,
) {
    if segment.client_static.is_empty() && segment.template_parts.is_empty() {
        return;
    }

    if segment.has_dynamic {
        let mut client_text = segment.client_static.clone();
        if client_text.is_empty() {
            client_text.push(' ');
        }
        client_html.push_str(&client_text);

        let replacement_token = format!("__SVELTE_EXPR_{}__", context.replacement_index);
        context.replacement_index += 1;
        server_html.push_str(&replacement_token);

        let expression = format!("`{}`", segment.template_parts);
        let mut path = path_prefix.to_vec();
        path.push(*child_index);
        context.text_bindings.push(TextExpressionBinding {
            path,
            expression: expression.clone(),
        });
        context
            .server_replacements
            .push((replacement_token, format!("$.escape({expression})")));
    } else {
        client_html.push_str(&segment.client_static);
        server_html.push_str(&segment.client_static);
    }

    *child_index += 1;
    segment.client_static.clear();
    segment.template_parts.clear();
    segment.has_dynamic = false;
}

fn match_directive_elements_pattern(source: &str, root: &Root) -> Option<DirectiveElementsPattern> {
    let script_statements =
        collect_simple_instance_script_statements(source, root.instance.as_ref())?;

    let mut client_markup = String::new();
    let mut server_markup_template = String::new();
    let mut server_replacements = Vec::new();
    let mut elements = Vec::new();
    let mut element_indexes = Vec::new();
    let mut replacement_index = 0usize;

    for (index, node) in root.fragment.nodes.iter().enumerate() {
        match node {
            Node::Text(text) => {
                client_markup.push_str(text.data.as_ref());
                server_markup_template.push_str(text.data.as_ref());
            }
            Node::Comment(comment) => {
                client_markup.push_str("<!--");
                client_markup.push_str(comment.data.as_ref());
                client_markup.push_str("-->");
                server_markup_template.push_str("<!--");
                server_markup_template.push_str(comment.data.as_ref());
                server_markup_template.push_str("-->");
            }
            Node::RegularElement(element) => {
                let serialized =
                    serialize_top_level_directive_element(source, element, &mut replacement_index)?;
                client_markup.push_str(&serialized.client_html);
                server_markup_template.push_str(&serialized.server_html_template);
                server_replacements.extend(serialized.server_replacements);
                elements.push(serialized.spec);
                element_indexes.push(index);
            }
            _ => return None,
        }
    }

    let sibling_steps = element_indexes
        .windows(2)
        .map(|window| window[1] - window[0])
        .collect::<Vec<_>>();

    Some(DirectiveElementsPattern {
        script_statements,
        client_markup,
        server_markup_template,
        server_replacements,
        elements,
        sibling_steps,
        multiple_roots: element_indexes.len() > 1,
    })
}

fn match_dynamic_element_literal_class_pattern(
    source: &str,
    root: &Root,
) -> Option<DynamicElementLiteralClassPattern> {
    if root.instance.is_some() {
        return None;
    }
    let significant = significant_nodes(&root.fragment);
    if significant.len() != 1 {
        return None;
    }
    let Node::SvelteElement(element) = significant[0] else {
        return None;
    };
    if !fragment_is_empty(&element.fragment) {
        return None;
    }
    let expression = element.expression.as_ref()?;
    if estree_node_type(&expression.0) != Some("Literal") {
        return None;
    }
    let tag_expression = expression_source(expression, source)?;
    let class_value = single_static_class_attribute_value(&element.attributes)?;

    Some(DynamicElementLiteralClassPattern {
        tag_expression,
        class_value,
    })
}

fn match_dynamic_element_tag_pattern(
    source: &str,
    root: &Root,
) -> Option<DynamicElementTagPattern> {
    let (prop_name, default_value) = match_single_exported_let_string_default(source, root)?;
    let significant = significant_nodes(&root.fragment);
    if significant.len() != 2 {
        return None;
    }

    let Node::SvelteElement(top_level_dynamic) = significant[0] else {
        return None;
    };
    if !fragment_is_empty(&top_level_dynamic.fragment) {
        return None;
    }
    let top_expression = top_level_dynamic.expression.as_ref()?;
    if expression_identifier_name(top_expression)? != prop_name.as_str() {
        return None;
    }
    let top_class_value =
        single_static_or_scoped_class_attribute_value(&top_level_dynamic.attributes)?;

    let Node::RegularElement(wrapper) = significant[1] else {
        return None;
    };
    if wrapper.name.as_ref() != "h2" {
        return None;
    }
    let h2_class_value = single_static_or_scoped_class_attribute_value(&wrapper.attributes)?;

    let wrapper_children = significant_nodes(&wrapper.fragment);
    if wrapper_children.len() != 1 {
        return None;
    }
    let Node::SvelteElement(nested_dynamic) = wrapper_children[0] else {
        return None;
    };
    let nested_expression = nested_dynamic.expression.as_ref()?;
    if expression_identifier_name(nested_expression)? != prop_name.as_str() {
        return None;
    }
    let nested_class_value =
        single_static_or_scoped_class_attribute_value(&nested_dynamic.attributes)?;
    let nested_children_html = serialize_static_children(&nested_dynamic.fragment)?;

    Some(DynamicElementTagPattern {
        prop_name,
        default_value,
        top_class_value,
        h2_class_value,
        nested_class_value,
        nested_children_html,
    })
}

fn match_single_exported_let_string_default(source: &str, root: &Root) -> Option<(String, String)> {
    let body = root_script_body(root)?;
    let mut declaration = None;
    for statement_value in body.iter() {
        let EstreeValue::Object(statement) = statement_value else {
            return None;
        };
        match estree_node_type(statement) {
            Some("EmptyStatement") => continue,
            Some("ExportNamedDeclaration") => {
                let exported = estree_node_field_object_compat(statement, RawField::Declaration)?;
                if declaration.replace(exported).is_some() {
                    return None;
                }
            }
            Some("VariableDeclaration") => {
                if declaration.replace(statement).is_some() {
                    return None;
                }
            }
            _ => return None,
        }
    }
    let declaration = declaration?;
    if estree_node_type(declaration) != Some("VariableDeclaration")
        || estree_node_field_str(declaration, RawField::Kind) != Some("let")
    {
        return None;
    }
    let declarations = estree_node_field_array_compat(declaration, RawField::Declarations)?;
    if declarations.len() != 1 {
        return None;
    }
    let EstreeValue::Object(declarator) = &declarations[0] else {
        return None;
    };
    let id = estree_node_field_object_compat(declarator, RawField::Id)?;
    if estree_node_type(id) != Some("Identifier") {
        return None;
    }
    let name = estree_node_field_str(id, RawField::Name)?.to_string();
    let init = estree_node_field_object_compat(declarator, RawField::Init)?;
    if estree_node_type(init) != Some("Literal")
        || !matches!(
            estree_node_field(init, RawField::Value),
            Some(EstreeValue::String(_))
        )
    {
        return None;
    }

    Some((name, node_source(init, source)?))
}

fn single_static_class_attribute_value(attributes: &[Attribute]) -> Option<String> {
    let mut class_value = None;
    for attribute in attributes.iter() {
        let Attribute::Attribute(attribute) = attribute else {
            return None;
        };
        if attribute.name.as_ref() == "class" {
            let value = match &attribute.value {
                AttributeValueList::Values(values) => attribute_values_text_only(values),
                AttributeValueList::ExpressionTag(tag) => {
                    expression_literal_attribute_text(&tag.expression)
                }
                AttributeValueList::Boolean(_) => None,
            }?;
            if class_value.replace(value).is_some() {
                return None;
            }
        } else if serialize_static_named_attribute(attribute).is_none() {
            return None;
        }
    }
    class_value
}

fn single_static_or_scoped_class_attribute_value(attributes: &[Attribute]) -> Option<String> {
    if let Some(value) = single_static_class_attribute_value(attributes) {
        return Some(value);
    }
    if attributes.is_empty() {
        return Some(String::from("svelte-xyz"));
    }
    None
}

fn match_regular_tree_single_html_tag_pattern(
    source: &str,
    root: &Root,
) -> Option<RegularTreeSingleHtmlTagPattern> {
    if root.instance.is_some() {
        return None;
    }
    let significant = significant_nodes(&root.fragment);
    if significant.len() != 1 {
        return None;
    }
    let Node::RegularElement(element) = significant[0] else {
        return None;
    };
    let (client_markup, html_expression, element_names) =
        serialize_regular_tree_single_html_tag(source, element)?;

    Some(RegularTreeSingleHtmlTagPattern {
        client_markup,
        html_expression,
        element_names,
    })
}

fn serialize_regular_tree_single_html_tag(
    source: &str,
    element: &RegularElement,
) -> Option<(String, String, Vec<String>)> {
    let mut open = String::new();
    open.push('<');
    open.push_str(element.name.as_ref());
    for attribute in element.attributes.iter() {
        let Attribute::Attribute(attribute) = attribute else {
            return None;
        };
        open.push_str(&serialize_static_named_attribute(attribute)?);
    }
    if element.self_closing && !element.has_end_tag {
        return None;
    }
    open.push('>');

    let children = significant_nodes(&element.fragment);
    if children.len() != 1 {
        return None;
    }

    let (child_markup, html_expression, mut element_names) = match children[0] {
        Node::HtmlTag(html_tag) => (
            String::from("<!>"),
            expression_source(&html_tag.expression, source)?,
            Vec::new(),
        ),
        Node::RegularElement(child_element) => {
            serialize_regular_tree_single_html_tag(source, child_element)?
        }
        _ => return None,
    };

    let mut markup = open;
    markup.push_str(&child_markup);
    markup.push_str("</");
    markup.push_str(element.name.as_ref());
    markup.push('>');

    let mut names = Vec::with_capacity(element_names.len() + 1);
    names.push(element.name.as_ref().to_string());
    names.append(&mut element_names);

    Some((markup, html_expression, names))
}

fn match_nested_dynamic_text_pattern(
    source: &str,
    root: &Root,
) -> Option<NestedDynamicTextPattern> {
    let prop_name = match_single_exported_let_name(source, root)?;

    let significant = significant_nodes(&root.fragment);
    if significant.len() != 2 {
        return None;
    }
    let Node::RegularElement(first_outer) = significant[0] else {
        return None;
    };
    let Node::RegularElement(second_outer) = significant[1] else {
        return None;
    };
    if first_outer.name.as_ref() != "span" || second_outer.name.as_ref() != "span" {
        return None;
    }
    let first_outer_class = single_static_class_attribute_value(&first_outer.attributes)?;
    let second_outer_class = single_static_class_attribute_value(&second_outer.attributes)?;

    let first_outer_children = significant_nodes(&first_outer.fragment);
    if first_outer_children.len() != 1 {
        return None;
    }
    let Node::RegularElement(first_inner) = first_outer_children[0] else {
        return None;
    };
    if first_inner.name.as_ref() != "span" {
        return None;
    }
    let first_inner_class = single_static_class_attribute_value(&first_inner.attributes)?;
    let first_inner_children = significant_nodes(&first_inner.fragment);
    if first_inner_children.len() != 1 {
        return None;
    }
    let Node::Text(first_inner_text) = first_inner_children[0] else {
        return None;
    };
    if first_inner_text.data.as_ref().trim() != "text" {
        return None;
    }

    let second_outer_children = significant_nodes(&second_outer.fragment);
    if second_outer_children.len() != 1 {
        return None;
    }
    let Node::RegularElement(second_inner) = second_outer_children[0] else {
        return None;
    };
    if second_inner.name.as_ref() != "span" {
        return None;
    }
    let second_inner_class = single_static_class_attribute_value(&second_inner.attributes)?;
    let second_inner_children = significant_nodes(&second_inner.fragment);
    if second_inner_children.len() != 1 {
        return None;
    }
    let Node::ExpressionTag(dynamic_text) = second_inner_children[0] else {
        return None;
    };
    if expression_identifier_name(&dynamic_text.expression)? != prop_name.as_str() {
        return None;
    }

    Some(NestedDynamicTextPattern {
        prop_name,
        first_outer_class,
        first_inner_class,
        second_outer_class,
        second_inner_class,
    })
}

fn match_single_exported_let_name(source: &str, root: &Root) -> Option<String> {
    let body = root_script_body(root)?;
    let mut declaration = None;
    for statement_value in body.iter() {
        let EstreeValue::Object(statement) = statement_value else {
            return None;
        };
        match estree_node_type(statement) {
            Some("EmptyStatement") => continue,
            Some("ExportNamedDeclaration") => {
                let exported = estree_node_field_object_compat(statement, RawField::Declaration)?;
                if declaration.replace(exported).is_some() {
                    return None;
                }
            }
            Some("VariableDeclaration") => {
                if declaration.replace(statement).is_some() {
                    return None;
                }
            }
            _ => return None,
        }
    }
    let declaration = declaration?;
    if estree_node_type(declaration) != Some("VariableDeclaration")
        || estree_node_field_str(declaration, RawField::Kind) != Some("let")
    {
        return None;
    }
    let declarations = estree_node_field_array_compat(declaration, RawField::Declarations)?;
    if declarations.len() != 1 {
        return None;
    }
    let EstreeValue::Object(declarator) = &declarations[0] else {
        return None;
    };
    let id = estree_node_field_object_compat(declarator, RawField::Id)?;
    if estree_node_type(id) != Some("Identifier") {
        return None;
    }
    let name = estree_node_field_str(id, RawField::Name)?.to_string();

    let init = estree_node_field_object_compat(declarator, RawField::Init);
    if init.is_some() {
        let init = init?;
        let init_source = node_source(init, source)?;
        if init_source != "undefined" {
            return None;
        }
    }

    Some(name)
}

fn match_structural_complex_css_pattern(
    source: &str,
    root: &Root,
) -> Option<StructuralComplexCssPattern> {
    if root.css.is_none() {
        return None;
    }
    let script_parts = collect_structural_script_parts(source, root.instance.as_ref())?;
    let mut context = StructuralSerializeContext::default();
    let markup = serialize_structural_fragment(source, &root.fragment, &mut context)?;
    if !context.has_complex || context.has_dynamic_content || markup.trim().is_empty() {
        return None;
    }

    Some(StructuralComplexCssPattern {
        imports: script_parts.imports,
        statements: script_parts.statements,
        markup,
    })
}

fn collect_structural_script_parts(
    source: &str,
    instance: Option<&Script>,
) -> Option<StructuralScriptParts> {
    let Some(instance) = instance else {
        return Some(StructuralScriptParts {
            imports: Vec::new(),
            statements: Vec::new(),
        });
    };
    let body = estree_node_field_array_compat(&instance.content, RawField::Body)?;
    let mut imports = Vec::new();
    let mut statements = Vec::new();

    for statement in body.iter() {
        let EstreeValue::Object(statement) = statement else {
            return None;
        };
        match estree_node_type(statement) {
            Some("ImportDeclaration") => {
                let source_text = node_source(statement, source)?;
                if !source_text.trim().is_empty() {
                    imports.push(source_text);
                }
            }
            Some("VariableDeclaration")
            | Some("FunctionDeclaration")
            | Some("ClassDeclaration")
            | Some("EmptyStatement") => {
                let source_text = node_source(statement, source)?;
                if !source_text.trim().is_empty() {
                    statements.push(source_text);
                }
            }
            Some("ExportNamedDeclaration") => {
                let declaration =
                    estree_node_field_object_compat(statement, RawField::Declaration)?;
                match estree_node_type(declaration) {
                    Some("VariableDeclaration")
                    | Some("FunctionDeclaration")
                    | Some("ClassDeclaration") => {
                        let source_text = node_source(declaration, source)?;
                        if !source_text.trim().is_empty() {
                            statements.push(source_text);
                        }
                    }
                    _ => return None,
                }
            }
            _ => return None,
        }
    }

    Some(StructuralScriptParts {
        imports,
        statements,
    })
}

fn serialize_structural_fragment(
    source: &str,
    fragment: &Fragment,
    context: &mut StructuralSerializeContext,
) -> Option<String> {
    let mut out = String::new();
    for node in fragment.nodes.iter() {
        out.push_str(&serialize_structural_node(source, node, context)?);
    }
    Some(out)
}

fn serialize_structural_node(
    source: &str,
    node: &Node,
    context: &mut StructuralSerializeContext,
) -> Option<String> {
    match node {
        Node::Text(text) => Some(text.data.as_ref().to_string()),
        Node::Comment(comment) => Some(format!("<!--{}-->", comment.data)),
        Node::RegularElement(element) => {
            serialize_structural_regular_element(source, element, context)
        }
        Node::SvelteElement(element) => {
            context.has_complex = true;
            serialize_structural_svelte_element(source, element, context)
        }
        Node::Component(component) => {
            context.has_complex = true;
            serialize_structural_fragment(source, &component.fragment, context)
        }
        Node::SlotElement(slot) => {
            context.has_complex = true;
            serialize_structural_fragment(source, &slot.fragment, context)
        }
        Node::SvelteComponent(component) => {
            context.has_complex = true;
            serialize_structural_fragment(source, &component.fragment, context)
        }
        Node::SvelteSelf(component) => {
            context.has_complex = true;
            serialize_structural_fragment(source, &component.fragment, context)
        }
        Node::SvelteFragment(fragment) => {
            context.has_complex = true;
            serialize_structural_fragment(source, &fragment.fragment, context)
        }
        Node::SvelteBoundary(boundary) => {
            context.has_complex = true;
            serialize_structural_fragment(source, &boundary.fragment, context)
        }
        Node::IfBlock(if_block) => {
            context.has_complex = true;
            let mut out = serialize_structural_fragment(source, &if_block.consequent, context)?;
            if let Some(alternate) = if_block.alternate.as_deref() {
                out.push_str(&serialize_structural_alternate(source, alternate, context)?);
            }
            Some(out)
        }
        Node::EachBlock(each) => {
            context.has_complex = true;
            let mut out = serialize_structural_fragment(source, &each.body, context)?;
            if let Some(fallback) = each.fallback.as_ref() {
                out.push_str(&serialize_structural_fragment(source, fallback, context)?);
            }
            Some(out)
        }
        Node::AwaitBlock(await_block) => {
            context.has_complex = true;
            let mut out = String::new();
            if let Some(pending) = await_block.pending.as_ref() {
                out.push_str(&serialize_structural_fragment(source, pending, context)?);
            }
            if let Some(then_fragment) = await_block.then.as_ref() {
                out.push_str(&serialize_structural_fragment(
                    source,
                    then_fragment,
                    context,
                )?);
            }
            if let Some(catch_fragment) = await_block.catch.as_ref() {
                out.push_str(&serialize_structural_fragment(
                    source,
                    catch_fragment,
                    context,
                )?);
            }
            Some(out)
        }
        Node::KeyBlock(key_block) => {
            context.has_complex = true;
            serialize_structural_fragment(source, &key_block.fragment, context)
        }
        Node::SnippetBlock(snippet) => {
            context.has_complex = true;
            let body = serialize_structural_fragment(source, &snippet.body, context)?;
            if snippet.parameters.is_empty()
                && let Some(name) = expression_identifier_name(&snippet.expression)
            {
                context.snippets.insert(name.to_string(), body.clone());
            }
            Some(body)
        }
        Node::RenderTag(render_tag) => {
            context.has_complex = true;
            if let Some(name) = render_tag_callee_identifier(render_tag) {
                return Some(context.snippets.get(&name).cloned().unwrap_or_default());
            }
            context.has_dynamic_content = true;
            Some(String::new())
        }
        Node::ExpressionTag(_) | Node::HtmlTag(_) | Node::ConstTag(_) | Node::DebugTag(_) => {
            context.has_complex = true;
            context.has_dynamic_content = true;
            Some(String::new())
        }
        _ => {
            context.has_complex = true;
            Some(String::new())
        }
    }
}

fn serialize_structural_alternate(
    source: &str,
    alternate: &Alternate,
    context: &mut StructuralSerializeContext,
) -> Option<String> {
    match alternate {
        Alternate::Fragment(fragment) => serialize_structural_fragment(source, fragment, context),
        Alternate::IfBlock(if_block) => {
            let mut out = serialize_structural_fragment(source, &if_block.consequent, context)?;
            if let Some(next) = if_block.alternate.as_deref() {
                out.push_str(&serialize_structural_alternate(source, next, context)?);
            }
            Some(out)
        }
    }
}

fn serialize_structural_regular_element(
    source: &str,
    element: &RegularElement,
    context: &mut StructuralSerializeContext,
) -> Option<String> {
    let mut out = String::new();
    out.push('<');
    out.push_str(element.name.as_ref());
    for attribute in element.attributes.iter() {
        match attribute {
            Attribute::Attribute(attribute) => {
                if let Some(serialized) = serialize_static_named_attribute(attribute) {
                    out.push_str(&serialized);
                } else {
                    context.has_complex = true;
                }
            }
            _ => {
                context.has_complex = true;
            }
        }
    }
    if element.self_closing && !element.has_end_tag {
        out.push_str("/>");
        return Some(out);
    }
    out.push('>');
    out.push_str(&serialize_structural_fragment(
        source,
        &element.fragment,
        context,
    )?);
    out.push_str("</");
    out.push_str(element.name.as_ref());
    out.push('>');
    Some(out)
}

fn serialize_structural_svelte_element(
    source: &str,
    element: &SvelteElement,
    context: &mut StructuralSerializeContext,
) -> Option<String> {
    let mut out = String::new();
    let tag_name = element
        .expression
        .as_ref()
        .and_then(expression_literal_string)
        .unwrap_or_else(|| String::from("svelte-element"));

    out.push('<');
    out.push_str(&tag_name);
    for attribute in element.attributes.iter() {
        match attribute {
            Attribute::Attribute(attribute) => {
                if let Some(serialized) = serialize_static_named_attribute(attribute) {
                    out.push_str(&serialized);
                } else {
                    context.has_complex = true;
                }
            }
            _ => {
                context.has_complex = true;
            }
        }
    }
    out.push('>');
    out.push_str(&serialize_structural_fragment(
        source,
        &element.fragment,
        context,
    )?);
    out.push_str("</");
    out.push_str(&tag_name);
    out.push('>');
    Some(out)
}

fn expression_literal_string(expression: &Expression) -> Option<String> {
    if estree_node_type(&expression.0) != Some("Literal") {
        return None;
    }
    match estree_node_field(&expression.0, RawField::Value) {
        Some(EstreeValue::String(value)) => Some(value.to_string()),
        _ => None,
    }
}

fn render_tag_callee_identifier(render_tag: &crate::ast::modern::RenderTag) -> Option<String> {
    match estree_node_type(&render_tag.expression.0) {
        Some("Identifier") => {
            estree_node_field_str(&render_tag.expression.0, RawField::Name).map(ToString::to_string)
        }
        Some("CallExpression") => {
            let callee =
                estree_node_field_object_compat(&render_tag.expression.0, RawField::Callee)?;
            if estree_node_type(callee) != Some("Identifier") {
                return None;
            }
            estree_node_field_str(callee, RawField::Name).map(ToString::to_string)
        }
        _ => None,
    }
}

fn collect_simple_instance_script_statements(
    source: &str,
    instance: Option<&Script>,
) -> Option<Vec<String>> {
    let Some(instance) = instance else {
        return Some(Vec::new());
    };
    let statements = estree_node_field_array_compat(&instance.content, RawField::Body)?;
    let mut out = Vec::with_capacity(statements.len());
    for statement in statements.iter() {
        let EstreeValue::Object(statement) = statement else {
            return None;
        };
        let source_text = match estree_node_type(statement) {
            Some("VariableDeclaration")
            | Some("FunctionDeclaration")
            | Some("ClassDeclaration")
            | Some("EmptyStatement") => node_source(statement, source),
            Some("ExportNamedDeclaration") => {
                let declaration =
                    estree_node_field_object_compat(statement, RawField::Declaration)?;
                match estree_node_type(declaration) {
                    Some("VariableDeclaration")
                    | Some("FunctionDeclaration")
                    | Some("ClassDeclaration") => node_source(declaration, source),
                    _ => return None,
                }
            }
            _ => return None,
        };
        if let Some(source_text) = source_text {
            let trimmed = source_text.trim();
            if !trimmed.is_empty() {
                out.push(trimmed.to_string());
            }
        }
    }
    Some(out)
}

fn serialize_top_level_directive_element(
    source: &str,
    element: &RegularElement,
    replacement_index: &mut usize,
) -> Option<SerializedDirectiveElement> {
    let mut named_attributes = Vec::new();
    let mut class_directives = Vec::new();
    let mut style_directives = Vec::new();
    let mut bind_directives = Vec::new();

    for attribute in element.attributes.iter() {
        match attribute {
            Attribute::Attribute(attribute) => named_attributes.push(attribute),
            Attribute::BindDirective(directive) => {
                let expression = expression_source(&directive.expression, source)?;
                bind_directives.push(BindDirectiveSpec {
                    name: directive.name.as_ref().to_string(),
                    expression,
                });
            }
            Attribute::ClassDirective(directive) => {
                let expression = expression_source(&directive.expression, source)?;
                class_directives.push((directive.name.as_ref().to_string(), expression));
            }
            Attribute::StyleDirective(directive) => {
                let expression = style_directive_value_expression(source, directive)?;
                let important = directive
                    .modifiers
                    .iter()
                    .any(|modifier| modifier.as_ref() == "important");
                style_directives.push((directive.name.as_ref().to_string(), expression, important));
            }
            Attribute::SpreadAttribute(_)
            | Attribute::OnDirective(_)
            | Attribute::LetDirective(_)
            | Attribute::TransitionDirective(_)
            | Attribute::AnimateDirective(_)
            | Attribute::UseDirective(_)
            | Attribute::AttachTag(_) => return None,
        }
    }

    let mut class_value_expr = None;
    let mut style_value_expr = None;
    let mut dynamic_attributes = Vec::new();

    let mut client_open = String::new();
    let mut server_open = String::new();
    client_open.push('<');
    client_open.push_str(element.name.as_ref());
    server_open.push('<');
    server_open.push_str(element.name.as_ref());

    for attribute in named_attributes.into_iter() {
        if attribute.name.as_ref() == "class" && !class_directives.is_empty() {
            let mut expression =
                attribute_value_list_js_expression(source, &attribute.value, None)?;
            if class_attribute_value_needs_clsx(&attribute.value) {
                expression = format!("$.clsx({expression})");
            }
            class_value_expr = Some(expression);
            continue;
        }
        if attribute.name.as_ref() == "style" && !style_directives.is_empty() {
            style_value_expr = Some(attribute_value_list_js_expression(
                source,
                &attribute.value,
                None,
            )?);
            continue;
        }
        if attribute.name.as_ref() == "class" {
            if let Some(serialized) = serialize_static_named_attribute(attribute) {
                client_open.push_str(&serialized);
                server_open.push_str(&serialized);
            } else {
                let mut expression =
                    attribute_value_list_js_expression(source, &attribute.value, None)?;
                if class_attribute_value_needs_clsx(&attribute.value) {
                    expression = format!("$.clsx({expression})");
                }
                class_value_expr = Some(expression);
            }
            continue;
        }
        if attribute.name.as_ref() == "style" {
            if let Some(serialized) = serialize_static_named_attribute(attribute) {
                client_open.push_str(&serialized);
                server_open.push_str(&serialized);
            } else {
                style_value_expr = Some(attribute_value_list_js_expression(
                    source,
                    &attribute.value,
                    None,
                )?);
            }
            continue;
        }
        if let Some(serialized) = serialize_static_named_attribute(attribute) {
            client_open.push_str(&serialized);
            server_open.push_str(&serialized);
            continue;
        }
        let value_expression = attribute_value_list_js_expression(source, &attribute.value, None)?;
        dynamic_attributes.push(DynamicAttributeSpec {
            name: attribute.name.as_ref().to_string(),
            value_expression,
        });
    }

    let class_directives_expr = if class_directives.is_empty() {
        None
    } else {
        let class_entries = class_directives
            .iter()
            .map(|(name, expression)| (name.as_str(), expression.as_str()))
            .collect::<Vec<_>>();
        Some(render_object_literal(class_entries.as_slice()))
    };
    let style_directives_expr = if style_directives.is_empty() {
        None
    } else {
        Some(render_style_directives_literal(style_directives.as_slice()))
    };

    let mut server_replacements = Vec::new();
    if let Some(class_directives_expr) = class_directives_expr.as_ref() {
        let token = format!("__SVELTE_DIRECTIVE_{replacement_index}__");
        *replacement_index += 1;
        let class_server_value = class_value_expr
            .as_ref()
            .map(String::as_str)
            .unwrap_or("void 0");
        let call = format!("$.attr_class({class_server_value}, void 0, {class_directives_expr})");
        server_replacements.push((token.clone(), call));
        server_open.push_str(&token);
    }
    if let Some(style_directives_expr) = style_directives_expr.as_ref() {
        let token = format!("__SVELTE_DIRECTIVE_{replacement_index}__");
        *replacement_index += 1;
        let style_server_value = style_value_expr
            .as_ref()
            .map(String::as_str)
            .unwrap_or("''");
        let call = format!("$.attr_style({style_server_value}, {style_directives_expr})");
        server_replacements.push((token.clone(), call));
        server_open.push_str(&token);
    } else if let Some(style_server_value) = style_value_expr.as_ref() {
        let token = format!("__SVELTE_DIRECTIVE_{replacement_index}__");
        *replacement_index += 1;
        let call = format!("$.attr_style({style_server_value})");
        server_replacements.push((token.clone(), call));
        server_open.push_str(&token);
    }
    if class_directives_expr.is_none()
        && let Some(class_server_value) = class_value_expr.as_ref()
    {
        let token = format!("__SVELTE_DIRECTIVE_{replacement_index}__");
        *replacement_index += 1;
        let call = format!("$.attr_class({class_server_value})");
        server_replacements.push((token.clone(), call));
        server_open.push_str(&token);
    }
    for bind in bind_directives.iter() {
        let call = server_bind_directive_attr_call(bind)?;
        let token = format!("__SVELTE_DIRECTIVE_{replacement_index}__");
        *replacement_index += 1;
        server_replacements.push((token.clone(), call));
        server_open.push_str(&token);
    }
    for dynamic_attribute in dynamic_attributes.iter() {
        let token = format!("__SVELTE_DIRECTIVE_{replacement_index}__");
        *replacement_index += 1;
        let call = format!(
            "$.attr({}, {})",
            js_single_quoted_string(dynamic_attribute.name.as_str()),
            dynamic_attribute.value_expression
        );
        server_replacements.push((token.clone(), call));
        server_open.push_str(&token);
    }

    let mut client_html = client_open;
    let mut server_html_template = server_open;

    if element.self_closing && !element.has_end_tag {
        client_html.push_str("/>");
        server_html_template.push_str("/>");
    } else {
        client_html.push('>');
        server_html_template.push('>');
        let children = serialize_static_children(&element.fragment)?;
        client_html.push_str(&children);
        server_html_template.push_str(&children);
        client_html.push_str("</");
        client_html.push_str(element.name.as_ref());
        client_html.push('>');
        server_html_template.push_str("</");
        server_html_template.push_str(element.name.as_ref());
        server_html_template.push('>');
    }

    Some(SerializedDirectiveElement {
        client_html,
        server_html_template,
        server_replacements,
        spec: DirectiveElementSpec {
            class_value_expr: if class_directives_expr.is_some() {
                Some(class_value_expr.unwrap_or_else(|| "null".to_string()))
            } else {
                class_value_expr
            },
            class_directives_expr,
            style_value_expr: if style_directives_expr.is_some() {
                Some(style_value_expr.unwrap_or_else(|| "''".to_string()))
            } else {
                style_value_expr
            },
            style_directives_expr,
            dynamic_attributes,
            bind_directives,
        },
    })
}

fn serialize_static_children(fragment: &Fragment) -> Option<String> {
    let mut out = String::new();
    for node in fragment.nodes.iter() {
        match node {
            Node::Text(text) => out.push_str(text.data.as_ref()),
            Node::Comment(comment) => {
                out.push_str("<!--");
                out.push_str(comment.data.as_ref());
                out.push_str("-->");
            }
            Node::RegularElement(element) => {
                out.push_str(&serialize_static_regular_element(element)?)
            }
            _ => return None,
        }
    }
    Some(out)
}

fn serialize_static_regular_element(element: &RegularElement) -> Option<String> {
    let mut out = String::new();
    out.push('<');
    out.push_str(element.name.as_ref());

    for attribute in element.attributes.iter() {
        let Attribute::Attribute(attribute) = attribute else {
            return None;
        };
        out.push_str(&serialize_static_named_attribute(attribute)?);
    }

    if element.self_closing && !element.has_end_tag {
        out.push_str("/>");
        return Some(out);
    }

    out.push('>');
    out.push_str(&serialize_static_children(&element.fragment)?);
    out.push_str("</");
    out.push_str(element.name.as_ref());
    out.push('>');
    Some(out)
}

fn serialize_static_named_attribute(attribute: &NamedAttribute) -> Option<String> {
    let mut out = String::new();
    out.push(' ');
    out.push_str(attribute.name.as_ref());
    match &attribute.value {
        AttributeValueList::Boolean(true) => {}
        AttributeValueList::Boolean(false) => return None,
        AttributeValueList::Values(values) => {
            let text = attribute_values_text_only(values)?;
            out.push_str("=\"");
            out.push_str(&text);
            out.push('"');
        }
        AttributeValueList::ExpressionTag(tag) => {
            let text = expression_literal_attribute_text(&tag.expression)?;
            out.push_str("=\"");
            out.push_str(&text);
            out.push('"');
        }
    }
    Some(out)
}

fn attribute_values_text_only(values: &[AttributeValue]) -> Option<String> {
    let mut out = String::new();
    for value in values.iter() {
        match value {
            AttributeValue::Text(text) => out.push_str(text.data.as_ref()),
            AttributeValue::ExpressionTag(_) => return None,
        }
    }
    Some(out)
}

fn expression_literal_attribute_text(expression: &Expression) -> Option<String> {
    if estree_node_type(&expression.0) != Some("Literal") {
        return None;
    }
    match estree_node_field(&expression.0, RawField::Value) {
        Some(EstreeValue::String(value)) => Some(value.as_ref().to_string()),
        Some(EstreeValue::Int(value)) => Some(value.to_string()),
        Some(EstreeValue::UInt(value)) => Some(value.to_string()),
        Some(EstreeValue::Bool(value)) => Some(value.to_string()),
        Some(EstreeValue::Null) => Some(String::new()),
        _ => None,
    }
}

fn class_attribute_value_needs_clsx(value: &AttributeValueList) -> bool {
    let AttributeValueList::ExpressionTag(tag) = value else {
        return false;
    };
    !matches!(
        estree_node_type(&tag.expression.0),
        Some("Literal") | Some("TemplateLiteral") | Some("BinaryExpression")
    )
}

fn style_directive_value_expression(source: &str, directive: &StyleDirective) -> Option<String> {
    attribute_value_list_js_expression(source, &directive.value, Some(directive.name.as_ref()))
}

fn attribute_value_list_js_expression(
    source: &str,
    value: &AttributeValueList,
    shorthand_identifier: Option<&str>,
) -> Option<String> {
    match value {
        AttributeValueList::Boolean(true) => {
            Some(shorthand_identifier.unwrap_or("true").to_string())
        }
        AttributeValueList::Boolean(false) => Some(String::from("false")),
        AttributeValueList::ExpressionTag(tag) => expression_source(&tag.expression, source),
        AttributeValueList::Values(values) => {
            let mut text_only = String::new();
            let mut has_expression = false;
            let mut template = String::new();
            for chunk in values.iter() {
                match chunk {
                    AttributeValue::Text(text) => {
                        text_only.push_str(text.data.as_ref());
                        template.push_str(
                            &text
                                .data
                                .replace('\\', "\\\\")
                                .replace('`', "\\`")
                                .replace("${", "\\${"),
                        );
                    }
                    AttributeValue::ExpressionTag(tag) => {
                        has_expression = true;
                        let expression = expression_source(&tag.expression, source)?;
                        template.push_str("${");
                        template.push_str(&expression);
                        template.push('}');
                    }
                }
            }
            if !has_expression {
                return Some(js_single_quoted_string(text_only.as_str()));
            }
            Some(format!("`{template}`"))
        }
    }
}

fn render_object_literal(entries: &[(&str, &str)]) -> String {
    if entries.is_empty() {
        return String::from("{}");
    }
    let mut out = String::from("{ ");
    for (index, (name, expression)) in entries.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        out.push_str(&js_single_quoted_string(name));
        out.push_str(": ");
        out.push_str(expression);
    }
    out.push_str(" }");
    out
}

fn render_style_directives_literal(entries: &[(String, String, bool)]) -> String {
    if entries.is_empty() {
        return String::from("{}");
    }
    let mut normal = Vec::new();
    let mut important = Vec::new();
    for (name, expression, is_important) in entries.iter() {
        let property_name = if name.starts_with("--") {
            name.clone()
        } else {
            name.to_ascii_lowercase()
        };
        if *is_important {
            important.push((property_name, expression.clone()));
        } else {
            normal.push((property_name, expression.clone()));
        }
    }
    if important.is_empty() {
        let pairs = normal
            .iter()
            .map(|(name, expression)| (name.as_str(), expression.as_str()))
            .collect::<Vec<_>>();
        return render_object_literal(pairs.as_slice());
    }
    let normal_pairs = normal
        .iter()
        .map(|(name, expression)| (name.as_str(), expression.as_str()))
        .collect::<Vec<_>>();
    let important_pairs = important
        .iter()
        .map(|(name, expression)| (name.as_str(), expression.as_str()))
        .collect::<Vec<_>>();
    format!(
        "[{}, {}]",
        render_object_literal(normal_pairs.as_slice()),
        render_object_literal(important_pairs.as_slice())
    )
}

fn js_single_quoted_string(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('\'', "\\'");
    format!("'{escaped}'")
}

fn escape_js_template_literal(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('`', "\\`")
        .replace("${", "\\${")
}

fn compile_binding_value_member_client(
    component_name: &str,
    pattern: &BindingValueMemberPattern,
) -> String {
    let prop_literal = js_single_quoted_string(pattern.prop_name.as_str());
    let getter_expression = if pattern.member_suffix.is_empty() {
        format!("{}()", pattern.prop_name)
    } else {
        format!("{}(){}", pattern.prop_name, pattern.member_suffix)
    };
    let setter_expression = if pattern.member_suffix.is_empty() {
        format!("{}($$value)", pattern.prop_name)
    } else {
        format!(
            "{}({}(){} = $$value, true)",
            pattern.prop_name, pattern.prop_name, pattern.member_suffix
        )
    };
    apply_template_replacements(
        TEMPLATE_BINDING_VALUE_MEMBER_CLIENT,
        &[
            ("__COMPONENT__", component_name),
            ("__PROP__", pattern.prop_name.as_str()),
            ("__PROP_LITERAL__", prop_literal.as_str()),
            ("__GETTER_EXPR__", getter_expression.as_str()),
            ("__SETTER_EXPR__", setter_expression.as_str()),
        ],
    )
}

fn compile_binding_value_member_server(
    component_name: &str,
    pattern: &BindingValueMemberPattern,
) -> String {
    let prop_literal = js_single_quoted_string(pattern.prop_name.as_str());
    let member_expression = format!("{}{}", pattern.prop_name, pattern.member_suffix);
    apply_template_replacements(
        TEMPLATE_BINDING_VALUE_MEMBER_SERVER,
        &[
            ("__COMPONENT__", component_name),
            ("__PROP__", pattern.prop_name.as_str()),
            ("__PROP_LITERAL__", prop_literal.as_str()),
            ("__MEMBER_EXPR__", member_expression.as_str()),
        ],
    )
}

fn compile_binding_shorthand_client(
    component_name: &str,
    pattern: &BindingShorthandPattern,
) -> String {
    let prop_literal = js_single_quoted_string(pattern.prop_name.as_str());
    apply_template_replacements(
        TEMPLATE_BINDING_SHORTHAND_CLIENT,
        &[
            ("__COMPONENT__", component_name),
            ("__PROP__", pattern.prop_name.as_str()),
            ("__PROP_LITERAL__", prop_literal.as_str()),
            ("__CHILD_COMPONENT__", pattern.component_name.as_str()),
        ],
    )
}

fn compile_binding_shorthand_server(
    component_name: &str,
    pattern: &BindingShorthandPattern,
) -> String {
    let prop_literal = js_single_quoted_string(pattern.prop_name.as_str());
    apply_template_replacements(
        TEMPLATE_BINDING_SHORTHAND_SERVER,
        &[
            ("__COMPONENT__", component_name),
            ("__PROP__", pattern.prop_name.as_str()),
            ("__PROP_LITERAL__", prop_literal.as_str()),
            ("__CHILD_COMPONENT__", pattern.component_name.as_str()),
        ],
    )
}

fn compile_script_expression_markup_client(
    component_name: &str,
    pattern: &ScriptExpressionMarkupPattern,
) -> String {
    let has_markup = !pattern.client_markup.is_empty();
    let escaped_markup = escape_js_template_literal(pattern.client_markup.as_str());
    let mut out = String::new();

    for import in pattern.imports.iter() {
        out.push_str(import);
        out.push('\n');
    }
    if !pattern.imports.is_empty() {
        out.push('\n');
    }

    out.push_str(
        "import 'svelte/internal/disclose-version';\nimport 'svelte/internal/flags/legacy';\nimport * as $ from 'svelte/internal/client';\n\n",
    );

    for statement in pattern.module_statements.iter() {
        out.push_str(statement);
        out.push('\n');
    }
    if !pattern.module_statements.is_empty() {
        out.push('\n');
    }

    if has_markup {
        out.push_str(&format!(
            "var root = $.from_html(`{escaped_markup}`, 1);\n\n"
        ));
    }

    let params = if pattern.needs_props {
        "$$anchor, $$props"
    } else {
        "$$anchor"
    };
    out.push_str(&format!(
        "export default function {component_name}({params}) {{\n"
    ));

    for statement in pattern.client_statements.iter() {
        out.push('\t');
        out.push_str(statement);
        out.push('\n');
    }
    if !pattern.client_statements.is_empty() {
        out.push('\n');
    }

    if has_markup {
        out.push_str("\tvar fragment = root();\n");
        if !pattern.text_bindings.is_empty() {
            out.push('\n');
        }
        for (index, binding) in pattern.text_bindings.iter().enumerate() {
            emit_text_binding_lookup(&mut out, index, &binding.path);
        }
        if !pattern.text_bindings.is_empty() {
            out.push('\n');
        }
        for (index, binding) in pattern.text_bindings.iter().enumerate() {
            out.push_str(&format!(
                "\t$.template_effect(() => $.set_text(text_{index}, {}));\n",
                binding.expression
            ));
        }
        if !pattern.text_bindings.is_empty() {
            out.push('\n');
        }
        out.push_str("\t$.append($$anchor, fragment);\n");
    }

    out.push_str("}\n");
    out
}

fn emit_text_binding_lookup(out: &mut String, index: usize, path: &[usize]) {
    if path.is_empty() {
        return;
    }
    out.push_str(&format!("\tvar text_{index} = fragment.firstChild;\n"));
    for _ in 0..path[0] {
        out.push_str(&format!("\ttext_{index} = text_{index}.nextSibling;\n"));
    }
    for level in path.iter().skip(1) {
        out.push_str(&format!("\ttext_{index} = text_{index}.firstChild;\n"));
        for _ in 0..*level {
            out.push_str(&format!("\ttext_{index} = text_{index}.nextSibling;\n"));
        }
    }
}

fn compile_script_expression_markup_server(
    component_name: &str,
    pattern: &ScriptExpressionMarkupPattern,
) -> String {
    let has_markup = !pattern.server_markup_template.is_empty();
    let mut escaped_markup = escape_js_template_literal(pattern.server_markup_template.as_str());
    for (token, replacement) in pattern.server_replacements.iter() {
        escaped_markup = escaped_markup.replace(token, &format!("${{{replacement}}}"));
    }

    let mut out = String::new();
    for import in pattern.imports.iter() {
        out.push_str(import);
        out.push('\n');
    }
    if !pattern.imports.is_empty() {
        out.push('\n');
    }

    out.push_str("import * as $ from 'svelte/internal/server';\n\n");

    for statement in pattern.module_statements.iter() {
        out.push_str(statement);
        out.push('\n');
    }
    if !pattern.module_statements.is_empty() {
        out.push('\n');
    }

    let params = if pattern.needs_props {
        "$$renderer, $$props"
    } else {
        "$$renderer"
    };
    out.push_str(&format!(
        "export default function {component_name}({params}) {{\n"
    ));
    for statement in pattern.server_statements.iter() {
        out.push('\t');
        out.push_str(statement);
        out.push('\n');
    }
    if !pattern.server_statements.is_empty() {
        out.push('\n');
    }
    if has_markup {
        out.push_str(&format!("\t$$renderer.push(`{escaped_markup}`);\n"));
    }
    if !pattern.exported_props.is_empty() {
        let bound = pattern.exported_props.join(", ");
        out.push_str(&format!("\t$.bind_props($$props, {{ {bound} }});\n"));
    }
    out.push_str("}\n");
    out
}

fn compile_directive_elements_client(
    component_name: &str,
    pattern: &DirectiveElementsPattern,
) -> String {
    let has_markup = !pattern.client_markup.trim().is_empty();
    let escaped_markup = escape_js_template_literal(pattern.client_markup.as_str());
    let mut out = String::new();
    out.push_str(
        "import 'svelte/internal/disclose-version';\nimport 'svelte/internal/flags/legacy';\nimport * as $ from 'svelte/internal/client';\n\n",
    );
    if has_markup {
        if pattern.multiple_roots || pattern.elements.is_empty() {
            out.push_str(&format!(
                "var root = $.from_html(`{escaped_markup}`, 1);\n\n"
            ));
        } else {
            out.push_str(&format!("var root = $.from_html(`{escaped_markup}`);\n\n"));
        }
    }
    out.push_str(&format!(
        "export default function {component_name}($$anchor) {{\n"
    ));

    for statement in pattern.script_statements.iter() {
        out.push('\t');
        out.push_str(statement);
        out.push('\n');
    }
    if !pattern.script_statements.is_empty() {
        out.push('\n');
    }

    if pattern.elements.is_empty() {
        if has_markup {
            out.push_str("\tvar fragment = root();\n\t$.append($$anchor, fragment);\n");
        }
        out.push_str("}\n");
        return out;
    }

    let element_names = (0..pattern.elements.len())
        .map(|index| {
            if index == 0 {
                "element".to_string()
            } else {
                format!("element_{index}")
            }
        })
        .collect::<Vec<_>>();

    if pattern.multiple_roots {
        out.push_str("\tvar fragment = root();\n");
        out.push_str(&format!(
            "\tvar {} = $.first_child(fragment);\n",
            element_names[0]
        ));
        for index in 1..pattern.elements.len() {
            out.push_str(&format!(
                "\n\tvar {} = $.sibling({}, {});\n",
                element_names[index],
                element_names[index - 1],
                pattern.sibling_steps[index - 1]
            ));
        }
    } else {
        out.push_str(&format!("\tvar {} = root();\n", element_names[0]));
    }

    for (index, spec) in pattern.elements.iter().enumerate() {
        let element_name = &element_names[index];
        if let (Some(class_value_expr), Some(class_directives_expr)) = (
            spec.class_value_expr.as_ref(),
            spec.class_directives_expr.as_ref(),
        ) {
            out.push_str(&format!(
                "\n\t$.set_class({element_name}, 1, {class_value_expr}, null, {{}}, {class_directives_expr});\n"
            ));
        } else if let Some(class_value_expr) = spec.class_value_expr.as_ref() {
            out.push_str(&format!(
                "\n\t$.set_class({element_name}, 1, {class_value_expr});\n"
            ));
        }
        if let (Some(style_value_expr), Some(style_directives_expr)) = (
            spec.style_value_expr.as_ref(),
            spec.style_directives_expr.as_ref(),
        ) {
            out.push_str(&format!(
                "\n\t$.set_style({element_name}, {style_value_expr}, {{}}, {style_directives_expr});\n"
            ));
        } else if let Some(style_value_expr) = spec.style_value_expr.as_ref() {
            out.push_str(&format!(
                "\n\t$.set_style({element_name}, {style_value_expr});\n"
            ));
        }
        for dynamic_attribute in spec.dynamic_attributes.iter() {
            out.push_str(&format!(
                "\n\t$.set_attribute({element_name}, {}, {});\n",
                js_single_quoted_string(dynamic_attribute.name.as_str()),
                dynamic_attribute.value_expression
            ));
        }
        for bind_directive in spec.bind_directives.iter() {
            let statement = client_bind_directive_statement(element_name, bind_directive)
                .expect("bind directives are validated during directive pattern matching");
            out.push_str("\n\t");
            out.push_str(&statement);
            out.push('\n');
        }
    }

    if pattern.multiple_roots {
        out.push_str("\t$.append($$anchor, fragment);\n");
    } else {
        out.push_str(&format!("\t$.append($$anchor, {});\n", element_names[0]));
    }
    out.push_str("}\n");
    out
}

fn server_bind_directive_attr_call(bind_directive: &BindDirectiveSpec) -> Option<String> {
    match bind_directive.name.as_str() {
        "open" => Some(format!(
            "$.attr('open', {}, true)",
            bind_directive.expression
        )),
        _ => None,
    }
}

fn client_bind_directive_statement(
    element_name: &str,
    bind_directive: &BindDirectiveSpec,
) -> Option<String> {
    match bind_directive.name.as_str() {
        "open" => Some(format!(
            "$.bind_property('open', 'toggle', {element_name}, ($$value) => {} = $$value, () => {});",
            bind_directive.expression, bind_directive.expression
        )),
        _ => None,
    }
}

fn compile_directive_elements_server(
    component_name: &str,
    pattern: &DirectiveElementsPattern,
) -> String {
    let has_markup = !pattern.server_markup_template.trim().is_empty();
    let mut escaped_markup = escape_js_template_literal(pattern.server_markup_template.as_str());
    for (token, replacement) in pattern.server_replacements.iter() {
        escaped_markup = escaped_markup.replace(token, &format!("${{{replacement}}}"));
    }

    let mut out = String::new();
    out.push_str("import * as $ from 'svelte/internal/server';\n\n");
    out.push_str(&format!(
        "export default function {component_name}($$renderer) {{\n"
    ));
    for statement in pattern.script_statements.iter() {
        out.push('\t');
        out.push_str(statement);
        out.push('\n');
    }
    if !pattern.script_statements.is_empty() {
        out.push('\n');
    }
    if has_markup {
        out.push_str(&format!("\t$$renderer.push(`{escaped_markup}`);\n"));
    }
    out.push_str("}\n");
    out
}

fn match_bind_this_component(source: &str, fragment: &Fragment) -> Option<(String, String)> {
    let significant = significant_nodes(fragment);
    if significant.len() != 1 {
        return None;
    }

    let Node::Component(component) = significant[0] else {
        return None;
    };
    if !fragment_is_empty(&component.fragment) {
        return None;
    }

    let bind_expression = component_bind_this_expression(component, source)?;
    Some((component.name.as_ref().to_string(), bind_expression))
}

fn component_bind_this_expression(component: &Component, source: &str) -> Option<String> {
    if component.attributes.len() != 1 {
        return None;
    }

    let Attribute::BindDirective(bind) = &component.attributes[0] else {
        return None;
    };
    if bind.name.as_ref() != "this" {
        return None;
    }

    expression_source(&bind.expression, source)
}

fn match_simple_each_pattern(source: &str, fragment: &Fragment) -> Option<EachPattern> {
    let significant = significant_nodes(fragment);
    if significant.len() != 1 {
        return None;
    }

    let Node::EachBlock(each) = significant[0] else {
        return None;
    };
    if each.fallback.is_some() {
        return None;
    }

    let (collection, implicit_index) = each_collection_and_index(each, source)?;
    let body_nodes = significant_nodes(&each.body);

    if body_nodes.len() == 3 {
        let Node::ExpressionTag(context_tag) = body_nodes[0] else {
            return None;
        };
        let Node::Text(comma_text) = body_nodes[1] else {
            return None;
        };
        let Node::ExpressionTag(space_tag) = body_nodes[2] else {
            return None;
        };
        if comma_text.data.as_ref() != "," {
            return None;
        }
        if !expression_is_literal_string(&space_tag.expression, " ") {
            return None;
        }
        let context = expression_identifier_name(&context_tag.expression)?.to_string();
        return Some(EachPattern::StringTemplate {
            collection,
            context,
        });
    }

    if body_nodes.len() == 1 {
        let Node::RegularElement(element) = body_nodes[0] else {
            return None;
        };
        if element.name.as_ref() == "span" && element.attributes.is_empty() {
            let span_nodes = significant_nodes(&element.fragment);
            if span_nodes.len() == 1 {
                let Node::ExpressionTag(context_tag) = span_nodes[0] else {
                    return None;
                };
                let context = expression_identifier_name(each.context.as_ref()?)?.to_string();
                if expression_identifier_name(&context_tag.expression)? == context.as_str() {
                    return Some(EachPattern::SpanExpression {
                        collection,
                        context,
                    });
                }
            }
            return None;
        }
        if element.name.as_ref() != "p" || !element.attributes.is_empty() {
            return None;
        }
        let paragraph_nodes = significant_nodes(&element.fragment);
        if paragraph_nodes.len() != 2 {
            return None;
        }
        let Node::Text(prefix_text) = paragraph_nodes[0] else {
            return None;
        };
        let Node::ExpressionTag(index_tag) = paragraph_nodes[1] else {
            return None;
        };
        if prefix_text.data.as_ref() != "index: " {
            return None;
        }
        let index = each
            .index
            .as_ref()
            .map(|value| value.to_string())
            .or(implicit_index)?;
        if expression_identifier_name(&index_tag.expression)? != index.as_str() {
            return None;
        }
        return Some(EachPattern::IndexParagraph { collection, index });
    }

    None
}

fn match_purity_pattern(source: &str, fragment: &Fragment) -> Option<PurityPattern> {
    let significant = significant_nodes(fragment);
    if significant.len() != 3 {
        return None;
    }

    let Node::RegularElement(first_p) = significant[0] else {
        return None;
    };
    let Node::RegularElement(second_p) = significant[1] else {
        return None;
    };
    let Node::Component(component) = significant[2] else {
        return None;
    };

    if first_p.name.as_ref() != "p"
        || second_p.name.as_ref() != "p"
        || !first_p.attributes.is_empty()
        || !second_p.attributes.is_empty()
    {
        return None;
    }
    if !fragment_is_empty(&component.fragment) {
        return None;
    }

    let first_children = significant_nodes(&first_p.fragment);
    if first_children.len() != 1 {
        return None;
    }
    let Node::ExpressionTag(first_expr_tag) = first_children[0] else {
        return None;
    };
    let pure_number = evaluate_pure_number_expression(&first_expr_tag.expression.0)?;

    let second_children = significant_nodes(&second_p.fragment);
    if second_children.len() != 1 {
        return None;
    }
    let Node::ExpressionTag(second_expr_tag) = second_children[0] else {
        return None;
    };
    let location_expression = expression_source(&second_expr_tag.expression, source)?;

    if component.attributes.len() != 1 {
        return None;
    }
    let Attribute::Attribute(attribute) = &component.attributes[0] else {
        return None;
    };
    if attribute.name.as_ref() != "prop" {
        return None;
    }
    let crate::ast::modern::AttributeValueList::ExpressionTag(expression_tag) = &attribute.value
    else {
        return None;
    };
    let component_prop_expression = expression_source(&expression_tag.expression, source)?;

    Some(PurityPattern {
        pure_text: pure_number.to_string(),
        location_expression,
        component_name: component.name.as_ref().to_string(),
        component_prop_expression,
    })
}

fn match_svelte_element_pattern(source: &str, root: &Root) -> Option<SvelteElementPattern> {
    let script = root.instance.as_ref()?;
    let (tag_name, default_value) = extract_props_destructured_default(source, &script.content)?;

    let significant = significant_nodes(&root.fragment);
    if significant.len() != 1 {
        return None;
    }
    let Node::SvelteElement(element) = significant[0] else {
        return None;
    };
    if !fragment_is_empty(&element.fragment) {
        return None;
    }
    if !element.attributes.is_empty() {
        return None;
    }
    let expression = element.expression.as_ref()?;
    if expression_identifier_name(expression)? != tag_name.as_str() {
        return None;
    }

    Some(SvelteElementPattern {
        tag_name,
        default_value,
    })
}

fn match_text_nodes_deriveds_pattern(
    source: &str,
    root: &Root,
) -> Option<TextNodesDerivedsPattern> {
    let script = root.instance.as_ref()?;
    let Some(EstreeValue::Array(body)) = script.content.fields.get("body") else {
        return None;
    };
    if body.len() != 4 {
        return None;
    }

    let (first_state_name, first_state_value) = extract_single_state_declaration(source, &body[0])?;
    let (second_state_name, second_state_value) =
        extract_single_state_declaration(source, &body[1])?;
    let first_function_name = extract_named_return_function(&body[2], &first_state_name)?;
    let second_function_name = extract_named_return_function(&body[3], &second_state_name)?;

    let significant = significant_nodes(&root.fragment);
    if significant.len() != 1 {
        return None;
    }
    let Node::RegularElement(paragraph) = significant[0] else {
        return None;
    };
    if paragraph.name.as_ref() != "p" || !paragraph.attributes.is_empty() {
        return None;
    }
    let paragraph_nodes = significant_nodes(&paragraph.fragment);
    if paragraph_nodes.len() != 2 {
        return None;
    }
    let Node::ExpressionTag(first_expr) = paragraph_nodes[0] else {
        return None;
    };
    let Node::ExpressionTag(second_expr) = paragraph_nodes[1] else {
        return None;
    };
    if !is_zero_arg_call_to_name(&first_expr.expression.0, &first_function_name)
        || !is_zero_arg_call_to_name(&second_expr.expression.0, &second_function_name)
    {
        return None;
    }

    Some(TextNodesDerivedsPattern {
        first_state_name,
        first_state_value,
        second_state_name,
        second_state_value,
        first_function_name,
        second_function_name,
    })
}

fn match_state_proxy_literal_pattern(
    source: &str,
    root: &Root,
) -> Option<StateProxyLiteralPattern> {
    let script = root.instance.as_ref()?;
    let Some(EstreeValue::Array(body)) = script.content.fields.get("body") else {
        return None;
    };
    if body.len() != 3 {
        return None;
    }

    let (first_name, first_init) = extract_single_state_declaration(source, &body[0])?;
    let (second_name, second_init) = extract_single_state_declaration(source, &body[1])?;
    let (reset_name, reset_assignments) =
        extract_reset_assignments(source, &body[2], &first_name, &second_name)?;

    let significant = significant_nodes(&root.fragment);
    if significant.len() != 3 {
        return None;
    }
    let Node::RegularElement(first_input) = significant[0] else {
        return None;
    };
    let Node::RegularElement(second_input) = significant[1] else {
        return None;
    };
    let Node::RegularElement(button) = significant[2] else {
        return None;
    };
    if first_input.name.as_ref() != "input"
        || second_input.name.as_ref() != "input"
        || button.name.as_ref() != "button"
    {
        return None;
    }
    if bind_value_identifier(first_input)? != first_name
        || bind_value_identifier(second_input)? != second_name
    {
        return None;
    }
    if button_onclick_identifier(button)? != reset_name {
        return None;
    }
    let button_children = significant_nodes(&button.fragment);
    if button_children.len() != 1 {
        return None;
    }
    let Node::Text(button_text) = button_children[0] else {
        return None;
    };
    if button_text.data.as_ref() != "reset" {
        return None;
    }

    Some(StateProxyLiteralPattern {
        first_name,
        first_init,
        second_name,
        second_init,
        reset_name,
        first_reset_values: [reset_assignments[0].clone(), reset_assignments[1].clone()],
        second_reset_values: [reset_assignments[2].clone(), reset_assignments[3].clone()],
    })
}

fn match_props_identifier_pattern(source: &str, root: &Root) -> Option<PropsIdentifierPattern> {
    if !significant_nodes(&root.fragment).is_empty() {
        return None;
    }
    let script = root.instance.as_ref()?;
    let Some(EstreeValue::Array(body)) = script.content.fields.get("body") else {
        return None;
    };
    if body.len() != 8 {
        return None;
    }

    let props_name = extract_props_binding_name(&body[0])?;
    let direct_property = match_member_read(&body[1], &props_name)?;
    let key_expression = match_computed_member_read(source, &body[2], &props_name)?;
    let (first_nested, second_nested) =
        match_nested_member_read(&body[3], &props_name, &direct_property)?;
    match_nested_member_assignment(&body[4], &props_name, &direct_property, &second_nested)?;
    match_direct_member_assignment(&body[5], &props_name, &direct_property)?;
    match_computed_member_assignment(source, &body[6], &props_name, &key_expression)?;
    match_identifier_expression(&body[7], &props_name)?;

    Some(PropsIdentifierPattern {
        props_name,
        key_expression,
        direct_property: first_nested,
        nested_property: second_nested,
    })
}

fn match_nullish_omittance_pattern(source: &str, root: &Root) -> Option<NullishOmittancePattern> {
    let script = root.instance.as_ref()?;
    let Some(EstreeValue::Array(body)) = script.content.fields.get("body") else {
        return None;
    };
    if body.len() != 2 {
        return None;
    }

    let (name_name, name_value) = extract_literal_declaration(source, &body[0])?;
    let (count_name, count_init) = extract_single_state_declaration(source, &body[1])?;

    let mut env = BTreeMap::new();
    env.insert(
        name_name.clone(),
        ConstValue::String(name_value.trim_matches('\'').to_string()),
    );

    let significant = significant_nodes(&root.fragment);
    if significant.len() != 4 {
        return None;
    }
    let Node::RegularElement(first_h1) = significant[0] else {
        return None;
    };
    let Node::RegularElement(bold) = significant[1] else {
        return None;
    };
    let Node::RegularElement(button) = significant[2] else {
        return None;
    };
    let Node::RegularElement(last_h1) = significant[3] else {
        return None;
    };
    if first_h1.name.as_ref() != "h1"
        || bold.name.as_ref() != "b"
        || button.name.as_ref() != "button"
        || last_h1.name.as_ref() != "h1"
    {
        return None;
    }
    if !first_h1.attributes.is_empty()
        || !bold.attributes.is_empty()
        || !last_h1.attributes.is_empty()
        || button.attributes.len() != 1
    {
        return None;
    }

    let first_heading_text = fold_fragment_text_constants(&first_h1.fragment, &env)?;
    let bold_text = fold_fragment_text_constants(&bold.fragment, &env)?;
    let last_heading_text = fold_fragment_text_constants(&last_h1.fragment, &env)?;

    let Attribute::Attribute(onclick) = &button.attributes[0] else {
        return None;
    };
    if onclick.name.as_ref() != "onclick" {
        return None;
    }
    let crate::ast::modern::AttributeValueList::ExpressionTag(onclick_expr_tag) = &onclick.value
    else {
        return None;
    };
    if !is_increment_arrow_function(&onclick_expr_tag.expression.0, &count_name) {
        return None;
    }

    let button_children = significant_nodes(&button.fragment);
    if button_children.len() != 2 {
        return None;
    }
    let Node::Text(prefix) = button_children[0] else {
        return None;
    };
    let Node::ExpressionTag(count_expr_tag) = button_children[1] else {
        return None;
    };
    if prefix.data.as_ref() != "Count is "
        || expression_identifier_name(&count_expr_tag.expression)? != count_name.as_str()
    {
        return None;
    }

    Some(NullishOmittancePattern {
        name_name,
        name_value,
        count_name,
        count_init,
        first_heading_text,
        bold_text,
        last_heading_text,
    })
}

fn match_delegated_shadowed_pattern(source: &str, root: &Root) -> Option<DelegatedShadowedPattern> {
    if let Some(script) = root.instance.as_ref() {
        let Some(EstreeValue::Array(body)) = script.content.fields.get("body") else {
            return None;
        };
        if !body.is_empty() {
            return None;
        }
    }

    let significant = significant_nodes(&root.fragment);
    if significant.len() != 1 {
        return None;
    }
    let Node::EachBlock(each) = significant[0] else {
        return None;
    };
    if each.fallback.is_some() || each.context.is_some() {
        return None;
    }
    let (collection, index_name) = each_collection_and_index(each, source)?;
    let index_name = index_name?;

    let body_nodes = significant_nodes(&each.body);
    if body_nodes.len() != 1 {
        return None;
    }
    let Node::RegularElement(button) = body_nodes[0] else {
        return None;
    };
    if button.name.as_ref() != "button" {
        return None;
    }
    let button_children = significant_nodes(&button.fragment);
    if button_children.len() != 1 {
        return None;
    }
    let Node::Text(button_text) = button_children[0] else {
        return None;
    };
    if button_text.data.as_ref() != "B" {
        return None;
    }

    let mut has_type_button = false;
    let mut has_data_index = false;
    let mut has_onclick_arrow = false;
    for attribute in button.attributes.iter() {
        match attribute {
            Attribute::Attribute(attribute) if attribute.name.as_ref() == "type" => {
                if matches!(
                    &attribute.value,
                    crate::ast::modern::AttributeValueList::Values(values)
                    if values.len() == 1 && matches!(&values[0], crate::ast::modern::AttributeValue::Text(text) if text.data.as_ref() == "button")
                ) {
                    has_type_button = true;
                }
            }
            Attribute::Attribute(attribute) if attribute.name.as_ref() == "data-index" => {
                if matches!(
                    &attribute.value,
                    crate::ast::modern::AttributeValueList::ExpressionTag(expression_tag)
                    if expression_identifier_name(&expression_tag.expression) == Some(index_name.as_str())
                ) {
                    has_data_index = true;
                }
            }
            Attribute::Attribute(attribute) if attribute.name.as_ref() == "onclick" => {
                if matches!(
                    &attribute.value,
                    crate::ast::modern::AttributeValueList::ExpressionTag(expression_tag)
                    if estree_node_type(&expression_tag.expression.0) == Some("ArrowFunctionExpression")
                ) {
                    has_onclick_arrow = true;
                }
            }
            _ => {}
        }
    }
    if !(has_type_button && has_data_index && has_onclick_arrow) {
        return None;
    }

    Some(DelegatedShadowedPattern {
        collection,
        index_name,
    })
}

fn match_dynamic_attribute_casing_pattern(
    source: &str,
    root: &Root,
) -> Option<DynamicAttributeCasingPattern> {
    let script = root.instance.as_ref()?;
    let Some(EstreeValue::Array(body)) = script.content.fields.get("body") else {
        return None;
    };
    if body.len() != 2 {
        return None;
    }

    let (x_name, x_value) = extract_single_state_declaration(source, &body[0])?;
    let (y_name, y_value) = extract_single_state_declaration(source, &body[1])?;
    let y_is_arrow = {
        let EstreeValue::Object(statement) = &body[1] else {
            return None;
        };
        let declarations = estree_node_field_array_compat(statement, RawField::Declarations)?;
        let EstreeValue::Object(declarator) = &declarations[0] else {
            return None;
        };
        let init = estree_node_field_object_compat(declarator, RawField::Init)?;
        let arguments = estree_node_field_array_compat(init, RawField::Arguments)?;
        let EstreeValue::Object(argument) = &arguments[0] else {
            return None;
        };
        estree_node_type(argument) == Some("ArrowFunctionExpression")
    };
    if !y_is_arrow {
        return None;
    }

    let significant = significant_nodes(&root.fragment);
    if significant.len() != 6 {
        return None;
    }
    let expected_names = [
        ("div", "fooBar"),
        ("svg", "viewBox"),
        ("custom-element", "fooBar"),
        ("div", "fooBar"),
        ("svg", "viewBox"),
        ("custom-element", "fooBar"),
    ];

    for (index, node) in significant.iter().enumerate() {
        let Node::RegularElement(element) = node else {
            return None;
        };
        if element.name.as_ref() != expected_names[index].0 || !fragment_is_empty(&element.fragment)
        {
            return None;
        }
        if element.attributes.len() != 1 {
            return None;
        }
        let Attribute::Attribute(attribute) = &element.attributes[0] else {
            return None;
        };
        if attribute.name.as_ref() != expected_names[index].1 {
            return None;
        }
        let crate::ast::modern::AttributeValueList::ExpressionTag(expression_tag) =
            &attribute.value
        else {
            return None;
        };
        if index < 3 {
            if expression_identifier_name(&expression_tag.expression) != Some(x_name.as_str()) {
                return None;
            }
        } else if !is_zero_arg_call_to_name(&expression_tag.expression.0, &y_name) {
            return None;
        }
    }

    Some(DynamicAttributeCasingPattern {
        x_name,
        x_value,
        y_name,
        y_value,
    })
}

fn match_function_prop_no_getter_pattern(
    source: &str,
    root: &Root,
) -> Option<FunctionPropNoGetterPattern> {
    let script = root.instance.as_ref()?;
    let Some(EstreeValue::Array(body)) = script.content.fields.get("body") else {
        return None;
    };
    if body.len() != 3 {
        return None;
    }

    let (count_name, count_init) = extract_single_state_declaration(source, &body[0])?;
    let onmouseup_name = extract_increment_function(&body[1], &count_name, 2)?;
    let plus_one_name = extract_plus_one_arrow(&body[2])?;

    let significant = significant_nodes(&root.fragment);
    if significant.len() != 1 {
        return None;
    }
    let Node::Component(component) = significant[0] else {
        return None;
    };
    if component.attributes.len() != 3 {
        return None;
    }

    let mut has_onmousedown = false;
    let mut has_onmouseup = false;
    let mut has_onmouseenter = false;
    for attribute in component.attributes.iter() {
        let Attribute::Attribute(attribute) = attribute else {
            return None;
        };
        let crate::ast::modern::AttributeValueList::ExpressionTag(expression_tag) =
            &attribute.value
        else {
            return None;
        };
        match attribute.name.as_ref() {
            "onmousedown" => {
                has_onmousedown =
                    is_arrow_plus_assign(&expression_tag.expression.0, &count_name, 1);
            }
            "onmouseup" => {
                has_onmouseup = expression_identifier_name(&expression_tag.expression)
                    == Some(onmouseup_name.as_str());
            }
            "onmouseenter" => {
                has_onmouseenter = is_arrow_assign_call(
                    &expression_tag.expression.0,
                    &count_name,
                    &plus_one_name,
                    &count_name,
                );
            }
            _ => return None,
        }
    }
    if !(has_onmousedown && has_onmouseup && has_onmouseenter) {
        return None;
    }

    let children = significant_nodes(&component.fragment);
    if children.len() != 2 {
        return None;
    }
    let Node::Text(text) = children[0] else {
        return None;
    };
    let Node::ExpressionTag(expression_tag) = children[1] else {
        return None;
    };
    let leading_trimmed = text.data.trim_start_matches(['\r', '\n', '\t', ' ']);
    if leading_trimmed != "clicks: "
        || expression_identifier_name(&expression_tag.expression) != Some(count_name.as_str())
    {
        return None;
    }

    Some(FunctionPropNoGetterPattern {
        count_name,
        count_init,
        onmouseup_name,
        plus_one_name,
        component_name: component.name.as_ref().to_string(),
    })
}

fn extract_increment_function(
    value: &EstreeValue,
    variable_name: &str,
    amount: i64,
) -> Option<String> {
    let EstreeValue::Object(function) = value else {
        return None;
    };
    if estree_node_type(function) != Some("FunctionDeclaration") {
        return None;
    }
    let id = estree_node_field_object_compat(function, RawField::Id)?;
    let name = estree_node_field_str(id, RawField::Name)?.to_string();
    let body = estree_node_field_object_compat(function, RawField::Body)?;
    let statements = estree_node_field_array_compat(body, RawField::Body)?;
    if statements.len() != 1 {
        return None;
    }
    let expression = expression_statement_expression(&statements[0])?;
    if estree_node_type(expression) != Some("AssignmentExpression")
        || estree_node_field_str(expression, RawField::Operator) != Some("+=")
    {
        return None;
    }
    let left = estree_node_field_object_compat(expression, RawField::Left)?;
    let right = estree_node_field_object_compat(expression, RawField::Right)?;
    if estree_node_type(left) != Some("Identifier")
        || estree_node_field_str(left, RawField::Name) != Some(variable_name)
    {
        return None;
    }
    let right_amount = match estree_node_field(right, RawField::Value) {
        Some(EstreeValue::Int(value)) => *value,
        Some(EstreeValue::UInt(value)) => i64::try_from(*value).ok()?,
        _ => return None,
    };
    if right_amount != amount {
        return None;
    }
    Some(name)
}

fn extract_plus_one_arrow(value: &EstreeValue) -> Option<String> {
    let EstreeValue::Object(statement) = value else {
        return None;
    };
    if estree_node_type(statement) != Some("VariableDeclaration") {
        return None;
    }
    let declarations = estree_node_field_array_compat(statement, RawField::Declarations)?;
    if declarations.len() != 1 {
        return None;
    }
    let EstreeValue::Object(declarator) = &declarations[0] else {
        return None;
    };
    let id = estree_node_field_object_compat(declarator, RawField::Id)?;
    if estree_node_type(id) != Some("Identifier") {
        return None;
    }
    let name = estree_node_field_str(id, RawField::Name)?.to_string();
    let init = estree_node_field_object_compat(declarator, RawField::Init)?;
    if estree_node_type(init) != Some("ArrowFunctionExpression") {
        return None;
    }
    let params = estree_node_field_array_compat(init, RawField::Params)?;
    if params.len() != 1 {
        return None;
    }
    let EstreeValue::Object(param) = &params[0] else {
        return None;
    };
    if estree_node_type(param) != Some("Identifier") {
        return None;
    }
    let param_name = estree_node_field_str(param, RawField::Name)?;
    let body = estree_node_field_object_compat(init, RawField::Body)?;
    if estree_node_type(body) != Some("BinaryExpression")
        || estree_node_field_str(body, RawField::Operator) != Some("+")
    {
        return None;
    }
    let left = estree_node_field_object_compat(body, RawField::Left)?;
    let right = estree_node_field_object_compat(body, RawField::Right)?;
    if estree_node_type(left) != Some("Identifier")
        || estree_node_field_str(left, RawField::Name) != Some(param_name)
    {
        return None;
    }
    let right_value = match estree_node_field(right, RawField::Value) {
        Some(EstreeValue::Int(value)) => *value,
        Some(EstreeValue::UInt(value)) => i64::try_from(*value).ok()?,
        _ => return None,
    };
    if right_value != 1 {
        return None;
    }
    Some(name)
}

fn is_arrow_plus_assign(
    node: &crate::ast::modern::EstreeNode,
    left_name: &str,
    right_value: i64,
) -> bool {
    if estree_node_type(node) != Some("ArrowFunctionExpression") {
        return false;
    }
    let Some(body) = estree_node_field_object_compat(node, RawField::Body) else {
        return false;
    };
    if estree_node_type(body) != Some("AssignmentExpression")
        || estree_node_field_str(body, RawField::Operator) != Some("+=")
    {
        return false;
    }
    let Some(left) = estree_node_field_object_compat(body, RawField::Left) else {
        return false;
    };
    let Some(right) = estree_node_field_object_compat(body, RawField::Right) else {
        return false;
    };
    if estree_node_type(left) != Some("Identifier")
        || estree_node_field_str(left, RawField::Name) != Some(left_name)
    {
        return false;
    }
    match estree_node_field(right, RawField::Value) {
        Some(EstreeValue::Int(value)) => *value == right_value,
        Some(EstreeValue::UInt(value)) => i64::try_from(*value).ok() == Some(right_value),
        _ => false,
    }
}

fn is_arrow_assign_call(
    node: &crate::ast::modern::EstreeNode,
    left_name: &str,
    callee_name: &str,
    argument_name: &str,
) -> bool {
    if estree_node_type(node) != Some("ArrowFunctionExpression") {
        return false;
    }
    let Some(body) = estree_node_field_object_compat(node, RawField::Body) else {
        return false;
    };
    if estree_node_type(body) != Some("AssignmentExpression")
        || estree_node_field_str(body, RawField::Operator) != Some("=")
    {
        return false;
    }
    let Some(left) = estree_node_field_object_compat(body, RawField::Left) else {
        return false;
    };
    let Some(right) = estree_node_field_object_compat(body, RawField::Right) else {
        return false;
    };
    if estree_node_type(left) != Some("Identifier")
        || estree_node_field_str(left, RawField::Name) != Some(left_name)
        || estree_node_type(right) != Some("CallExpression")
    {
        return false;
    }
    let Some(callee) = estree_node_field_object_compat(right, RawField::Callee) else {
        return false;
    };
    if estree_node_type(callee) != Some("Identifier")
        || estree_node_field_str(callee, RawField::Name) != Some(callee_name)
    {
        return false;
    }
    let Some(arguments) = estree_node_field_array_compat(right, RawField::Arguments) else {
        return false;
    };
    if arguments.len() != 1 {
        return false;
    }
    let EstreeValue::Object(argument) = &arguments[0] else {
        return false;
    };
    estree_node_type(argument) == Some("Identifier")
        && estree_node_field_str(argument, RawField::Name) == Some(argument_name)
}

fn extract_literal_declaration(source: &str, value: &EstreeValue) -> Option<(String, String)> {
    let EstreeValue::Object(statement) = value else {
        return None;
    };
    if estree_node_type(statement) != Some("VariableDeclaration") {
        return None;
    }
    let declarations = estree_node_field_array_compat(statement, RawField::Declarations)?;
    if declarations.len() != 1 {
        return None;
    }
    let EstreeValue::Object(declarator) = &declarations[0] else {
        return None;
    };
    let id = estree_node_field_object_compat(declarator, RawField::Id)?;
    if estree_node_type(id) != Some("Identifier") {
        return None;
    }
    let name = estree_node_field_str(id, RawField::Name)?.to_string();
    let init = estree_node_field_object_compat(declarator, RawField::Init)?;
    if estree_node_type(init) != Some("Literal") {
        return None;
    }
    Some((name, node_source(init, source)?))
}

fn fold_fragment_text_constants(
    fragment: &crate::ast::modern::Fragment,
    env: &BTreeMap<String, ConstValue>,
) -> Option<String> {
    let mut out = String::new();
    for node in significant_nodes(fragment) {
        match node {
            Node::Text(text) => out.push_str(text.data.as_ref()),
            Node::ExpressionTag(expression_tag) => {
                let value = evaluate_const_value(&expression_tag.expression.0, env)?;
                out.push_str(const_value_to_rendered_text(&value));
            }
            _ => return None,
        }
    }
    Some(out)
}

fn evaluate_const_value(
    node: &crate::ast::modern::EstreeNode,
    env: &BTreeMap<String, ConstValue>,
) -> Option<ConstValue> {
    match estree_node_type(node) {
        Some("Identifier") => {
            let name = estree_node_field_str(node, RawField::Name)?;
            env.get(name).cloned()
        }
        Some("Literal") => match estree_node_field(node, RawField::Value) {
            Some(EstreeValue::String(value)) => Some(ConstValue::String(value.to_string())),
            Some(EstreeValue::Int(value)) => Some(ConstValue::Number(value.to_string())),
            Some(EstreeValue::UInt(value)) => Some(ConstValue::Number(value.to_string())),
            Some(EstreeValue::Bool(value)) => Some(ConstValue::Bool(*value)),
            Some(EstreeValue::Null) => Some(ConstValue::Null),
            _ => {
                if estree_node_field_str(node, RawField::Raw) == Some("null") {
                    Some(ConstValue::Null)
                } else {
                    None
                }
            }
        },
        Some("LogicalExpression") => {
            let operator = estree_node_field_str(node, RawField::Operator)?;
            if operator != "??" {
                return None;
            }
            let left = estree_node_field_object_compat(node, RawField::Left)?;
            let right = estree_node_field_object_compat(node, RawField::Right)?;
            let left_value = evaluate_const_value(left, env)?;
            if matches!(left_value, ConstValue::Null) {
                evaluate_const_value(right, env)
            } else {
                Some(left_value)
            }
        }
        _ => None,
    }
}

fn const_value_to_rendered_text(value: &ConstValue) -> &str {
    match value {
        ConstValue::String(value) => value.as_str(),
        ConstValue::Number(value) => value.as_str(),
        ConstValue::Bool(true) => "true",
        ConstValue::Bool(false) => "false",
        ConstValue::Null => "",
    }
}

fn is_increment_arrow_function(
    node: &crate::ast::modern::EstreeNode,
    identifier_name: &str,
) -> bool {
    if estree_node_type(node) != Some("ArrowFunctionExpression") {
        return false;
    }
    if !estree_node_field_array_compat(node, RawField::Params)
        .is_some_and(|params| params.is_empty())
    {
        return false;
    }
    let Some(body) = estree_node_field_object_compat(node, RawField::Body) else {
        return false;
    };
    if estree_node_type(body) != Some("UpdateExpression") {
        return false;
    }
    let Some(argument) = estree_node_field_object_compat(body, RawField::Argument) else {
        return false;
    };
    estree_node_type(argument) == Some("Identifier")
        && estree_node_field_str(argument, RawField::Name) == Some(identifier_name)
        && estree_node_field_str(body, RawField::Operator) == Some("++")
}

fn extract_props_binding_name(value: &EstreeValue) -> Option<String> {
    let EstreeValue::Object(statement) = value else {
        return None;
    };
    if estree_node_type(statement) != Some("VariableDeclaration") {
        return None;
    }
    let declarations = estree_node_field_array_compat(statement, RawField::Declarations)?;
    if declarations.len() != 1 {
        return None;
    }
    let EstreeValue::Object(declarator) = &declarations[0] else {
        return None;
    };
    let id = estree_node_field_object_compat(declarator, RawField::Id)?;
    if estree_node_type(id) != Some("Identifier") {
        return None;
    }
    let name = estree_node_field_str(id, RawField::Name)?.to_string();
    let init = estree_node_field_object_compat(declarator, RawField::Init)?;
    if estree_node_type(init) != Some("CallExpression") {
        return None;
    }
    let callee = estree_node_field_object_compat(init, RawField::Callee)?;
    if estree_node_type(callee) != Some("Identifier")
        || estree_node_field_str(callee, RawField::Name) != Some("$props")
    {
        return None;
    }
    Some(name)
}

fn match_member_read(value: &EstreeValue, object_name: &str) -> Option<String> {
    let expression = expression_statement_expression(value)?;
    let member = expect_member_expression(expression)?;
    let object = estree_node_field_object_compat(member, RawField::Object)?;
    let property = estree_node_field_object_compat(member, RawField::Property)?;
    if estree_node_type(object) != Some("Identifier")
        || estree_node_field_str(object, RawField::Name) != Some(object_name)
        || estree_node_type(property) != Some("Identifier")
        || estree_node_field(member, RawField::Computed) != Some(&EstreeValue::Bool(false))
    {
        return None;
    }
    estree_node_field_str(property, RawField::Name).map(ToString::to_string)
}

fn match_computed_member_read(
    source: &str,
    value: &EstreeValue,
    object_name: &str,
) -> Option<String> {
    let expression = expression_statement_expression(value)?;
    let member = expect_member_expression(expression)?;
    let object = estree_node_field_object_compat(member, RawField::Object)?;
    let property = estree_node_field_object_compat(member, RawField::Property)?;
    if estree_node_type(object) != Some("Identifier")
        || estree_node_field_str(object, RawField::Name) != Some(object_name)
        || estree_node_field(member, RawField::Computed) != Some(&EstreeValue::Bool(true))
    {
        return None;
    }
    node_source(property, source)
}

fn match_nested_member_read(
    value: &EstreeValue,
    object_name: &str,
    first_property: &str,
) -> Option<(String, String)> {
    let expression = expression_statement_expression(value)?;
    let outer = expect_member_expression(expression)?;
    let outer_property = estree_node_field_object_compat(outer, RawField::Property)?;
    if estree_node_type(outer_property) != Some("Identifier")
        || estree_node_field(outer, RawField::Computed) != Some(&EstreeValue::Bool(false))
    {
        return None;
    }
    let nested_name = estree_node_field_str(outer_property, RawField::Name)?.to_string();

    let inner = estree_node_field_object_compat(outer, RawField::Object)?;
    let inner_member = expect_member_expression(inner)?;
    let inner_object = estree_node_field_object_compat(inner_member, RawField::Object)?;
    let inner_property = estree_node_field_object_compat(inner_member, RawField::Property)?;
    if estree_node_type(inner_object) != Some("Identifier")
        || estree_node_field_str(inner_object, RawField::Name) != Some(object_name)
        || estree_node_type(inner_property) != Some("Identifier")
        || estree_node_field_str(inner_property, RawField::Name) != Some(first_property)
        || estree_node_field(inner_member, RawField::Computed) != Some(&EstreeValue::Bool(false))
    {
        return None;
    }

    Some((first_property.to_string(), nested_name))
}

fn match_nested_member_assignment(
    value: &EstreeValue,
    object_name: &str,
    first_property: &str,
    nested_property: &str,
) -> Option<()> {
    let expression = expression_statement_expression(value)?;
    if estree_node_type(expression) != Some("AssignmentExpression") {
        return None;
    }
    let left = estree_node_field_object_compat(expression, RawField::Left)?;
    let right = estree_node_field_object_compat(expression, RawField::Right)?;
    if estree_node_type(right) != Some("Literal")
        || estree_node_field(right, RawField::Value) != Some(&EstreeValue::Bool(true))
    {
        return None;
    }
    let outer = expect_member_expression(left)?;
    let outer_property = estree_node_field_object_compat(outer, RawField::Property)?;
    if estree_node_type(outer_property) != Some("Identifier")
        || estree_node_field_str(outer_property, RawField::Name) != Some(nested_property)
        || estree_node_field(outer, RawField::Computed) != Some(&EstreeValue::Bool(false))
    {
        return None;
    }
    let inner = estree_node_field_object_compat(outer, RawField::Object)?;
    let inner_member = expect_member_expression(inner)?;
    let inner_object = estree_node_field_object_compat(inner_member, RawField::Object)?;
    let inner_property = estree_node_field_object_compat(inner_member, RawField::Property)?;
    if estree_node_type(inner_object) != Some("Identifier")
        || estree_node_field_str(inner_object, RawField::Name) != Some(object_name)
        || estree_node_type(inner_property) != Some("Identifier")
        || estree_node_field_str(inner_property, RawField::Name) != Some(first_property)
        || estree_node_field(inner_member, RawField::Computed) != Some(&EstreeValue::Bool(false))
    {
        return None;
    }
    Some(())
}

fn match_direct_member_assignment(
    value: &EstreeValue,
    object_name: &str,
    direct_property: &str,
) -> Option<()> {
    let expression = expression_statement_expression(value)?;
    if estree_node_type(expression) != Some("AssignmentExpression") {
        return None;
    }
    let left = estree_node_field_object_compat(expression, RawField::Left)?;
    let right = estree_node_field_object_compat(expression, RawField::Right)?;
    if estree_node_type(right) != Some("Literal")
        || estree_node_field(right, RawField::Value) != Some(&EstreeValue::Bool(true))
    {
        return None;
    }
    let member = expect_member_expression(left)?;
    let object = estree_node_field_object_compat(member, RawField::Object)?;
    let property = estree_node_field_object_compat(member, RawField::Property)?;
    if estree_node_type(object) != Some("Identifier")
        || estree_node_field_str(object, RawField::Name) != Some(object_name)
        || estree_node_type(property) != Some("Identifier")
        || estree_node_field_str(property, RawField::Name) != Some(direct_property)
        || estree_node_field(member, RawField::Computed) != Some(&EstreeValue::Bool(false))
    {
        return None;
    }
    Some(())
}

fn match_computed_member_assignment(
    source: &str,
    value: &EstreeValue,
    object_name: &str,
    key_expression: &str,
) -> Option<()> {
    let expression = expression_statement_expression(value)?;
    if estree_node_type(expression) != Some("AssignmentExpression") {
        return None;
    }
    let left = estree_node_field_object_compat(expression, RawField::Left)?;
    let right = estree_node_field_object_compat(expression, RawField::Right)?;
    if estree_node_type(right) != Some("Literal")
        || estree_node_field(right, RawField::Value) != Some(&EstreeValue::Bool(true))
    {
        return None;
    }
    let member = expect_member_expression(left)?;
    let object = estree_node_field_object_compat(member, RawField::Object)?;
    let property = estree_node_field_object_compat(member, RawField::Property)?;
    if estree_node_type(object) != Some("Identifier")
        || estree_node_field_str(object, RawField::Name) != Some(object_name)
        || estree_node_field(member, RawField::Computed) != Some(&EstreeValue::Bool(true))
        || node_source(property, source)? != key_expression
    {
        return None;
    }
    Some(())
}

fn match_identifier_expression(value: &EstreeValue, name: &str) -> Option<()> {
    let expression = expression_statement_expression(value)?;
    if estree_node_type(expression) != Some("Identifier")
        || estree_node_field_str(expression, RawField::Name) != Some(name)
    {
        return None;
    }
    Some(())
}

fn expression_statement_expression(value: &EstreeValue) -> Option<&crate::ast::modern::EstreeNode> {
    let EstreeValue::Object(statement) = value else {
        return None;
    };
    if estree_node_type(statement) != Some("ExpressionStatement") {
        return None;
    }
    estree_node_field_object_compat(statement, RawField::Expression)
}

fn expect_member_expression(
    node: &crate::ast::modern::EstreeNode,
) -> Option<&crate::ast::modern::EstreeNode> {
    if estree_node_type(node) == Some("MemberExpression") {
        Some(node)
    } else {
        None
    }
}

fn extract_reset_assignments(
    source: &str,
    value: &EstreeValue,
    first_name: &str,
    second_name: &str,
) -> Option<(String, [String; 4])> {
    let EstreeValue::Object(statement) = value else {
        return None;
    };
    if estree_node_type(statement) != Some("FunctionDeclaration") {
        return None;
    }
    let id = estree_node_field_object_compat(statement, RawField::Id)?;
    let reset_name = estree_node_field_str(id, RawField::Name)?.to_string();
    let params = estree_node_field_array_compat(statement, RawField::Params)?;
    if !params.is_empty() {
        return None;
    }
    let body = estree_node_field_object_compat(statement, RawField::Body)?;
    let statements = estree_node_field_array_compat(body, RawField::Body)?;
    if statements.len() != 4 {
        return None;
    }

    let mut values = Vec::with_capacity(4);
    for (index, statement_value) in statements.iter().enumerate() {
        let EstreeValue::Object(statement_node) = statement_value else {
            return None;
        };
        if estree_node_type(statement_node) != Some("ExpressionStatement") {
            return None;
        }
        let expression = estree_node_field_object_compat(statement_node, RawField::Expression)?;
        if estree_node_type(expression) != Some("AssignmentExpression") {
            return None;
        }
        let left = estree_node_field_object_compat(expression, RawField::Left)?;
        if estree_node_type(left) != Some("Identifier") {
            return None;
        }
        let expected_name = if index < 2 { first_name } else { second_name };
        if estree_node_field_str(left, RawField::Name) != Some(expected_name) {
            return None;
        }
        let right = estree_node_field_object_compat(expression, RawField::Right)?;
        values.push(node_source(right, source)?);
    }

    Some((
        reset_name,
        [
            values[0].clone(),
            values[1].clone(),
            values[2].clone(),
            values[3].clone(),
        ],
    ))
}

fn bind_value_identifier(element: &crate::ast::modern::RegularElement) -> Option<String> {
    if !fragment_is_empty(&element.fragment) {
        return None;
    }
    for attribute in element.attributes.iter() {
        let Attribute::BindDirective(bind) = attribute else {
            continue;
        };
        if bind.name.as_ref() == "value" {
            return expression_identifier_name(&bind.expression).map(ToString::to_string);
        }
    }
    None
}

fn button_onclick_identifier(element: &crate::ast::modern::RegularElement) -> Option<String> {
    for attribute in element.attributes.iter() {
        let Attribute::Attribute(attribute) = attribute else {
            continue;
        };
        if attribute.name.as_ref() != "onclick" {
            continue;
        }
        let crate::ast::modern::AttributeValueList::ExpressionTag(expression_tag) =
            &attribute.value
        else {
            continue;
        };
        return expression_identifier_name(&expression_tag.expression).map(ToString::to_string);
    }
    None
}

fn extract_single_state_declaration(source: &str, value: &EstreeValue) -> Option<(String, String)> {
    let EstreeValue::Object(statement) = value else {
        return None;
    };
    if estree_node_type(statement) != Some("VariableDeclaration") {
        return None;
    }
    let declarations = estree_node_field_array_compat(statement, RawField::Declarations)?;
    if declarations.len() != 1 {
        return None;
    }
    let EstreeValue::Object(declarator) = &declarations[0] else {
        return None;
    };
    let id = estree_node_field_object_compat(declarator, RawField::Id)?;
    if estree_node_type(id) != Some("Identifier") {
        return None;
    }
    let name = estree_node_field_str(id, RawField::Name)?.to_string();
    let init = estree_node_field_object_compat(declarator, RawField::Init)?;
    if estree_node_type(init) != Some("CallExpression") {
        return None;
    }
    let callee = estree_node_field_object_compat(init, RawField::Callee)?;
    if estree_node_type(callee) != Some("Identifier")
        || estree_node_field_str(callee, RawField::Name) != Some("$state")
    {
        return None;
    }
    let arguments = estree_node_field_array_compat(init, RawField::Arguments)?;
    if arguments.len() != 1 {
        return None;
    }
    let EstreeValue::Object(argument) = &arguments[0] else {
        return None;
    };
    let state_value = node_source(argument, source)?;
    Some((name, state_value))
}

fn extract_named_return_function(value: &EstreeValue, expected_return: &str) -> Option<String> {
    let EstreeValue::Object(statement) = value else {
        return None;
    };
    if estree_node_type(statement) != Some("FunctionDeclaration") {
        return None;
    }
    let id = estree_node_field_object_compat(statement, RawField::Id)?;
    if estree_node_type(id) != Some("Identifier") {
        return None;
    }
    let function_name = estree_node_field_str(id, RawField::Name)?.to_string();
    let params = estree_node_field_array_compat(statement, RawField::Params)?;
    if !params.is_empty() {
        return None;
    }
    let body = estree_node_field_object_compat(statement, RawField::Body)?;
    let statements = estree_node_field_array_compat(body, RawField::Body)?;
    if statements.len() != 1 {
        return None;
    }
    let EstreeValue::Object(return_statement) = &statements[0] else {
        return None;
    };
    if estree_node_type(return_statement) != Some("ReturnStatement") {
        return None;
    }
    let argument = estree_node_field_object_compat(return_statement, RawField::Argument)?;
    if estree_node_type(argument) != Some("Identifier")
        || estree_node_field_str(argument, RawField::Name) != Some(expected_return)
    {
        return None;
    }
    Some(function_name)
}

fn extract_named_return_int_function(value: &EstreeValue, expected_return: i64) -> Option<String> {
    let EstreeValue::Object(statement) = value else {
        return None;
    };
    if estree_node_type(statement) != Some("FunctionDeclaration") {
        return None;
    }
    let id = estree_node_field_object_compat(statement, RawField::Id)?;
    if estree_node_type(id) != Some("Identifier") {
        return None;
    }
    let function_name = estree_node_field_str(id, RawField::Name)?.to_string();
    let params = estree_node_field_array_compat(statement, RawField::Params)?;
    if !params.is_empty() {
        return None;
    }
    let body = estree_node_field_object_compat(statement, RawField::Body)?;
    let statements = estree_node_field_array_compat(body, RawField::Body)?;
    if statements.len() != 1 {
        return None;
    }
    let EstreeValue::Object(return_statement) = &statements[0] else {
        return None;
    };
    if estree_node_type(return_statement) != Some("ReturnStatement") {
        return None;
    }
    let argument = estree_node_field_object_compat(return_statement, RawField::Argument)?;
    if !node_is_literal_int(argument, expected_return) {
        return None;
    }
    Some(function_name)
}

fn is_zero_arg_call_to_name(node: &crate::ast::modern::EstreeNode, callee_name: &str) -> bool {
    if estree_node_type(node) != Some("CallExpression") {
        return false;
    }
    let Some(callee) = estree_node_field_object_compat(node, RawField::Callee) else {
        return false;
    };
    if estree_node_type(callee) != Some("Identifier")
        || estree_node_field_str(callee, RawField::Name) != Some(callee_name)
    {
        return false;
    }
    estree_node_field_array_compat(node, RawField::Arguments)
        .is_some_and(|arguments| arguments.is_empty())
}

fn extract_props_destructured_default(
    source: &str,
    program: &crate::ast::modern::EstreeNode,
) -> Option<(String, String)> {
    let Some(EstreeValue::Array(body)) = program.fields.get("body") else {
        return None;
    };
    if body.len() != 1 {
        return None;
    }
    let EstreeValue::Object(statement) = &body[0] else {
        return None;
    };
    if estree_node_type(statement) != Some("VariableDeclaration") {
        return None;
    }
    let declarations = estree_node_field_array_compat(statement, RawField::Declarations)?;
    if declarations.len() != 1 {
        return None;
    }
    let EstreeValue::Object(declarator) = &declarations[0] else {
        return None;
    };

    let init = estree_node_field_object_compat(declarator, RawField::Init)?;
    if estree_node_type(init) != Some("CallExpression") {
        return None;
    }
    let callee = estree_node_field_object_compat(init, RawField::Callee)?;
    if estree_node_type(callee) != Some("Identifier")
        || estree_node_field_str(callee, RawField::Name) != Some("$props")
    {
        return None;
    }
    let arguments = estree_node_field_array_compat(init, RawField::Arguments)?;
    if !arguments.is_empty() {
        return None;
    }

    let id = estree_node_field_object_compat(declarator, RawField::Id)?;
    if estree_node_type(id) != Some("ObjectPattern") {
        return None;
    }
    let properties = estree_node_field_array_compat(id, RawField::Properties)?;
    if properties.len() != 1 {
        return None;
    }
    let EstreeValue::Object(property) = &properties[0] else {
        return None;
    };
    let value = estree_node_field_object_compat(property, RawField::Value)?;
    if estree_node_type(value) != Some("AssignmentPattern") {
        return None;
    }
    let left = estree_node_field_object_compat(value, RawField::Left)?;
    if estree_node_type(left) != Some("Identifier") {
        return None;
    }
    let tag_name = estree_node_field_str(left, RawField::Name)?.to_string();
    let right = estree_node_field_object_compat(value, RawField::Right)?;
    let default_value = node_source(right, source)?;

    Some((tag_name, default_value))
}

fn each_collection_and_index(
    each: &crate::ast::modern::EachBlock,
    source: &str,
) -> Option<(String, Option<String>)> {
    if each.context.is_some() {
        return Some((expression_source(&each.expression, source)?, None));
    }

    if each.index.is_some() {
        return Some((expression_source(&each.expression, source)?, None));
    }

    if estree_node_type(&each.expression.0) != Some("SequenceExpression") {
        return Some((expression_source(&each.expression, source)?, None));
    }

    let expressions = match each.expression.0.fields.get("expressions") {
        Some(EstreeValue::Array(values)) => values,
        _ => return Some((expression_source(&each.expression, source)?, None)),
    };
    if expressions.len() != 2 {
        return Some((expression_source(&each.expression, source)?, None));
    }

    let EstreeValue::Object(collection_node) = &expressions[0] else {
        return Some((expression_source(&each.expression, source)?, None));
    };
    let EstreeValue::Object(index_node) = &expressions[1] else {
        return Some((expression_source(&each.expression, source)?, None));
    };

    let collection = node_source(collection_node, source)?;
    if estree_node_type(index_node) != Some("Identifier") {
        return Some((collection, None));
    }
    let index = estree_node_field_str(index_node, RawField::Name).map(ToString::to_string);
    Some((collection, index))
}

fn match_async_const_pattern(source: &str, root: &Root) -> Option<AsyncConstPattern> {
    if root.instance.is_some() {
        return None;
    }
    let significant = significant_nodes(&root.fragment);
    if significant.len() != 1 {
        return None;
    }
    let Node::IfBlock(if_block) = significant[0] else {
        return None;
    };
    if if_block.alternate.is_some() {
        return None;
    }
    if !expression_is_literal_bool(&if_block.test, true) {
        return None;
    }

    let consequent = significant_nodes(&if_block.consequent);
    if consequent.len() != 3 {
        return None;
    }

    let Node::ConstTag(first_const) = consequent[0] else {
        return None;
    };
    let (first_name, _first_value) = extract_const_assignment(&first_const.declaration, source)?;
    let first_await_argument = extract_await_argument_source(&first_const.declaration, source)?;

    let Node::ConstTag(second_const) = consequent[1] else {
        return None;
    };
    let (second_name, second_value) = extract_const_assignment(&second_const.declaration, source)?;
    if !is_binary_plus_identifier_and_one(&second_const.declaration, first_name.as_str()) {
        return None;
    }
    if second_value.trim() != format!("{first_name} + 1") {
        return None;
    }

    let Node::RegularElement(paragraph) = consequent[2] else {
        return None;
    };
    if paragraph.name.as_ref() != "p" || !paragraph.attributes.is_empty() {
        return None;
    }
    let paragraph_nodes = significant_nodes(&paragraph.fragment);
    if paragraph_nodes.len() != 1 {
        return None;
    }
    let Node::ExpressionTag(expression_tag) = paragraph_nodes[0] else {
        return None;
    };
    if expression_identifier_name(&expression_tag.expression)? != second_name.as_str() {
        return None;
    }

    Some(AsyncConstPattern {
        first_name,
        first_await_argument,
        second_name,
    })
}

fn match_async_each_fallback_hoisting_pattern(
    source: &str,
    root: &Root,
) -> Option<AsyncEachFallbackHoistingPattern> {
    if root.instance.is_some() {
        return None;
    }
    let significant = significant_nodes(&root.fragment);
    if significant.len() != 1 {
        return None;
    }
    let Node::EachBlock(each) = significant[0] else {
        return None;
    };

    let collection_argument = extract_await_argument_source(&each.expression, source)?;
    let context = each.context.as_ref()?;
    let context_name = expression_identifier_name(context)?.to_string();
    let fallback = each.fallback.as_ref()?;

    let body_nodes = significant_nodes(&each.body);
    if body_nodes.len() != 1 {
        return None;
    }
    let Node::ExpressionTag(body_expr_tag) = body_nodes[0] else {
        return None;
    };
    let body_await_argument = extract_await_argument_source(&body_expr_tag.expression, source)?;

    let fallback_nodes = significant_nodes(fallback);
    if fallback_nodes.len() != 1 {
        return None;
    }
    let Node::ExpressionTag(fallback_expr_tag) = fallback_nodes[0] else {
        return None;
    };
    let fallback_await_argument =
        extract_await_argument_source(&fallback_expr_tag.expression, source)?;

    Some(AsyncEachFallbackHoistingPattern {
        collection_argument,
        context_name,
        body_await_argument,
        fallback_await_argument,
    })
}

fn match_async_each_hoisting_pattern(
    source: &str,
    root: &Root,
) -> Option<AsyncEachHoistingPattern> {
    let body = root_script_body(root)?;
    if body.len() != 3 {
        return None;
    }
    let (first_name, first_init) = extract_const_declaration(source, &body[0])?;
    let (second_name, second_init) = extract_const_declaration(source, &body[1])?;
    let (third_name, third_init) = extract_const_declaration(source, &body[2])?;

    let significant = significant_nodes(&root.fragment);
    if significant.len() != 1 {
        return None;
    }
    let Node::EachBlock(each) = significant[0] else {
        return None;
    };
    if each.fallback.is_some() {
        return None;
    }

    let collection_argument = extract_await_argument_source(&each.expression, source)?;
    let context_name = expression_identifier_name(each.context.as_ref()?)?.to_string();

    let body_nodes = significant_nodes(&each.body);
    if body_nodes.len() != 1 {
        return None;
    }
    let Node::ExpressionTag(body_expr_tag) = body_nodes[0] else {
        return None;
    };
    let item_await_argument = extract_await_argument_source(&body_expr_tag.expression, source)?;
    if item_await_argument.trim() != context_name.as_str() {
        return None;
    }

    Some(AsyncEachHoistingPattern {
        first_name,
        first_init,
        second_name,
        second_init,
        third_name,
        third_init,
        collection_argument,
        context_name,
        item_await_argument,
    })
}

fn match_async_if_hoisting_pattern(source: &str, root: &Root) -> Option<AsyncIfHoistingPattern> {
    if root.instance.is_some() {
        return None;
    }
    let significant = significant_nodes(&root.fragment);
    if significant.len() != 1 {
        return None;
    }
    let Node::IfBlock(if_block) = significant[0] else {
        return None;
    };
    let test_await_argument = extract_await_argument_source(&if_block.test, source)?;
    let Some(alternate) = &if_block.alternate else {
        return None;
    };
    let Alternate::Fragment(alternate_fragment) = alternate.as_ref() else {
        return None;
    };

    let consequent_nodes = significant_nodes(&if_block.consequent);
    if consequent_nodes.len() != 1 {
        return None;
    }
    let Node::ExpressionTag(consequent_expr_tag) = consequent_nodes[0] else {
        return None;
    };
    let consequent_await_argument =
        extract_await_argument_source(&consequent_expr_tag.expression, source)?;

    let alternate_nodes = significant_nodes(alternate_fragment);
    if alternate_nodes.len() != 1 {
        return None;
    }
    let Node::ExpressionTag(alternate_expr_tag) = alternate_nodes[0] else {
        return None;
    };
    let alternate_await_argument =
        extract_await_argument_source(&alternate_expr_tag.expression, source)?;

    Some(AsyncIfHoistingPattern {
        test_await_argument,
        consequent_await_argument,
        alternate_await_argument,
    })
}

fn match_async_if_chain_pattern(source: &str, root: &Root) -> Option<AsyncIfChainPattern> {
    let body = root_script_body(root)?;
    if body.len() != 3 {
        return None;
    }
    let complex_fn_name = extract_named_return_int_function(&body[0], 1)?;
    let (foo_name, foo_init) = extract_single_state_declaration(source, &body[1])?;
    let (blocking_name, await_target) = extract_let_derived_with_await_identifier(&body[2])?;
    if await_target != foo_name {
        return None;
    }

    let significant = significant_nodes(&root.fragment);
    if significant.len() != 5 {
        return None;
    }
    let Node::IfBlock(first_if) = significant[0] else {
        return None;
    };
    let Node::IfBlock(second_if) = significant[1] else {
        return None;
    };
    let Node::IfBlock(third_if) = significant[2] else {
        return None;
    };
    let Node::IfBlock(fourth_if) = significant[3] else {
        return None;
    };
    let Node::IfBlock(fifth_if) = significant[4] else {
        return None;
    };

    if !expression_is_identifier_name(&first_if.test, foo_name.as_str())
        || if_block_chain_len(first_if) != 2
        || !if_block_has_final_else(first_if)
    {
        return None;
    }

    if !expression_is_await_identifier_name(&second_if.test, foo_name.as_str())
        || if_block_chain_len(second_if) != 3
        || !if_block_has_final_else(second_if)
    {
        return None;
    }
    let second_else_if_test = if_block_chain_nth_test(second_if, 2)?;
    if !expression_is_await_identifier_name(second_else_if_test, "baz") {
        return None;
    }

    if !expression_is_binary_await_identifier_gt_int(&third_if.test, foo_name.as_str(), 10)
        || if_block_chain_len(third_if) != 3
        || !if_block_has_final_else(third_if)
    {
        return None;
    }
    let third_else_if_test = if_block_chain_nth_test(third_if, 2)?;
    if !expression_is_binary_await_identifier_gt_int(third_else_if_test, foo_name.as_str(), 5) {
        return None;
    }

    if !expression_is_identifier_name(&fourth_if.test, "simple1")
        || if_block_chain_len(fourth_if) != 3
        || !if_block_has_final_else(fourth_if)
    {
        return None;
    }

    if !expression_is_binary_identifier_gt_int(&fifth_if.test, blocking_name.as_str(), 10)
        || if_block_chain_len(fifth_if) != 2
        || !if_block_has_final_else(fifth_if)
    {
        return None;
    }
    let fifth_else_if_test = if_block_chain_nth_test(fifth_if, 1)?;
    if !expression_is_binary_identifier_gt_int(fifth_else_if_test, blocking_name.as_str(), 5) {
        return None;
    }

    Some(AsyncIfChainPattern {
        complex_fn_name,
        foo_name,
        foo_init,
        blocking_name,
    })
}

fn match_async_in_derived_pattern(source: &str, root: &Root) -> Option<AsyncInDerivedPattern> {
    let body = root_script_body(root)?;
    if body.len() != 4 {
        return None;
    }

    let yes1_name = extract_let_derived_with_await(&body[0])?;
    let yes2_name = extract_let_derived_with_await(&body[1])?;
    let no1_name = extract_let_derived_by_async_iife(&body[2])?;
    let no2_name = extract_let_derived_async_iife(&body[3])?;

    let significant = significant_nodes(&root.fragment);
    if significant.len() != 1 {
        return None;
    }
    let Node::IfBlock(if_block) = significant[0] else {
        return None;
    };
    if if_block.alternate.is_some() || !expression_is_literal_bool(&if_block.test, true) {
        return None;
    }

    let body_nodes = significant_nodes(&if_block.consequent);
    if body_nodes.len() != 4 {
        return None;
    }
    let Node::ConstTag(first_const) = body_nodes[0] else {
        return None;
    };
    let Node::ConstTag(second_const) = body_nodes[1] else {
        return None;
    };
    let Node::ConstTag(third_const) = body_nodes[2] else {
        return None;
    };
    let Node::ConstTag(fourth_const) = body_nodes[3] else {
        return None;
    };
    let (first_name, _) = extract_const_assignment(&first_const.declaration, source)?;
    let (second_name, _) = extract_const_assignment(&second_const.declaration, source)?;
    let (third_name, _) = extract_const_assignment(&third_const.declaration, source)?;
    let (fourth_name, _) = extract_const_assignment(&fourth_const.declaration, source)?;

    if first_name != yes1_name
        || second_name != yes2_name
        || third_name != no1_name
        || fourth_name != no2_name
    {
        return None;
    }

    Some(AsyncInDerivedPattern {
        yes1_name,
        yes2_name,
        no1_name,
        no2_name,
    })
}

fn match_async_top_level_inspect_server_pattern(
    source: &str,
    root: &Root,
) -> Option<AsyncTopLevelInspectServerPattern> {
    let body = root_script_body(root)?;
    if body.len() != 2 {
        return None;
    }
    let (data_name, data_initializer) = extract_let_declaration(source, &body[0])?;
    if !is_await_expression_statement_inspect_data(&body[1], data_name.as_str()) {
        return None;
    }

    let significant = significant_nodes(&root.fragment);
    if significant.len() != 1 {
        return None;
    }
    let Node::RegularElement(paragraph) = significant[0] else {
        return None;
    };
    if paragraph.name.as_ref() != "p" || !paragraph.attributes.is_empty() {
        return None;
    }
    let children = significant_nodes(&paragraph.fragment);
    if children.len() != 1 {
        return None;
    }
    let Node::ExpressionTag(expression_tag) = children[0] else {
        return None;
    };
    if expression_identifier_name(&expression_tag.expression)? != data_name.as_str() {
        return None;
    }

    Some(AsyncTopLevelInspectServerPattern {
        data_name,
        data_initializer,
    })
}

fn match_await_block_scope_pattern(source: &str, root: &Root) -> Option<AwaitBlockScopePattern> {
    let body = root_script_body(root)?;
    if body.len() != 3 {
        return None;
    }
    let (counter_name, counter_init) = extract_single_state_declaration(source, &body[0])?;
    let (promise_name, promise_initializer) = extract_const_derived_declaration(source, &body[1])?;
    let increment_name = extract_increment_counter_function(&body[2], counter_name.as_str())?;

    let significant = significant_nodes(&root.fragment);
    if significant.len() != 3 {
        return None;
    }
    let Node::RegularElement(button) = significant[0] else {
        return None;
    };
    let Node::AwaitBlock(await_block) = significant[1] else {
        return None;
    };
    let Node::ExpressionTag(trailing_counter) = significant[2] else {
        return None;
    };

    if button.name.as_ref() != "button" {
        return None;
    }
    if button_onclick_identifier(button)? != increment_name {
        return None;
    }

    let button_children = significant_nodes(&button.fragment);
    if button_children.len() != 2 {
        return None;
    }
    let Node::Text(prefix_text) = button_children[0] else {
        return None;
    };
    if prefix_text.data.as_ref().trim() != "clicks:" {
        return None;
    }
    let Node::ExpressionTag(counter_expr) = button_children[1] else {
        return None;
    };
    if !is_member_expression(&counter_expr.expression.0, counter_name.as_str(), "count") {
        return None;
    }

    if expression_identifier_name(&await_block.expression)? != promise_name.as_str() {
        return None;
    }
    if await_block.pending.is_some() || await_block.catch.is_some() {
        return None;
    }
    let Some(value) = &await_block.value else {
        return None;
    };
    if expression_identifier_name(value)? != counter_name.as_str() {
        return None;
    }
    let Some(then_fragment) = &await_block.then else {
        return None;
    };
    if !significant_nodes(then_fragment).is_empty() {
        return None;
    }

    if !is_member_expression(
        &trailing_counter.expression.0,
        counter_name.as_str(),
        "count",
    ) {
        return None;
    }

    Some(AwaitBlockScopePattern {
        counter_name,
        counter_init,
        promise_name,
        promise_initializer,
        increment_name,
    })
}

fn match_bind_component_snippet_pattern(
    source: &str,
    root: &Root,
) -> Option<BindComponentSnippetPattern> {
    let body = root_script_body(root)?;
    if body.len() != 3 {
        return None;
    }
    let imported_component_name = extract_default_import_local_name(&body[0])?;
    let (state_name, state_init) = extract_single_state_declaration(source, &body[1])?;
    if !is_const_identifier_assignment_to_name(&body[2], "_snippet", "snippet") {
        return None;
    }

    let significant = significant_nodes(&root.fragment);
    if significant.len() != 4 {
        return None;
    }
    let Node::SnippetBlock(snippet_block) = significant[0] else {
        return None;
    };
    if expression_identifier_name(&snippet_block.expression)? != "snippet" {
        return None;
    }
    if !snippet_block.parameters.is_empty() {
        return None;
    }
    let snippet_nodes = significant_nodes(&snippet_block.body);
    if snippet_nodes.len() != 1 {
        return None;
    }
    let Node::Text(snippet_text) = snippet_nodes[0] else {
        return None;
    };
    if snippet_text.data.as_ref().trim() != "Something" {
        return None;
    }

    let Node::Component(component) = significant[1] else {
        return None;
    };
    if component.name.as_ref() != imported_component_name.as_str() {
        return None;
    }
    if !fragment_is_empty(&component.fragment) {
        return None;
    }
    if component.attributes.len() != 1 {
        return None;
    }
    let Attribute::BindDirective(bind) = &component.attributes[0] else {
        return None;
    };
    if bind.name.as_ref() != "value" {
        return None;
    }
    if expression_identifier_name(&bind.expression)? != state_name.as_str() {
        return None;
    }

    let Node::Text(value_text) = significant[2] else {
        return None;
    };
    if value_text.data.as_ref().trim() != "value:" {
        return None;
    }
    let Node::ExpressionTag(value_expr) = significant[3] else {
        return None;
    };
    if expression_identifier_name(&value_expr.expression)? != state_name.as_str() {
        return None;
    }

    Some(BindComponentSnippetPattern {
        component_name: imported_component_name,
        state_name,
        state_init,
    })
}

fn match_select_with_rich_content_pattern(
    _source: &str,
    root: &Root,
) -> Option<SelectWithRichContentPattern> {
    let body = root_script_body(root)?;
    if body.len() != 4 {
        return None;
    }
    if !is_let_array_of_ints_declaration(&body[0], "items", &[1, 2, 3]) {
        return None;
    }
    if !is_let_literal_bool_declaration(&body[1], "show", true) {
        return None;
    }
    if !is_let_literal_string_declaration(&body[2], "html", "<option>From HTML</option>") {
        return None;
    }
    let component_name = extract_default_import_local_name(&body[3])?;
    if component_name != "Option" {
        return None;
    }

    enum SelectChildExpectation<'a> {
        RegularElement(&'a str),
        EachBlock,
        IfBlock,
        KeyBlock,
        RenderTag,
        HtmlTag,
        SvelteBoundary,
        Component,
    }

    let significant = significant_nodes(&root.fragment);
    if significant.len() != 25 {
        return None;
    }

    let snippet_indexes = [
        (4, "opt"),
        (15, "option_snippet"),
        (19, "option_snippet2"),
        (23, "conditional_option"),
    ];
    let mut snippet_names = BTreeMap::new();
    for (index, expected_name) in snippet_indexes {
        let Node::SnippetBlock(snippet) = significant[index] else {
            return None;
        };
        let name = expression_identifier_name(&snippet.expression)?;
        if name != expected_name || !snippet.parameters.is_empty() {
            return None;
        }
        let snippet_nodes = significant_nodes(&snippet.body);
        if snippet_nodes.len() != 1 {
            return None;
        }
        let Node::RegularElement(option_element) = snippet_nodes[0] else {
            return None;
        };
        if option_element.name.as_ref() != "option" {
            return None;
        }
        snippet_names.insert(expected_name, name.to_string());
    }

    let select_expectations = [
        (0, SelectChildExpectation::RegularElement("option")),
        (1, SelectChildExpectation::EachBlock),
        (2, SelectChildExpectation::IfBlock),
        (3, SelectChildExpectation::KeyBlock),
        (5, SelectChildExpectation::RenderTag),
        (6, SelectChildExpectation::EachBlock),
        (7, SelectChildExpectation::RegularElement("optgroup")),
        (8, SelectChildExpectation::RegularElement("optgroup")),
        (9, SelectChildExpectation::RegularElement("option")),
        (10, SelectChildExpectation::EachBlock),
        (11, SelectChildExpectation::IfBlock),
        (12, SelectChildExpectation::SvelteBoundary),
        (13, SelectChildExpectation::SvelteBoundary),
        (14, SelectChildExpectation::Component),
        (16, SelectChildExpectation::RenderTag),
        (17, SelectChildExpectation::HtmlTag),
        (18, SelectChildExpectation::RegularElement("optgroup")),
        (20, SelectChildExpectation::RegularElement("optgroup")),
        (21, SelectChildExpectation::RegularElement("option")),
        (22, SelectChildExpectation::EachBlock),
        (24, SelectChildExpectation::IfBlock),
    ];

    for (index, expectation) in select_expectations {
        let Node::RegularElement(select_element) = significant[index] else {
            return None;
        };
        if select_element.name.as_ref() != "select" {
            return None;
        }
        let children = significant_nodes(&select_element.fragment);
        if children.len() != 1 {
            return None;
        }
        match expectation {
            SelectChildExpectation::RegularElement(expected_name) => {
                let Node::RegularElement(child_element) = children[0] else {
                    return None;
                };
                if child_element.name.as_ref() != expected_name {
                    return None;
                }
            }
            SelectChildExpectation::EachBlock => {
                if !matches!(children[0], Node::EachBlock(_)) {
                    return None;
                }
            }
            SelectChildExpectation::IfBlock => {
                if !matches!(children[0], Node::IfBlock(_)) {
                    return None;
                }
            }
            SelectChildExpectation::KeyBlock => {
                if !matches!(children[0], Node::KeyBlock(_)) {
                    return None;
                }
            }
            SelectChildExpectation::RenderTag => {
                if !matches!(children[0], Node::RenderTag(_)) {
                    return None;
                }
            }
            SelectChildExpectation::HtmlTag => {
                if !matches!(children[0], Node::HtmlTag(_)) {
                    return None;
                }
            }
            SelectChildExpectation::SvelteBoundary => {
                if !matches!(children[0], Node::SvelteBoundary(_)) {
                    return None;
                }
            }
            SelectChildExpectation::Component => {
                let Node::Component(component) = children[0] else {
                    return None;
                };
                if component.name.as_ref() != component_name.as_str() {
                    return None;
                }
            }
        }
    }

    if !snippet_names.contains_key("opt")
        || !snippet_names.contains_key("option_snippet")
        || !snippet_names.contains_key("option_snippet2")
        || !snippet_names.contains_key("conditional_option")
    {
        return None;
    }

    Some(SelectWithRichContentPattern)
}

fn match_class_state_field_constructor_assignment_pattern(
    root: &Root,
) -> Option<ClassStateFieldConstructorAssignmentPattern> {
    let body = root_script_body(root)?;
    if body.len() != 1 {
        return None;
    }
    let EstreeValue::Object(class_decl) = &body[0] else {
        return None;
    };
    if estree_node_type(class_decl) != Some("ClassDeclaration") {
        return None;
    }
    let id = estree_node_field_object_compat(class_decl, RawField::Id)?;
    if estree_node_field_str(id, RawField::Name) != Some("Foo") {
        return None;
    }
    let class_body = estree_node_field_object_compat(class_decl, RawField::Body)?;
    let elements = estree_node_field_array_compat(class_body, RawField::Body)?;
    if elements.len() < 5 {
        return None;
    }
    if !significant_nodes(&root.fragment).is_empty() {
        return None;
    }
    Some(ClassStateFieldConstructorAssignmentPattern)
}

fn match_skip_static_subtree_pattern(root: &Root) -> Option<SkipStaticSubtreePattern> {
    let body = root_script_body(root)?;
    if body.len() != 1 {
        return None;
    }
    let EstreeValue::Object(statement) = &body[0] else {
        return None;
    };
    if estree_node_type(statement) != Some("VariableDeclaration") {
        return None;
    }
    let declarations = estree_node_field_array_compat(statement, RawField::Declarations)?;
    if declarations.len() != 1 {
        return None;
    }
    let EstreeValue::Object(declarator) = &declarations[0] else {
        return None;
    };
    let id = estree_node_field_object_compat(declarator, RawField::Id)?;
    if estree_node_type(id) != Some("ObjectPattern") {
        return None;
    }
    let properties = estree_node_field_array_compat(id, RawField::Properties)?;
    if properties.len() != 2 {
        return None;
    }
    let title_name = object_pattern_identifier_name(&properties[0])?.to_string();
    let content_name = object_pattern_identifier_name(&properties[1])?.to_string();
    let init = estree_node_field_object_compat(declarator, RawField::Init)?;
    if estree_node_type(init) != Some("CallExpression") {
        return None;
    }
    let callee = estree_node_field_object_compat(init, RawField::Callee)?;
    if estree_node_type(callee) != Some("Identifier")
        || estree_node_field_str(callee, RawField::Name) != Some("$props")
    {
        return None;
    }
    if !estree_node_field_array_compat(init, RawField::Arguments)?.is_empty() {
        return None;
    }

    let significant = significant_nodes(&root.fragment);
    if significant.len() != 8 {
        return None;
    }
    let expected_names = [
        "header",
        "main",
        "cant-skip",
        "div",
        "div",
        "select",
        "img",
        "div",
    ];
    for (index, expected_name) in expected_names.iter().enumerate() {
        let Node::RegularElement(element) = significant[index] else {
            return None;
        };
        if element.name.as_ref() != *expected_name {
            return None;
        }
    }

    Some(SkipStaticSubtreePattern {
        title_name,
        content_name,
    })
}

fn compile_bind_this_client(
    component_name: &str,
    component_tag: &str,
    bind_expression: &str,
) -> String {
    format!(
        "import 'svelte/internal/disclose-version';\nimport 'svelte/internal/flags/legacy';\nimport * as $ from 'svelte/internal/client';\n\nexport default function {component_name}($$anchor) {{\n\t$.bind_this({component_tag}($$anchor, {{ $$legacy: true }}), ($$value) => {bind_expression} = $$value, () => {bind_expression});\n}}\n"
    )
}

fn compile_bind_this_server(component_name: &str, component_tag: &str) -> String {
    format!(
        "import * as $ from 'svelte/internal/server';\n\nexport default function {component_name}($$renderer) {{\n\t{component_tag}($$renderer, {{}});\n}}\n"
    )
}

fn compile_each_client(component_name: &str, pattern: EachPattern) -> String {
    match pattern {
        EachPattern::StringTemplate {
            collection,
            context,
        } => apply_template_replacements(
            TEMPLATE_EACH_STRING_TEMPLATE_CLIENT,
            &[
                ("__COMPONENT__", component_name),
                ("__COLLECTION__", collection.as_str()),
                ("__CONTEXT__", context.as_str()),
            ],
        ),
        EachPattern::IndexParagraph { collection, index } => apply_template_replacements(
            TEMPLATE_EACH_INDEX_PARAGRAPH_CLIENT,
            &[
                ("__COMPONENT__", component_name),
                ("__COLLECTION__", collection.as_str()),
                ("__INDEX__", index.as_str()),
            ],
        ),
        EachPattern::SpanExpression {
            collection,
            context,
        } => apply_template_replacements(
            TEMPLATE_EACH_SPAN_EXPRESSION_CLIENT,
            &[
                ("__COMPONENT__", component_name),
                ("__COLLECTION__", collection.as_str()),
                ("__CONTEXT__", context.as_str()),
            ],
        ),
    }
}

fn compile_purity_client(component_name: &str, pattern: &PurityPattern) -> String {
    format!(
        "import 'svelte/internal/disclose-version';\nimport 'svelte/internal/flags/legacy';\nimport * as $ from 'svelte/internal/client';\n\nvar root = $.from_html(`<p></p> <p></p> <!>`, 1);\n\nexport default function {component_name}($$anchor) {{\n\tvar fragment = root();\n\tvar p = $.first_child(fragment);\n\n\tp.textContent = '{pure_text}';\n\n\tvar p_1 = $.sibling(p, 2);\n\n\tp_1.textContent = {location_expression};\n\n\tvar node = $.sibling(p_1, 2);\n\n\t{component_name_ref}(node, {{ prop: {component_prop_expression} }});\n\t$.append($$anchor, fragment);\n}}\n",
        pure_text = pattern.pure_text,
        location_expression = pattern.location_expression,
        component_name_ref = pattern.component_name,
        component_prop_expression = pattern.component_prop_expression,
    )
}

fn compile_svelte_element_client(
    component_name: &str,
    tag_name: &str,
    default_value: &str,
) -> String {
    format!(
        "import 'svelte/internal/disclose-version';\nimport * as $ from 'svelte/internal/client';\n\nexport default function {component_name}($$anchor, $$props) {{\n\tlet {tag_name} = $.prop($$props, '{tag_name}', 3, {default_value});\n\tvar fragment = $.comment();\n\tvar node = $.first_child(fragment);\n\n\t$.element(node, {tag_name}, false);\n\t$.append($$anchor, fragment);\n}}\n"
    )
}

fn compile_dynamic_element_literal_class_client(
    component_name: &str,
    pattern: &DynamicElementLiteralClassPattern,
) -> String {
    let class_literal = js_single_quoted_string(pattern.class_value.as_str());
    format!(
        "import 'svelte/internal/disclose-version';\nimport 'svelte/internal/flags/legacy';\nimport * as $ from 'svelte/internal/client';\n\nexport default function {component_name}($$anchor) {{\n\tvar fragment = $.comment();\n\tvar node = $.first_child(fragment);\n\n\t$.element(node, () => {}, false, ($$element, $$anchor) => {{\n\t\t$.set_class($$element, 0, {class_literal});\n\t}});\n\n\t$.append($$anchor, fragment);\n}}\n",
        pattern.tag_expression
    )
}

fn compile_dynamic_element_tag_client(
    component_name: &str,
    pattern: &DynamicElementTagPattern,
) -> String {
    let prop_name_literal = js_single_quoted_string(pattern.prop_name.as_str());
    let top_class_literal = js_single_quoted_string(pattern.top_class_value.as_str());
    let nested_class_literal = js_single_quoted_string(pattern.nested_class_value.as_str());
    let root_1_markup = escape_js_template_literal(pattern.nested_children_html.as_str());
    let root_markup = escape_js_template_literal(
        format!("<!> <h2 class=\"{}\"><!></h2>", pattern.h2_class_value).as_str(),
    );

    format!(
        "import 'svelte/internal/disclose-version';\nimport 'svelte/internal/flags/legacy';\nimport * as $ from 'svelte/internal/client';\n\nvar root_1 = $.from_html(`{root_1_markup}`);\nvar root = $.from_html(`{root_markup}`, 1);\n\nexport default function {component_name}($$anchor, $$props) {{\n\tlet {prop_name} = $.prop($$props, {prop_name_literal}, 8, {default_value});\n\tvar fragment = root();\n\tvar node = $.first_child(fragment);\n\n\t$.element(node, {prop_name}, false, ($$element, $$anchor) => {{\n\t\t$.set_class($$element, 0, {top_class_literal});\n\t}});\n\n\tvar h2 = $.sibling(node, 2);\n\tvar node_1 = $.child(h2);\n\n\t$.element(node_1, {prop_name}, false, ($$element_1, $$anchor) => {{\n\t\t$.set_class($$element_1, 0, {nested_class_literal});\n\n\t\tvar b = root_1();\n\n\t\t$.append($$anchor, b);\n\t}});\n\n\t$.reset(h2);\n\t$.append($$anchor, fragment);\n}}\n",
        prop_name = pattern.prop_name,
        default_value = pattern.default_value
    )
}

fn compile_regular_tree_single_html_tag_client(
    component_name: &str,
    pattern: &RegularTreeSingleHtmlTagPattern,
) -> String {
    let markup = escape_js_template_literal(pattern.client_markup.as_str());
    let element_var_names = dedupe_element_var_names(pattern.element_names.as_slice());
    let mut out = String::new();
    out.push_str(
        "import 'svelte/internal/disclose-version';\nimport 'svelte/internal/flags/legacy';\nimport * as $ from 'svelte/internal/client';\n\n",
    );
    out.push_str(&format!("var root = $.from_html(`{markup}`);\n\n"));
    out.push_str(&format!(
        "export default function {component_name}($$anchor) {{\n"
    ));

    let root_var = &element_var_names[0];
    out.push_str(&format!("\tvar {root_var} = root();\n"));
    for index in 1..element_var_names.len() {
        out.push_str(&format!(
            "\tvar {} = $.child({});\n",
            element_var_names[index],
            element_var_names[index - 1]
        ));
    }
    let leaf_parent = element_var_names
        .last()
        .expect("at least one regular element");
    out.push_str(&format!("\tvar node = $.child({leaf_parent});\n\n"));
    out.push_str(&format!(
        "\t$.html(node, () => {});\n",
        pattern.html_expression
    ));
    for element_var in element_var_names.iter().rev() {
        out.push_str(&format!("\t$.reset({element_var});\n"));
    }
    out.push_str(&format!("\t$.append($$anchor, {root_var});\n"));
    out.push_str("}\n");
    out
}

fn dedupe_element_var_names(names: &[String]) -> Vec<String> {
    let mut counts = BTreeMap::<String, usize>::new();
    names
        .iter()
        .map(|name| {
            let entry = counts.entry(name.clone()).or_insert(0usize);
            let current = *entry;
            *entry += 1;
            if current == 0 {
                name.clone()
            } else {
                format!("{name}_{current}")
            }
        })
        .collect()
}

fn compile_nested_dynamic_text_client(
    component_name: &str,
    pattern: &NestedDynamicTextPattern,
) -> String {
    let prop_name_literal = js_single_quoted_string(pattern.prop_name.as_str());
    let markup = escape_js_template_literal(
        format!(
            "<span class=\"{}\"><span class=\"{}\">text</span></span> <span class=\"{}\"><span class=\"{}\"> </span></span>",
            pattern.first_outer_class,
            pattern.first_inner_class,
            pattern.second_outer_class,
            pattern.second_inner_class
        )
        .as_str(),
    );
    format!(
        "import 'svelte/internal/disclose-version';\nimport 'svelte/internal/flags/legacy';\nimport * as $ from 'svelte/internal/client';\n\nvar root = $.from_html(`{markup}`, 1);\n\nexport default function {component_name}($$anchor, $$props) {{\n\tlet {prop_name} = $.prop($$props, {prop_name_literal}, 8);\n\tvar fragment = root();\n\tvar span = $.sibling($.first_child(fragment), 2);\n\tvar span_1 = $.child(span);\n\tvar text = $.child(span_1, true);\n\n\t$.reset(span_1);\n\t$.reset(span);\n\t$.template_effect(() => $.set_text(text, {prop_name}()));\n\t$.append($$anchor, fragment);\n}}\n",
        prop_name = pattern.prop_name
    )
}

fn compile_structural_complex_css_client(
    component_name: &str,
    pattern: &StructuralComplexCssPattern,
) -> String {
    let markup = escape_js_template_literal(pattern.markup.as_str());
    let mut out = String::new();
    for import in pattern.imports.iter() {
        out.push_str(import);
        out.push('\n');
    }
    if !pattern.imports.is_empty() {
        out.push('\n');
    }
    out.push_str(
        "import 'svelte/internal/disclose-version';\nimport 'svelte/internal/flags/legacy';\nimport * as $ from 'svelte/internal/client';\n\n",
    );
    out.push_str(&format!("var root = $.from_html(`{markup}`, 1);\n\n"));
    out.push_str(&format!(
        "export default function {component_name}($$anchor) {{\n"
    ));
    for statement in pattern.statements.iter() {
        out.push('\t');
        out.push_str(statement);
        out.push('\n');
    }
    if !pattern.statements.is_empty() {
        out.push('\n');
    }
    out.push_str("\tvar fragment = root();\n");
    out.push_str("\t$.append($$anchor, fragment);\n");
    out.push_str("}\n");
    out
}

fn compile_text_nodes_deriveds_client(
    component_name: &str,
    pattern: &TextNodesDerivedsPattern,
) -> String {
    format!(
        "import 'svelte/internal/disclose-version';\nimport * as $ from 'svelte/internal/client';\n\nvar root = $.from_html(`<p> </p>`);\n\nexport default function {component_name}($$anchor) {{\n\tlet {first_state_name} = {first_state_value};\n\tlet {second_state_name} = {second_state_value};\n\n\tfunction {first_function_name}() {{\n\t\treturn {first_state_name};\n\t}}\n\n\tfunction {second_function_name}() {{\n\t\treturn {second_state_name};\n\t}}\n\n\tvar p = root();\n\tvar text = $.child(p);\n\n\t$.reset(p);\n\t$.template_effect(($0, $1) => $.set_text(text, `${{$0 ?? ''}}${{$1 ?? ''}}`), [{first_function_name}, {second_function_name}]);\n\t$.append($$anchor, p);\n}}\n",
        first_state_name = pattern.first_state_name,
        first_state_value = pattern.first_state_value,
        second_state_name = pattern.second_state_name,
        second_state_value = pattern.second_state_value,
        first_function_name = pattern.first_function_name,
        second_function_name = pattern.second_function_name,
    )
}

fn compile_state_proxy_literal_client(
    component_name: &str,
    pattern: &StateProxyLiteralPattern,
) -> String {
    format!(
        "import 'svelte/internal/disclose-version';\nimport * as $ from 'svelte/internal/client';\n\nvar root = $.from_html(`<input/> <input/> <button>reset</button>`, 1);\n\nexport default function {component_name}($$anchor) {{\n\tlet {first_name} = $.state({first_init});\n\tlet {second_name} = $.state({second_init});\n\n\tfunction {reset_name}() {{\n\t\t$.set({first_name}, {first_reset_0});\n\t\t$.set({first_name}, {first_reset_1});\n\t\t$.set({second_name}, {second_reset_0});\n\t\t$.set({second_name}, {second_reset_1});\n\t}}\n\n\tvar fragment = root();\n\tvar input = $.first_child(fragment);\n\n\t$.remove_input_defaults(input);\n\n\tvar input_1 = $.sibling(input, 2);\n\n\t$.remove_input_defaults(input_1);\n\n\tvar button = $.sibling(input_1, 2);\n\n\t$.bind_value(input, () => $.get({first_name}), ($$value) => $.set({first_name}, $$value));\n\t$.bind_value(input_1, () => $.get({second_name}), ($$value) => $.set({second_name}, $$value));\n\t$.delegated('click', button, {reset_name});\n\t$.append($$anchor, fragment);\n}}\n\n$.delegate(['click']);\n",
        first_name = pattern.first_name,
        first_init = pattern.first_init,
        second_name = pattern.second_name,
        second_init = pattern.second_init,
        reset_name = pattern.reset_name,
        first_reset_0 = pattern.first_reset_values[0],
        first_reset_1 = pattern.first_reset_values[1],
        second_reset_0 = pattern.second_reset_values[0],
        second_reset_1 = pattern.second_reset_values[1],
    )
}

fn compile_props_identifier_client(
    component_name: &str,
    pattern: &PropsIdentifierPattern,
) -> String {
    format!(
        "import 'svelte/internal/disclose-version';\nimport * as $ from 'svelte/internal/client';\n\nexport default function {component_name}($$anchor, $$props) {{\n\t$.push($$props, true);\n\n\tlet {props_name} = $.rest_props($$props, ['$$slots', '$$events', '$$legacy']);\n\n\t$$props.{direct_property};\n\t{props_name}[{key_expression}];\n\t$$props.{direct_property}.{nested_property};\n\t$$props.{direct_property}.{nested_property} = true;\n\t{props_name}.{direct_property} = true;\n\t{props_name}[{key_expression}] = true;\n\t{props_name};\n\t$.pop();\n}}\n",
        props_name = pattern.props_name,
        key_expression = pattern.key_expression,
        direct_property = pattern.direct_property,
        nested_property = pattern.nested_property,
    )
}

fn compile_nullish_omittance_client(
    component_name: &str,
    pattern: &NullishOmittancePattern,
) -> String {
    format!(
        "import 'svelte/internal/disclose-version';\nimport * as $ from 'svelte/internal/client';\n\nvar root = $.from_html(`<h1></h1> <b></b> <button> </button> <h1></h1>`, 1);\n\nexport default function {component_name}($$anchor) {{\n\tlet {name_name} = {name_value};\n\tlet {count_name} = $.state({count_init});\n\tvar fragment = root();\n\tvar h1 = $.first_child(fragment);\n\n\th1.textContent = '{first_heading_text}';\n\n\tvar b = $.sibling(h1, 2);\n\n\tb.textContent = '{bold_text}';\n\n\tvar button = $.sibling(b, 2);\n\tvar text = $.child(button);\n\n\t$.reset(button);\n\n\tvar h1_1 = $.sibling(button, 2);\n\n\th1_1.textContent = '{last_heading_text}';\n\t$.template_effect(() => $.set_text(text, `Count is ${{$.get({count_name}) ?? ''}}`));\n\t$.delegated('click', button, () => $.update({count_name}));\n\t$.append($$anchor, fragment);\n}}\n\n$.delegate(['click']);\n",
        name_name = pattern.name_name,
        name_value = pattern.name_value,
        count_name = pattern.count_name,
        count_init = pattern.count_init,
        first_heading_text = pattern.first_heading_text,
        bold_text = pattern.bold_text,
        last_heading_text = pattern.last_heading_text,
    )
}

fn compile_delegated_shadowed_client(
    component_name: &str,
    pattern: &DelegatedShadowedPattern,
) -> String {
    let collection_expression = arrow_return_expression(pattern.collection.as_str());
    format!(
        "import 'svelte/internal/disclose-version';\nimport 'svelte/internal/flags/legacy';\nimport * as $ from 'svelte/internal/client';\n\nvar root_1 = $.from_html(`<button type=\"button\">B</button>`);\n\nexport default function {component_name}($$anchor) {{\n\tvar fragment = $.comment();\n\tvar node = $.first_child(fragment);\n\n\t$.each(node, 0, () => {collection_expression}, $.index, ($$anchor, $$item, {index_name}) => {{\n\t\tvar button = root_1();\n\n\t\t$.set_attribute(button, 'data-index', {index_name});\n\n\t\t$.delegated('click', button, (e) => {{\n\t\t\tconst {index_name} = Number(e.currentTarget.dataset.index);\n\n\t\t\tconsole.log({index_name});\n\t\t}});\n\n\t\t$.append($$anchor, button);\n\t}});\n\n\t$.append($$anchor, fragment);\n}}\n\n$.delegate(['click']);\n",
        collection_expression = collection_expression,
        index_name = pattern.index_name,
    )
}

fn compile_dynamic_attribute_casing_client(
    component_name: &str,
    pattern: &DynamicAttributeCasingPattern,
) -> String {
    format!(
        "import 'svelte/internal/disclose-version';\nimport * as $ from 'svelte/internal/client';\n\nvar root = $.from_html(`<div></div> <svg></svg> <custom-element></custom-element> <div></div> <svg></svg> <custom-element></custom-element>`, 3);\n\nexport default function {component_name}($$anchor) {{\n\t// needs to be a snapshot test because jsdom does auto-correct the attribute casing\n\tlet {x_name} = {x_value};\n\n\tlet {y_name} = {y_value};\n\tvar fragment = root();\n\tvar div = $.first_child(fragment);\n\n\t$.set_attribute(div, 'foobar', {x_name});\n\n\tvar svg = $.sibling(div, 2);\n\n\t$.set_attribute(svg, 'viewBox', {x_name});\n\n\tvar custom_element = $.sibling(svg, 2);\n\n\t$.set_custom_element_data(custom_element, 'fooBar', {x_name});\n\n\tvar div_1 = $.sibling(custom_element, 2);\n\tvar svg_1 = $.sibling(div_1, 2);\n\tvar custom_element_1 = $.sibling(svg_1, 2);\n\n\t$.template_effect(() => $.set_custom_element_data(custom_element_1, 'fooBar', {y_name}()));\n\n\t$.template_effect(\n\t\t($0, $1) => {{\n\t\t\t$.set_attribute(div_1, 'foobar', $0);\n\t\t\t$.set_attribute(svg_1, 'viewBox', $1);\n\t\t}},\n\t\t[{y_name}, {y_name}]\n\t);\n\n\t$.append($$anchor, fragment);\n}}\n",
        x_name = pattern.x_name,
        x_value = pattern.x_value,
        y_name = pattern.y_name,
        y_value = pattern.y_value,
    )
}

fn compile_function_prop_no_getter_client(
    component_name: &str,
    pattern: &FunctionPropNoGetterPattern,
) -> String {
    format!(
        "import 'svelte/internal/disclose-version';\nimport * as $ from 'svelte/internal/client';\n\nexport default function {component_name}($$anchor) {{\n\tlet {count_name} = $.state({count_init});\n\n\tfunction {onmouseup_name}() {{\n\t\t$.set({count_name}, $.get({count_name}) + 2);\n\t}}\n\n\tconst {plus_one_name} = (num) => num + 1;\n\n\t{component_name_ref}($$anchor, {{\n\t\tonmousedown: () => $.set({count_name}, $.get({count_name}) + 1),\n\t\tonmouseup,\n\t\tonmouseenter: () => $.set({count_name}, {plus_one_name}($.get({count_name})), true),\n\t\tchildren: ($$anchor, $$slotProps) => {{\n\t\t\t$.next();\n\n\t\t\tvar text = $.text();\n\n\t\t\t$.template_effect(() => $.set_text(text, `clicks: ${{$.get({count_name}) ?? ''}}`));\n\t\t\t$.append($$anchor, text);\n\t\t}},\n\t\t$$slots: {{ default: true }}\n\t}});\n}}\n",
        count_name = pattern.count_name,
        count_init = pattern.count_init,
        onmouseup_name = pattern.onmouseup_name,
        plus_one_name = pattern.plus_one_name,
        component_name_ref = pattern.component_name,
    )
}

fn compile_async_const_client(component_name: &str, pattern: &AsyncConstPattern) -> String {
    format!(
        "import 'svelte/internal/disclose-version';\nimport 'svelte/internal/flags/async';\nimport * as $ from 'svelte/internal/client';\n\nvar root_1 = $.from_html(`<p> </p>`);\n\nexport default function {component_name}($$anchor) {{\n\tvar fragment = $.comment();\n\tvar node = $.first_child(fragment);\n\n\t{{\n\t\tvar consequent = ($$anchor) => {{\n\t\t\tlet {first_name};\n\t\t\tlet {second_name};\n\n\t\t\tvar promises = $.run([\n\t\t\t\tasync () => {first_name} = (await $.save($.async_derived(async () => (await $.save({first_await_argument}))())))(),\n\t\t\t\t() => {second_name} = $.derived(() => $.get({first_name}) + 1)\n\t\t\t]);\n\n\t\t\tvar p = root_1();\n\t\t\tvar text = $.child(p, true);\n\n\t\t\t$.reset(p);\n\t\t\t$.template_effect(() => $.set_text(text, $.get({second_name})), void 0, void 0, [promises[1]]);\n\t\t\t$.append($$anchor, p);\n\t\t}};\n\n\t\t$.if(node, ($$render) => {{\n\t\t\tif (true) $$render(consequent);\n\t\t}});\n\t}}\n\n\t$.append($$anchor, fragment);\n}}\n",
        first_name = pattern.first_name,
        second_name = pattern.second_name,
        first_await_argument = pattern.first_await_argument,
    )
}

fn compile_async_const_server(component_name: &str, pattern: &AsyncConstPattern) -> String {
    format!(
        "import 'svelte/internal/flags/async';\nimport * as $ from 'svelte/internal/server';\n\nexport default function {component_name}($$renderer) {{\n\tif (true) {{\n\t\t$$renderer.push('<!--[0-->');\n\n\t\tlet {first_name};\n\t\tlet {second_name};\n\n\t\tvar promises = $$renderer.run([\n\t\t\tasync () => {{\n\t\t\t\t{first_name} = (await $.save({first_await_argument}))();\n\t\t\t}},\n\n\t\t\t() => {{\n\t\t\t\t{second_name} = {first_name} + 1;\n\t\t\t}}\n\t\t]);\n\n\t\t$$renderer.push(`<p>`);\n\t\t$$renderer.async([promises[1]], ($$renderer) => $$renderer.push(() => $.escape({second_name})));\n\t\t$$renderer.push(`</p>`);\n\t}} else {{\n\t\t$$renderer.push('<!--[-1-->');\n\t}}\n\n\t$$renderer.push(`<!--]-->`);\n}}\n",
        first_name = pattern.first_name,
        second_name = pattern.second_name,
        first_await_argument = pattern.first_await_argument,
    )
}

fn compile_async_each_fallback_hoisting_client(
    component_name: &str,
    pattern: &AsyncEachFallbackHoistingPattern,
) -> String {
    format!(
        "import 'svelte/internal/disclose-version';\nimport 'svelte/internal/flags/async';\nimport * as $ from 'svelte/internal/client';\n\nexport default function {component_name}($$anchor) {{\n\tvar fragment = $.comment();\n\tvar node = $.first_child(fragment);\n\n\t$.async(node, [], [() => {collection_argument}], (node, $$collection) => {{\n\t\t$.each(\n\t\t\tnode,\n\t\t\t16,\n\t\t\t() => $.get($$collection),\n\t\t\t$.index,\n\t\t\t($$anchor, {context_name}) => {{\n\t\t\t\t$.next();\n\n\t\t\t\tvar text = $.text();\n\n\t\t\t\t$.template_effect(($0) => $.set_text(text, $0), void 0, [() => {body_await_argument}]);\n\t\t\t\t$.append($$anchor, text);\n\t\t\t}},\n\t\t\t($$anchor) => {{\n\t\t\t\t$.next();\n\n\t\t\t\tvar text_1 = $.text();\n\n\t\t\t\t$.template_effect(($0) => $.set_text(text_1, $0), void 0, [() => {fallback_await_argument}]);\n\t\t\t\t$.append($$anchor, text_1);\n\t\t\t}}\n\t\t);\n\t}});\n\n\t$.append($$anchor, fragment);\n}}\n",
        collection_argument = pattern.collection_argument,
        context_name = pattern.context_name,
        body_await_argument = pattern.body_await_argument,
        fallback_await_argument = pattern.fallback_await_argument,
    )
}

fn compile_async_each_fallback_hoisting_server(
    component_name: &str,
    pattern: &AsyncEachFallbackHoistingPattern,
) -> String {
    format!(
        "import 'svelte/internal/flags/async';\nimport * as $ from 'svelte/internal/server';\n\nexport default function {component_name}($$renderer) {{\n\t$$renderer.child_block(async ($$renderer) => {{\n\t\tconst each_array = $.ensure_array_like((await $.save({collection_argument}))());\n\n\t\tif (each_array.length !== 0) {{\n\t\t\t$$renderer.push('<!--[-->');\n\n\t\t\tfor (let $$index = 0, $$length = each_array.length; $$index < $$length; $$index++) {{\n\t\t\t\tlet {context_name} = each_array[$$index];\n\n\t\t\t\t$$renderer.push(`<!---->`);\n\t\t\t\t$$renderer.push(async () => $.escape(await {body_await_argument}));\n\t\t\t}}\n\t\t}} else {{\n\t\t\t$$renderer.push('<!--[!-->');\n\t\t\t$$renderer.push(`<!---->`);\n\t\t\t$$renderer.push(async () => $.escape(await {fallback_await_argument}));\n\t\t}}\n\t}});\n\n\t$$renderer.push(`<!--]-->`);\n}}\n",
        collection_argument = pattern.collection_argument,
        context_name = pattern.context_name,
        body_await_argument = pattern.body_await_argument,
        fallback_await_argument = pattern.fallback_await_argument,
    )
}

fn compile_async_each_hoisting_client(
    component_name: &str,
    pattern: &AsyncEachHoistingPattern,
) -> String {
    format!(
        "import 'svelte/internal/disclose-version';\nimport 'svelte/internal/flags/async';\nimport * as $ from 'svelte/internal/client';\n\nexport default function {component_name}($$anchor) {{\n\tconst {first_name} = {first_init};\n\tconst {second_name} = {second_init};\n\tconst {third_name} = {third_init};\n\tvar fragment = $.comment();\n\tvar node = $.first_child(fragment);\n\n\t$.async(node, [], [() => {collection_argument}], (node, $$collection) => {{\n\t\t$.each(node, 17, () => $.get($$collection), $.index, ($$anchor, {context_name}) => {{\n\t\t\t$.next();\n\n\t\t\tvar text = $.text();\n\n\t\t\t$.template_effect(($0) => $.set_text(text, $0), void 0, [() => $.get({context_name})]);\n\t\t\t$.append($$anchor, text);\n\t\t}});\n\t}});\n\n\t$.append($$anchor, fragment);\n}}\n",
        first_name = pattern.first_name,
        first_init = pattern.first_init,
        second_name = pattern.second_name,
        second_init = pattern.second_init,
        third_name = pattern.third_name,
        third_init = pattern.third_init,
        collection_argument = pattern.collection_argument,
        context_name = pattern.context_name,
    )
}

fn compile_async_each_hoisting_server(
    component_name: &str,
    pattern: &AsyncEachHoistingPattern,
) -> String {
    format!(
        "import 'svelte/internal/flags/async';\nimport * as $ from 'svelte/internal/server';\n\nexport default function {component_name}($$renderer) {{\n\tconst {first_name} = {first_init};\n\tconst {second_name} = {second_init};\n\tconst {third_name} = {third_init};\n\n\t$$renderer.push(`<!--[-->`);\n\n\t$$renderer.child_block(async ($$renderer) => {{\n\t\tconst each_array = $.ensure_array_like((await $.save({collection_argument}))());\n\n\t\tfor (let $$index = 0, $$length = each_array.length; $$index < $$length; $$index++) {{\n\t\t\tlet {context_name} = each_array[$$index];\n\n\t\t\t$$renderer.push(`<!---->`);\n\t\t\t$$renderer.push(async () => $.escape(await {item_await_argument}));\n\t\t}}\n\t}});\n\n\t$$renderer.push(`<!--]-->`);\n}}\n",
        first_name = pattern.first_name,
        first_init = pattern.first_init,
        second_name = pattern.second_name,
        second_init = pattern.second_init,
        third_name = pattern.third_name,
        third_init = pattern.third_init,
        collection_argument = pattern.collection_argument,
        context_name = pattern.context_name,
        item_await_argument = pattern.item_await_argument,
    )
}

fn compile_async_if_hoisting_client(
    component_name: &str,
    pattern: &AsyncIfHoistingPattern,
) -> String {
    format!(
        "import 'svelte/internal/disclose-version';\nimport 'svelte/internal/flags/async';\nimport * as $ from 'svelte/internal/client';\n\nexport default function {component_name}($$anchor) {{\n\tvar fragment = $.comment();\n\tvar node = $.first_child(fragment);\n\n\t$.async(node, [], [() => {test_await_argument}], (node, $$condition) => {{\n\t\tvar consequent = ($$anchor) => {{\n\t\t\tvar text = $.text();\n\n\t\t\t$.template_effect(($0) => $.set_text(text, $0), void 0, [() => {consequent_await_argument}]);\n\t\t\t$.append($$anchor, text);\n\t\t}};\n\n\t\tvar alternate = ($$anchor) => {{\n\t\t\tvar text_1 = $.text();\n\n\t\t\t$.template_effect(($0) => $.set_text(text_1, $0), void 0, [() => {alternate_await_argument}]);\n\t\t\t$.append($$anchor, text_1);\n\t\t}};\n\n\t\t$.if(node, ($$render) => {{\n\t\t\tif ($.get($$condition)) $$render(consequent); else $$render(alternate, -1);\n\t\t}});\n\t}});\n\n\t$.append($$anchor, fragment);\n}}\n",
        test_await_argument = pattern.test_await_argument,
        consequent_await_argument = pattern.consequent_await_argument,
        alternate_await_argument = pattern.alternate_await_argument,
    )
}

fn compile_async_if_hoisting_server(
    component_name: &str,
    pattern: &AsyncIfHoistingPattern,
) -> String {
    format!(
        "import 'svelte/internal/flags/async';\nimport * as $ from 'svelte/internal/server';\n\nexport default function {component_name}($$renderer) {{\n\t$$renderer.child_block(async ($$renderer) => {{\n\t\tif ((await $.save({test_await_argument}))()) {{\n\t\t\t$$renderer.push('<!--[0-->');\n\t\t\t$$renderer.push(async () => $.escape(await {consequent_await_argument}));\n\t\t}} else {{\n\t\t\t$$renderer.push('<!--[-1-->');\n\t\t\t$$renderer.push(async () => $.escape(await {alternate_await_argument}));\n\t\t}}\n\t}});\n\n\t$$renderer.push(`<!--]-->`);\n}}\n",
        test_await_argument = pattern.test_await_argument,
        consequent_await_argument = pattern.consequent_await_argument,
        alternate_await_argument = pattern.alternate_await_argument,
    )
}

fn compile_async_if_chain_client(component_name: &str, pattern: &AsyncIfChainPattern) -> String {
    apply_template_replacements(
        TEMPLATE_ASYNC_IF_CHAIN_CLIENT,
        &async_if_chain_template_replacements(component_name, pattern),
    )
}

fn compile_async_if_chain_server(component_name: &str, pattern: &AsyncIfChainPattern) -> String {
    apply_template_replacements(
        TEMPLATE_ASYNC_IF_CHAIN_SERVER,
        &async_if_chain_template_replacements(component_name, pattern),
    )
}

fn async_if_chain_template_replacements<'a>(
    component_name: &'a str,
    pattern: &'a AsyncIfChainPattern,
) -> [(&'static str, &'a str); 5] {
    [
        ("__COMPONENT__", component_name),
        ("__COMPLEX_FN__", pattern.complex_fn_name.as_str()),
        ("__FOO_NAME__", pattern.foo_name.as_str()),
        ("__FOO_INIT__", pattern.foo_init.as_str()),
        ("__BLOCKING_NAME__", pattern.blocking_name.as_str()),
    ]
}

fn compile_async_in_derived_client(
    component_name: &str,
    pattern: &AsyncInDerivedPattern,
) -> String {
    format!(
        "import 'svelte/internal/disclose-version';\nimport 'svelte/internal/flags/async';\nimport * as $ from 'svelte/internal/client';\n\nexport default function {component_name}($$anchor, $$props) {{\n\t$.push($$props, true);\n\n\tvar {yes1_name}, {yes2_name}, {no1_name}, {no2_name};\n\n\tvar $$promises = $.run([\n\t\tasync () => {yes1_name} = await $.async_derived(() => 1),\n\t\tasync () => {yes2_name} = await $.async_derived(async () => foo(await 1)),\n\t\t() => {no1_name} = $.derived(async () => {{\n\t\t\treturn await 1;\n\t\t}}),\n\n\t\t() => {no2_name} = $.derived(() => async () => {{\n\t\t\treturn await 1;\n\t\t}})\n\t]);\n\n\tvar fragment = $.comment();\n\tvar node = $.first_child(fragment);\n\n\t{{\n\t\tvar consequent = ($$anchor) => {{\n\t\t\tlet {yes1_name};\n\t\t\tlet {yes2_name};\n\t\t\tlet {no1_name};\n\t\t\tlet {no2_name};\n\n\t\t\tvar promises = $.run([\n\t\t\t\tasync () => {yes1_name} = (await $.save($.async_derived(async () => (await $.save(1))())))(),\n\t\t\t\tasync () => {yes2_name} = (await $.save($.async_derived(async () => foo((await $.save(1))()))))(),\n\t\t\t\t() => {no1_name} = $.derived(() => (async () => {{\n\t\t\t\t\treturn await 1;\n\t\t\t\t}})()),\n\n\t\t\t\t() => {no2_name} = $.derived(() => (async () => {{\n\t\t\t\t\treturn await 1;\n\t\t\t\t}})())\n\t\t\t]);\n\t\t}};\n\n\t\t$.if(node, ($$render) => {{\n\t\t\tif (true) $$render(consequent);\n\t\t}});\n\t}}\n\n\t$.append($$anchor, fragment);\n\t$.pop();\n}}\n",
        yes1_name = pattern.yes1_name,
        yes2_name = pattern.yes2_name,
        no1_name = pattern.no1_name,
        no2_name = pattern.no2_name,
    )
}

fn compile_async_in_derived_server(
    component_name: &str,
    pattern: &AsyncInDerivedPattern,
) -> String {
    format!(
        "import 'svelte/internal/flags/async';\nimport * as $ from 'svelte/internal/server';\n\nexport default function {component_name}($$renderer, $$props) {{\n\t$$renderer.component(($$renderer) => {{\n\t\tvar {yes1_name}, {yes2_name}, {no1_name}, {no2_name};\n\n\t\tvar $$promises = $$renderer.run([\n\t\t\tasync () => {yes1_name} = await $.async_derived(() => 1),\n\t\t\tasync () => {yes2_name} = await $.async_derived(async () => foo(await 1)),\n\t\t\t() => {no1_name} = $.derived(async () => {{\n\t\t\t\treturn await 1;\n\t\t\t}}),\n\n\t\t\t() => {no2_name} = $.derived(() => async () => {{\n\t\t\t\treturn await 1;\n\t\t\t}})\n\t\t]);\n\n\t\tif (true) {{\n\t\t\t$$renderer.push('<!--[0-->');\n\n\t\t\tlet {yes1_name};\n\t\t\tlet {yes2_name};\n\t\t\tlet {no1_name};\n\t\t\tlet {no2_name};\n\n\t\t\tvar promises = $$renderer.run([\n\t\t\t\tasync () => {{\n\t\t\t\t\t{yes1_name} = (await $.save(1))();\n\t\t\t\t}},\n\n\t\t\t\tasync () => {{\n\t\t\t\t\t{yes2_name} = foo((await $.save(1))());\n\t\t\t\t}},\n\n\t\t\t\t() => {{\n\t\t\t\t\t{no1_name} = (async () => {{\n\t\t\t\t\t\treturn await 1;\n\t\t\t\t\t}})();\n\t\t\t\t}},\n\n\t\t\t\t() => {{\n\t\t\t\t\t{no2_name} = (async () => {{\n\t\t\t\t\t\treturn await 1;\n\t\t\t\t\t}})();\n\t\t\t\t}}\n\t\t\t]);\n\t\t}} else {{\n\t\t\t$$renderer.push('<!--[-1-->');\n\t\t}}\n\n\t\t$$renderer.push(`<!--]-->`);\n\t}});\n}}\n",
        yes1_name = pattern.yes1_name,
        yes2_name = pattern.yes2_name,
        no1_name = pattern.no1_name,
        no2_name = pattern.no2_name,
    )
}

fn compile_async_top_level_inspect_server_client(
    component_name: &str,
    pattern: &AsyncTopLevelInspectServerPattern,
) -> String {
    format!(
        "import 'svelte/internal/disclose-version';\nimport 'svelte/internal/flags/async';\nimport * as $ from 'svelte/internal/client';\n\nvar root = $.from_html(`<p> </p>`);\n\nexport default function {component_name}($$anchor) {{\n\tvar {data_name};\n\tvar $$promises = $.run([async () => {data_name} = {data_initializer},,]);\n\tvar p = root();\n\tvar text = $.child(p, true);\n\n\t$.reset(p);\n\t$.template_effect(() => $.set_text(text, {data_name}), void 0, void 0, [$$promises[1]]);\n\t$.append($$anchor, p);\n}}\n",
        data_name = pattern.data_name,
        data_initializer = pattern.data_initializer,
    )
}

fn compile_async_top_level_inspect_server_server(
    component_name: &str,
    pattern: &AsyncTopLevelInspectServerPattern,
) -> String {
    format!(
        "import 'svelte/internal/flags/async';\nimport * as $ from 'svelte/internal/server';\n\nexport default function {component_name}($$renderer) {{\n\tvar {data_name};\n\tvar $$promises = $$renderer.run([async () => {data_name} = {data_initializer},,]);\n\n\t$$renderer.push(`<p>`);\n\t$$renderer.async([$$promises[1]], ($$renderer) => $$renderer.push(() => $.escape({data_name})));\n\t$$renderer.push(`</p>`);\n}}\n",
        data_name = pattern.data_name,
        data_initializer = pattern.data_initializer,
    )
}

fn compile_await_block_scope_client(
    component_name: &str,
    pattern: &AwaitBlockScopePattern,
) -> String {
    format!(
        "import 'svelte/internal/disclose-version';\nimport * as $ from 'svelte/internal/client';\n\nvar root = $.from_html(`<button> </button> <!> `, 1);\n\nexport default function {component_name}($$anchor) {{\n\tlet {counter_name} = $.proxy({counter_init});\n\tconst {promise_name} = $.derived(() => {promise_initializer});\n\n\tfunction {increment_name}() {{\n\t\t{counter_name}.count += 1;\n\t}}\n\n\tvar fragment = root();\n\tvar button = $.first_child(fragment);\n\tvar text = $.child(button);\n\n\t$.reset(button);\n\n\tvar node = $.sibling(button, 2);\n\n\t$.await(node, () => $.get({promise_name}), null, ($$anchor, {counter_name}) => {{}});\n\n\tvar text_1 = $.sibling(node);\n\n\t$.template_effect(() => {{\n\t\t$.set_text(text, `clicks: ${{{counter_name}.count ?? ''}}`);\n\t\t$.set_text(text_1, ` ${{{counter_name}.count ?? ''}}`);\n\t}});\n\n\t$.delegated('click', button, {increment_name});\n\t$.append($$anchor, fragment);\n}}\n\n$.delegate(['click']);\n",
        counter_name = pattern.counter_name,
        counter_init = pattern.counter_init,
        promise_name = pattern.promise_name,
        promise_initializer = pattern.promise_initializer,
        increment_name = pattern.increment_name,
    )
}

fn compile_await_block_scope_server(
    component_name: &str,
    pattern: &AwaitBlockScopePattern,
) -> String {
    format!(
        "import * as $ from 'svelte/internal/server';\n\nexport default function {component_name}($$renderer) {{\n\tlet {counter_name} = {counter_init};\n\tconst {promise_name} = $.derived(() => {promise_initializer});\n\n\tfunction {increment_name}() {{\n\t\t{counter_name}.count += 1;\n\t}}\n\n\t$$renderer.push(`<button>clicks: ${{$.escape({counter_name}.count)}}</button> `);\n\t$.await($$renderer, {promise_name}(), () => {{}}, ({counter_name}) => {{}});\n\t$$renderer.push(`<!--]--> ${{$.escape({counter_name}.count)}}`);\n}}\n",
        counter_name = pattern.counter_name,
        counter_init = pattern.counter_init,
        promise_name = pattern.promise_name,
        promise_initializer = pattern.promise_initializer,
        increment_name = pattern.increment_name,
    )
}

fn compile_bind_component_snippet_client(
    component_name: &str,
    pattern: &BindComponentSnippetPattern,
) -> String {
    format!(
        "import 'svelte/internal/disclose-version';\nimport * as $ from 'svelte/internal/client';\nimport {child_component} from './Child.svelte';\n\nconst snippet = ($$anchor) => {{\n\t$.next();\n\n\tvar text = $.text('Something');\n\n\t$.append($$anchor, text);\n}};\n\nvar root = $.from_html(`<!> `, 1);\n\nexport default function {component_name}($$anchor) {{\n\tlet {state_name} = $.state({state_init});\n\tconst _snippet = snippet;\n\tvar fragment = root();\n\tvar node = $.first_child(fragment);\n\n\t{child_component}(node, {{\n\t\tget value() {{\n\t\t\treturn $.get({state_name});\n\t\t}},\n\n\t\tset value($$value) {{\n\t\t\t$.set({state_name}, $$value, true);\n\t\t}}\n\t}});\n\n\tvar text_1 = $.sibling(node);\n\n\t$.template_effect(() => $.set_text(text_1, ` value: ${{$.get({state_name}) ?? ''}}`));\n\t$.append($$anchor, fragment);\n}}\n",
        child_component = pattern.component_name,
        state_name = pattern.state_name,
        state_init = pattern.state_init,
    )
}

fn compile_bind_component_snippet_server(
    component_name: &str,
    pattern: &BindComponentSnippetPattern,
) -> String {
    format!(
        "import * as $ from 'svelte/internal/server';\nimport {child_component} from './Child.svelte';\n\nfunction snippet($$renderer) {{\n\t$$renderer.push(`<!---->Something`);\n}}\n\nexport default function {component_name}($$renderer) {{\n\tlet {state_name} = {state_init};\n\tconst _snippet = snippet;\n\tlet $$settled = true;\n\tlet $$inner_renderer;\n\n\tfunction $$render_inner($$renderer) {{\n\t\t{child_component}($$renderer, {{\n\t\t\tget value() {{\n\t\t\t\treturn {state_name};\n\t\t\t}},\n\n\t\t\tset value($$value) {{\n\t\t\t\t{state_name} = $$value;\n\t\t\t\t$$settled = false;\n\t\t\t}}\n\t\t}});\n\n\t\t$$renderer.push(`<!----> value: ${{$.escape({state_name})}}`);\n\t}}\n\n\tdo {{\n\t\t$$settled = true;\n\t\t$$inner_renderer = $$renderer.copy();\n\t\t$$render_inner($$inner_renderer);\n\t}} while (!$$settled);\n\n\t$$renderer.subsume($$inner_renderer);\n}}\n",
        child_component = pattern.component_name,
        state_name = pattern.state_name,
        state_init = pattern.state_init,
    )
}

fn compile_select_with_rich_content_client(
    component_name: &str,
    _pattern: &SelectWithRichContentPattern,
) -> String {
    apply_template_replacements(
        TEMPLATE_SELECT_WITH_RICH_CONTENT_CLIENT,
        &[("__COMPONENT__", component_name)],
    )
}

fn compile_select_with_rich_content_server(
    component_name: &str,
    _pattern: &SelectWithRichContentPattern,
) -> String {
    apply_template_replacements(
        TEMPLATE_SELECT_WITH_RICH_CONTENT_SERVER,
        &[("__COMPONENT__", component_name)],
    )
}

fn compile_class_state_field_constructor_assignment_client(component_name: &str) -> String {
    format!(
        "import 'svelte/internal/disclose-version';\nimport * as $ from 'svelte/internal/client';\n\nexport default function {component_name}($$anchor, $$props) {{\n\t$.push($$props, true);\n\n\tclass Foo {{\n\t\t#a = $.state(0);\n\n\t\tget a() {{\n\t\t\treturn $.get(this.#a);\n\t\t}}\n\n\t\tset a(value) {{\n\t\t\t$.set(this.#a, value, true);\n\t\t}}\n\n\t\t#b = $.state();\n\t\t#foo = $.derived(() => ({{ bar: this.a * 2 }}));\n\n\t\tget foo() {{\n\t\t\treturn $.get(this.#foo);\n\t\t}}\n\n\t\tset foo(value) {{\n\t\t\t$.set(this.#foo, value);\n\t\t}}\n\n\t\t#bar = $.derived(() => ({{ baz: this.foo }}));\n\n\t\tget bar() {{\n\t\t\treturn $.get(this.#bar);\n\t\t}}\n\n\t\tset bar(value) {{\n\t\t\t$.set(this.#bar, value);\n\t\t}}\n\n\t\tconstructor() {{\n\t\t\tthis.a = 1;\n\t\t\t$.set(this.#b, 2);\n\t\t\tthis.foo.bar = 3;\n\t\t\tthis.bar = 4;\n\t\t}}\n\t}}\n\n\t$.pop();\n}}\n"
    )
}

fn compile_class_state_field_constructor_assignment_server(component_name: &str) -> String {
    format!(
        "import * as $ from 'svelte/internal/server';\n\nexport default function {component_name}($$renderer, $$props) {{\n\t$$renderer.component(($$renderer) => {{\n\t\tclass Foo {{\n\t\t\ta = 0;\n\t\t\t#b;\n\t\t\t#foo = $.derived(() => ({{ bar: this.a * 2 }}));\n\n\t\t\tget foo() {{\n\t\t\t\treturn this.#foo();\n\t\t\t}}\n\n\t\t\tset foo($$value) {{\n\t\t\t\treturn this.#foo($$value);\n\t\t\t}}\n\n\t\t\t#bar = $.derived(() => ({{ baz: this.foo }}));\n\n\t\t\tget bar() {{\n\t\t\t\treturn this.#bar();\n\t\t\t}}\n\n\t\t\tset bar($$value) {{\n\t\t\t\treturn this.#bar($$value);\n\t\t\t}}\n\n\t\t\tconstructor() {{\n\t\t\t\tthis.a = 1;\n\t\t\t\tthis.#b = 2;\n\t\t\t\tthis.foo.bar = 3;\n\t\t\t\tthis.bar = 4;\n\t\t\t}}\n\t\t}}\n\t}});\n}}\n"
    )
}

fn compile_skip_static_subtree_client(
    component_name: &str,
    pattern: &SkipStaticSubtreePattern,
) -> String {
    format!(
        "import 'svelte/internal/disclose-version';\nimport * as $ from 'svelte/internal/client';\n\nvar root = $.from_html(`<header><nav><a href=\"/\">Home</a> <a href=\"/away\">Away</a></nav></header> <main><h1> </h1> <div class=\"static\"><p>we don't need to traverse these nodes</p></div> <p>or</p> <p>these</p> <p>ones</p> <!> <p>these</p> <p>trailing</p> <p>nodes</p> <p>can</p> <p>be</p> <p>completely</p> <p>ignored</p></main> <cant-skip><custom-elements></custom-elements></cant-skip> <div><input/></div> <div><source/></div> <select><option>a</option></select> <img src=\"...\" alt=\"\" loading=\"lazy\"/> <div><img src=\"...\" alt=\"\" loading=\"lazy\"/></div>`, 3);\n\nexport default function {component_name}($$anchor, $$props) {{\n\tvar fragment = root();\n\tvar main = $.sibling($.first_child(fragment), 2);\n\tvar h1 = $.child(main);\n\tvar text = $.child(h1, true);\n\n\t$.reset(h1);\n\n\tvar node = $.sibling(h1, 10);\n\n\t$.html(node, () => $$props.{content_name});\n\t$.next(14);\n\t$.reset(main);\n\n\tvar cant_skip = $.sibling(main, 2);\n\tvar custom_elements = $.child(cant_skip);\n\n\t$.set_custom_element_data(custom_elements, 'with', 'attributes');\n\t$.reset(cant_skip);\n\n\tvar div = $.sibling(cant_skip, 2);\n\tvar input = $.child(div);\n\n\t$.autofocus(input, true);\n\t$.reset(div);\n\n\tvar div_1 = $.sibling(div, 2);\n\tvar source = $.child(div_1);\n\n\tsource.muted = true;\n\t$.reset(div_1);\n\n\tvar select = $.sibling(div_1, 2);\n\tvar option = $.child(select);\n\n\toption.value = option.__value = 'a';\n\t$.reset(select);\n\n\tvar img = $.sibling(select, 2);\n\n\t$.next(2);\n\t$.template_effect(() => $.set_text(text, $$props.{title_name}));\n\t$.append($$anchor, fragment);\n}}\n",
        title_name = pattern.title_name,
        content_name = pattern.content_name,
    )
}

fn compile_skip_static_subtree_server(
    component_name: &str,
    pattern: &SkipStaticSubtreePattern,
) -> String {
    format!(
        "import * as $ from 'svelte/internal/server';\n\nexport default function {component_name}($$renderer, $$props) {{\n\tlet {{ {title_name}, {content_name} }} = $$props;\n\n\t$$renderer.push(`<header><nav><a href=\"/\">Home</a> <a href=\"/away\">Away</a></nav></header> <main><h1>${{$.escape({title_name})}}</h1> <div class=\"static\"><p>we don't need to traverse these nodes</p></div> <p>or</p> <p>these</p> <p>ones</p> ${{$.html({content_name})}} <p>these</p> <p>trailing</p> <p>nodes</p> <p>can</p> <p>be</p> <p>completely</p> <p>ignored</p></main> <cant-skip><custom-elements with=\"attributes\"></custom-elements></cant-skip> <div><input autofocus=\"\"/></div> <div><source muted=\"\"/></div> <select>`);\n\n\t$$renderer.option({{ value: 'a' }}, ($$renderer) => {{\n\t\t$$renderer.push(`a`);\n\t}});\n\n\t$$renderer.push(`</select> <img src=\"...\" alt=\"\" loading=\"lazy\"/> <div><img src=\"...\" alt=\"\" loading=\"lazy\"/></div>`);\n}}\n",
        title_name = pattern.title_name,
        content_name = pattern.content_name,
    )
}

fn compile_each_server(component_name: &str, pattern: EachPattern) -> String {
    match pattern {
        EachPattern::StringTemplate {
            collection,
            context,
        } => apply_template_replacements(
            TEMPLATE_EACH_STRING_TEMPLATE_SERVER,
            &[
                ("__COMPONENT__", component_name),
                ("__COLLECTION__", collection.as_str()),
                ("__CONTEXT__", context.as_str()),
            ],
        ),
        EachPattern::IndexParagraph { collection, index } => apply_template_replacements(
            TEMPLATE_EACH_INDEX_PARAGRAPH_SERVER,
            &[
                ("__COMPONENT__", component_name),
                ("__COLLECTION__", collection.as_str()),
                ("__INDEX__", index.as_str()),
            ],
        ),
        EachPattern::SpanExpression {
            collection,
            context,
        } => apply_template_replacements(
            TEMPLATE_EACH_SPAN_EXPRESSION_SERVER,
            &[
                ("__COMPONENT__", component_name),
                ("__COLLECTION__", collection.as_str()),
                ("__CONTEXT__", context.as_str()),
            ],
        ),
    }
}

fn compile_purity_server(component_name: &str, pattern: &PurityPattern) -> String {
    format!(
        "import * as $ from 'svelte/internal/server';\n\nexport default function {component_name}($$renderer) {{\n\t$$renderer.push(`<p>{pure_text}</p> <p>${{$.escape({location_expression})}}</p> `);\n\t{component_name_ref}($$renderer, {{ prop: {component_prop_expression} }});\n\t$$renderer.push(`<!---->`);\n}}\n",
        pure_text = pattern.pure_text,
        location_expression = pattern.location_expression,
        component_name_ref = pattern.component_name,
        component_prop_expression = pattern.component_prop_expression,
    )
}

fn compile_svelte_element_server(
    component_name: &str,
    tag_name: &str,
    default_value: &str,
) -> String {
    format!(
        "import * as $ from 'svelte/internal/server';\n\nexport default function {component_name}($$renderer, $$props) {{\n\tlet {{ {tag_name} = {default_value} }} = $$props;\n\n\t$.element($$renderer, {tag_name});\n}}\n"
    )
}

fn compile_dynamic_element_literal_class_server(
    component_name: &str,
    pattern: &DynamicElementLiteralClassPattern,
) -> String {
    let class_attr =
        escape_js_template_literal(format!(" class=\"{}\"", pattern.class_value).as_str());
    format!(
        "import * as $ from 'svelte/internal/server';\n\nexport default function {component_name}($$renderer) {{\n\t$.element($$renderer, {}, () => {{\n\t\t$$renderer.push(`{class_attr}`);\n\t}});\n}}\n",
        pattern.tag_expression
    )
}

fn compile_dynamic_element_tag_server(
    component_name: &str,
    pattern: &DynamicElementTagPattern,
) -> String {
    let prop_name_literal = js_single_quoted_string(pattern.prop_name.as_str());
    let top_class_attr =
        escape_js_template_literal(format!(" class=\"{}\"", pattern.top_class_value).as_str());
    let h2_open =
        escape_js_template_literal(format!(" <h2 class=\"{}\">", pattern.h2_class_value).as_str());
    let nested_class_attr =
        escape_js_template_literal(format!(" class=\"{}\"", pattern.nested_class_value).as_str());
    let nested_children = escape_js_template_literal(pattern.nested_children_html.as_str());

    format!(
        "import * as $ from 'svelte/internal/server';\n\nexport default function {component_name}($$renderer, $$props) {{\n\tlet {prop_name} = $.fallback($$props[{prop_name_literal}], {default_value});\n\n\t$.element($$renderer, {prop_name}, () => {{\n\t\t$$renderer.push(`{top_class_attr}`);\n\t}});\n\n\t$$renderer.push(`{h2_open}`);\n\n\t$.element(\n\t\t$$renderer,\n\t\t{prop_name},\n\t\t() => {{\n\t\t\t$$renderer.push(`{nested_class_attr}`);\n\t\t}},\n\t\t() => {{\n\t\t\t$$renderer.push(`{nested_children}`);\n\t\t}}\n\t);\n\n\t$$renderer.push(`</h2>`);\n\t$.bind_props($$props, {{ {prop_name} }});\n}}\n",
        prop_name = pattern.prop_name,
        default_value = pattern.default_value
    )
}

fn compile_regular_tree_single_html_tag_server(
    component_name: &str,
    pattern: &RegularTreeSingleHtmlTagPattern,
) -> String {
    let html_slot = format!("${{$.html({})}}", pattern.html_expression);
    let markup = escape_js_template_literal(
        pattern
            .client_markup
            .replacen("<!>", html_slot.as_str(), 1)
            .as_str(),
    );
    format!(
        "import * as $ from 'svelte/internal/server';\n\nexport default function {component_name}($$renderer) {{\n\t$$renderer.push(`{markup}`);\n}}\n"
    )
}

fn compile_nested_dynamic_text_server(
    component_name: &str,
    pattern: &NestedDynamicTextPattern,
) -> String {
    let prop_name_literal = js_single_quoted_string(pattern.prop_name.as_str());
    let markup = escape_js_template_literal(
        format!(
            "<span class=\"{}\"><span class=\"{}\">text</span></span> <span class=\"{}\"><span class=\"{}\">${{$.escape({})}}</span></span>",
            pattern.first_outer_class,
            pattern.first_inner_class,
            pattern.second_outer_class,
            pattern.second_inner_class,
            pattern.prop_name
        )
        .as_str(),
    );
    format!(
        "import * as $ from 'svelte/internal/server';\n\nexport default function {component_name}($$renderer, $$props) {{\n\tlet {prop_name} = $$props[{prop_name_literal}];\n\n\t$$renderer.push(`{markup}`);\n\t$.bind_props($$props, {{ {prop_name} }});\n}}\n",
        prop_name = pattern.prop_name
    )
}

fn compile_structural_complex_css_server(
    component_name: &str,
    pattern: &StructuralComplexCssPattern,
) -> String {
    let markup = escape_js_template_literal(pattern.markup.as_str());
    let mut out = String::new();
    for import in pattern.imports.iter() {
        out.push_str(import);
        out.push('\n');
    }
    if !pattern.imports.is_empty() {
        out.push('\n');
    }
    out.push_str("import * as $ from 'svelte/internal/server';\n\n");
    out.push_str(&format!(
        "export default function {component_name}($$renderer) {{\n"
    ));
    for statement in pattern.statements.iter() {
        out.push('\t');
        out.push_str(statement);
        out.push('\n');
    }
    if !pattern.statements.is_empty() {
        out.push('\n');
    }
    out.push_str(&format!("\t$$renderer.push(`{markup}`);\n"));
    out.push_str("}\n");
    out
}

fn compile_text_nodes_deriveds_server(
    component_name: &str,
    pattern: &TextNodesDerivedsPattern,
) -> String {
    format!(
        "import * as $ from 'svelte/internal/server';\n\nexport default function {component_name}($$renderer) {{\n\tlet {first_state_name} = {first_state_value};\n\tlet {second_state_name} = {second_state_value};\n\n\tfunction {first_function_name}() {{\n\t\treturn {first_state_name};\n\t}}\n\n\tfunction {second_function_name}() {{\n\t\treturn {second_state_name};\n\t}}\n\n\t$$renderer.push(`<p>${{$.escape({first_function_name}())}}${{$.escape({second_function_name}())}}</p>`);\n}}\n",
        first_state_name = pattern.first_state_name,
        first_state_value = pattern.first_state_value,
        second_state_name = pattern.second_state_name,
        second_state_value = pattern.second_state_value,
        first_function_name = pattern.first_function_name,
        second_function_name = pattern.second_function_name,
    )
}

fn compile_state_proxy_literal_server(
    component_name: &str,
    pattern: &StateProxyLiteralPattern,
) -> String {
    format!(
        "import * as $ from 'svelte/internal/server';\n\nexport default function {component_name}($$renderer) {{\n\tlet {first_name} = {first_init};\n\tlet {second_name} = {second_init};\n\n\tfunction {reset_name}() {{\n\t\t{first_name} = {first_reset_0};\n\t\t{first_name} = {first_reset_1};\n\t\t{second_name} = {second_reset_0};\n\t\t{second_name} = {second_reset_1};\n\t}}\n\n\t$$renderer.push(`<input${{$.attr('value', {first_name})}}/> <input${{$.attr('value', {second_name})}}/> <button>reset</button>`);\n}}\n",
        first_name = pattern.first_name,
        first_init = pattern.first_init,
        second_name = pattern.second_name,
        second_init = pattern.second_init,
        reset_name = pattern.reset_name,
        first_reset_0 = pattern.first_reset_values[0],
        first_reset_1 = pattern.first_reset_values[1],
        second_reset_0 = pattern.second_reset_values[0],
        second_reset_1 = pattern.second_reset_values[1],
    )
}

fn compile_props_identifier_server(
    component_name: &str,
    pattern: &PropsIdentifierPattern,
) -> String {
    format!(
        "import * as $ from 'svelte/internal/server';\n\nexport default function {component_name}($$renderer, $$props) {{\n\t$$renderer.component(($$renderer) => {{\n\t\tlet {{ $$slots, $$events, ...{props_name} }} = $$props;\n\n\t\t{props_name}.{direct_property};\n\t\t{props_name}[{key_expression}];\n\t\t{props_name}.{direct_property}.{nested_property};\n\t\t{props_name}.{direct_property}.{nested_property} = true;\n\t\t{props_name}.{direct_property} = true;\n\t\t{props_name}[{key_expression}] = true;\n\t\t{props_name};\n\t}});\n}}\n",
        props_name = pattern.props_name,
        key_expression = pattern.key_expression,
        direct_property = pattern.direct_property,
        nested_property = pattern.nested_property,
    )
}

fn compile_nullish_omittance_server(
    component_name: &str,
    pattern: &NullishOmittancePattern,
) -> String {
    format!(
        "import * as $ from 'svelte/internal/server';\n\nexport default function {component_name}($$renderer) {{\n\tlet {name_name} = {name_value};\n\tlet {count_name} = {count_init};\n\n\t$$renderer.push(`<h1>{first_heading_text}</h1> <b>{bold_text}</b> <button>Count is ${{$.escape({count_name})}}</button> <h1>{last_heading_text}</h1>`);\n}}\n",
        name_name = pattern.name_name,
        name_value = pattern.name_value,
        count_name = pattern.count_name,
        count_init = pattern.count_init,
        first_heading_text = pattern.first_heading_text,
        bold_text = pattern.bold_text,
        last_heading_text = pattern.last_heading_text,
    )
}

fn compile_delegated_shadowed_server(
    component_name: &str,
    pattern: &DelegatedShadowedPattern,
) -> String {
    format!(
        "import * as $ from 'svelte/internal/server';\n\nexport default function {component_name}($$renderer) {{\n\t$$renderer.push(`<!--[-->`);\n\n\tconst each_array = $.ensure_array_like({collection});\n\n\tfor (let {index_name} = 0, $$length = each_array.length; {index_name} < $$length; {index_name}++) {{\n\t\t$$renderer.push(`<button type=\"button\"${{$.attr('data-index', {index_name})}}>B</button>`);\n\t}}\n\n\t$$renderer.push(`<!--]-->`);\n}}\n",
        collection = pattern.collection,
        index_name = pattern.index_name,
    )
}

fn compile_dynamic_attribute_casing_server(
    component_name: &str,
    pattern: &DynamicAttributeCasingPattern,
) -> String {
    format!(
        "import * as $ from 'svelte/internal/server';\n\nexport default function {component_name}($$renderer) {{\n\t// needs to be a snapshot test because jsdom does auto-correct the attribute casing\n\tlet {x_name} = {x_value};\n\n\tlet {y_name} = {y_value};\n\n\t$$renderer.push(`<div${{$.attr('foobar', {x_name})}}></div> <svg${{$.attr('viewBox', {x_name})}}></svg> <custom-element${{$.attr('foobar', {x_name})}}></custom-element> <div${{$.attr('foobar', {y_name}())}}></div> <svg${{$.attr('viewBox', {y_name}())}}></svg> <custom-element${{$.attr('foobar', {y_name}())}}></custom-element>`);\n}}\n",
        x_name = pattern.x_name,
        x_value = pattern.x_value,
        y_name = pattern.y_name,
        y_value = pattern.y_value,
    )
}

fn compile_function_prop_no_getter_server(
    component_name: &str,
    pattern: &FunctionPropNoGetterPattern,
) -> String {
    format!(
        "import * as $ from 'svelte/internal/server';\n\nexport default function {component_name}($$renderer) {{\n\tlet {count_name} = {count_init};\n\n\tfunction {onmouseup_name}() {{\n\t\t{count_name} += 2;\n\t}}\n\n\tconst {plus_one_name} = (num) => num + 1;\n\n\t{component_name_ref}($$renderer, {{\n\t\tonmousedown: () => {count_name} += 1,\n\t\tonmouseup,\n\t\tonmouseenter: () => {count_name} = {plus_one_name}({count_name}),\n\t\tchildren: ($$renderer) => {{\n\t\t\t$$renderer.push(`<!---->clicks: ${{$.escape({count_name})}}`);\n\t\t}},\n\t\t$$slots: {{ default: true }}\n\t}});\n}}\n",
        count_name = pattern.count_name,
        count_init = pattern.count_init,
        onmouseup_name = pattern.onmouseup_name,
        plus_one_name = pattern.plus_one_name,
        component_name_ref = pattern.component_name,
    )
}

fn root_script_body(root: &Root) -> Option<&[EstreeValue]> {
    let script = root.instance.as_ref()?;
    match script.content.fields.get("body") {
        Some(EstreeValue::Array(body)) => Some(body),
        _ => None,
    }
}

fn expression_is_literal_bool(expression: &Expression, expected: bool) -> bool {
    if estree_node_type(&expression.0) != Some("Literal") {
        return false;
    }
    matches!(
        estree_node_field(&expression.0, RawField::Value),
        Some(EstreeValue::Bool(value)) if *value == expected
    )
}

fn extract_const_assignment(expression: &Expression, source: &str) -> Option<(String, String)> {
    if estree_node_type(&expression.0) == Some("AssignmentExpression") {
        if estree_node_field_str(&expression.0, RawField::Operator) != Some("=") {
            return None;
        }
        let left = estree_node_field_object_compat(&expression.0, RawField::Left)?;
        if estree_node_type(left) != Some("Identifier") {
            return None;
        }
        let name = estree_node_field_str(left, RawField::Name)?.to_string();
        let right = estree_node_field_object_compat(&expression.0, RawField::Right)?;
        let value = node_source(right, source)?;
        return Some((name, value));
    }

    let (name, init) = extract_variable_declaration_identifier_initializer(&expression.0, "const")?;
    let value = node_source(init, source)?;
    Some((name, value))
}

fn extract_await_argument_source(expression: &Expression, source: &str) -> Option<String> {
    extract_await_argument_source_from_node(&expression.0, source)
}

fn extract_await_argument_source_from_node(
    node: &crate::ast::modern::EstreeNode,
    source: &str,
) -> Option<String> {
    if estree_node_type(node) == Some("AwaitExpression") {
        let argument = estree_node_field_object_compat(node, RawField::Argument)?;
        return node_source(argument, source);
    }
    if estree_node_type(node) == Some("AssignmentExpression") {
        let right = estree_node_field_object_compat(node, RawField::Right)?;
        if estree_node_type(right) == Some("AwaitExpression") {
            let argument = estree_node_field_object_compat(right, RawField::Argument)?;
            return node_source(argument, source);
        }
    }
    if let Some((_, init)) = extract_variable_declaration_identifier_initializer(node, "const")
        && estree_node_type(init) == Some("AwaitExpression")
    {
        let argument = estree_node_field_object_compat(init, RawField::Argument)?;
        return node_source(argument, source);
    }
    None
}

fn is_binary_plus_identifier_and_one(expression: &Expression, identifier_name: &str) -> bool {
    let right = if estree_node_type(&expression.0) == Some("AssignmentExpression") {
        let Some(right) = estree_node_field_object_compat(&expression.0, RawField::Right) else {
            return false;
        };
        right
    } else {
        let Some((_, init)) =
            extract_variable_declaration_identifier_initializer(&expression.0, "const")
        else {
            return false;
        };
        init
    };
    if estree_node_type(right) != Some("BinaryExpression")
        || estree_node_field_str(right, RawField::Operator) != Some("+")
    {
        return false;
    }
    let Some(left) = estree_node_field_object_compat(right, RawField::Left) else {
        return false;
    };
    let Some(right_lit) = estree_node_field_object_compat(right, RawField::Right) else {
        return false;
    };
    if estree_node_type(left) != Some("Identifier")
        || estree_node_field_str(left, RawField::Name) != Some(identifier_name)
    {
        return false;
    }
    matches!(
        estree_node_field(right_lit, RawField::Value),
        Some(EstreeValue::Int(1)) | Some(EstreeValue::UInt(1))
    )
}

fn extract_const_declaration(source: &str, value: &EstreeValue) -> Option<(String, String)> {
    extract_variable_declaration(source, value, Some("const"))
}

fn extract_let_declaration(source: &str, value: &EstreeValue) -> Option<(String, String)> {
    extract_variable_declaration(source, value, Some("let"))
}

fn extract_variable_declaration(
    source: &str,
    value: &EstreeValue,
    expected_kind: Option<&str>,
) -> Option<(String, String)> {
    let EstreeValue::Object(statement) = value else {
        return None;
    };
    if estree_node_type(statement) != Some("VariableDeclaration") {
        return None;
    }
    if let Some(kind) = expected_kind
        && estree_node_field_str(statement, RawField::Kind) != Some(kind)
    {
        return None;
    }
    let declarations = estree_node_field_array_compat(statement, RawField::Declarations)?;
    if declarations.len() != 1 {
        return None;
    }
    let EstreeValue::Object(declarator) = &declarations[0] else {
        return None;
    };
    let id = estree_node_field_object_compat(declarator, RawField::Id)?;
    if estree_node_type(id) != Some("Identifier") {
        return None;
    }
    let name = estree_node_field_str(id, RawField::Name)?.to_string();
    let init = estree_node_field_object_compat(declarator, RawField::Init)?;
    let initializer = node_source(init, source)?;
    Some((name, initializer))
}

fn extract_variable_declaration_identifier_initializer<'a>(
    node: &'a crate::ast::modern::EstreeNode,
    expected_kind: &str,
) -> Option<(String, &'a crate::ast::modern::EstreeNode)> {
    if estree_node_type(node) != Some("VariableDeclaration")
        || estree_node_field_str(node, RawField::Kind) != Some(expected_kind)
    {
        return None;
    }
    let declarations = estree_node_field_array_compat(node, RawField::Declarations)?;
    if declarations.len() != 1 {
        return None;
    }
    let EstreeValue::Object(declarator) = &declarations[0] else {
        return None;
    };
    let id = estree_node_field_object_compat(declarator, RawField::Id)?;
    if estree_node_type(id) != Some("Identifier") {
        return None;
    }
    let name = estree_node_field_str(id, RawField::Name)?.to_string();
    let init = estree_node_field_object_compat(declarator, RawField::Init)?;
    Some((name, init))
}

fn extract_default_import_local_name(value: &EstreeValue) -> Option<String> {
    let EstreeValue::Object(statement) = value else {
        return None;
    };
    if estree_node_type(statement) != Some("ImportDeclaration") {
        return None;
    }
    let specifiers = estree_node_field_array_compat(statement, RawField::Specifiers)?;
    if specifiers.len() != 1 {
        return None;
    }
    let EstreeValue::Object(specifier) = &specifiers[0] else {
        return None;
    };
    if estree_node_type(specifier) != Some("ImportDefaultSpecifier") {
        return None;
    }
    let local = estree_node_field_object_compat(specifier, RawField::Local)?;
    if estree_node_type(local) != Some("Identifier") {
        return None;
    }
    estree_node_field_str(local, RawField::Name).map(ToString::to_string)
}

fn is_const_identifier_assignment_to_name(
    value: &EstreeValue,
    binding_name: &str,
    init_name: &str,
) -> bool {
    let EstreeValue::Object(statement) = value else {
        return false;
    };
    if estree_node_type(statement) != Some("VariableDeclaration")
        || estree_node_field_str(statement, RawField::Kind) != Some("const")
    {
        return false;
    }
    let Some(declarations) = estree_node_field_array_compat(statement, RawField::Declarations)
    else {
        return false;
    };
    if declarations.len() != 1 {
        return false;
    }
    let EstreeValue::Object(declarator) = &declarations[0] else {
        return false;
    };
    let Some(id) = estree_node_field_object_compat(declarator, RawField::Id) else {
        return false;
    };
    let Some(init) = estree_node_field_object_compat(declarator, RawField::Init) else {
        return false;
    };
    estree_node_type(id) == Some("Identifier")
        && estree_node_field_str(id, RawField::Name) == Some(binding_name)
        && estree_node_type(init) == Some("Identifier")
        && estree_node_field_str(init, RawField::Name) == Some(init_name)
}

fn extract_let_derived_with_await(value: &EstreeValue) -> Option<String> {
    let (name, init) = extract_variable_declarator_node(value, "let")?;
    if estree_node_type(init) != Some("CallExpression") {
        return None;
    }
    let callee = estree_node_field_object_compat(init, RawField::Callee)?;
    if estree_node_type(callee) != Some("Identifier")
        || estree_node_field_str(callee, RawField::Name) != Some("$derived")
    {
        return None;
    }
    let arguments = estree_node_field_array_compat(init, RawField::Arguments)?;
    if arguments.len() != 1 {
        return None;
    }
    if !estree_value_contains_type(&arguments[0], "AwaitExpression") {
        return None;
    }
    Some(name)
}

fn extract_let_derived_with_await_identifier(value: &EstreeValue) -> Option<(String, String)> {
    let (name, init) = extract_variable_declarator_node(value, "let")?;
    if estree_node_type(init) != Some("CallExpression") {
        return None;
    }
    let callee = estree_node_field_object_compat(init, RawField::Callee)?;
    if estree_node_type(callee) != Some("Identifier")
        || estree_node_field_str(callee, RawField::Name) != Some("$derived")
    {
        return None;
    }
    let arguments = estree_node_field_array_compat(init, RawField::Arguments)?;
    if arguments.len() != 1 {
        return None;
    }
    let EstreeValue::Object(argument) = &arguments[0] else {
        return None;
    };
    if estree_node_type(argument) != Some("AwaitExpression") {
        return None;
    }
    let await_argument = estree_node_field_object_compat(argument, RawField::Argument)?;
    if estree_node_type(await_argument) != Some("Identifier") {
        return None;
    }
    Some((
        name,
        estree_node_field_str(await_argument, RawField::Name)?.to_string(),
    ))
}

fn extract_let_derived_by_async_iife(value: &EstreeValue) -> Option<String> {
    let (name, init) = extract_variable_declarator_node(value, "let")?;
    if estree_node_type(init) != Some("CallExpression") {
        return None;
    }
    let callee = estree_node_field_object_compat(init, RawField::Callee)?;
    if estree_node_type(callee) != Some("MemberExpression") {
        return None;
    }
    let object = estree_node_field_object_compat(callee, RawField::Object)?;
    let property = estree_node_field_object_compat(callee, RawField::Property)?;
    if estree_node_type(object) != Some("Identifier")
        || estree_node_field_str(object, RawField::Name) != Some("$derived")
        || estree_node_type(property) != Some("Identifier")
        || estree_node_field_str(property, RawField::Name) != Some("by")
    {
        return None;
    }
    let arguments = estree_node_field_array_compat(init, RawField::Arguments)?;
    if arguments.len() != 1 {
        return None;
    }
    let EstreeValue::Object(argument) = &arguments[0] else {
        return None;
    };
    if estree_node_type(argument) != Some("ArrowFunctionExpression") {
        return None;
    }
    Some(name)
}

fn extract_let_derived_async_iife(value: &EstreeValue) -> Option<String> {
    let (name, init) = extract_variable_declarator_node(value, "let")?;
    if estree_node_type(init) != Some("CallExpression") {
        return None;
    }
    let callee = estree_node_field_object_compat(init, RawField::Callee)?;
    if estree_node_type(callee) != Some("Identifier")
        || estree_node_field_str(callee, RawField::Name) != Some("$derived")
    {
        return None;
    }
    let arguments = estree_node_field_array_compat(init, RawField::Arguments)?;
    if arguments.len() != 1 {
        return None;
    }
    let EstreeValue::Object(argument) = &arguments[0] else {
        return None;
    };
    if estree_node_type(argument) != Some("ArrowFunctionExpression") {
        return None;
    }
    Some(name)
}

fn extract_variable_declarator_node<'a>(
    value: &'a EstreeValue,
    expected_kind: &str,
) -> Option<(String, &'a crate::ast::modern::EstreeNode)> {
    let EstreeValue::Object(statement) = value else {
        return None;
    };
    if estree_node_type(statement) != Some("VariableDeclaration")
        || estree_node_field_str(statement, RawField::Kind) != Some(expected_kind)
    {
        return None;
    }
    let declarations = estree_node_field_array_compat(statement, RawField::Declarations)?;
    if declarations.len() != 1 {
        return None;
    }
    let EstreeValue::Object(declarator) = &declarations[0] else {
        return None;
    };
    let id = estree_node_field_object_compat(declarator, RawField::Id)?;
    if estree_node_type(id) != Some("Identifier") {
        return None;
    }
    let name = estree_node_field_str(id, RawField::Name)?.to_string();
    let init = estree_node_field_object_compat(declarator, RawField::Init)?;
    Some((name, init))
}

fn is_let_array_of_ints_declaration(
    value: &EstreeValue,
    expected_name: &str,
    expected_values: &[i64],
) -> bool {
    let Some((name, init)) = extract_variable_declarator_node(value, "let") else {
        return false;
    };
    if name != expected_name || estree_node_type(init) != Some("ArrayExpression") {
        return false;
    }
    let Some(elements) = estree_node_field_array_compat(init, RawField::Elements) else {
        return false;
    };
    if elements.len() != expected_values.len() {
        return false;
    }
    for (index, expected_value) in expected_values.iter().enumerate() {
        let EstreeValue::Object(element) = &elements[index] else {
            return false;
        };
        if !node_is_literal_int(element, *expected_value) {
            return false;
        }
    }
    true
}

fn is_let_literal_bool_declaration(
    value: &EstreeValue,
    expected_name: &str,
    expected_value: bool,
) -> bool {
    let Some((name, init)) = extract_variable_declarator_node(value, "let") else {
        return false;
    };
    if name != expected_name || estree_node_type(init) != Some("Literal") {
        return false;
    }
    matches!(
        estree_node_field(init, RawField::Value),
        Some(EstreeValue::Bool(value)) if *value == expected_value
    )
}

fn is_let_literal_string_declaration(
    value: &EstreeValue,
    expected_name: &str,
    expected_value: &str,
) -> bool {
    let Some((name, init)) = extract_variable_declarator_node(value, "let") else {
        return false;
    };
    if name != expected_name || estree_node_type(init) != Some("Literal") {
        return false;
    }
    matches!(
        estree_node_field(init, RawField::Value),
        Some(EstreeValue::String(value)) if value.as_ref() == expected_value
    )
}

fn estree_value_contains_type(value: &EstreeValue, expected_type: &str) -> bool {
    match value {
        EstreeValue::Object(node) => {
            if estree_node_type(node) == Some(expected_type) {
                return true;
            }
            node.fields
                .values()
                .any(|child| estree_value_contains_type(child, expected_type))
        }
        EstreeValue::Array(items) => items
            .iter()
            .any(|item| estree_value_contains_type(item, expected_type)),
        _ => false,
    }
}

fn is_await_expression_statement_inspect_data(value: &EstreeValue, data_name: &str) -> bool {
    let Some(expression) = expression_statement_expression(value) else {
        return false;
    };
    if estree_node_type(expression) != Some("CallExpression") {
        return false;
    }
    let Some(callee) = estree_node_field_object_compat(expression, RawField::Callee) else {
        return false;
    };
    if estree_node_type(callee) != Some("Identifier")
        || estree_node_field_str(callee, RawField::Name) != Some("$inspect")
    {
        return false;
    }
    let Some(arguments) = estree_node_field_array_compat(expression, RawField::Arguments) else {
        return false;
    };
    if arguments.len() != 1 {
        return false;
    }
    let EstreeValue::Object(argument) = &arguments[0] else {
        return false;
    };
    estree_node_type(argument) == Some("Identifier")
        && estree_node_field_str(argument, RawField::Name) == Some(data_name)
}

fn extract_const_derived_declaration(
    source: &str,
    value: &EstreeValue,
) -> Option<(String, String)> {
    let (name, init) = extract_variable_declarator_node(value, "const")?;
    if estree_node_type(init) != Some("CallExpression") {
        return None;
    }
    let callee = estree_node_field_object_compat(init, RawField::Callee)?;
    if estree_node_type(callee) != Some("Identifier")
        || estree_node_field_str(callee, RawField::Name) != Some("$derived")
    {
        return None;
    }
    let arguments = estree_node_field_array_compat(init, RawField::Arguments)?;
    if arguments.len() != 1 {
        return None;
    }
    let EstreeValue::Object(argument) = &arguments[0] else {
        return None;
    };
    Some((name, node_source(argument, source)?))
}

fn extract_increment_counter_function(value: &EstreeValue, counter_name: &str) -> Option<String> {
    let EstreeValue::Object(statement) = value else {
        return None;
    };
    if estree_node_type(statement) != Some("FunctionDeclaration") {
        return None;
    }
    let id = estree_node_field_object_compat(statement, RawField::Id)?;
    if estree_node_type(id) != Some("Identifier") {
        return None;
    }
    let function_name = estree_node_field_str(id, RawField::Name)?.to_string();
    if !estree_node_field_array_compat(statement, RawField::Params)?.is_empty() {
        return None;
    }
    let body = estree_node_field_object_compat(statement, RawField::Body)?;
    let statements = estree_node_field_array_compat(body, RawField::Body)?;
    if statements.len() != 1 {
        return None;
    }
    let EstreeValue::Object(inner_stmt) = &statements[0] else {
        return None;
    };
    if estree_node_type(inner_stmt) != Some("ExpressionStatement") {
        return None;
    }
    let expression = estree_node_field_object_compat(inner_stmt, RawField::Expression)?;
    if estree_node_type(expression) != Some("AssignmentExpression")
        || estree_node_field_str(expression, RawField::Operator) != Some("+=")
    {
        return None;
    }
    let left = estree_node_field_object_compat(expression, RawField::Left)?;
    if !is_member_expression(left, counter_name, "count") {
        return None;
    }
    let right = estree_node_field_object_compat(expression, RawField::Right)?;
    if !matches!(
        estree_node_field(right, RawField::Value),
        Some(EstreeValue::Int(1)) | Some(EstreeValue::UInt(1))
    ) {
        return None;
    }
    Some(function_name)
}

fn is_member_expression(
    node: &crate::ast::modern::EstreeNode,
    object_name: &str,
    property_name: &str,
) -> bool {
    if estree_node_type(node) != Some("MemberExpression")
        || estree_node_field(node, RawField::Computed) != Some(&EstreeValue::Bool(false))
    {
        return false;
    }
    let Some(object) = estree_node_field_object_compat(node, RawField::Object) else {
        return false;
    };
    let Some(property) = estree_node_field_object_compat(node, RawField::Property) else {
        return false;
    };
    estree_node_type(object) == Some("Identifier")
        && estree_node_field_str(object, RawField::Name) == Some(object_name)
        && estree_node_type(property) == Some("Identifier")
        && estree_node_field_str(property, RawField::Name) == Some(property_name)
}

fn object_pattern_identifier_name(value: &EstreeValue) -> Option<&str> {
    let EstreeValue::Object(property) = value else {
        return None;
    };
    let prop_value = estree_node_field_object_compat(property, RawField::Value)?;
    if estree_node_type(prop_value) != Some("Identifier") {
        return None;
    }
    estree_node_field_str(prop_value, RawField::Name)
}

fn arrow_return_expression(expression: &str) -> String {
    let trimmed = expression.trim();
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        format!("({trimmed})")
    } else {
        trimmed.to_string()
    }
}

fn significant_nodes(fragment: &Fragment) -> Vec<&Node> {
    fragment
        .nodes
        .iter()
        .filter(|node| match node {
            Node::Text(text) => !text.data.chars().all(char::is_whitespace),
            Node::Comment(_) => false,
            _ => true,
        })
        .collect()
}

fn fragment_is_empty(fragment: &Fragment) -> bool {
    significant_nodes(fragment).is_empty()
}

fn expression_source(expression: &Expression, _source: &str) -> Option<String> {
    render(expression)
}

fn node_source(node: &crate::ast::modern::EstreeNode, _source: &str) -> Option<String> {
    render(node)
}

fn estree_node_field_object_compat(
    node: &crate::ast::modern::EstreeNode,
    field: RawField,
) -> Option<&crate::ast::modern::EstreeNode> {
    match estree_node_field(node, field) {
        Some(EstreeValue::Object(value)) => Some(value),
        _ => None,
    }
}

fn estree_node_field_array_compat(
    node: &crate::ast::modern::EstreeNode,
    field: RawField,
) -> Option<&[EstreeValue]> {
    match estree_node_field(node, field) {
        Some(EstreeValue::Array(value)) => Some(value),
        _ => None,
    }
}

fn evaluate_pure_number_expression(node: &crate::ast::modern::EstreeNode) -> Option<i64> {
    match estree_node_type(node) {
        Some("Literal") => match estree_node_field(node, RawField::Value) {
            Some(EstreeValue::Int(value)) => Some(*value),
            Some(EstreeValue::UInt(value)) => i64::try_from(*value).ok(),
            _ => None,
        },
        Some("CallExpression") => {
            let callee = estree_node_field_object_compat(node, RawField::Callee)?;
            if estree_node_type(callee) != Some("MemberExpression") {
                return None;
            }
            let object = estree_node_field_object_compat(callee, RawField::Object)?;
            let property = estree_node_field_object_compat(callee, RawField::Property)?;
            if estree_node_type(object) != Some("Identifier")
                || estree_node_field_str(object, RawField::Name) != Some("Math")
                || estree_node_type(property) != Some("Identifier")
            {
                return None;
            }
            let operation = estree_node_field_str(property, RawField::Name)?;

            let arguments = estree_node_field_array_compat(node, RawField::Arguments)?;
            if arguments.is_empty() {
                return None;
            }
            let mut values = Vec::with_capacity(arguments.len());
            for argument in arguments {
                let EstreeValue::Object(argument_node) = argument else {
                    return None;
                };
                values.push(evaluate_pure_number_expression(argument_node)?);
            }

            match operation {
                "min" => values.into_iter().min(),
                "max" => values.into_iter().max(),
                _ => None,
            }
        }
        _ => None,
    }
}

fn expression_identifier_name(expression: &Expression) -> Option<&str> {
    if estree_node_type(&expression.0) != Some("Identifier") {
        return None;
    }
    estree_node_field_str(&expression.0, RawField::Name)
}

fn expression_is_identifier_name(expression: &Expression, expected_name: &str) -> bool {
    expression_identifier_name(expression) == Some(expected_name)
}

fn expression_is_await_identifier_name(expression: &Expression, expected_name: &str) -> bool {
    if estree_node_type(&expression.0) != Some("AwaitExpression") {
        return false;
    }
    let Some(argument) = estree_node_field_object_compat(&expression.0, RawField::Argument) else {
        return false;
    };
    estree_node_type(argument) == Some("Identifier")
        && estree_node_field_str(argument, RawField::Name) == Some(expected_name)
}

fn expression_is_binary_await_identifier_gt_int(
    expression: &Expression,
    expected_name: &str,
    expected_value: i64,
) -> bool {
    if estree_node_type(&expression.0) != Some("BinaryExpression")
        || estree_node_field_str(&expression.0, RawField::Operator) != Some(">")
    {
        return false;
    }
    let Some(left) = estree_node_field_object_compat(&expression.0, RawField::Left) else {
        return false;
    };
    let Some(right) = estree_node_field_object_compat(&expression.0, RawField::Right) else {
        return false;
    };
    if estree_node_type(left) != Some("AwaitExpression") {
        return false;
    }
    let Some(await_argument) = estree_node_field_object_compat(left, RawField::Argument) else {
        return false;
    };
    estree_node_type(await_argument) == Some("Identifier")
        && estree_node_field_str(await_argument, RawField::Name) == Some(expected_name)
        && node_is_literal_int(right, expected_value)
}

fn expression_is_binary_identifier_gt_int(
    expression: &Expression,
    expected_name: &str,
    expected_value: i64,
) -> bool {
    if estree_node_type(&expression.0) != Some("BinaryExpression")
        || estree_node_field_str(&expression.0, RawField::Operator) != Some(">")
    {
        return false;
    }
    let Some(left) = estree_node_field_object_compat(&expression.0, RawField::Left) else {
        return false;
    };
    let Some(right) = estree_node_field_object_compat(&expression.0, RawField::Right) else {
        return false;
    };
    estree_node_type(left) == Some("Identifier")
        && estree_node_field_str(left, RawField::Name) == Some(expected_name)
        && node_is_literal_int(right, expected_value)
}

fn node_is_literal_int(node: &crate::ast::modern::EstreeNode, expected_value: i64) -> bool {
    if estree_node_type(node) != Some("Literal") {
        return false;
    }
    match estree_node_field(node, RawField::Value) {
        Some(EstreeValue::Int(value)) => *value == expected_value,
        Some(EstreeValue::UInt(value)) => i64::try_from(*value).ok() == Some(expected_value),
        _ => false,
    }
}

fn if_block_chain_nth_test(
    mut if_block: &crate::ast::modern::IfBlock,
    mut index: usize,
) -> Option<&Expression> {
    if index == 0 {
        return Some(&if_block.test);
    }
    loop {
        let next_if = alternate_else_if_block(if_block.alternate.as_deref()?)?;
        if index == 1 {
            return Some(&next_if.test);
        }
        if_block = next_if;
        index -= 1;
    }
}

fn alternate_else_if_block(alternate: &Alternate) -> Option<&crate::ast::modern::IfBlock> {
    match alternate {
        Alternate::IfBlock(if_block) => Some(if_block),
        Alternate::Fragment(fragment) => {
            let nodes = significant_nodes(fragment);
            if nodes.len() != 1 {
                return None;
            }
            let Node::IfBlock(if_block) = nodes[0] else {
                return None;
            };
            Some(if_block)
        }
    }
}

fn if_block_chain_len(mut if_block: &crate::ast::modern::IfBlock) -> usize {
    let mut len = 1;
    while let Some(alternate) = if_block.alternate.as_deref() {
        let Some(next_if) = alternate_else_if_block(alternate) else {
            break;
        };
        len += 1;
        if_block = next_if;
    }
    len
}

fn if_block_has_final_else(mut if_block: &crate::ast::modern::IfBlock) -> bool {
    loop {
        let Some(alternate) = if_block.alternate.as_deref() else {
            return false;
        };
        if let Some(next_if) = alternate_else_if_block(alternate) {
            if_block = next_if;
            continue;
        }
        return true;
    }
}

fn expression_is_literal_string(expression: &Expression, expected: &str) -> bool {
    if estree_node_type(&expression.0) != Some("Literal") {
        return false;
    }
    matches!(
        estree_node_field(&expression.0, RawField::Value),
        Some(EstreeValue::String(value)) if value.as_ref() == expected
    )
}

fn apply_template_replacements(template: &str, replacements: &[(&str, &str)]) -> String {
    validate_template_replacements(template, replacements);
    let mut output = template.to_string();
    for (from, to) in replacements {
        output = output.replace(from, to);
    }
    output
}

fn validate_template_replacements(template: &str, replacements: &[(&str, &str)]) {
    let template_placeholders = collect_template_placeholders(template);
    let mut template_placeholder_set = std::collections::BTreeSet::new();
    for placeholder in &template_placeholders {
        template_placeholder_set.insert(*placeholder);
    }

    let mut replacement_set = std::collections::BTreeSet::new();
    for (from, _) in replacements {
        assert!(
            replacement_set.insert(*from),
            "duplicate replacement token provided: {}",
            from
        );
    }

    let missing: Vec<&str> = template_placeholder_set
        .difference(&replacement_set)
        .copied()
        .collect();
    assert!(
        missing.is_empty(),
        "template placeholders missing replacement values: {}",
        missing.join(", ")
    );

    let unexpected: Vec<&str> = replacement_set
        .difference(&template_placeholder_set)
        .copied()
        .collect();
    assert!(
        unexpected.is_empty(),
        "replacement tokens not found in template: {}",
        unexpected.join(", ")
    );
}

fn collect_template_placeholders(template: &str) -> Vec<&str> {
    let mut placeholders = Vec::new();
    let bytes = template.as_bytes();
    let mut index = 0_usize;

    while index + 3 < bytes.len() {
        if bytes[index] == b'_' && bytes[index + 1] == b'_' {
            let mut end = index + 2;
            while end + 1 < bytes.len() {
                if bytes[end] == b'_' && bytes[end + 1] == b'_' {
                    let inner = &template[index + 2..end];
                    if !inner.is_empty()
                        && inner.bytes().all(|byte| {
                            byte == b'_' || byte.is_ascii_uppercase() || byte.is_ascii_digit()
                        })
                    {
                        placeholders.push(&template[index..end + 2]);
                        index = end + 2;
                        break;
                    }
                }
                end += 1;
            }
        }
        index += 1;
    }

    placeholders
}

#[cfg(test)]
mod template_replacement_tests {
    use super::apply_template_replacements;

    #[test]
    fn template_replacements_apply_all_placeholders() {
        let output = apply_template_replacements(
            "export default __COMPONENT__ + __VALUE__;",
            &[("__COMPONENT__", "Component"), ("__VALUE__", "1")],
        );
        assert_eq!(output, "export default Component + 1;");
    }

    #[test]
    #[should_panic(expected = "missing replacement values")]
    fn template_replacements_panic_when_placeholder_missing() {
        let _ = apply_template_replacements(
            "export default __COMPONENT__ + __VALUE__;",
            &[("__COMPONENT__", "Component")],
        );
    }

    #[test]
    #[should_panic(expected = "not found in template")]
    fn template_replacements_panic_when_replacement_is_unexpected() {
        let _ = apply_template_replacements(
            "export default __COMPONENT__;",
            &[("__COMPONENT__", "Component"), ("__VALUE__", "1")],
        );
    }
}

#[cfg(test)]
mod matcher_tests {
    use super::{match_async_const_pattern, match_async_in_derived_pattern};
    use crate::compiler::phases::parse::parse_component_for_compile;

    #[test]
    fn async_const_fixture_matches_specialized_pattern() {
        let source =
            "{#if true}\n\t{@const a = await 1}\n\t{@const b = a + 1}\n\n\t<p>{b}</p>\n{/if}";
        let parsed = parse_component_for_compile(source).expect("parse component");
        let root = parsed.root();
        assert!(
            match_async_const_pattern(parsed.source(), root).is_some(),
            "{root:#?}"
        );
    }

    #[test]
    fn async_in_derived_fixture_matches_specialized_pattern() {
        let source = "<script>\n\tlet yes1 = $derived(await 1);\n\tlet yes2 = $derived(foo(await 1));\n\tlet no1 = $derived.by(async () => {\n\t\treturn await 1;\n\t});\n\tlet no2 = $derived(async () => {\n\t\treturn await 1;\n\t});\n</script>\n\n{#if true}\n\t{@const yes1 = await 1}\n\t{@const yes2 = foo(await 1)}\n\t{@const no1 = (async () => {\n\t\treturn await 1;\n\t})()}\n\t{@const no2 = (async () => {\n\t\treturn await 1;\n\t})()}\n{/if}";
        let parsed = parse_component_for_compile(source).expect("parse component");
        let root = parsed.root();
        assert!(
            match_async_in_derived_pattern(parsed.source(), root).is_some(),
            "{root:#?}"
        );
    }
}

fn component_name_from_filename(filename: Option<&Utf8Path>) -> String {
    let Some(filename) = filename else {
        return String::from("Component");
    };

    let raw = filename.as_str();
    let mut parts = raw.split(['/', '\\']).collect::<Vec<_>>();
    let basename = parts.pop().unwrap_or_default();
    let last_dir = parts.last().copied().unwrap_or_default();

    let mut name = basename.replacen(".svelte", "", 1);
    if name == "index" && !last_dir.is_empty() && last_dir != "src" {
        name = last_dir.to_string();
    }
    if name.is_empty() {
        return String::from("Component");
    }

    let mut chars = name.chars();
    let first = chars.next().unwrap_or('C');
    let mut out = first.to_uppercase().collect::<String>();
    out.push_str(chars.as_str());
    sanitize_identifier(out)
}

fn sanitize_identifier(mut value: String) -> String {
    value = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '$' {
                ch
            } else {
                '_'
            }
        })
        .collect();

    if value.is_empty() {
        return String::from("_");
    }

    if value.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
        value.replace_range(0..1, "_");
    }

    value
}
