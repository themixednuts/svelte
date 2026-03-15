use std::collections::BTreeSet;
use std::marker::PhantomData;

use camino::Utf8Path;
use oxc_ast::ast::{
    ChainElement, Declaration, Expression as OxcExpression, Statement as OxcStatement,
};
use oxc_span::GetSpan;
use svelte_syntax::ParsedJsProgram;

use crate::ast::modern::{
    Alternate, Attribute, AttributeValue, AttributeValueList, AwaitBlock, DebugTag, Expression,
    Fragment, IfBlock, KeyBlock, NamedAttribute, Node, RegularElement, RenderTag, Root,
    SvelteBoundary, SvelteElement, SvelteFragment, SvelteHead, TitleElement,
};
use crate::js::Render;

const TEMPLATE_COMPONENT_CLIENT: &str = include_str!("templates/component.client.js");
const TEMPLATE_COMPONENT_SERVER: &str = include_str!("templates/component.server.js");

pub(crate) trait RenderBackend {
    const TEMPLATE: &'static str;
}

pub(crate) struct ClientRenderBackend;

impl RenderBackend for ClientRenderBackend {
    const TEMPLATE: &'static str = TEMPLATE_COMPONENT_CLIENT;
}

pub(crate) struct ServerRenderBackend;

impl RenderBackend for ServerRenderBackend {
    const TEMPLATE: &'static str = TEMPLATE_COMPONENT_SERVER;
}

pub(crate) fn compile_generic_markup_js<B: RenderBackend>(
    _source: &str,
    root: &Root,
    filename: Option<&Utf8Path>,
) -> Option<String> {
    let component_name = component_name_from_filename(filename);
    let scripts = collect_script_sections(root)?;

    let mut renderer = GenericRenderer::<B>::new(&component_name);
    renderer.render_fragment(&root.fragment, 1)?;
    let template_body = renderer.finish();

    Some(render_component_template::<B>(
        &component_name,
        &scripts.module_body,
        &scripts.instance_body,
        template_body.trim_end(),
    ))
}

struct GenericRenderer<'a, B: RenderBackend> {
    component_name: &'a str,
    output: String,
    each_counter: usize,
    temp_counter: usize,
    marker: PhantomData<B>,
}

impl<'a, B: RenderBackend> GenericRenderer<'a, B> {
    fn new(component_name: &'a str) -> Self {
        Self {
            component_name,
            output: String::new(),
            each_counter: 0,
            temp_counter: 0,
            marker: PhantomData,
        }
    }

    fn finish(self) -> String {
        self.output
    }

    fn push_line(&mut self, indent: usize, line: &str) {
        self.output.push_str(&"\t".repeat(indent));
        self.output.push_str(line);
        self.output.push('\n');
    }

    fn next_temp(&mut self, prefix: &str) -> String {
        self.temp_counter += 1;
        format!("{prefix}_{}", self.temp_counter)
    }

    fn render_fragment(&mut self, fragment: &Fragment, indent: usize) -> Option<()> {
        for node in fragment.nodes.iter() {
            self.render_node(node, indent)?;
        }
        Some(())
    }

    fn render_node(&mut self, node: &Node, indent: usize) -> Option<()> {
        match node {
            Node::Text(text) => {
                if text.raw.is_empty() {
                    return Some(());
                }
                self.push_line(
                    indent,
                    &format!(
                        "$$renderer.push(`{}`);",
                        escape_js_template_literal(text.raw.as_ref())
                    ),
                );
                Some(())
            }
            Node::Comment(comment) => {
                self.push_line(
                    indent,
                    &format!(
                        "$$renderer.push(`<!--{}-->`);",
                        escape_js_template_literal(comment.data.as_ref())
                    ),
                );
                Some(())
            }
            Node::ExpressionTag(tag) => {
                let expression = tag.expression.render()?;
                self.push_line(indent, "$$renderer.push(`<!---->`);");
                self.push_line(indent, &format!("$$renderer.push($.escape({expression}));"));
                Some(())
            }
            Node::HtmlTag(tag) => {
                let expression = tag.expression.render()?;
                self.push_line(indent, &format!("$$renderer.push($.html({expression}));"));
                Some(())
            }
            Node::RenderTag(tag) => self.render_render_tag(tag, indent),
            Node::ConstTag(tag) => self.render_const_tag(tag, indent),
            Node::DebugTag(tag) => self.render_debug_tag(tag, indent),
            Node::RegularElement(element) => self.render_regular_element(element, indent),
            Node::IfBlock(if_block) => self.render_if_block(if_block, indent),
            Node::EachBlock(each_block) => self.render_each_block(each_block, indent),
            Node::KeyBlock(key_block) => self.render_key_block(key_block, indent),
            Node::AwaitBlock(await_block) => self.render_await_block(await_block, indent),
            Node::SnippetBlock(snippet) => self.render_snippet_block(snippet, indent),
            Node::Component(component) => self.render_static_component_call(
                component.name.as_ref(),
                &component.attributes,
                &component.fragment,
                indent,
            ),
            Node::SvelteComponent(component) => {
                let expression = component.expression.as_ref()?.render()?;
                self.render_dynamic_component_call(
                    &expression,
                    &component.attributes,
                    &component.fragment,
                    indent,
                )
            }
            Node::SvelteSelf(component) => self.render_static_component_call(
                self.component_name,
                &component.attributes,
                &component.fragment,
                indent,
            ),
            Node::SvelteElement(element) => self.render_svelte_element(element, indent),
            Node::SvelteFragment(fragment) => self.render_svelte_fragment(fragment, indent),
            Node::SvelteHead(head) => self.render_svelte_head(head, indent),
            Node::TitleElement(title) => self.render_title_element(title, indent),
            Node::SvelteBoundary(boundary) => self.render_svelte_boundary(boundary, indent),
            Node::SvelteBody(body) => self.render_fragment(&body.fragment, indent),
            Node::SvelteWindow(window) => self.render_fragment(&window.fragment, indent),
            Node::SvelteDocument(document) => self.render_fragment(&document.fragment, indent),
            Node::SlotElement(slot) => self.render_slot_element(slot, indent),
        }
    }

