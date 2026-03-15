use crate::api::{ElementKind, SvelteElementKind, classify_element_name, is_void_element_name};
use crate::ast::modern::{
    Attribute, AttributeValue, AttributeValueKind, Comment, Css, CssBlock, CssBlockChild,
    CssCombinator, CssComplexSelector, CssNode, CssPseudoClassSelector, CssRelativeSelector,
    CssRule, CssSelectorList, CssSimpleSelector, EachBlock, Expression, ExpressionTag, Fragment,
    IfBlock, Node, Options, Script,
};
use crate::ast::{Document, Root};
use crate::js::{codegen_options, render};
use oxc_codegen::Codegen;
use svelte_syntax::JsProgram;

const LINE_BREAK_THRESHOLD: usize = 50;

enum FragmentItem<'a> {
    Node(&'a Node),
    Text(String),
}

fn flush_fragment_group<'a>(
    groups: &mut Vec<Vec<FragmentItem<'a>>>,
    current: &mut Vec<FragmentItem<'a>>,
) {
    if !current.is_empty() {
        groups.push(std::mem::take(current));
    }
}

pub fn print_document(ast: &Document, options: &crate::api::PrintOptions) -> String {
    match &ast.root {
        Root::Modern(root) => render_root(ast.source(), root, options),
        Root::Legacy(_) => ast.source().to_string(),
    }
}

pub fn print_modern_root(
    source: &str,
    root: &crate::ast::modern::Root,
    options: &crate::api::PrintOptions,
) -> String {
    render_root(source, root, options)
}

pub fn print_modern_fragment(source: &str, fragment: &Fragment) -> String {
    render_fragment(source, fragment).text
}

pub fn print_modern_node(source: &str, node: &Node) -> String {
    render_node(source, node).text
}

pub fn print_modern_script(
    source: &str,
    script: &Script,
    options: &crate::api::PrintOptions,
) -> String {
    render_script(source, script, 0, options)
}

pub fn print_modern_css(source: &str, style: &Css) -> String {
    render_stylesheet(source, style, 0)
}

pub fn print_modern_css_node(node: &CssNode) -> String {
    render_css_node(node, 0)
}

pub fn print_modern_attribute(source: &str, attribute: &Attribute) -> String {
    render_attribute(source, attribute)
}

pub fn print_modern_options(source: &str, options: &Options) -> String {
    let mut out = String::from("<svelte:options");
    for attribute in options.attributes.iter() {
        out.push(' ');
        out.push_str(&render_attribute(source, attribute));
    }
    out.push_str(" />");
    out
}

pub fn print_modern_comment(comment: &Comment) -> String {
    format!("<!--{}-->", comment.data)
}

fn render_root(
    source: &str,
    root: &crate::ast::modern::Root,
    options: &crate::api::PrintOptions,
) -> String {
    let mut sections = Vec::new();

    if let Some(doctype) = extract_leading_doctype(source) {
        sections.push(doctype);
    }

    if let Some(options) = &root.options {
        let mut out = String::from("<svelte:options");
        for attribute in options.attributes.iter() {
            out.push(' ');
            out.push_str(&render_attribute(source, attribute));
        }
        out.push_str(" />");
        sections.push(out);
    }

    if let Some(module) = &root.module {
        sections.push(render_script(source, module, 0, options));
    }

    if let Some(instance) = &root.instance {
        sections.push(render_script(source, instance, 0, options));
    }

    let fragment = render_fragment(source, &root.fragment);
    if !fragment.text.is_empty() {
        sections.push(fragment.text);
    }

    if let Some(css) = &root.css {
        sections.push(render_stylesheet(source, css, 0));
    }

    sections.join("\n\n")
}

fn render_script(
    source: &str,
    script: &Script,
    depth: usize,
    options: &crate::api::PrintOptions,
) -> String {
    let mut out = String::from("<script");
    let attrs = render_attributes(source, &script.attributes, depth);
    out.push_str(&attrs.text);
    out.push('>');

    let body = render_program_body(source, &script.content, depth + 1, options);
    if !body.is_empty() {
        out.push('\n');
        out.push_str(&body);
        out.push('\n');
    }

    out.push_str("</script>");
    out
}

fn render_program_body(
    _source: &str,
    program: &JsProgram,
    depth: usize,
    _options: &crate::api::PrintOptions,
) -> String {
    render_program_body_with_codegen(program, depth)
}

