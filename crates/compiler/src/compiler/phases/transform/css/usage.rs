//! CSS usage context built from AST traversal.

use std::{collections::BTreeMap, sync::Arc};

use oxc_ast::ast::{
    ArrayExpressionElement, Expression as OxcExpression, ObjectPropertyKind, PropertyKey,
};
use oxc_ast::match_expression;
use rustc_hash::FxHashSet;

use crate::ast::modern::{
    Attribute, AttributeValue, AttributeValueKind, Expression, Fragment, Node, Root,
};

type CssName = Arc<str>;
type CssNameSet = FxHashSet<CssName>;
type CssAttributeMap = BTreeMap<CssName, CssName>;

#[derive(Debug)]
pub(crate) struct CssUsageContext {
    pub(crate) classes: CssNameSet,
    pub(crate) ids: CssNameSet,
    pub(crate) tags: CssNameSet,
    pub(crate) dynamic_attributes: CssNameSet,
    pub(crate) class_name_unbounded: bool,
    pub(crate) render_parent_elements: FxHashSet<usize>,
    pub(crate) root_has_render: bool,
    pub(crate) has_render_tags: bool,
    pub(crate) allow_pruning: bool,
    pub(crate) has_dynamic_svelte_element: bool,
    pub(crate) has_dynamic_markup: bool,
    pub(crate) has_each_blocks: bool,
    pub(crate) has_non_each_dynamic_markup: bool,
    pub(crate) has_slot_tags: bool,
    pub(crate) has_component_like_elements: bool,
    pub(crate) dev: bool,
    pub(crate) each_first_tags: CssNameSet,
    pub(crate) each_first_classes: CssNameSet,
    pub(crate) each_last_tags: CssNameSet,
    pub(crate) each_last_classes: CssNameSet,
    pub(crate) each_before_tags: CssNameSet,
    pub(crate) each_before_classes: CssNameSet,
    pub(crate) each_after_tags: CssNameSet,
    pub(crate) each_after_classes: CssNameSet,
    pub(crate) elements: Box<[CssElementUsage]>,
}

#[derive(Debug, Default)]
struct CssUsageBuilder {
    classes: CssNameSet,
    ids: CssNameSet,
    tags: CssNameSet,
    dynamic_attributes: CssNameSet,
    class_name_unbounded: bool,
    render_parent_elements: FxHashSet<usize>,
    root_has_render: bool,
    has_render_tags: bool,
    has_spread_attributes: bool,
    allow_pruning: bool,
    has_dynamic_svelte_element: bool,
    has_dynamic_markup: bool,
    has_each_blocks: bool,
    has_non_each_dynamic_markup: bool,
    has_slot_tags: bool,
    has_component_like_elements: bool,
    dev: bool,
    each_first_tags: CssNameSet,
    each_first_classes: CssNameSet,
    each_last_tags: CssNameSet,
    each_last_classes: CssNameSet,
    each_before_tags: CssNameSet,
    each_before_classes: CssNameSet,
    each_after_tags: CssNameSet,
    each_after_classes: CssNameSet,
    elements: Vec<CssElementUsage>,
}

