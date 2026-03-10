//! CSS usage context built from AST traversal.

use std::collections::{BTreeMap, HashSet};

use crate::api::modern::{
    RawField, estree_node_field, estree_node_field_array, estree_node_field_object,
    estree_node_field_str, estree_node_type,
};
use crate::ast::modern::{
    Attribute, AttributeValue, AttributeValueList, Expression, Fragment, Node, Root,
};

#[derive(Debug, Default)]
pub(crate) struct CssUsageContext {
    pub(crate) classes: HashSet<String>,
    pub(crate) ids: HashSet<String>,
    pub(crate) tags: HashSet<String>,
    pub(crate) dynamic_attributes: HashSet<String>,
    pub(crate) class_name_unbounded: bool,
    pub(crate) render_parent_elements: HashSet<usize>,
    pub(crate) root_has_render: bool,
    pub(crate) has_render_tags: bool,
    pub(crate) has_spread_attributes: bool,
    pub(crate) allow_pruning: bool,
    pub(crate) has_dynamic_svelte_element: bool,
    pub(crate) has_dynamic_markup: bool,
    pub(crate) has_each_blocks: bool,
    pub(crate) has_non_each_dynamic_markup: bool,
    pub(crate) has_slot_tags: bool,
    pub(crate) has_component_like_elements: bool,
    pub(crate) dev: bool,
    pub(crate) each_first_tags: HashSet<String>,
    pub(crate) each_first_classes: HashSet<String>,
    pub(crate) each_last_tags: HashSet<String>,
    pub(crate) each_last_classes: HashSet<String>,
    pub(crate) each_before_tags: HashSet<String>,
    pub(crate) each_before_classes: HashSet<String>,
    pub(crate) each_after_tags: HashSet<String>,
    pub(crate) each_after_classes: HashSet<String>,
    pub(crate) elements: Vec<CssElementUsage>,
}

#[derive(Debug, Clone)]
pub(crate) struct CssElementUsage {
    pub(crate) tag: String,
    pub(crate) attributes: BTreeMap<String, String>,
    pub(crate) parent: Option<usize>,
    pub(crate) depth: usize,
    pub(crate) component_depth: Option<usize>,
    pub(crate) optional: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct CssAttributeFilter {
    pub(crate) name: String,
    pub(crate) value: String,
    pub(crate) case_insensitive: bool,
    pub(crate) match_kind: CssAttributeMatchKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CssAttributeMatchKind {
    Exact,
    Word,
    Prefix,
    Suffix,
    Contains,
    Dash,
}

#[derive(Clone, Copy)]
pub(crate) enum EachBoundaryKind {
    First,
    Last,
    Before,
    After,
}

#[derive(Clone, Copy)]
pub(crate) struct BoundaryCandidates<'a> {
    pub(crate) tags: &'a HashSet<String>,
    pub(crate) classes: &'a HashSet<String>,
}

impl CssUsageContext {
    pub(crate) fn each_boundary_candidates(
        &self,
        kind: EachBoundaryKind,
    ) -> BoundaryCandidates<'_> {
        match kind {
            EachBoundaryKind::First => BoundaryCandidates {
                tags: &self.each_first_tags,
                classes: &self.each_first_classes,
            },
            EachBoundaryKind::Last => BoundaryCandidates {
                tags: &self.each_last_tags,
                classes: &self.each_last_classes,
            },
            EachBoundaryKind::Before => BoundaryCandidates {
                tags: &self.each_before_tags,
                classes: &self.each_before_classes,
            },
            EachBoundaryKind::After => BoundaryCandidates {
                tags: &self.each_after_tags,
                classes: &self.each_after_classes,
            },
        }
    }
}