    fn render_render_tag(&mut self, tag: &RenderTag, indent: usize) -> Option<()> {
        let call = render_render_tag_call(&tag.expression)?;
        self.push_line(indent, &format!("{call};"));
        self.push_line(indent, "$$renderer.push(`<!---->`);");
        Some(())
    }

    fn render_const_tag(
        &mut self,
        tag: &crate::ast::modern::ConstTag,
        indent: usize,
    ) -> Option<()> {
        let mut declaration = tag.declaration.render()?;
        if !declaration.ends_with(';') {
            declaration.push(';');
        }
        self.push_line(indent, &declaration);
        Some(())
    }

    fn render_debug_tag(&mut self, tag: &DebugTag, indent: usize) -> Option<()> {
        let object_fields = tag
            .identifiers
            .iter()
            .map(|identifier| {
                let name = identifier.name.as_ref();
                format!("{name}: {name}")
            })
            .collect::<Vec<_>>()
            .join(", ");
        self.push_line(indent, &format!("console.log({{{object_fields}}});"));
        self.push_line(indent, "debugger;");
        Some(())
    }

    fn render_regular_element(&mut self, element: &RegularElement, indent: usize) -> Option<()> {
        self.push_line(
            indent,
            &format!(
                "$$renderer.push(`<{}`);",
                escape_js_template_literal(element.name.as_ref())
            ),
        );
        for attribute in element.attributes.iter() {
            self.render_html_attribute(attribute, indent)?;
        }

        if element.self_closing && !element.has_end_tag {
            self.push_line(indent, "$$renderer.push(`/>`);");
            return Some(());
        }

        self.push_line(indent, "$$renderer.push(`>`);");
        self.render_fragment(&element.fragment, indent)?;
        self.push_line(
            indent,
            &format!(
                "$$renderer.push(`</{}>`);",
                escape_js_template_literal(element.name.as_ref())
            ),
        );
        Some(())
    }

    fn render_html_attribute(&mut self, attribute: &Attribute, indent: usize) -> Option<()> {
        match attribute {
            Attribute::Attribute(attribute) => self.render_named_html_attribute(attribute, indent),
            Attribute::SpreadAttribute(spread) => {
                let expression = spread.expression.render()?;
                self.push_line(
                    indent,
                    &format!("$$renderer.push($.attributes({expression}, null, null, null));"),
                );
                Some(())
            }
            Attribute::ClassDirective(directive) => {
                let expression = directive.expression.render()?;
                let key = js_single_quoted_string(directive.name.as_ref());
                self.push_line(
                    indent,
                    &format!(
                        "$$renderer.push($.attr_class('', null, {{ {key}: ({expression}) }}));"
                    ),
                );
                Some(())
            }
            Attribute::StyleDirective(directive) => {
                let expression = self.attribute_value_expression(&directive.value)?;
                let key = js_single_quoted_string(directive.name.as_ref());
                self.push_line(
                    indent,
                    &format!("$$renderer.push($.attr_style('', {{ {key}: ({expression}) }}));"),
                );
                Some(())
            }
            Attribute::BindDirective(directive) if directive.name.as_ref() == "this" => Some(()),
            Attribute::BindDirective(directive) => {
                let expression = directive.expression.render()?;
                let name = js_single_quoted_string(directive.name.as_ref());
                self.push_line(
                    indent,
                    &format!("$$renderer.push($.attr({name}, {expression}, false));"),
                );
                Some(())
            }
            Attribute::OnDirective(_)
            | Attribute::LetDirective(_)
            | Attribute::TransitionDirective(_)
            | Attribute::AnimateDirective(_)
            | Attribute::UseDirective(_)
            | Attribute::AttachTag(_) => Some(()),
        }
    }