fn render_program_body_with_codegen(
    program: &JsProgram,
    depth: usize,
) -> String {
    if program.program().body.is_empty() {
        return String::new();
    }

    let rendered = Codegen::new()
        .with_options(codegen_options())
        .with_source_text(program.source())
        .build(program.program())
        .code;

    normalize_indentation(rendered.trim_end(), depth)
}

fn render_stylesheet(source: &str, style: &Css, depth: usize) -> String {
    let mut out = String::from("<style");
    let attrs = render_attributes(source, &style.attributes, depth);
    out.push_str(&attrs.text);
    out.push('>');

    if !style.children.is_empty() {
        out.push('\n');
        let mut started = false;
        for child in style.children.iter() {
            if started {
                out.push('\n');
                out.push('\n');
            }
            out.push_str(&render_css_node(child, depth + 1));
            started = true;
        }
        out.push('\n');
    }

    out.push_str("</style>");
    out
}

fn render_css_node(node: &CssNode, depth: usize) -> String {
    match node {
        CssNode::Rule(rule) => render_css_rule(rule, depth),
        CssNode::Atrule(atrule) => {
            let mut out = format!("{}@{}", tabs(depth), atrule.name);
            if !atrule.prelude.is_empty() {
                out.push(' ');
                out.push_str(atrule.prelude.as_ref());
            }
            if let Some(block) = &atrule.block {
                out.push(' ');
                out.push_str(&render_css_block(block, depth));
            } else {
                out.push(';');
            }
            out
        }
    }
}

fn render_css_rule(rule: &CssRule, depth: usize) -> String {
    let prelude = render_css_selector_list(&rule.prelude, depth);
    let mut out = prelude;
    out.push(' ');
    out.push_str(&render_css_block(&rule.block, depth));
    out
}

fn render_css_selector_list(list: &CssSelectorList, depth: usize) -> String {
    let mut out = String::new();
    for (idx, selector) in list.children.iter().enumerate() {
        if idx > 0 {
            out.push(',');
            out.push('\n');
        }
        out.push_str(&tabs(depth));
        out.push_str(&render_css_complex_selector(selector));
    }
    out
}

fn render_css_complex_selector(selector: &CssComplexSelector) -> String {
    let mut out = String::new();
    for child in selector.children.iter() {
        out.push_str(&render_css_relative_selector(child));
    }
    out
}

fn render_css_relative_selector(selector: &CssRelativeSelector) -> String {
    let mut out = String::new();
    if let Some(CssCombinator { name, .. }) = &selector.combinator {
        if name.as_ref() == " " {
            out.push(' ');
        } else {
            out.push(' ');
            out.push_str(name.as_ref());
            out.push(' ');
        }
    }
    for simple in selector.selectors.iter() {
        out.push_str(&render_css_simple_selector(simple));
    }
    out
}

fn render_css_simple_selector(selector: &CssSimpleSelector) -> String {
    match selector {
        CssSimpleSelector::TypeSelector(node) => node.name.to_string(),
        CssSimpleSelector::IdSelector(node) => format!("#{}", node.name),
        CssSimpleSelector::ClassSelector(node) => format!(".{}", node.name),
        CssSimpleSelector::PseudoElementSelector(node) => format!("::{}", node.name),
        CssSimpleSelector::PseudoClassSelector(node) => render_css_pseudo_class(node),
        CssSimpleSelector::AttributeSelector(node) => {
            let mut out = format!("[{}", node.name);
            if let Some(matcher) = &node.matcher {
                out.push_str(matcher.as_ref());
                out.push('"');
                out.push_str(node.value.as_deref().unwrap_or_default());
                out.push('"');
                if let Some(flags) = &node.flags {
                    out.push(' ');
                    out.push_str(flags.as_ref());
                }
            }
            out.push(']');
            out
        }
        CssSimpleSelector::Nth(node) => node.value.to_string(),
        CssSimpleSelector::Percentage(node) => format!("{}%", node.value),
        CssSimpleSelector::NestingSelector(_) => "&".to_string(),
    }
}

fn render_css_pseudo_class(selector: &CssPseudoClassSelector) -> String {
    let mut out = format!(":{}", selector.name);
    if let Some(args) = &selector.args {
        out.push('(');
        for (idx, selector) in args.children.iter().enumerate() {
            if idx > 0 {
                out.push_str(", ");
            }
            out.push_str(&render_css_complex_selector(selector));
        }
        out.push(')');
    }
    out
}