impl CssUsageBuilder {
    fn finish(self) -> CssUsageContext {
        CssUsageContext {
            classes: self.classes,
            ids: self.ids,
            tags: self.tags,
            dynamic_attributes: self.dynamic_attributes,
            class_name_unbounded: self.class_name_unbounded,
            render_parent_elements: self.render_parent_elements,
            root_has_render: self.root_has_render,
            has_render_tags: self.has_render_tags,
            allow_pruning: self.allow_pruning,
            has_dynamic_svelte_element: self.has_dynamic_svelte_element,
            has_dynamic_markup: self.has_dynamic_markup,
            has_each_blocks: self.has_each_blocks,
            has_non_each_dynamic_markup: self.has_non_each_dynamic_markup,
            has_slot_tags: self.has_slot_tags,
            has_component_like_elements: self.has_component_like_elements,
            dev: self.dev,
            each_first_tags: self.each_first_tags,
            each_first_classes: self.each_first_classes,
            each_last_tags: self.each_last_tags,
            each_last_classes: self.each_last_classes,
            each_before_tags: self.each_before_tags,
            each_before_classes: self.each_before_classes,
            each_after_tags: self.each_after_tags,
            each_after_classes: self.each_after_classes,
            elements: self.elements.into_boxed_slice(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CssElementUsage {
    pub(crate) node_start: usize,
    pub(crate) tag: CssName,
    pub(crate) attributes: CssAttributeMap,
    pub(crate) parent: Option<usize>,
    pub(crate) depth: usize,
    pub(crate) component_depth: Option<usize>,
    pub(crate) optional: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct CssAttributeFilter {
    pub(crate) name: CssName,
    pub(crate) value: CssName,
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

#[derive(Debug, Clone, Copy)]
pub(crate) enum EachBoundaryKind {
    First,
    Last,
    Before,
    After,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct BoundaryCandidates<'a> {
    pub(crate) tags: &'a CssNameSet,
    pub(crate) classes: &'a CssNameSet,
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
    tokens: CssNameSet,
    unbounded: bool,
}

fn analyze_class_expression(expression: &Expression) -> ClassValueAnalysis {
    if let Some(variants) = class_string_variants_from_expression(expression) {
        return ClassValueAnalysis {
            tokens: class_tokens_from_variants(&variants),
            unbounded: false,
        };
    }

    let mut tokens = CssNameSet::default();
    let bounded = collect_class_candidates_from_expression(expression, &mut tokens);
    ClassValueAnalysis {
        tokens,
        unbounded: !bounded,
    }
}

fn collect_class_candidates_from_expression(
    expression: &Expression,
    classes: &mut CssNameSet,
) -> bool {
    expression
        .oxc_expression()
        .is_some_and(|expression| collect_class_candidates_from_oxc(expression, classes))
}

fn collect_class_candidates_from_oxc(
    expression: &OxcExpression<'_>,
    classes: &mut CssNameSet,
) -> bool {
    match expression {
        OxcExpression::ArrayExpression(expression) => {
            let mut bounded = true;
            for value in &expression.elements {
                bounded &= collect_class_candidates_from_array_element(value, classes);
            }
            bounded
        }
        OxcExpression::ObjectExpression(expression) => {
            let mut bounded = true;
            for property in &expression.properties {
                match property {
                    ObjectPropertyKind::ObjectProperty(property) => {
                        if property.computed {
                            bounded = false;
                            continue;
                        }
                        bounded &=
                            extract_object_property_key_tokens_from_oxc(&property.key, classes);
                    }
                    ObjectPropertyKind::SpreadProperty(_) => bounded = false,
                }
            }
            bounded
        }
        OxcExpression::ConditionalExpression(expression) => {
            let left_bounded = collect_class_candidates_from_oxc(&expression.consequent, classes);
            let right_bounded = collect_class_candidates_from_oxc(&expression.alternate, classes);
            left_bounded && right_bounded
        }
        OxcExpression::LogicalExpression(expression) => {
            let mut bounded = collect_class_candidates_from_oxc(&expression.right, classes);
            if expression.operator.as_str() != "&&" {
                bounded &= collect_class_candidates_from_oxc(&expression.left, classes);
            }
            bounded
        }
        OxcExpression::CallExpression(expression) => {
            let mut bounded = true;
            for argument in &expression.arguments {
                let Some(node) = argument.as_expression() else {
                    bounded = false;
                    continue;
                };
                bounded &= collect_class_candidates_from_oxc(node, classes);
            }
            bounded
        }
        OxcExpression::BinaryExpression(_) | OxcExpression::TemplateLiteral(_) => {
            if let Some(variants) = class_string_variants_from_oxc(expression) {
                classes.extend(class_tokens_from_variants(&variants));
                true
            } else {
                false
            }
        }
        OxcExpression::StringLiteral(value) => {
            add_class_tokens_from_string(value.value.as_str(), classes);
            true
        }
        OxcExpression::BooleanLiteral(_)
        | OxcExpression::NumericLiteral(_)
        | OxcExpression::BigIntLiteral(_)
        | OxcExpression::NullLiteral(_) => true,
        OxcExpression::TSAsExpression(expression) => {
            collect_class_candidates_from_oxc(&expression.expression, classes)
        }
        OxcExpression::TSSatisfiesExpression(expression) => {
            collect_class_candidates_from_oxc(&expression.expression, classes)
        }
        OxcExpression::TSNonNullExpression(expression) => {
            collect_class_candidates_from_oxc(&expression.expression, classes)
        }
        OxcExpression::ParenthesizedExpression(expression) => {
            collect_class_candidates_from_oxc(&expression.expression, classes)
        }
        OxcExpression::ChainExpression(_) => false,
        _ => false,
    }
}

fn collect_class_candidates_from_array_element(
    value: &ArrayExpressionElement<'_>,
    classes: &mut CssNameSet,
) -> bool {
    if value.is_elision() || value.is_spread() {
        return false;
    }

    match value {
        match_expression!(ArrayExpressionElement) => {
            collect_class_candidates_from_oxc(value.to_expression(), classes)
        }
        _ => false,
    }
}

fn extract_object_property_key_tokens_from_oxc(
    key: &PropertyKey<'_>,
    classes: &mut CssNameSet,
) -> bool {
    match key {
        PropertyKey::StaticIdentifier(identifier) => {
            classes.insert(shared_name(identifier.name.as_str()));
            true
        }
        PropertyKey::StringLiteral(value) => {
            add_class_tokens_from_string(value.value.as_str(), classes);
            true
        }
        _ => false,
    }
}

fn add_class_tokens_from_string(value: &str, classes: &mut CssNameSet) {
    for token in value.split_ascii_whitespace() {
        if !token.is_empty() {
            classes.insert(shared_name(token));
        }
    }
}

fn class_tokens_from_variants(variants: &[String]) -> CssNameSet {
    let mut tokens = CssNameSet::default();
    for value in variants {
        add_class_tokens_from_string(value, &mut tokens);
    }
    tokens
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

fn static_attribute_text_from_value(value: &AttributeValueKind) -> Option<String> {
    match value {
        AttributeValueKind::Boolean(_) => None,
        AttributeValueKind::ExpressionTag(tag) => {
            static_attribute_text_from_expression(&tag.expression)
        }
        AttributeValueKind::Values(values) => {
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
    static_attribute_text_from_oxc(expression.oxc_expression()?)
}

fn static_attribute_text_from_oxc(expression: &OxcExpression<'_>) -> Option<String> {
    match expression {
        OxcExpression::StringLiteral(value) => Some(value.value.to_string()),
        OxcExpression::BooleanLiteral(value) => Some(value.value.to_string()),
        OxcExpression::NumericLiteral(value) => Some(value.value.to_string()),
        OxcExpression::BigIntLiteral(value) => {
            Some(value.raw.as_ref().unwrap_or(&value.value).to_string())
        }
        OxcExpression::NullLiteral(_) => None,
        OxcExpression::BinaryExpression(expression) => {
            if expression.operator.as_str() != "+" {
                return None;
            }
            let left = static_attribute_text_from_oxc(&expression.left)?;
            let right = static_attribute_text_from_oxc(&expression.right)?;
            Some(format!("{left}{right}"))
        }
        OxcExpression::TemplateLiteral(expression) => {
            let mut out = String::new();
            for quasi in &expression.quasis {
                out.push_str(quasi.value.cooked.as_deref().unwrap_or(""));
            }
            Some(out)
        }
        OxcExpression::TSAsExpression(expression) => {
            static_attribute_text_from_oxc(&expression.expression)
        }
        OxcExpression::TSSatisfiesExpression(expression) => {
            static_attribute_text_from_oxc(&expression.expression)
        }
        OxcExpression::TSNonNullExpression(expression) => {
            static_attribute_text_from_oxc(&expression.expression)
        }
        OxcExpression::ParenthesizedExpression(expression) => {
            static_attribute_text_from_oxc(&expression.expression)
        }
        OxcExpression::ChainExpression(_) => None,
        _ => None,
    }
}

/// Build CSS usage context by walking the AST instead of scanning source.
pub(crate) fn build_css_usage_context(root: &Root, dev: bool) -> CssUsageContext {
    let mut context = CssUsageBuilder {
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

    context.finish()
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

fn collect_ast_flags_and_ranges(fragment: &Fragment, context: &mut CssUsageBuilder) {
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

fn analyze_class_attribute_value(value: &AttributeValueKind) -> ClassValueAnalysis {
    match value {
        AttributeValueKind::Boolean(_) => ClassValueAnalysis::default(),
        AttributeValueKind::ExpressionTag(tag) => analyze_class_expression(&tag.expression),
        AttributeValueKind::Values(values) => {
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
                class_string_variants_from_expression(&tag.expression)?
            }
        };
        variants = cartesian_join_variants(&variants, &next_parts)?;
    }

    Some(variants)
}

fn collect_element_usages_from_fragment(
    fragment: &Fragment,
    context: &mut CssUsageBuilder,
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
                    context,
                    ElementVisit {
                        node_start: el.start,
                        is_component_like: false,
                        optional: regular_element_is_optional(el),
                        parent,
                        depth,
                        component_depth,
                    },
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
                    context,
                    ElementVisit {
                        node_start: el.start,
                        is_component_like: false,
                        optional: false,
                        parent,
                        depth,
                        component_depth,
                    },
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
                process_element(
                    el.name.as_ref(),
                    el.attributes.as_ref(),
                    context,
                    ElementVisit {
                        node_start: el.start,
                        is_component_like: false,
                        optional: el.expression.is_some(),
                        parent,
                        depth,
                        component_depth,
                    },
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

fn class_string_variants_from_expression(expression: &Expression) -> Option<Vec<String>> {
    class_string_variants_from_oxc(expression.oxc_expression()?)
}

fn class_string_variants_from_oxc(expression: &OxcExpression<'_>) -> Option<Vec<String>> {
    match expression {
        OxcExpression::StringLiteral(value) => Some(vec![value.value.to_string()]),
        OxcExpression::BooleanLiteral(_) | OxcExpression::NumericLiteral(_) => {
            Some(vec![String::new()])
        }
        OxcExpression::ConditionalExpression(expression) => {
            let consequent = class_string_variants_from_oxc(&expression.consequent)?;
            let alternate = class_string_variants_from_oxc(&expression.alternate)?;
            merge_variant_sets(&consequent, &alternate)
        }
        OxcExpression::LogicalExpression(expression) => {
            let right = class_string_variants_from_oxc(&expression.right)?;
            match expression.operator.as_str() {
                "&&" => merge_variant_sets(&[String::new()], &right),
                "||" | "??" => {
                    let left = class_string_variants_from_oxc(&expression.left)?;
                    merge_variant_sets(&left, &right)
                }
                _ => None,
            }
        }
        OxcExpression::BinaryExpression(expression) => {
            if expression.operator.as_str() != "+" {
                return None;
            }
            let left = class_string_variants_from_oxc(&expression.left)?;
            let right = class_string_variants_from_oxc(&expression.right)?;
            cartesian_join_variants(&left, &right)
        }
        OxcExpression::TemplateLiteral(expression) => {
            let mut variants = vec![String::new()];
            for quasi in &expression.quasis {
                let chunk = quasi.value.cooked.as_deref().unwrap_or("");
                variants = cartesian_join_variants(&variants, &[chunk.to_string()])?;
            }
            Some(variants)
        }
        OxcExpression::TSAsExpression(expression) => {
            class_string_variants_from_oxc(&expression.expression)
        }
        OxcExpression::TSSatisfiesExpression(expression) => {
            class_string_variants_from_oxc(&expression.expression)
        }
        OxcExpression::TSNonNullExpression(expression) => {
            class_string_variants_from_oxc(&expression.expression)
        }
        OxcExpression::ParenthesizedExpression(expression) => {
            class_string_variants_from_oxc(&expression.expression)
        }
        OxcExpression::ChainExpression(_) => None,
        _ => None,
    }
}

fn shared_name(value: impl Into<Arc<str>>) -> CssName {
    value.into()
}

#[derive(Clone, Copy)]
struct ElementVisit {
    node_start: usize,
    is_component_like: bool,
    optional: bool,
    parent: Option<usize>,
    depth: usize,
    component_depth: Option<usize>,
}

fn process_element(
    name: &str,
    attributes: &[Attribute],
    context: &mut CssUsageBuilder,
    visit: ElementVisit,
) {
    let tag = shared_name(name.to_ascii_lowercase());
    if visit.is_component_like {
        context.has_component_like_elements = true;
    }
    context.tags.insert(Arc::clone(&tag));

    let mut attrs = CssAttributeMap::new();
    for attr in attributes {
        match attr {
            Attribute::Attribute(named) => {
                let key = shared_name(named.name.as_ref().to_ascii_lowercase());
                let static_value = static_attribute_text_from_value(&named.value);
                if key.as_ref() == "class" {
                    let analysis = analyze_class_attribute_value(&named.value);
                    context.classes.extend(analysis.tokens);
                    context.class_name_unbounded |= analysis.unbounded;
                } else if key.as_ref() == "id"
                    && let Some(value) = static_value.as_ref()
                {
                    let value = shared_name(value.clone());
                    context.ids.insert(Arc::clone(&value));
                    attrs.insert(Arc::clone(&key), value);
                }
                if static_value.is_none()
                    && (matches!(named.value, AttributeValueKind::ExpressionTag(_))
                        || matches!(
                            &named.value,
                            AttributeValueKind::Values(values)
                                if values.iter().any(|value| matches!(value, AttributeValue::ExpressionTag(_)))
                        ))
                {
                    context.dynamic_attributes.insert(Arc::clone(&key));
                }
                if let Some(value) = static_value {
                    attrs.insert(key, shared_name(value));
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
                    context.classes.insert(shared_name(name_str));
                }
            }
            _ => {}
        }
    }

    let _element_index = context.elements.len();
    context.elements.push(CssElementUsage {
        node_start: visit.node_start,
        tag,
        attributes: attrs,
        parent: visit.parent,
        depth: visit.depth,
        component_depth: visit.component_depth,
        optional: visit.optional,
    });
}

fn add_each_candidate_sets(
    context: &mut CssUsageBuilder,
    tags: &CssNameSet,
    classes: &CssNameSet,
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


#[derive(Default, Clone)]
struct BoundaryTokenSet {
    tags: CssNameSet,
    classes: CssNameSet,
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

fn collect_each_boundary_candidates_from_ast(fragment: &Fragment, context: &mut CssUsageBuilder) {
    collect_each_boundary_candidates_in_fragment(fragment, context);
}

fn collect_each_boundary_candidates_in_fragment(
    fragment: &Fragment,
    context: &mut CssUsageBuilder,
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

fn recurse_collect_each_candidates(node: &Node, context: &mut CssUsageBuilder) {
    node.for_each_child_fragment(|fragment| {
        collect_each_boundary_candidates_in_fragment(fragment, context);
    });
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
    current.tags.insert(shared_name(name.to_ascii_lowercase()));
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
                    out.classes.insert(shared_name(name));
                }
            }
            _ => {}
        }
    }
    out
}