    fn render_named_html_attribute(
        &mut self,
        attribute: &NamedAttribute,
        indent: usize,
    ) -> Option<()> {
        let attribute_name = escape_js_template_literal(attribute.name.as_ref());
        match &attribute.value {
            AttributeValueList::Boolean(true) => {
                self.push_line(indent, &format!("$$renderer.push(` {attribute_name}`);"));
                Some(())
            }
            AttributeValueList::Boolean(false) => None,
            AttributeValueList::ExpressionTag(tag) => {
                let expression = tag.expression.render()?;
                self.push_line(indent, &format!("$$renderer.push(` {attribute_name}=\"`);"));
                self.push_line(
                    indent,
                    &format!("$$renderer.push($.escape({expression}, true));"),
                );
                self.push_line(indent, "$$renderer.push(`\"`);");
                Some(())
            }
            AttributeValueList::Values(values) => {
                self.push_line(indent, &format!("$$renderer.push(` {attribute_name}=\"`);"));
                for value in values.iter() {
                    match value {
                        AttributeValue::Text(text) => {
                            self.push_line(
                                indent,
                                &format!(
                                    "$$renderer.push(`{}`);",
                                    escape_js_template_literal(text.raw.as_ref())
                                ),
                            );
                        }
                        AttributeValue::ExpressionTag(tag) => {
                            let expression = tag.expression.render()?;
                            self.push_line(
                                indent,
                                &format!("$$renderer.push($.escape({expression}, true));"),
                            );
                        }
                    }
                }
                self.push_line(indent, "$$renderer.push(`\"`);");
                Some(())
            }
        }
    }

    fn render_static_component_call(
        &mut self,
        callee: &str,
        attributes: &[Attribute],
        fragment: &Fragment,
        indent: usize,
    ) -> Option<()> {
        let props = self.build_component_props(attributes, fragment, indent)?;
        self.push_line(indent, &format!("({callee})($$renderer, {props});"));
        self.push_line(indent, "$$renderer.push(`<!---->`);");
        Some(())
    }

    fn render_dynamic_component_call(
        &mut self,
        callee_expression: &str,
        attributes: &[Attribute],
        fragment: &Fragment,
        indent: usize,
    ) -> Option<()> {
        let component_id = self.next_temp("$$component");
        let props = self.build_component_props(attributes, fragment, indent)?;

        self.push_line(
            indent,
            &format!("const {component_id} = {callee_expression};"),
        );
        self.push_line(indent, &format!("if ({component_id}) {{"));
        self.push_line(indent + 1, "$$renderer.push(`<!--[-->`);");
        self.push_line(
            indent + 1,
            &format!("({component_id})($$renderer, {props});"),
        );
        self.push_line(indent + 1, "$$renderer.push(`<!--]-->`);");
        self.push_line(indent, "} else {");
        self.push_line(indent + 1, "$$renderer.push(`<!--[!-->`);");
        self.push_line(indent + 1, "$$renderer.push(`<!--]-->`);");
        self.push_line(indent, "}");
        Some(())
    }

    fn build_component_props(
        &mut self,
        attributes: &[Attribute],
        fragment: &Fragment,
        indent: usize,
    ) -> Option<String> {
        let mut parts: Vec<PropsPart> = Vec::new();
        let mut current = Vec::<(String, String)>::new();
        let mut has_children_prop = false;

        for attribute in attributes.iter() {
            match attribute {
                Attribute::Attribute(named) => {
                    if named.name.as_ref() == "children" {
                        has_children_prop = true;
                    }
                    let value = self.attribute_value_expression(&named.value)?;
                    current.push((named.name.as_ref().to_string(), value));
                }
                Attribute::SpreadAttribute(spread) => {
                    flush_props_group(&mut parts, &mut current);
                    parts.push(PropsPart::Spread(spread.expression.render()?));
                }
                Attribute::BindDirective(directive) if directive.name.as_ref() == "this" => {}
                Attribute::BindDirective(directive) => {
                    let value = directive.expression.render()?;
                    current.push((directive.name.as_ref().to_string(), value));
                }
                Attribute::ClassDirective(directive) => {
                    let value = directive.expression.render()?;
                    let key = directive.name.as_ref();
                    current.push((
                        "class".to_string(),
                        format!(
                            "$.to_class('', null, {{ {}: ({value}) }})",
                            js_single_quoted_string(key)
                        ),
                    ));
                }
                Attribute::StyleDirective(directive) => {
                    let value = self.attribute_value_expression(&directive.value)?;
                    let key = directive.name.as_ref();
                    current.push((
                        "style".to_string(),
                        format!(
                            "$.to_style('', {{ {}: ({value}) }})",
                            js_single_quoted_string(key)
                        ),
                    ));
                }
                Attribute::OnDirective(directive) => {
                    let value = directive
                        .expression
                        .render()
                        .unwrap_or_else(|| "() => {}".to_string());
                    current.push((format!("on{}", directive.name), value));
                }
                Attribute::LetDirective(_)
                | Attribute::TransitionDirective(_)
                | Attribute::AnimateDirective(_)
                | Attribute::UseDirective(_)
                | Attribute::AttachTag(_) => {}
            }
        }

        if !fragment.nodes.is_empty() && !has_children_prop {
            let children_name = self.next_temp("$$children");
            self.push_line(
                indent,
                &format!("const {children_name} = ($$renderer) => {{"),
            );
            self.render_fragment(fragment, indent + 1)?;
            self.push_line(indent, "};");

            current.push(("children".to_string(), children_name));
            current.push(("$$slots".to_string(), "{ default: true }".to_string()));
        }

        flush_props_group(&mut parts, &mut current);
        Some(render_props_parts(&parts))
    }