fn is_void_element(tag: &str) -> bool {
    matches!(
        tag,
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

const MAX_CLASS_VALUE_VARIANTS: usize = 128;

#[derive(Default)]
struct ClassValueAnalysis {
    tokens: HashSet<String>,
    unbounded: bool,
}

fn analyze_class_expression(expression: &Expression) -> ClassValueAnalysis {
    if let Some(variants) = class_string_variants_from_raw(&expression.0) {
        return ClassValueAnalysis {
            tokens: class_tokens_from_variants(&variants),
            unbounded: false,
        };
    }

    let mut tokens = HashSet::new();
    let bounded = collect_class_candidates_from_raw(&expression.0, &mut tokens);
    ClassValueAnalysis {
        tokens,
        unbounded: !bounded,
    }
}

fn collect_class_candidates_from_raw(
    raw: &crate::ast::modern::EstreeNode,
    classes: &mut HashSet<String>,
) -> bool {
    let ty = match estree_node_type(raw) {
        Some(t) => t,
        None => return false,
    };

    match ty {
        "ArrayExpression" => {
            let Some(elements) = estree_node_field_array(raw, RawField::Elements) else {
                return false;
            };

            let mut bounded = true;
            for value in elements.iter() {
                let Some(node) = estree_value_to_object(value) else {
                    bounded = false;
                    continue;
                };
                bounded &= collect_class_candidates_from_raw(node, classes);
            }
            bounded
        }
        "ObjectExpression" => {
            let Some(properties) = estree_node_field_array(raw, RawField::Properties) else {
                return false;
            };

            let mut bounded = true;
            for value in properties.iter() {
                let Some(node) = estree_value_to_object(value) else {
                    bounded = false;
                    continue;
                };

                match estree_node_type(node) {
                    Some("Property") => {
                        if matches!(
                            estree_node_field(node, RawField::Computed),
                            Some(crate::ast::modern::EstreeValue::Bool(true))
                        ) {
                            bounded = false;
                            continue;
                        }
                        let Some(key) = estree_node_field_object(node, RawField::Key) else {
                            bounded = false;
                            continue;
                        };
                        bounded &= extract_object_property_key_tokens(key, classes);
                    }
                    Some("SpreadElement") => bounded = false,
                    _ => bounded = false,
                }
            }
            bounded
        }
        "ConditionalExpression" => {
            let Some(consequent) = estree_node_field_object(raw, RawField::Consequent) else {
                return false;
            };
            let Some(alternate) = estree_node_field_object(raw, RawField::Alternate) else {
                return false;
            };
            let left_bounded = collect_class_candidates_from_raw(consequent, classes);
            let right_bounded = collect_class_candidates_from_raw(alternate, classes);
            left_bounded && right_bounded
        }
        "LogicalExpression" => {
            let operator = estree_node_field_str(raw, RawField::Operator).unwrap_or_default();
            let Some(right) = estree_node_field_object(raw, RawField::Right) else {
                return false;
            };

            let mut bounded = collect_class_candidates_from_raw(right, classes);
            if operator != "&&" {
                let Some(left) = estree_node_field_object(raw, RawField::Left) else {
                    return false;
                };
                bounded &= collect_class_candidates_from_raw(left, classes);
            }
            bounded
        }
        "CallExpression" => {
            let Some(arguments) = estree_node_field_array(raw, RawField::Arguments) else {
                return false;
            };

            let mut bounded = true;
            for value in arguments.iter() {
                let Some(node) = estree_value_to_object(value) else {
                    bounded = false;
                    continue;
                };
                bounded &= collect_class_candidates_from_raw(node, classes);
            }
            bounded
        }
        "BinaryExpression" | "TemplateLiteral" => {
            if let Some(variants) = class_string_variants_from_raw(raw) {
                classes.extend(class_tokens_from_variants(&variants));
                true
            } else {
                false
            }
        }
        "Literal" => {
            if let Some(value) = estree_string_value(raw) {
                add_class_tokens_from_string(value, classes);
            }
            true
        }
        "TSAsExpression"
        | "TSSatisfiesExpression"
        | "TSNonNullExpression"
        | "ParenthesizedExpression"
        | "ChainExpression" => estree_node_field_object(raw, RawField::Expression)
            .is_some_and(|inner| collect_class_candidates_from_raw(inner, classes)),
        "Identifier" => false,
        _ => false,
    }
}

fn estree_value_to_object(
    val: &crate::ast::modern::EstreeValue,
) -> Option<&crate::ast::modern::EstreeNode> {
    match val {
        crate::ast::modern::EstreeValue::Object(n) => Some(n),
        _ => None,
    }
}

fn extract_string_tokens_from_raw_value(
    val: &crate::ast::modern::EstreeValue,
    classes: &mut HashSet<String>,
) {
    if let crate::ast::modern::EstreeValue::String(s) = val {
        add_class_tokens_from_string(s, classes);
    }
}

fn extract_object_property_key_tokens(
    node: &crate::ast::modern::EstreeNode,
    classes: &mut HashSet<String>,
) -> bool {
    match estree_node_type(node) {
        Some("Identifier") => {
            if let Some(name) = estree_node_field_str(node, RawField::Name) {
                classes.insert(name.to_string());
            }
            true
        }
        Some("Literal") => {
            if let Some(val) = estree_node_field(node, RawField::Value) {
                extract_string_tokens_from_raw_value(val, classes);
            }
            true
        }
        _ => false,
    }
}

fn add_class_tokens_from_string(value: &str, classes: &mut HashSet<String>) {
    for token in value.split_ascii_whitespace() {
        if !token.is_empty() {
            classes.insert(token.to_string());
        }
    }
}

fn class_tokens_from_variants(variants: &[String]) -> HashSet<String> {
    let mut tokens = HashSet::new();
    for value in variants {
        add_class_tokens_from_string(value, &mut tokens);
    }
    tokens
}

fn class_string_variants_from_raw(raw: &crate::ast::modern::EstreeNode) -> Option<Vec<String>> {
    match estree_node_type(raw)? {
        "Literal" => {
            if let Some(value) = estree_string_value(raw) {
                Some(vec![value.to_string()])
            } else {
                Some(vec![String::new()])
            }
        }
        "ConditionalExpression" => {
            let consequent = class_string_variants_from_raw(estree_node_field_object(
                raw,
                RawField::Consequent,
            )?)?;
            let alternate = class_string_variants_from_raw(estree_node_field_object(
                raw,
                RawField::Alternate,
            )?)?;
            merge_variant_sets(&consequent, &alternate)
        }
        "LogicalExpression" => {
            let operator = estree_node_field_str(raw, RawField::Operator)?;
            let right =
                class_string_variants_from_raw(estree_node_field_object(raw, RawField::Right)?)?;
            match operator {
                "&&" => merge_variant_sets(&[String::new()], &right),
                "||" | "??" => {
                    let left = class_string_variants_from_raw(estree_node_field_object(
                        raw,
                        RawField::Left,
                    )?)?;
                    merge_variant_sets(&left, &right)
                }
                _ => None,
            }
        }
        "BinaryExpression" => {
            if estree_node_field_str(raw, RawField::Operator)? != "+" {
                return None;
            }
            let left =
                class_string_variants_from_raw(estree_node_field_object(raw, RawField::Left)?)?;
            let right =
                class_string_variants_from_raw(estree_node_field_object(raw, RawField::Right)?)?;
            cartesian_join_variants(&left, &right)
        }
        "TemplateLiteral" => {
            let quasis = estree_node_field_array(raw, RawField::Quasis)?;
            let mut variants = vec![String::new()];
            for value in quasis.iter() {
                let quasi = estree_value_to_object(value)?;
                let cooked = estree_node_field_object(quasi, RawField::Value)
                    .and_then(|node| estree_node_field(node, RawField::Cooked));
                let chunk = match cooked {
                    Some(crate::ast::modern::EstreeValue::String(value)) => value.as_ref(),
                    _ => "",
                };
                variants = cartesian_join_variants(&variants, &[chunk.to_string()])?;
            }
            Some(variants)
        }
        "TSAsExpression"
        | "TSSatisfiesExpression"
        | "TSNonNullExpression"
        | "ParenthesizedExpression"
        | "ChainExpression" => {
            class_string_variants_from_raw(estree_node_field_object(raw, RawField::Expression)?)
        }
        _ => None,
    }
}

fn merge_variant_sets(left: &[String], right: &[String]) -> Option<Vec<String>> {
    let mut merged = left.to_vec();
    for value in right {
        if !merged.iter().any(|existing| existing == value) {
            merged.push(value.clone());
            if merged.len() > MAX_CLASS_VALUE_VARIANTS {
                return None;
            }
        }
    }
    Some(merged)
}

fn cartesian_join_variants(left: &[String], right: &[String]) -> Option<Vec<String>> {
    if left.len().saturating_mul(right.len()) > MAX_CLASS_VALUE_VARIANTS {
        return None;
    }

    let mut combined = Vec::new();
    for left_value in left {
        for right_value in right {
            let next = format!("{left_value}{right_value}");
            if !combined.iter().any(|existing| existing == &next) {
                combined.push(next);
                if combined.len() > MAX_CLASS_VALUE_VARIANTS {
                    return None;
                }
            }
        }
    }
    Some(combined)
}

fn estree_string_value(node: &crate::ast::modern::EstreeNode) -> Option<&str> {
    match estree_node_field(node, RawField::Value) {
        Some(crate::ast::modern::EstreeValue::String(value)) => Some(value.as_ref()),
        _ => None,
    }
}

fn static_attribute_text_from_value(value: &AttributeValueList) -> Option<String> {
    match value {
        AttributeValueList::Boolean(_) => None,
        AttributeValueList::ExpressionTag(tag) => {
            static_attribute_text_from_expression(&tag.expression)
        }
        AttributeValueList::Values(values) => {
            let mut out = String::new();
            for value in values.iter() {
                match value {
                    AttributeValue::Text(text) => out.push_str(&text.data),
                    AttributeValue::ExpressionTag(tag) => {
                        out.push_str(&static_attribute_text_from_expression(&tag.expression)?);
                    }
                }
            }
            Some(out)
        }
    }
}

fn static_attribute_text_from_expression(expression: &Expression) -> Option<String> {
    static_attribute_text_from_raw(&expression.0)
}

fn static_attribute_text_from_raw(raw: &crate::ast::modern::EstreeNode) -> Option<String> {
    match estree_node_type(raw)? {
        "Literal" => match estree_node_field(raw, RawField::Value)? {
            crate::ast::modern::EstreeValue::String(value) => Some(value.to_string()),
            crate::ast::modern::EstreeValue::Bool(value) => Some(value.to_string()),
            crate::ast::modern::EstreeValue::Int(value) => Some(value.to_string()),
            crate::ast::modern::EstreeValue::UInt(value) => Some(value.to_string()),
            crate::ast::modern::EstreeValue::Number(value) => Some(value.to_string()),
            crate::ast::modern::EstreeValue::Null => None,
            crate::ast::modern::EstreeValue::Object(_)
            | crate::ast::modern::EstreeValue::Array(_) => None,
        },
        "BinaryExpression" => {
            if estree_node_field_str(raw, RawField::Operator)? != "+" {
                return None;
            }
            let left =
                static_attribute_text_from_raw(estree_node_field_object(raw, RawField::Left)?)?;
            let right =
                static_attribute_text_from_raw(estree_node_field_object(raw, RawField::Right)?)?;
            Some(format!("{left}{right}"))
        }
        "TemplateLiteral" => {
            let quasis = estree_node_field_array(raw, RawField::Quasis)?;
            let mut out = String::new();
            for value in quasis.iter() {
                let quasi = estree_value_to_object(value)?;
                let cooked = estree_node_field_object(quasi, RawField::Value)
                    .and_then(|node| estree_node_field(node, RawField::Cooked));
                if let Some(crate::ast::modern::EstreeValue::String(value)) = cooked {
                    out.push_str(value);
                }
            }
            Some(out)
        }
        "TSAsExpression"
        | "TSSatisfiesExpression"
        | "TSNonNullExpression"
        | "ParenthesizedExpression"
        | "ChainExpression" => {
            static_attribute_text_from_raw(estree_node_field_object(raw, RawField::Expression)?)
        }
        _ => None,
    }
}

/// Build CSS usage context by walking the AST instead of scanning source.
pub(crate) fn build_css_usage_context(root: &Root, dev: bool) -> CssUsageContext {
    let mut context = CssUsageContext {
        dev,
        ..Default::default()
    };

    // Derive flags from AST
    collect_ast_flags_and_ranges(&root.fragment, &mut context);

    // has_spread_attributes: walk AST for SpreadAttribute
    context.has_spread_attributes = has_spread_attributes_from_ast(&root.fragment);

    // has_dynamic_svelte_element: svelte:element with dynamic this=
    context.has_dynamic_svelte_element = has_dynamic_svelte_element_from_ast(&root.fragment);

    // allow_pruning: check for unprunable class expressions via AST
    // Collect element usages from AST (includes class extraction from attributes via process_element)
    collect_element_usages_from_fragment(&root.fragment, &mut context, None, 0, None);

    // Collect each boundary candidates directly from AST structure
    collect_each_boundary_candidates_from_ast(&root.fragment, &mut context);

    context.allow_pruning =
        !context.has_spread_attributes && !has_unprunable_class_expression_from_ast(&root.fragment);

    context
}

fn has_spread_attributes_from_ast(fragment: &Fragment) -> bool {
    for node in fragment.nodes.iter() {
        match node {
            Node::RegularElement(el) => {
                if el
                    .attributes
                    .iter()
                    .any(|a| matches!(a, Attribute::SpreadAttribute(_)))
                {
                    return true;
                }
                if has_spread_attributes_from_ast(&el.fragment) {
                    return true;
                }
            }
            Node::Component(el) => {
                if el
                    .attributes
                    .iter()
                    .any(|a| matches!(a, Attribute::SpreadAttribute(_)))
                {
                    return true;
                }
                if has_spread_attributes_from_ast(&el.fragment) {
                    return true;
                }
            }
            Node::SlotElement(el) => {
                if el
                    .attributes
                    .iter()
                    .any(|a| matches!(a, Attribute::SpreadAttribute(_)))
                {
                    return true;
                }
                if has_spread_attributes_from_ast(&el.fragment) {
                    return true;
                }
            }
            Node::EachBlock(b) => {
                if has_spread_attributes_from_ast(&b.body) {
                    return true;
                }
                if let Some(ref f) = b.fallback
                    && has_spread_attributes_from_ast(f)
                {
                    return true;
                }
            }
            Node::IfBlock(b) => {
                if has_spread_attributes_from_ast(&b.consequent) {
                    return true;
                }
                if let Some(ref alt) = b.alternate {
                    match alt.as_ref() {
                        crate::ast::modern::Alternate::Fragment(f) => {
                            if has_spread_attributes_from_ast(f) {
                                return true;
                            }
                        }
                        crate::ast::modern::Alternate::IfBlock(ib) => {
                            if has_spread_attributes_from_ast(&ib.consequent) {
                                return true;
                            }
                        }
                    }
                }
            }
            Node::KeyBlock(b) if has_spread_attributes_from_ast(&b.fragment) => {
                return true;
            }
            Node::AwaitBlock(b) => {
                if let Some(ref p) = b.pending
                    && has_spread_attributes_from_ast(p)
                {
                    return true;
                }
                if let Some(ref t) = b.then
                    && has_spread_attributes_from_ast(t)
                {
                    return true;
                }
                if let Some(ref c) = b.catch
                    && has_spread_attributes_from_ast(c)
                {
                    return true;
                }
            }
            Node::SnippetBlock(b) if has_spread_attributes_from_ast(&b.body) => {
                return true;
            }
            _ => {}
        }
    }
    false
}

fn collect_ast_flags_and_ranges(fragment: &Fragment, context: &mut CssUsageContext) {
    for node in fragment.nodes.iter() {
        match node {
            Node::EachBlock(block) => {
                context.has_each_blocks = true;
                context.has_dynamic_markup = true;
                collect_ast_flags_and_ranges(&block.body, context);
                if let Some(ref fallback) = block.fallback {
                    collect_ast_flags_and_ranges(fallback, context);
                }
            }
            Node::IfBlock(block) => {
                context.has_dynamic_markup = true;
                context.has_non_each_dynamic_markup = true;
                collect_ast_flags_and_ranges(&block.consequent, context);
                if let Some(ref alt) = block.alternate {
                    match alt.as_ref() {
                        crate::ast::modern::Alternate::Fragment(f) => {
                            collect_ast_flags_and_ranges(f, context)
                        }
                        crate::ast::modern::Alternate::IfBlock(ib) => {
                            context.has_dynamic_markup = true;
                            collect_ast_flags_and_ranges(&ib.consequent, context);
                        }
                    }
                }
            }
            Node::KeyBlock(block) => {
                context.has_dynamic_markup = true;
                context.has_non_each_dynamic_markup = true;
                collect_ast_flags_and_ranges(&block.fragment, context);
            }
            Node::AwaitBlock(block) => {
                context.has_dynamic_markup = true;
                context.has_non_each_dynamic_markup = true;
                if let Some(ref p) = block.pending {
                    collect_ast_flags_and_ranges(p, context);
                }
                if let Some(ref t) = block.then {
                    collect_ast_flags_and_ranges(t, context);
                }
                if let Some(ref c) = block.catch {
                    collect_ast_flags_and_ranges(c, context);
                }
            }
            Node::SnippetBlock(block) => {
                context.has_dynamic_markup = true;
                context.has_non_each_dynamic_markup = true;
                collect_ast_flags_and_ranges(&block.body, context);
            }
            Node::RenderTag(_) => {
                context.has_render_tags = true;
                context.has_dynamic_markup = true;
                context.has_non_each_dynamic_markup = true;
            }
            Node::HtmlTag(_) | Node::ExpressionTag(_) => {
                context.has_dynamic_markup = true;
                context.has_non_each_dynamic_markup = true;
            }
            Node::SlotElement(_) => {
                context.has_slot_tags = true;
                context.has_dynamic_markup = true;
                context.has_non_each_dynamic_markup = true;
            }
            Node::Component(el) => {
                let is_component_like = el
                    .name
                    .chars()
                    .next()
                    .is_some_and(|ch| ch.is_ascii_uppercase())
                    || el.name.contains('.');
                if is_component_like {
                    context.has_component_like_elements = true;
                }
                if el.name.as_ref().starts_with("svelte:") || el.name.as_ref().starts_with(':') {
                    context.has_dynamic_markup = true;
                    context.has_non_each_dynamic_markup = true;
                }
                collect_ast_flags_and_ranges(&el.fragment, context);
            }
            Node::RegularElement(el) => {
                collect_ast_flags_and_ranges(&el.fragment, context);
            }
            Node::SvelteElement(el) => {
                context.has_dynamic_markup = true;
                collect_ast_flags_and_ranges(&el.fragment, context);
            }
            Node::SvelteComponent(el) => {
                context.has_dynamic_markup = true;
                context.has_non_each_dynamic_markup = true;
                collect_ast_flags_and_ranges(&el.fragment, context);
            }
            Node::SvelteSelf(el) => {
                context.has_dynamic_markup = true;
                context.has_non_each_dynamic_markup = true;
                collect_ast_flags_and_ranges(&el.fragment, context);
            }
            _ => {}
        }
    }
}

fn has_dynamic_svelte_element_from_ast(fragment: &Fragment) -> bool {
    for node in fragment.nodes.iter() {
        match node {
            Node::SvelteElement(el) if el.expression.is_some() => {
                return true;
            }
            Node::EachBlock(b) if has_dynamic_svelte_element_from_ast(&b.body) => {
                return true;
            }
            Node::IfBlock(b) if has_dynamic_svelte_element_from_ast(&b.consequent) => {
                return true;
            }
            Node::Component(c) if has_dynamic_svelte_element_from_ast(&c.fragment) => {
                return true;
            }
            Node::RegularElement(el) if has_dynamic_svelte_element_from_ast(&el.fragment) => {
                return true;
            }
            Node::SlotElement(s) if has_dynamic_svelte_element_from_ast(&s.fragment) => {
                return true;
            }
            Node::SvelteElement(el) if has_dynamic_svelte_element_from_ast(&el.fragment) => {
                return true;
            }
            _ => {}
        }
    }
    false
}

/// Check if any class value in the fragment has an unbounded candidate set.
fn has_unprunable_class_expression_from_ast(fragment: &Fragment) -> bool {
    for node in fragment.nodes.iter() {
        match node {
            Node::RegularElement(el) => {
                if collect_class_expr_from_attrs(el.attributes.as_ref()) {
                    return true;
                }
                if has_unprunable_class_expression_from_ast(&el.fragment) {
                    return true;
                }
            }
            Node::Component(el) => {
                if collect_class_expr_from_attrs(el.attributes.as_ref()) {
                    return true;
                }
                if has_unprunable_class_expression_from_ast(&el.fragment) {
                    return true;
                }
            }
            Node::SlotElement(el) => {
                if collect_class_expr_from_attrs(el.attributes.as_ref()) {
                    return true;
                }
                if has_unprunable_class_expression_from_ast(&el.fragment) {
                    return true;
                }
            }
            Node::EachBlock(b) => {
                if has_unprunable_class_expression_from_ast(&b.body) {
                    return true;
                }
                if let Some(ref f) = b.fallback
                    && has_unprunable_class_expression_from_ast(f)
                {
                    return true;
                }
            }
            Node::IfBlock(b) => {
                if has_unprunable_class_expression_from_ast(&b.consequent) {
                    return true;
                }
                if let Some(ref alt) = b.alternate {
                    match alt.as_ref() {
                        crate::ast::modern::Alternate::Fragment(f) => {
                            if has_unprunable_class_expression_from_ast(f) {
                                return true;
                            }
                        }
                        crate::ast::modern::Alternate::IfBlock(ib) => {
                            if has_unprunable_class_expression_from_ast(&ib.consequent) {
                                return true;
                            }
                        }
                    }
                }
            }
            Node::KeyBlock(b) if has_unprunable_class_expression_from_ast(&b.fragment) => {
                return true;
            }
            Node::AwaitBlock(b) => {
                if let Some(ref p) = b.pending
                    && has_unprunable_class_expression_from_ast(p)
                {
                    return true;
                }
                if let Some(ref t) = b.then
                    && has_unprunable_class_expression_from_ast(t)
                {
                    return true;
                }
                if let Some(ref c) = b.catch
                    && has_unprunable_class_expression_from_ast(c)
                {
                    return true;
                }
            }
            Node::SnippetBlock(b) if has_unprunable_class_expression_from_ast(&b.body) => {
                return true;
            }
            _ => {}
        }
    }
    false
}

/// Returns true if any class expression in attributes is unprunable.
fn collect_class_expr_from_attrs(attributes: &[Attribute]) -> bool {
    for attr in attributes.iter() {
        if let Attribute::Attribute(named) = attr {
            if named.name.as_ref() != "class" {
                continue;
            }
            if analyze_class_attribute_value(&named.value).unbounded {
                return true;
            }
        }
    }
    false
}

fn analyze_class_attribute_value(value: &AttributeValueList) -> ClassValueAnalysis {
    match value {
        AttributeValueList::Boolean(_) => ClassValueAnalysis::default(),
        AttributeValueList::ExpressionTag(tag) => analyze_class_expression(&tag.expression),
        AttributeValueList::Values(values) => {
            if let Some(variants) = class_string_variants_from_parts(values) {
                return ClassValueAnalysis {
                    tokens: class_tokens_from_variants(&variants),
                    unbounded: false,
                };
            }

            let mut analysis = ClassValueAnalysis::default();
            for value in values.iter() {
                match value {
                    AttributeValue::Text(text) => {
                        add_class_tokens_from_string(&text.data, &mut analysis.tokens)
                    }
                    AttributeValue::ExpressionTag(tag) => {
                        let inner = analyze_class_expression(&tag.expression);
                        analysis.tokens.extend(inner.tokens);
                        analysis.unbounded |= inner.unbounded;
                    }
                }
            }
            analysis
        }
    }
}

fn class_string_variants_from_parts(values: &[AttributeValue]) -> Option<Vec<String>> {
    let mut variants = vec![String::new()];

    for value in values {
        let next_parts = match value {
            AttributeValue::Text(text) => vec![text.data.to_string()],
            AttributeValue::ExpressionTag(tag) => {
                class_string_variants_from_raw(&tag.expression.0)?
            }
        };
        variants = cartesian_join_variants(&variants, &next_parts)?;
    }

    Some(variants)
}

fn collect_element_usages_from_fragment(
    fragment: &Fragment,
    context: &mut CssUsageContext,
    parent: Option<usize>,
    depth: usize,
    component_depth: Option<usize>,
) {
    for node in fragment.nodes.iter() {
        match node {
            Node::RenderTag(_) => {
                context.has_render_tags = true;
                if let Some(parent_index) = parent {
                    context.render_parent_elements.insert(parent_index);
                } else {
                    context.root_has_render = true;
                }
            }
            Node::RegularElement(el) => {
                process_element(
                    el.name.as_ref(),
                    el.attributes.as_ref(),
                    false,
                    regular_element_is_optional(el),
                    context,
                    parent,
                    depth,
                    component_depth,
                );
                if !is_void_element(el.name.as_ref()) {
                    let child_component_depth =
                        component_depth.map(|value| value.saturating_add(1));
                    collect_element_usages_from_fragment(
                        &el.fragment,
                        context,
                        Some(context.elements.len().saturating_sub(1)),
                        depth + 1,
                        child_component_depth,
                    );
                }
            }
            Node::Component(el) => {
                let next_component_depth = Some(component_depth.unwrap_or(0).saturating_add(1));
                collect_element_usages_from_fragment(
                    &el.fragment,
                    context,
                    parent,
                    depth,
                    next_component_depth,
                );
            }
            Node::SlotElement(el) => {
                process_element(
                    el.name.as_ref(),
                    el.attributes.as_ref(),
                    false,
                    false,
                    context,
                    parent,
                    depth,
                    component_depth,
                );
                let child_component_depth = component_depth.map(|value| value.saturating_add(1));
                collect_element_usages_from_fragment(
                    &el.fragment,
                    context,
                    Some(context.elements.len().saturating_sub(1)),
                    depth + 1,
                    child_component_depth,
                );
            }
            Node::SvelteElement(el) => {
                let optional = el.expression.is_some();
                process_element(
                    el.name.as_ref(),
                    el.attributes.as_ref(),
                    false,
                    optional,
                    context,
                    parent,
                    depth,
                    component_depth,
                );
                let child_component_depth = component_depth.map(|value| value.saturating_add(1));
                collect_element_usages_from_fragment(
                    &el.fragment,
                    context,
                    Some(context.elements.len().saturating_sub(1)),
                    depth + 1,
                    child_component_depth,
                );
            }
            Node::SvelteComponent(_)
            | Node::SvelteSelf(_)
            | Node::SvelteHead(_)
            | Node::SvelteBody(_)
            | Node::SvelteWindow(_)
            | Node::SvelteDocument(_)
            | Node::SvelteFragment(_)
            | Node::SvelteBoundary(_)
            | Node::TitleElement(_) => {
                let fragment = node.as_element().unwrap().fragment();
                let next_component_depth = Some(component_depth.unwrap_or(0).saturating_add(1));
                collect_element_usages_from_fragment(
                    fragment,
                    context,
                    parent,
                    depth,
                    next_component_depth,
                );
            }
            Node::EachBlock(block) => {
                collect_element_usages_from_fragment(
                    &block.body,
                    context,
                    parent,
                    depth,
                    component_depth,
                );
                if let Some(ref fallback) = block.fallback {
                    collect_element_usages_from_fragment(
                        fallback,
                        context,
                        parent,
                        depth,
                        component_depth,
                    );
                }
            }
            Node::IfBlock(block) => {
                collect_element_usages_from_fragment(
                    &block.consequent,
                    context,
                    parent,
                    depth,
                    component_depth,
                );
                if let Some(ref alt) = block.alternate {
                    match alt.as_ref() {
                        crate::ast::modern::Alternate::Fragment(f) => {
                            collect_element_usages_from_fragment(
                                f,
                                context,
                                parent,
                                depth,
                                component_depth,
                            );
                        }
                        crate::ast::modern::Alternate::IfBlock(ib) => {
                            collect_element_usages_from_fragment(
                                &ib.consequent,
                                context,
                                parent,
                                depth,
                                component_depth,
                            );
                        }
                    }
                }
            }
            Node::KeyBlock(block) => {
                collect_element_usages_from_fragment(
                    &block.fragment,
                    context,
                    parent,
                    depth,
                    component_depth,
                );
            }
            Node::AwaitBlock(block) => {
                if let Some(ref p) = block.pending {
                    collect_element_usages_from_fragment(
                        p,
                        context,
                        parent,
                        depth,
                        component_depth,
                    );
                }
                if let Some(ref t) = block.then {
                    collect_element_usages_from_fragment(
                        t,
                        context,
                        parent,
                        depth,
                        component_depth,
                    );
                }
                if let Some(ref c) = block.catch {
                    collect_element_usages_from_fragment(
                        c,
                        context,
                        parent,
                        depth,
                        component_depth,
                    );
                }
            }
            Node::SnippetBlock(block) => {
                collect_element_usages_from_fragment(
                    &block.body,
                    context,
                    parent,
                    depth,
                    component_depth,
                );
            }
            _ => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn process_element(
    name: &str,
    attributes: &[Attribute],
    is_component_like: bool,
    optional: bool,
    context: &mut CssUsageContext,
    parent: Option<usize>,
    depth: usize,
    component_depth: Option<usize>,
) {
    let tag = name.to_ascii_lowercase();
    if is_component_like {
        context.has_component_like_elements = true;
    }
    context.tags.insert(tag.clone());

    let mut attrs = BTreeMap::new();
    for attr in attributes {
        match attr {
            Attribute::Attribute(named) => {
                let key = named.name.as_ref().to_ascii_lowercase();
                let static_value = static_attribute_text_from_value(&named.value);
                if key == "class" {
                    let analysis = analyze_class_attribute_value(&named.value);
                    context.classes.extend(analysis.tokens);
                    context.class_name_unbounded |= analysis.unbounded;
                } else if key == "id"
                    && let Some(value) = static_value.as_ref()
                {
                    context.ids.insert(value.clone());
                    attrs.insert(key.clone(), value.clone());
                }
                if static_value.is_none()
                    && (matches!(named.value, AttributeValueList::ExpressionTag(_))
                        || matches!(
                            &named.value,
                            AttributeValueList::Values(values)
                                if values.iter().any(|value| matches!(value, AttributeValue::ExpressionTag(_)))
                        ))
                {
                    context.dynamic_attributes.insert(key.clone());
                }
                if let Some(value) = static_value {
                    attrs.insert(key, value);
                }
            }
            Attribute::ClassDirective(directive) => {
                let name_str = directive
                    .name
                    .as_ref()
                    .split('|')
                    .next()
                    .unwrap_or("")
                    .trim();
                if !name_str.is_empty() {
                    context.classes.insert(name_str.to_string());
                }
            }
            _ => {}
        }
    }

    let _element_index = context.elements.len();
    context.elements.push(CssElementUsage {
        tag: tag.clone(),
        attributes: attrs,
        parent,
        depth,
        component_depth,
        optional,
    });
}

fn add_each_candidate_sets(
    context: &mut CssUsageContext,
    tags: &HashSet<String>,
    classes: &HashSet<String>,
    kind: EachBoundaryKind,
) {
    let (tag_set, class_set) = match kind {
        EachBoundaryKind::First => (
            &mut context.each_first_tags,
            &mut context.each_first_classes,
        ),
        EachBoundaryKind::Last => (&mut context.each_last_tags, &mut context.each_last_classes),
        EachBoundaryKind::Before => (
            &mut context.each_before_tags,
            &mut context.each_before_classes,
        ),
        EachBoundaryKind::After => (
            &mut context.each_after_tags,
            &mut context.each_after_classes,
        ),
    };

    tag_set.extend(tags.iter().cloned());
    class_set.extend(classes.iter().cloned());
}

fn regular_element_is_optional(_element: &crate::ast::modern::RegularElement) -> bool {
    false
}

#[allow(dead_code)]
fn svelte_element_has_dynamic_this(attributes: &[Attribute]) -> bool {
    for attribute in attributes.iter() {
        if let Attribute::Attribute(named) = attribute {
            if named.name.as_ref() != "this" {
                continue;
            }
            if matches!(named.value, AttributeValueList::ExpressionTag(_)) {
                return true;
            }
            if let AttributeValueList::Values(values) = &named.value
                && values
                    .iter()
                    .any(|value| matches!(value, AttributeValue::ExpressionTag(_)))
            {
                return true;
            }
        }
    }
    false
}

#[derive(Default, Clone)]
struct BoundaryTokenSet {
    tags: HashSet<String>,
    classes: HashSet<String>,
}

impl BoundaryTokenSet {
    fn extend(&mut self, other: &BoundaryTokenSet) {
        self.tags.extend(other.tags.iter().cloned());
        self.classes.extend(other.classes.iter().cloned());
    }
}

#[derive(Default, Clone)]
struct RenderSummary {
    first: BoundaryTokenSet,
    last: BoundaryTokenSet,
    can_be_empty: bool,
}

fn collect_each_boundary_candidates_from_ast(fragment: &Fragment, context: &mut CssUsageContext) {
    collect_each_boundary_candidates_in_fragment(fragment, context);
}

fn collect_each_boundary_candidates_in_fragment(
    fragment: &Fragment,
    context: &mut CssUsageContext,
) {
    let nodes = fragment.nodes.as_ref();

    for (index, node) in nodes.iter().enumerate() {
        if let Node::EachBlock(block) = node {
            let block_summary = summarize_each_block(block);
            add_each_candidate_sets(
                context,
                &block_summary.first.tags,
                &block_summary.first.classes,
                EachBoundaryKind::First,
            );
            add_each_candidate_sets(
                context,
                &block_summary.last.tags,
                &block_summary.last.classes,
                EachBoundaryKind::Last,
            );

            let before = summarize_previous_sibling_boundary(&nodes[..index]);
            add_each_candidate_sets(
                context,
                &before.tags,
                &before.classes,
                EachBoundaryKind::Before,
            );

            let after = summarize_next_sibling_boundary(&nodes[index + 1..]);
            add_each_candidate_sets(
                context,
                &after.tags,
                &after.classes,
                EachBoundaryKind::After,
            );
        }

        recurse_collect_each_candidates(node, context);
    }
}

fn recurse_collect_each_candidates(node: &Node, context: &mut CssUsageContext) {
    match node {
        Node::RegularElement(element) => {
            collect_each_boundary_candidates_in_fragment(&element.fragment, context);
        }
        Node::Component(element) => {
            collect_each_boundary_candidates_in_fragment(&element.fragment, context);
        }
        Node::SlotElement(element) => {
            collect_each_boundary_candidates_in_fragment(&element.fragment, context);
        }
        Node::EachBlock(block) => {
            collect_each_boundary_candidates_in_fragment(&block.body, context);
            if let Some(ref fallback) = block.fallback {
                collect_each_boundary_candidates_in_fragment(fallback, context);
            }
        }
        Node::IfBlock(block) => {
            collect_each_boundary_candidates_in_fragment(&block.consequent, context);
            if let Some(ref alternate) = block.alternate {
                match alternate.as_ref() {
                    crate::ast::modern::Alternate::Fragment(fragment) => {
                        collect_each_boundary_candidates_in_fragment(fragment, context);
                    }
                    crate::ast::modern::Alternate::IfBlock(block) => {
                        collect_each_boundary_candidates_in_fragment(&block.consequent, context);
                    }
                }
            }
        }
        Node::KeyBlock(block) => {
            collect_each_boundary_candidates_in_fragment(&block.fragment, context);
        }
        Node::AwaitBlock(block) => {
            if let Some(ref pending) = block.pending {
                collect_each_boundary_candidates_in_fragment(pending, context);
            }
            if let Some(ref then) = block.then {
                collect_each_boundary_candidates_in_fragment(then, context);
            }
            if let Some(ref catch) = block.catch {
                collect_each_boundary_candidates_in_fragment(catch, context);
            }
        }
        Node::SnippetBlock(block) => {
            collect_each_boundary_candidates_in_fragment(&block.body, context);
        }
        Node::SvelteElement(element) => {
            collect_each_boundary_candidates_in_fragment(&element.fragment, context);
        }
        _ => {}
    }
}

fn summarize_previous_sibling_boundary(nodes: &[Node]) -> BoundaryTokenSet {
    let mut out = BoundaryTokenSet::default();
    for node in nodes.iter().rev() {
        let summary = summarize_node_render(node);
        out.extend(&summary.last);
        if !summary.can_be_empty {
            break;
        }
    }
    out
}

fn summarize_next_sibling_boundary(nodes: &[Node]) -> BoundaryTokenSet {
    let mut out = BoundaryTokenSet::default();
    for node in nodes.iter() {
        let summary = summarize_node_render(node);
        out.extend(&summary.first);
        if !summary.can_be_empty {
            break;
        }
    }
    out
}

fn summarize_each_block(block: &crate::ast::modern::EachBlock) -> RenderSummary {
    let body = summarize_fragment_render(&block.body);
    let fallback = block.fallback.as_ref().map(summarize_fragment_render);

    let mut summary = RenderSummary::default();
    summary.first.extend(&body.first);
    summary.last.extend(&body.last);
    if let Some(fallback) = fallback.as_ref() {
        summary.first.extend(&fallback.first);
        summary.last.extend(&fallback.last);
    }

    let body_can_be_empty = body.can_be_empty;
    let zero_iteration_can_be_empty = fallback.as_ref().is_none_or(|value| value.can_be_empty);
    summary.can_be_empty = body_can_be_empty || zero_iteration_can_be_empty;
    summary
}

fn summarize_fragment_render(fragment: &Fragment) -> RenderSummary {
    let mut first = BoundaryTokenSet::default();
    let mut last = BoundaryTokenSet::default();

    let mut can_prefix_be_empty = true;
    for node in fragment.nodes.iter() {
        if !can_prefix_be_empty {
            break;
        }
        let summary = summarize_node_render(node);
        first.extend(&summary.first);
        can_prefix_be_empty &= summary.can_be_empty;
    }

    let mut can_suffix_be_empty = true;
    for node in fragment.nodes.iter().rev() {
        if !can_suffix_be_empty {
            break;
        }
        let summary = summarize_node_render(node);
        last.extend(&summary.last);
        can_suffix_be_empty &= summary.can_be_empty;
    }

    RenderSummary {
        first,
        last,
        can_be_empty: can_prefix_be_empty,
    }
}

fn summarize_node_render(node: &Node) -> RenderSummary {
    match node {
        Node::RegularElement(element) => summarize_regular_or_slot_element(
            element.name.as_ref(),
            element.attributes.as_ref(),
            &element.fragment,
            is_void_element(element.name.as_ref()),
        ),
        Node::SlotElement(element) => summarize_regular_or_slot_element(
            element.name.as_ref(),
            element.attributes.as_ref(),
            &element.fragment,
            false,
        ),
        Node::Component(element) => summarize_fragment_render(&element.fragment),
        Node::EachBlock(block) => summarize_each_block(block),
        Node::IfBlock(block) => summarize_if_block(block),
        Node::KeyBlock(block) => summarize_fragment_render(&block.fragment),
        Node::AwaitBlock(block) => {
            let pending = block.pending.as_ref().map(summarize_fragment_render);
            let then = block.then.as_ref().map(summarize_fragment_render);
            let catch = block.catch.as_ref().map(summarize_fragment_render);

            let mut summary = RenderSummary::default();
            if let Some(pending) = pending.as_ref() {
                summary.first.extend(&pending.first);
                summary.last.extend(&pending.last);
            }
            if let Some(then) = then.as_ref() {
                summary.first.extend(&then.first);
                summary.last.extend(&then.last);
            }
            if let Some(catch) = catch.as_ref() {
                summary.first.extend(&catch.first);
                summary.last.extend(&catch.last);
            }
            summary.can_be_empty = pending.as_ref().is_none_or(|value| value.can_be_empty)
                || then.as_ref().is_none_or(|value| value.can_be_empty)
                || catch.as_ref().is_none_or(|value| value.can_be_empty);
            summary
        }
        Node::SnippetBlock(block) => summarize_fragment_render(&block.body),
        Node::SvelteElement(element) => summarize_regular_or_slot_element(
            element.name.as_ref(),
            element.attributes.as_ref(),
            &element.fragment,
            false,
        ),
        _ => RenderSummary {
            can_be_empty: true,
            ..RenderSummary::default()
        },
    }
}

fn summarize_regular_or_slot_element(
    name: &str,
    attributes: &[Attribute],
    fragment: &Fragment,
    void: bool,
) -> RenderSummary {
    let mut current = BoundaryTokenSet::default();
    current.tags.insert(name.to_ascii_lowercase());
    current.extend(&boundary_classes_from_attributes(attributes));

    if void {
        return RenderSummary {
            first: current.clone(),
            last: current,
            can_be_empty: false,
        };
    }

    let child = summarize_fragment_render(fragment);
    let mut first = BoundaryTokenSet::default();
    first.extend(&current);

    let mut last = BoundaryTokenSet::default();
    if !child.last.tags.is_empty() || !child.last.classes.is_empty() {
        last.extend(&child.last);
    }
    if child.can_be_empty || (child.last.tags.is_empty() && child.last.classes.is_empty()) {
        last.extend(&current);
    }

    RenderSummary {
        first,
        last,
        can_be_empty: false,
    }
}

fn summarize_if_block(block: &crate::ast::modern::IfBlock) -> RenderSummary {
    let consequent = summarize_fragment_render(&block.consequent);
    let alternate = block
        .alternate
        .as_ref()
        .map(|alternate| match alternate.as_ref() {
            crate::ast::modern::Alternate::Fragment(fragment) => {
                summarize_fragment_render(fragment)
            }
            crate::ast::modern::Alternate::IfBlock(block) => summarize_if_block(block),
        });

    let mut summary = RenderSummary::default();
    summary.first.extend(&consequent.first);
    summary.last.extend(&consequent.last);
    if let Some(alternate) = alternate.as_ref() {
        summary.first.extend(&alternate.first);
        summary.last.extend(&alternate.last);
    }
    summary.can_be_empty =
        consequent.can_be_empty || alternate.as_ref().is_none_or(|value| value.can_be_empty);
    summary
}

fn boundary_classes_from_attributes(attributes: &[Attribute]) -> BoundaryTokenSet {
    let mut out = BoundaryTokenSet::default();
    for attribute in attributes.iter() {
        match attribute {
            Attribute::Attribute(named) if named.name.as_ref() == "class" => {
                let analysis = analyze_class_attribute_value(&named.value);
                out.classes.extend(analysis.tokens);
            }
            Attribute::ClassDirective(directive) => {
                let name = directive
                    .name
                    .as_ref()
                    .split('|')
                    .next()
                    .unwrap_or("")
                    .trim();
                if !name.is_empty() {
                    out.classes.insert(name.to_string());
                }
            }
            _ => {}
        }
    }
    out
}