fn render_css_block(block: &CssBlock, depth: usize) -> String {
    let mut out = String::from("{");
    if !block.children.is_empty() {
        out.push('\n');
        let mut started = false;
        for child in block.children.iter() {
            if started {
                out.push('\n');
            }
            match child {
                CssBlockChild::Declaration(decl) => {
                    out.push_str(&tabs(depth + 1));
                    out.push_str(decl.property.as_ref());
                    out.push_str(": ");
                    out.push_str(decl.value.as_ref());
                    out.push(';');
                }
                CssBlockChild::Rule(rule) => out.push_str(&render_css_rule(rule, depth + 1)),
                CssBlockChild::Atrule(atrule) => out.push_str(&render_css_node(
                    &CssNode::Atrule(atrule.clone()),
                    depth + 1,
                )),
            }
            started = true;
        }
        out.push('\n');
        out.push_str(&tabs(depth));
    }
    out.push('}');
    out
}

fn render_fragment(source: &str, fragment: &Fragment) -> Rendered {
    let mut groups: Vec<Vec<FragmentItem<'_>>> = Vec::new();
    let mut current: Vec<FragmentItem<'_>> = Vec::new();

    for (idx, node) in fragment.nodes.iter().enumerate() {
        let prev = if idx > 0 {
            Some(&fragment.nodes[idx - 1])
        } else {
            None
        };
        let next = fragment.nodes.get(idx + 1);

        if let Node::Text(text) = node {
            let mut data = collapse_whitespace(text.data.as_ref());

            if idx == 0 {
                data = data.trim_start().to_string();
            }
            if idx + 1 == fragment.nodes.len() {
                data = data.trim_end().to_string();
            }
            if data.is_empty() {
                continue;
            }

            if data.starts_with(' ') && prev.is_some_and(|node| !is_expression_tag(node)) {
                flush_fragment_group(&mut groups, &mut current);
                data = data.trim_start().to_string();
            }

            if !data.is_empty() {
                current.push(FragmentItem::Text(data.clone()));
                if data.ends_with(' ') && next.is_some_and(|node| !is_expression_tag(node)) {
                    flush_fragment_group(&mut groups, &mut current);
                }
            }
            continue;
        }

        let block_element = is_block_element(node);
        if block_element {
            flush_fragment_group(&mut groups, &mut current);
        }
        current.push(FragmentItem::Node(node));
        if block_element {
            flush_fragment_group(&mut groups, &mut current);
        }
    }
    flush_fragment_group(&mut groups, &mut current);

    let mut pieces = Vec::new();
    let mut total_width = 0usize;
    let mut multiline = false;

    for group in groups.into_iter().filter(|group| !group.is_empty()) {
        let mut text = String::new();
        let mut group_multiline = false;
        let mut group_is_comment = true;
        for item in group {
            let piece = match item {
                FragmentItem::Node(node) => render_node(source, node),
                FragmentItem::Text(text) => rendered(text),
            };
            if !piece.is_comment {
                group_is_comment = false;
            }
            group_multiline |= piece.multiline;
            text.push_str(&piece.text);
        }

        let mut piece = rendered(text);
        piece.is_comment = group_is_comment;
        multiline |= piece.multiline || group_multiline;
        total_width += piece.width;
        pieces.push(piece);
    }

    multiline |= total_width > LINE_BREAK_THRESHOLD;

    let mut out = String::new();
    for i in 0..pieces.len() {
        out.push_str(&pieces[i].text);
        if i + 1 < pieces.len() {
            let next = &pieces[i + 1];
            if pieces[i].is_comment && next.is_comment {
                continue;
            }
            if pieces[i].multiline || next.multiline {
                out.push_str("\n\n");
            } else if multiline {
                out.push('\n');
            }
        }
    }

    let mut result = rendered(out);
    result.multiline |= multiline;
    result
}