    fn attribute_value_expression(&self, value: &AttributeValueList) -> Option<String> {
        match value {
            AttributeValueList::Boolean(true) => Some("true".to_string()),
            AttributeValueList::Boolean(false) => Some("false".to_string()),
            AttributeValueList::ExpressionTag(tag) => tag.expression.render(),
            AttributeValueList::Values(values) => {
                if values.is_empty() {
                    return Some("''".to_string());
                }
                if values.len() == 1
                    && let AttributeValue::Text(text) = &values[0]
                {
                    return Some(js_single_quoted_string(text.data.as_ref()));
                }

                let mut out = String::from("`");
                for item in values.iter() {
                    match item {
                        AttributeValue::Text(text) => {
                            out.push_str(&escape_js_template_literal(text.raw.as_ref()))
                        }
                        AttributeValue::ExpressionTag(tag) => {
                            out.push_str("${");
                            out.push_str(&tag.expression.render()?);
                            out.push('}');
                        }
                    }
                }
                out.push('`');
                Some(out)
            }
        }
    }

    fn render_slot_element(
        &mut self,
        slot: &crate::ast::modern::SlotElement,
        indent: usize,
    ) -> Option<()> {
        let mut name = "'default'".to_string();
        let mut parts: Vec<PropsPart> = Vec::new();
        let mut current = Vec::<(String, String)>::new();

        for attribute in slot.attributes.iter() {
            match attribute {
                Attribute::Attribute(named) => {
                    let value = self.attribute_value_expression(&named.value)?;
                    if named.name.as_ref() == "name" {
                        name = value;
                    } else if named.name.as_ref() != "slot" {
                        current.push((named.name.as_ref().to_string(), value));
                    }
                }
                Attribute::SpreadAttribute(spread) => {
                    flush_props_group(&mut parts, &mut current);
                    parts.push(PropsPart::Spread(spread.expression.render()?));
                }
                Attribute::BindDirective(directive) if directive.name.as_ref() == "this" => {}
                Attribute::BindDirective(directive) => {
                    current.push((
                        directive.name.as_ref().to_string(),
                        directive.expression.render()?,
                    ));
                }
                Attribute::ClassDirective(directive) => {
                    let value = directive.expression.render()?;
                    current.push((
                        "class".to_string(),
                        format!(
                            "$.to_class('', null, {{ {}: ({value}) }})",
                            js_single_quoted_string(directive.name.as_ref())
                        ),
                    ));
                }
                Attribute::StyleDirective(directive) => {
                    let value = self.attribute_value_expression(&directive.value)?;
                    current.push((
                        "style".to_string(),
                        format!(
                            "$.to_style('', {{ {}: ({value}) }})",
                            js_single_quoted_string(directive.name.as_ref())
                        ),
                    ));
                }
                Attribute::OnDirective(_)
                | Attribute::LetDirective(_)
                | Attribute::TransitionDirective(_)
                | Attribute::AnimateDirective(_)
                | Attribute::UseDirective(_)
                | Attribute::AttachTag(_) => {}
            }
        }
        flush_props_group(&mut parts, &mut current);
        let props = render_props_parts(&parts);

        let fallback = if slot.fragment.nodes.is_empty() {
            "null".to_string()
        } else {
            let fallback_name = self.next_temp("$$slot_fallback");
            self.push_line(
                indent,
                &format!("const {fallback_name} = ($$renderer) => {{"),
            );
            self.render_fragment(&slot.fragment, indent + 1)?;
            self.push_line(indent, "};");
            fallback_name
        };

        self.push_line(indent, "$$renderer.push(`<!--[-->`);");
        self.push_line(
            indent,
            &format!("$.slot($$renderer, $$props, {name}, {props}, {fallback});"),
        );
        self.push_line(indent, "$$renderer.push(`<!--]-->`);");
        Some(())
    }

    fn render_if_block(&mut self, if_block: &IfBlock, indent: usize) -> Option<()> {
        self.push_line(indent, "$$renderer.push(`<!--[-->`);");
        let mut else_if_index = 1usize;
        self.render_if_chain(if_block, indent, Some(ElseIfMarker(0)), &mut else_if_index)?;
        self.push_line(indent, "$$renderer.push(`<!--]-->`);");
        Some(())
    }

    fn render_if_chain(
        &mut self,
        if_block: &IfBlock,
        indent: usize,
        marker: Option<ElseIfMarker>,
        else_if_index: &mut usize,
    ) -> Option<()> {
        let test = if_block.test.render()?;
        self.push_line(indent, &format!("if ({test}) {{"));
        if let Some(marker) = marker {
            self.push_line(
                indent + 1,
                &format!("$$renderer.push(`<!--[{}-->`);", marker.0),
            );
        }
        self.render_fragment(&if_block.consequent, indent + 1)?;

        if let Some(alternate) = if_block.alternate.as_deref() {
            self.push_line(indent, "} else {");
            if let Some(next_if) = alternate_else_if_block(alternate) {
                let marker = ElseIfMarker(*else_if_index);
                *else_if_index += 1;
                self.render_if_chain(next_if, indent + 1, Some(marker), else_if_index)?;
            } else {
                self.push_line(indent + 1, "$$renderer.push(`<!--[-1-->`);");
                self.render_alternate_fragment(alternate, indent + 1)?;
            }
            self.push_line(indent, "}");
        } else {
            self.push_line(indent, "}");
        }
        Some(())
    }

    fn render_alternate_fragment(&mut self, alternate: &Alternate, indent: usize) -> Option<()> {
        match alternate {
            Alternate::Fragment(fragment) => self.render_fragment(fragment, indent),
            Alternate::IfBlock(if_block) => self.render_fragment(&if_block.consequent, indent),
        }
    }

    fn render_each_block(
        &mut self,
        each_block: &crate::ast::modern::EachBlock,
        indent: usize,
    ) -> Option<()> {
        self.each_counter += 1;
        let id = self.each_counter;
        let array_name = format!("$$each_array_{id}");
        let index_name = format!("$$index_{id}");
        let length_name = format!("$$length_{id}");
        let collection_expression = each_block.expression.render()?;

        self.push_line(indent, "$$renderer.push(`<!--[-->`);");
        self.push_line(
            indent,
            &format!("const {array_name} = $.ensure_array_like({collection_expression});"),
        );

        if each_block.fallback.is_some() {
            self.push_line(indent, &format!("if ({array_name}.length !== 0) {{"));
        }

        self.push_line(
            indent + usize::from(each_block.fallback.is_some()),
            &format!(
                "for (let {index_name} = 0, {length_name} = {array_name}.length; {index_name} < {length_name}; {index_name}++) {{"
            ),
        );

        if let Some(context) = each_block.context.as_ref() {
            let context_source = context.render()?;
            self.push_line(
                indent + usize::from(each_block.fallback.is_some()) + 1,
                &format!("let {context_source} = {array_name}[{index_name}];"),
            );
        }

        if let Some(index) = each_block.index.as_deref() {
            self.push_line(
                indent + usize::from(each_block.fallback.is_some()) + 1,
                &format!("let {index} = {index_name};"),
            );
        }

        self.render_fragment(
            &each_block.body,
            indent + usize::from(each_block.fallback.is_some()) + 1,
        )?;
        self.push_line(indent + usize::from(each_block.fallback.is_some()), "}");

        if let Some(fallback) = each_block.fallback.as_ref() {
            self.push_line(indent, "} else {");
            self.push_line(indent + 1, "$$renderer.push(`<!--[!-->`);");
            self.render_fragment(fallback, indent + 1)?;
            self.push_line(indent, "}");
        }

        self.push_line(indent, "$$renderer.push(`<!--]-->`);");
        Some(())
    }

    fn render_key_block(&mut self, key_block: &KeyBlock, indent: usize) -> Option<()> {
        // The server renderer only needs stable hydration markers around keyed content.
        self.push_line(indent, "$$renderer.push(`<!---->`);");
        self.render_fragment(&key_block.fragment, indent)?;
        self.push_line(indent, "$$renderer.push(`<!---->`);");
        Some(())
    }