fn render_node(source: &str, node: &Node) -> Rendered {
    match node {
        Node::Text(text) => rendered(text.data.to_string()),
        Node::Comment(comment) => rendered_comment(format!("<!--{}-->", comment.data)),
        Node::ExpressionTag(tag) => rendered(format!("{{{}}}", render_expression(&tag.expression))),
        Node::RenderTag(tag) => rendered(format!(
            "{{@render {}}}",
            render_expression(&tag.expression)
        )),
        Node::HtmlTag(tag) => rendered(format!("{{@html {}}}", render_expression(&tag.expression))),
        Node::ConstTag(tag) => {
            let declaration = render_expression(&tag.declaration);
            let declaration = declaration.trim();
            let declaration = declaration
                .strip_prefix("const ")
                .unwrap_or(declaration)
                .trim();
            let declaration = if declaration.ends_with(';') {
                declaration.to_string()
            } else {
                format!("{declaration};")
            };
            rendered(format!("{{@const {declaration}}}"))
        }
        Node::DebugTag(tag) => {
            let identifiers = tag
                .identifiers
                .iter()
                .map(|identifier| identifier.name.as_ref())
                .collect::<Vec<_>>()
                .join(", ");
            if identifiers.is_empty() {
                rendered("{@debug}".to_string())
            } else {
                rendered(format!("{{@debug {identifiers}}}"))
            }
        }
        Node::SvelteElement(el) => rendered(render_element_with_this(
            ElementRender {
                source,
                start: el.start,
                end: el.end,
                name: &el.name,
                attributes: &el.attributes,
                fragment: &el.fragment,
                component: false,
            },
            el.expression.as_ref(),
        )),
        Node::SvelteComponent(el) => rendered(render_element_with_this(
            ElementRender {
                source,
                start: el.start,
                end: el.end,
                name: &el.name,
                attributes: &el.attributes,
                fragment: &el.fragment,
                component: true,
            },
            el.expression.as_ref(),
        )),
        Node::IfBlock(block) => rendered(render_if_block(source, block)),
        Node::EachBlock(block) => rendered(render_each_block(source, block)),
        Node::KeyBlock(block) => {
            let mut out = format!("{{#key {}}}", render_expression(&block.expression));
            out.push_str(&render_block(source, &block.fragment, false));
            out.push_str("{/key}");
            rendered(out)
        }
        Node::AwaitBlock(block) => {
            let mut out = format!("{{#await {}}}", render_expression(&block.expression));
            if let Some(pending) = &block.pending {
                out.push_str(&render_block(source, pending, false));
            }
            if let Some(then) = &block.then {
                out.push_str("{:then");
                if let Some(value) = &block.value {
                    out.push(' ');
                    out.push_str(&render_expression(value));
                }
                out.push('}');
                out.push_str(&render_block(source, then, false));
            }
            if let Some(catch) = &block.catch {
                out.push_str("{:catch");
                if let Some(error) = &block.error {
                    out.push(' ');
                    out.push_str(&render_expression(error));
                }
                out.push('}');
                out.push_str(&render_block(source, catch, false));
            }
            out.push_str("{/await}");
            rendered(out)
        }
        Node::SnippetBlock(block) => {
            let mut out = format!("{{#snippet {}", render_expression(&block.expression));
            if let Some(type_params) = &block.type_params {
                out.push('<');
                out.push_str(type_params.as_ref());
                out.push('>');
            }
            out.push('(');
            for (idx, parameter) in block.parameters.iter().enumerate() {
                if idx > 0 {
                    out.push_str(", ");
                }
                out.push_str(&render_expression(parameter));
            }
            out.push_str(")}");
            out.push_str(&render_block(source, &block.body, false));
            out.push_str("{/snippet}");
            rendered(out)
        }
        _ => {
            let Some(el) = node.as_element() else {
                unreachable!()
            };
            let component = matches!(node, Node::Component(_));
            rendered(render_element(
                source,
                el.start(),
                el.end(),
                el.name(),
                el.attributes(),
                el.fragment(),
                component,
            ))
        }
    }
}

fn render_if_block(source: &str, block: &IfBlock) -> String {
    let mut out = String::new();
    if block.elseif {
        out.push_str("{:else if ");
        out.push_str(&render_expression(&block.test));
        out.push('}');
    } else {
        out.push_str("{#if ");
        out.push_str(&render_expression(&block.test));
        out.push('}');
    }

    out.push_str(&render_block(source, &block.consequent, false));

    if let Some(alternate) = &block.alternate {
        match alternate.as_ref() {
            crate::ast::modern::Alternate::Fragment(fragment)
                if fragment.nodes.len() == 1
                    && matches!(
                        fragment.nodes[0],
                        Node::IfBlock(IfBlock { elseif: true, .. })
                    ) =>
            {
                out.push_str(&render_fragment(source, fragment).text);
            }
            crate::ast::modern::Alternate::Fragment(fragment) => {
                out.push_str("{:else}");
                out.push_str(&render_block(source, fragment, false));
            }
            crate::ast::modern::Alternate::IfBlock(elseif) => {
                out.push_str(&render_if_block(source, elseif));
            }
        }
    }

    if !block.elseif {
        out.push_str("{/if}");
    }

    out
}

fn render_each_block(source: &str, block: &EachBlock) -> String {
    let mut out = format!("{{#each {}", render_expression(&block.expression));
    if let Some(context) = &block.context {
        out.push_str(" as ");
        out.push_str(&render_expression(context));
    }
    if let Some(index) = &block.index {
        out.push_str(", ");
        out.push_str(index.as_ref());
    }
    if let Some(key) = &block.key {
        out.push_str(" (");
        out.push_str(&render_expression(key));
        out.push(')');
    }
    out.push('}');
    out.push_str(&render_block(source, &block.body, false));

    if let Some(fallback) = &block.fallback {
        out.push_str("{:else}");
        out.push_str(&render_block(source, fallback, false));
    }

    out.push_str("{/each}");
    out
}

fn render_block(source: &str, fragment: &Fragment, allow_inline: bool) -> String {
    let child = render_fragment(source, fragment);
    if child.text.is_empty() {
        return String::new();
    }
    if allow_inline && !child.multiline {
        return child.text;
    }

    let mut out = String::new();
    out.push('\n');
    out.push_str(&indent_block(&child.text, 1));
    out.push('\n');
    out
}

fn render_element(
    source: &str,
    start: usize,
    end: usize,
    name: &str,
    attributes: &[Attribute],
    fragment: &Fragment,
    component: bool,
) -> String {
    render_element_core(
        ElementRender {
            source,
            start,
            end,
            name,
            attributes,
            fragment,
            component,
        },
        None,
    )
}

struct ElementRender<'a> {
    source: &'a str,
    start: usize,
    end: usize,
    name: &'a str,
    attributes: &'a [Attribute],
    fragment: &'a Fragment,
    component: bool,
}

fn render_element_core(element: ElementRender<'_>, expression: Option<&Expression>) -> String {
    let ElementRender {
        source,
        start,
        end,
        name,
        attributes,
        fragment,
        component,
    } = element;
    let mut out = format!("<{name}");

    // Render `this={expression}` first if present
    if let Some(expr) = expression {
        out.push_str(&format!(" this={{{}}}", render_expression(expr)));
    }

    let attrs = render_attributes(source, attributes, 0);
    out.push_str(&attrs.text);

    let component_like = component
        || matches!(
            classify_element_name(name),
            ElementKind::Svelte(SvelteElementKind::Component)
        );
    let is_self_closing =
        is_void_element_name(name) || (component_like && fragment.nodes.is_empty());

    if is_self_closing {
        if attrs.multiline {
            out.push_str("/>");
        } else {
            out.push_str(" />");
        }
        return out;
    }

    out.push('>');
    let allow_inline = !matches!(
        classify_element_name(name),
        ElementKind::Svelte(SvelteElementKind::Element)
    );
    if fragment.nodes.is_empty()
        && let Some(inner) = extract_raw_element_inner(source, start, end, name)
        && !inner.trim().is_empty()
    {
        out.push_str(inner.trim());
    } else {
        out.push_str(&render_block(source, fragment, allow_inline));
    }
    out.push_str("</");
    out.push_str(name);
    out.push('>');
    out
}

fn render_element_with_this(element: ElementRender<'_>, expression: Option<&Expression>) -> String {
    render_element_core(element, expression)
}

struct RenderedAttributes {
    text: String,
    multiline: bool,
}