    fn render_await_block(&mut self, await_block: &AwaitBlock, indent: usize) -> Option<()> {
        let expression = await_block.expression.render()?;
        let pending_name = self.next_temp("$$await_pending");
        let then_name = self.next_temp("$$await_then");

        self.push_line(indent, &format!("const {pending_name} = () => {{"));
        if let Some(pending) = await_block.pending.as_ref() {
            self.render_fragment(pending, indent + 1)?;
        }
        self.push_line(indent, "};");

        if let Some(value) = await_block.value.as_ref() {
            let binding = value.render()?;
            self.push_line(indent, &format!("const {then_name} = ({binding}) => {{"));
        } else {
            self.push_line(indent, &format!("const {then_name} = () => {{"));
        }
        if let Some(then_fragment) = await_block.then.as_ref() {
            self.render_fragment(then_fragment, indent + 1)?;
        }
        self.push_line(indent, "};");

        self.push_line(indent, "$$renderer.push(`<!--[-->`);");
        self.push_line(
            indent,
            &format!("$.await($$renderer, {expression}, {pending_name}, {then_name});"),
        );
        self.push_line(indent, "$$renderer.push(`<!--]-->`);");
        Some(())
    }

    fn render_snippet_block(
        &mut self,
        snippet: &crate::ast::modern::SnippetBlock,
        indent: usize,
    ) -> Option<()> {
        let name = snippet.expression.render()?;
        let mut params = Vec::with_capacity(snippet.parameters.len() + 1);
        params.push("$$renderer".to_string());
        for parameter in snippet.parameters.iter() {
            params.push(parameter.render()?);
        }

        self.push_line(
            indent,
            &format!("function {name}({}) {{", params.join(", ")),
        );
        self.render_fragment(&snippet.body, indent + 1)?;
        self.push_line(indent, "}");
        Some(())
    }

    fn render_svelte_element(&mut self, element: &SvelteElement, indent: usize) -> Option<()> {
        let tag = self.svelte_element_tag_expression(element)?;
        let has_renderable_attributes = element
            .attributes
            .iter()
            .any(|attribute| !is_svelte_element_this_attribute(attribute));
        let attributes_thunk = if !has_renderable_attributes {
            "null".to_string()
        } else {
            let attrs_name = self.next_temp("$$svelte_element_attrs");
            self.push_line(indent, &format!("const {attrs_name} = ($$renderer) => {{"));
            for attribute in element.attributes.iter() {
                if is_svelte_element_this_attribute(attribute) {
                    continue;
                }
                self.render_html_attribute(attribute, indent + 1)?;
            }
            self.push_line(indent, "};");
            attrs_name
        };
        let children_thunk = if element.fragment.nodes.is_empty() {
            "null".to_string()
        } else {
            let children_name = self.next_temp("$$svelte_element_children");
            self.push_line(
                indent,
                &format!("const {children_name} = ($$renderer) => {{"),
            );
            self.render_fragment(&element.fragment, indent + 1)?;
            self.push_line(indent, "};");
            children_name
        };

        self.push_line(
            indent,
            &format!("$.element($$renderer, {tag}, {attributes_thunk}, {children_thunk});"),
        );
        Some(())
    }

    fn svelte_element_tag_expression(&self, element: &SvelteElement) -> Option<String> {
        if let Some(expression) = element.expression.as_ref() {
            return expression.render();
        }

        element.attributes.iter().find_map(|attribute| {
            let Attribute::Attribute(named) = attribute else {
                return None;
            };
            if named.name.as_ref() != "this" {
                return None;
            }
            self.attribute_value_expression(&named.value)
        })
    }

    fn render_svelte_fragment(&mut self, fragment: &SvelteFragment, indent: usize) -> Option<()> {
        self.render_fragment(&fragment.fragment, indent)
    }

    fn render_svelte_head(&mut self, head: &SvelteHead, indent: usize) -> Option<()> {
        let head_id = js_single_quoted_string(self.component_name);
        self.push_line(
            indent,
            &format!("$.head({head_id}, $$renderer, ($$renderer) => {{"),
        );
        self.render_fragment(&head.fragment, indent + 1)?;
        self.push_line(indent, "});");
        Some(())
    }

    fn render_title_element(&mut self, title: &TitleElement, indent: usize) -> Option<()> {
        self.push_line(indent, "$$renderer.title(($$renderer) => {");
        self.push_line(indent + 1, "$$renderer.push(`<title>`);");
        for child in title.fragment.nodes.iter() {
            match child {
                Node::Text(text) => {
                    self.push_line(
                        indent + 1,
                        &format!(
                            "$$renderer.push(`{}`);",
                            escape_js_template_literal(text.raw.as_ref())
                        ),
                    );
                }
                Node::ExpressionTag(tag) => {
                    let expression = tag.expression.render()?;
                    self.push_line(
                        indent + 1,
                        &format!("$$renderer.push($.escape({expression}));"),
                    );
                }
                _ => self.render_node(child, indent + 1)?,
            }
        }
        self.push_line(indent + 1, "$$renderer.push(`</title>`);");
        self.push_line(indent, "});");
        Some(())
    }

    fn render_svelte_boundary(&mut self, boundary: &SvelteBoundary, indent: usize) -> Option<()> {
        self.push_line(indent, "$$renderer.boundary({}, ($$renderer) => {");
        self.render_fragment(&boundary.fragment, indent + 1)?;
        self.push_line(indent, "});");
        Some(())
    }
}