fn render_attributes(source: &str, attributes: &[Attribute], depth: usize) -> RenderedAttributes {
    if attributes.is_empty() {
        return RenderedAttributes {
            text: String::new(),
            multiline: false,
        };
    }

    let rendered = attributes
        .iter()
        .map(|attribute| render_attribute(source, attribute))
        .collect::<Vec<_>>();

    let length = rendered
        .iter()
        .map(|value| value.chars().count() + 1)
        .sum::<usize>()
        .saturating_sub(1);
    let multiline = length > LINE_BREAK_THRESHOLD;

    if multiline {
        let mut text = String::new();
        for attribute in rendered {
            text.push('\n');
            text.push_str(&tabs(depth + 1));
            text.push_str(&attribute);
        }
        text.push('\n');
        text.push_str(&tabs(depth));
        RenderedAttributes { text, multiline }
    } else {
        let text = rendered
            .into_iter()
            .map(|attribute| format!(" {attribute}"))
            .collect::<String>();
        RenderedAttributes { text, multiline }
    }
}

fn render_attribute(source: &str, attribute: &Attribute) -> String {
    match attribute {
        Attribute::Attribute(attribute) => {
            if let Some(raw) = source_slice(source, attribute.start, attribute.end) {
                let raw = raw.trim();
                if raw.starts_with("{...") && raw.ends_with('}') {
                    return raw.to_string();
                }
                if raw.starts_with('{') && raw.ends_with('}') {
                    let inner = raw.trim_start_matches('{').trim_end_matches('}').trim();
                    if is_identifier(inner) {
                        return format!("{inner}={{{inner}}}");
                    }
                }

                let raw_name = raw_attribute_name(raw).unwrap_or_default();
                let has_lost_directive =
                    raw_name.contains(':') && raw_name != attribute.name.as_ref();
                let has_empty_value = matches!(
                    &attribute.value,
                    AttributeValueKind::Values(values) if values.is_empty()
                );
                if attribute.name.is_empty() || has_lost_directive || has_empty_value {
                    return raw.to_string();
                }
            }

            let mut out = attribute.name.to_string();
            match &attribute.value {
                AttributeValueKind::Boolean(true) => {}
                AttributeValueKind::Boolean(false) => out.push_str("={false}"),
                AttributeValueKind::ExpressionTag(tag) => {
                    out.push('=');
                    out.push_str(&render_expression_tag(tag));
                }
                AttributeValueKind::Values(parts) => {
                    out.push('=');
                    let quote =
                        parts.len() > 1 || matches!(parts.first(), Some(AttributeValue::Text(_)));
                    if quote {
                        out.push('"');
                    }
                    for part in parts.iter() {
                        out.push_str(&render_attribute_value(source, part));
                    }
                    if quote {
                        out.push('"');
                    }
                }
            }
            out
        }
        Attribute::BindDirective(directive) => {
            let mut out = format!("bind:{}", directive.name);
            for modifier in directive.modifiers.iter() {
                out.push('|');
                out.push_str(modifier.as_ref());
            }
            let shorthand = directive.expression.identifier_name()
                .is_some_and(|name| name.as_ref() == directive.name.as_ref());
            if !shorthand && !directive.expression.is_empty() {
                out.push('=');
                out.push('{');
                out.push_str(&render_expression(&directive.expression));
                out.push('}');
            }
            out
        }
        Attribute::OnDirective(directive) => {
            let mut out = format!("on:{}", directive.name);
            for modifier in directive.modifiers.iter() {
                out.push('|');
                out.push_str(modifier.as_ref());
            }
            let shorthand = directive.expression.identifier_name()
                .is_some_and(|name| name.as_ref() == directive.name.as_ref());
            if !shorthand && !directive.expression.is_empty() {
                out.push('=');
                out.push('{');
                out.push_str(&render_expression(&directive.expression));
                out.push('}');
            }
            out
        }
        Attribute::ClassDirective(directive) => {
            let mut out = format!("class:{}", directive.name);
            for modifier in directive.modifiers.iter() {
                out.push('|');
                out.push_str(modifier.as_ref());
            }
            let shorthand = directive.expression.identifier_name()
                .is_some_and(|name| name.as_ref() == directive.name.as_ref());
            if !shorthand && !directive.expression.is_empty() {
                out.push('=');
                out.push('{');
                out.push_str(&render_expression(&directive.expression));
                out.push('}');
            }
            out
        }
        Attribute::LetDirective(directive) => {
            let mut out = format!("let:{}", directive.name);
            for modifier in directive.modifiers.iter() {
                out.push('|');
                out.push_str(modifier.as_ref());
            }
            let shorthand = directive.expression.identifier_name()
                .is_some_and(|name| name.as_ref() == directive.name.as_ref());
            if !shorthand && !directive.expression.is_empty() {
                out.push('=');
                out.push('{');
                out.push_str(&render_expression(&directive.expression));
                out.push('}');
            }
            out
        }
        Attribute::StyleDirective(directive) => {
            let mut out = format!("style:{}", directive.name);
            for modifier in directive.modifiers.iter() {
                out.push('|');
                out.push_str(modifier.as_ref());
            }
            match &directive.value {
                AttributeValueKind::Boolean(true) => {}
                AttributeValueKind::Boolean(false) => out.push_str("={false}"),
                AttributeValueKind::ExpressionTag(tag) => {
                    out.push('=');
                    out.push_str(&render_expression_tag(tag));
                }
                AttributeValueKind::Values(parts) => {
                    out.push('=');
                    let quote =
                        parts.len() > 1 || matches!(parts.first(), Some(AttributeValue::Text(_)));
                    if quote {
                        out.push('"');
                    }
                    for part in parts.iter() {
                        out.push_str(&render_attribute_value(source, part));
                    }
                    if quote {
                        out.push('"');
                    }
                }
            }
            out
        }
        Attribute::TransitionDirective(directive) => {
            let kind = if directive.intro && directive.outro {
                "transition"
            } else if directive.intro {
                "in"
            } else {
                "out"
            };
            let mut out = format!("{kind}:{}", directive.name);
            for modifier in directive.modifiers.iter() {
                out.push('|');
                out.push_str(modifier.as_ref());
            }
            let shorthand = directive.expression.identifier_name()
                .is_some_and(|name| name.as_ref() == directive.name.as_ref());
            if !shorthand && !directive.expression.is_empty() {
                out.push('=');
                out.push('{');
                out.push_str(&render_expression(&directive.expression));
                out.push('}');
            }
            out
        }
        Attribute::AnimateDirective(directive) => {
            let mut out = format!("animate:{}", directive.name);
            for modifier in directive.modifiers.iter() {
                out.push('|');
                out.push_str(modifier.as_ref());
            }
            let shorthand = directive.expression.identifier_name()
                .is_some_and(|name| name.as_ref() == directive.name.as_ref());
            if !shorthand && !directive.expression.is_empty() {
                out.push('=');
                out.push('{');
                out.push_str(&render_expression(&directive.expression));
                out.push('}');
            }
            out
        }
        Attribute::UseDirective(directive) => {
            let mut out = format!("use:{}", directive.name);
            for modifier in directive.modifiers.iter() {
                out.push('|');
                out.push_str(modifier.as_ref());
            }
            let shorthand = directive.expression.identifier_name()
                .is_some_and(|name| name.as_ref() == directive.name.as_ref());
            if !shorthand && !directive.expression.is_empty() {
                out.push('=');
                out.push('{');
                out.push_str(&render_expression(&directive.expression));
                out.push('}');
            }
            out
        }
        Attribute::AttachTag(tag) => {
            format!("{{@attach {}}}", render_expression(&tag.expression))
        }
        Attribute::SpreadAttribute(spread) => {
            format!("{{...{}}}", render_expression(&spread.expression))
        }
    }
}

fn render_attribute_value(_source: &str, value: &AttributeValue) -> String {
    match value {
        AttributeValue::Text(text) => text.raw.to_string(),
        AttributeValue::ExpressionTag(tag) => render_expression_tag(tag),
    }
}

fn render_expression_tag(tag: &ExpressionTag) -> String {
    format!("{{{}}}", render_expression(&tag.expression))
}

fn render_expression(expression: &Expression) -> String {
    render(expression).unwrap_or_default()
}

fn source_slice(source: &str, start: usize, end: usize) -> Option<&str> {
    if start > end || end > source.len() {
        return None;
    }
    // Keep source slicing centralized for printing paths.
    source.get(start..end)
}

fn extract_raw_element_inner<'a>(
    source: &'a str,
    start: usize,
    end: usize,
    name: &str,
) -> Option<&'a str> {
    let raw = source_slice(source, start, end)?;
    let open_end = find_tag_close_index(raw)?;
    let close = format!("</{name}>");
    let close_start = raw.rfind(&close)?;
    if close_start <= open_end {
        return None;
    }
    raw.get((open_end + 1)..close_start)
}

fn find_tag_close_index(raw: &str) -> Option<usize> {
    let mut in_single = false;
    let mut in_double = false;
    let mut in_template = false;
    let mut escaped = false;
    let mut depth_brace = 0usize;
    let mut depth_paren = 0usize;
    let mut depth_bracket = 0usize;

    for (idx, ch) in raw.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }

        if in_single {
            if ch == '\\' {
                escaped = true;
            } else if ch == '\'' {
                in_single = false;
            }
            continue;
        }

        if in_double {
            if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_double = false;
            }
            continue;
        }

        if in_template {
            if ch == '\\' {
                escaped = true;
            } else if ch == '`' {
                in_template = false;
            }
            continue;
        }

        match ch {
            '\'' => in_single = true,
            '"' => in_double = true,
            '`' => in_template = true,
            '{' => depth_brace += 1,
            '}' => depth_brace = depth_brace.saturating_sub(1),
            '(' => depth_paren += 1,
            ')' => depth_paren = depth_paren.saturating_sub(1),
            '[' => depth_bracket += 1,
            ']' => depth_bracket = depth_bracket.saturating_sub(1),
            '>' if depth_brace == 0 && depth_paren == 0 && depth_bracket == 0 => {
                return Some(idx);
            }
            _ => {}
        }
    }

    None
}