#[derive(Clone)]
enum PropsPart {
    Object(Vec<(String, String)>),
    Spread(String),
}

fn flush_props_group(parts: &mut Vec<PropsPart>, current: &mut Vec<(String, String)>) {
    if current.is_empty() {
        return;
    }
    let drained = std::mem::take(current);
    parts.push(PropsPart::Object(drained));
}

fn render_props_parts(parts: &[PropsPart]) -> String {
    if parts.is_empty() {
        return "{}".to_string();
    }
    if parts.len() == 1 {
        return match &parts[0] {
            PropsPart::Object(entries) => render_object_entries(entries),
            PropsPart::Spread(expression) => expression.clone(),
        };
    }

    let args = parts
        .iter()
        .map(|part| match part {
            PropsPart::Object(entries) => render_object_entries(entries),
            PropsPart::Spread(expression) => expression.clone(),
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("$.spread_props([{args}])")
}

fn render_object_entries(entries: &[(String, String)]) -> String {
    if entries.is_empty() {
        return "{}".to_string();
    }

    let body = entries
        .iter()
        .map(|(key, value)| format!("{}: {value}", js_property_key(key)))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{{ {body} }}")
}

fn is_svelte_element_this_attribute(attribute: &Attribute) -> bool {
    matches!(attribute, Attribute::Attribute(named) if named.name.as_ref() == "this")
}

fn js_property_key(key: &str) -> String {
    if is_js_identifier(key) {
        key.to_string()
    } else {
        js_single_quoted_string(key)
    }
}

struct ScriptSections {
    module_body: String,
    instance_body: String,
}

fn collect_script_sections(root: &Root) -> Option<ScriptSections> {
    let mut module_imports = Vec::<String>::new();
    let mut module_statements = Vec::<String>::new();
    let mut instance_statements = Vec::<String>::new();

    if let Some(module) = root.module.as_ref() {
        let parsed = parse_script(&module.content, ScriptParseMode::Module)?;
        module_imports.extend(parsed.imports);
        module_statements.extend(parsed.statements);
    }

    if let Some(instance) = root.instance.as_ref() {
        let parsed = parse_script(&instance.content, ScriptParseMode::Instance)?;
        module_imports.extend(parsed.imports);
        instance_statements.extend(parsed.statements);
    }

    let module_imports = dedupe_non_empty_lines(module_imports);
    let module_statements = dedupe_non_empty_lines(module_statements);
    let instance_statements = dedupe_non_empty_lines(instance_statements);

    let module_body = join_with_gap(&module_imports, &module_statements);
    let instance_body = instance_statements.join("\n");

    Some(ScriptSections {
        module_body,
        instance_body,
    })
}

#[derive(Clone, Copy)]
enum ScriptParseMode {
    Module,
    Instance,
}

struct ParsedScript {
    imports: Vec<String>,
    statements: Vec<String>,
}

fn parse_script(program: &ParsedJsProgram, mode: ScriptParseMode) -> Option<ParsedScript> {
    let mut imports = Vec::new();
    let mut statements = Vec::new();

    for statement in &program.program().body {
        match statement {
            OxcStatement::ImportDeclaration(_) => {
                imports.push(
                    program_statement_source(program, statement)?
                        .trim()
                        .to_string(),
                );
            }
            OxcStatement::ExportNamedDeclaration(statement) => match mode {
                ScriptParseMode::Module => {
                    statements.push(
                        program_spanned_source(program, &**statement)?
                            .trim()
                            .to_string(),
                    );
                }
                ScriptParseMode::Instance => {
                    if let Some(inner) = statement.declaration.as_ref()
                        && let Some(rendered) =
                            export_named_instance_declaration_source(program, inner)
                    {
                        statements.push(rendered.trim().to_string());
                    }
                }
            },
            OxcStatement::ExportDefaultDeclaration(_)
                if matches!(mode, ScriptParseMode::Instance) =>
            {
                // Instance `export default` has no valid direct equivalent here.
            }
            OxcStatement::ExportAllDeclaration(_) if matches!(mode, ScriptParseMode::Instance) => {
                // Ignore instance re-exports in this generic path.
            }
            _ => {
                let raw = program_statement_source(program, statement)?
                    .trim()
                    .to_string();
                let rewritten = rewrite_effect_calls(&raw);
                statements.push(rewritten);
            }
        }
    }

    Some(ParsedScript {
        imports: dedupe_non_empty_lines(imports),
        statements: dedupe_non_empty_lines(statements),
    })
}

/// Rewrite `$effect(` → `$.user_effect(` and `$effect.pre(` → `$.user_pre_effect(`.
fn rewrite_effect_calls(source: &str) -> String {
    // Must replace `$effect.pre(` before `$effect(` to avoid partial matches.
    source
        .replace("$effect.pre(", "$.user_pre_effect(")
        .replace("$effect(", "$.user_effect(")
}

fn dedupe_non_empty_lines(lines: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let key = trimmed.to_string();
        if seen.insert(key.clone()) {
            out.push(key);
        }
    }
    out
}

fn join_with_gap(head: &[String], tail: &[String]) -> String {
    if head.is_empty() && tail.is_empty() {
        return String::new();
    }
    if head.is_empty() {
        return tail.join("\n");
    }
    if tail.is_empty() {
        return head.join("\n");
    }
    format!("{}\n\n{}", head.join("\n"), tail.join("\n"))
}

#[derive(Clone, Copy)]
struct ElseIfMarker(usize);

fn alternate_else_if_block(alternate: &Alternate) -> Option<&IfBlock> {
    match alternate {
        Alternate::IfBlock(if_block) => Some(if_block),
        Alternate::Fragment(fragment) => {
            let significant = fragment
                .nodes
                .iter()
                .filter(|node| match node {
                    Node::Text(text) => !text.data.chars().all(char::is_whitespace),
                    Node::Comment(_) => false,
                    _ => true,
                })
                .collect::<Vec<_>>();
            if significant.len() != 1 {
                return None;
            }
            let Node::IfBlock(if_block) = significant[0] else {
                return None;
            };
            if if_block.elseif {
                Some(if_block)
            } else {
                None
            }
        }
    }
}

fn render_render_tag_call(expression: &Expression) -> Option<String> {
    let (callee_source, rendered_args, is_optional_call) = render_tag_call_parts(expression)?;
    if callee_source.is_empty() {
        return None;
    }

    let mut args = vec![String::from("$$renderer")];
    args.extend(rendered_args);
    let joined = args.join(", ");

    if is_optional_call {
        Some(format!("({callee_source})?.({joined})"))
    } else {
        Some(format!("({callee_source})({joined})"))
    }
}

fn render_tag_call_parts(expression: &Expression) -> Option<(String, Vec<String>, bool)> {
    let parsed = expression.parsed()?;
    let source = parsed.source();
    let node = parsed.expression();

    match node {
        OxcExpression::CallExpression(call) => Some((
            snippet(source, call.callee.span())?.to_string(),
            call.arguments
                .iter()
                .map(|argument| snippet(source, argument.span()).map(ToString::to_string))
                .collect::<Option<Vec<_>>>()?,
            false,
        )),
        OxcExpression::ChainExpression(chain) => {
            let ChainElement::CallExpression(call) = &chain.expression else {
                return None;
            };
            Some((
                snippet(source, call.callee.span())?.to_string(),
                call.arguments
                    .iter()
                    .map(|argument| snippet(source, argument.span()).map(ToString::to_string))
                    .collect::<Option<Vec<_>>>()?,
                true,
            ))
        }
        _ => None,
    }
}

pub(crate) fn program_statement_source<'a>(
    program: &'a ParsedJsProgram,
    statement: &OxcStatement<'a>,
) -> Option<&'a str> {
    snippet(program.source(), statement.span())
}