fn raw_attribute_name(raw: &str) -> Option<&str> {
    let trimmed = raw.trim_start();
    if trimmed.is_empty() {
        return None;
    }
    let end = trimmed
        .char_indices()
        .find_map(|(idx, ch)| (ch.is_whitespace() || ch == '=').then_some(idx))
        .unwrap_or(trimmed.len());
    trimmed.get(..end)
}

fn extract_leading_doctype(source: &str) -> Option<String> {
    let trimmed = source.trim_start();
    if !trimmed.to_ascii_lowercase().starts_with("<!doctype") {
        return None;
    }
    let end = trimmed.find('>')?;
    Some(trimmed[..=end].trim().to_string())
}

fn normalize_indentation(snippet: &str, depth: usize) -> String {
    let lines = snippet.lines().collect::<Vec<_>>();
    if lines.is_empty() {
        return String::new();
    }

    let trailing_trimmed = lines.iter().map(|line| line.trim_end()).collect::<Vec<_>>();

    let min_indent_rest = trailing_trimmed
        .iter()
        .skip(1)
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.chars().take_while(|ch| ch.is_whitespace()).count())
        .min()
        .unwrap_or(0);

    trailing_trimmed
        .iter()
        .enumerate()
        .map(|(idx, line)| {
            let stripped = if idx == 0 {
                line.trim_start()
            } else {
                strip_prefix_whitespace(line, min_indent_rest)
            };
            if stripped.is_empty() {
                return String::new();
            }
            format!("{}{}", tabs(depth), stripped)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first == '$' || first.is_ascii_alphabetic()) {
        return false;
    }
    chars.all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric())
}

fn strip_prefix_whitespace(text: &str, max: usize) -> &str {
    for (count, (idx, ch)) in text.char_indices().enumerate() {
        if count >= max || !ch.is_whitespace() {
            return &text[idx..];
        }
    }
    ""
}

fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_ws = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !in_ws {
                out.push(' ');
                in_ws = true;
            }
        } else {
            in_ws = false;
            out.push(ch);
        }
    }
    out
}

fn indent_block(text: &str, depth: usize) -> String {
    text.lines()
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("{}{}", tabs(depth), line)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn tabs(depth: usize) -> String {
    "\t".repeat(depth)
}

fn is_expression_tag(node: &Node) -> bool {
    matches!(node, Node::ExpressionTag(_))
}

fn is_block_element(node: &Node) -> bool {
    matches!(
        node,
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
            | Node::TitleElement(_)
            | Node::IfBlock(_)
            | Node::EachBlock(_)
            | Node::KeyBlock(_)
            | Node::AwaitBlock(_)
            | Node::SnippetBlock(_)
            | Node::RenderTag(_)
            | Node::HtmlTag(_)
            | Node::ConstTag(_)
    )
}

struct Rendered {
    text: String,
    multiline: bool,
    width: usize,
    is_comment: bool,
}

fn rendered(text: String) -> Rendered {
    let multiline = text.contains('\n');
    let width = if multiline { 0 } else { text.chars().count() };
    Rendered {
        text,
        multiline,
        width,
        is_comment: false,
    }
}

fn rendered_comment(text: String) -> Rendered {
    let mut value = rendered(text);
    value.is_comment = true;
    value
}