pub(crate) fn program_spanned_source<'a>(
    program: &'a ParsedJsProgram,
    node: &impl GetSpan,
) -> Option<&'a str> {
    snippet(program.source(), node.span())
}

fn export_named_instance_declaration_source<'a>(
    program: &'a ParsedJsProgram,
    declaration: &Declaration<'a>,
) -> Option<&'a str> {
    match declaration {
        Declaration::VariableDeclaration(_)
        | Declaration::FunctionDeclaration(_)
        | Declaration::ClassDeclaration(_) => snippet(program.source(), declaration.span()),
        _ => None,
    }
}

fn snippet(source: &str, span: oxc_span::Span) -> Option<&str> {
    source.get(span.start as usize..span.end as usize)
}

fn render_component_template<B: RenderBackend>(
    component_name: &str,
    module_body: &str,
    instance_body: &str,
    body: &str,
) -> String {
    let module_body = if module_body.is_empty() {
        String::new()
    } else {
        format!("{module_body}\n")
    };
    let instance_body = if instance_body.is_empty() {
        String::new()
    } else {
        indent_block(instance_body, 1)
    };
    B::TEMPLATE
        .replace("__COMPONENT__", component_name)
        .replace("__MODULE_BODY__", &module_body)
        .replace("__INSTANCE_BODY__", &instance_body)
        .replace("__BODY__", body)
}

fn indent_block(value: &str, level: usize) -> String {
    let indent = "\t".repeat(level);
    value
        .lines()
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("{indent}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn escape_js_template_literal(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('`', "\\`")
        .replace("${", "\\${")
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

fn js_single_quoted_string(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t");
    format!("'{escaped}'")
}

fn is_js_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_' || first == '$') {
        return false;
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '$')
}
