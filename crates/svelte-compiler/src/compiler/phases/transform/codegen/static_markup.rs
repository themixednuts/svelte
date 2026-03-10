use crate::api::{FragmentStrategy, GenerateTarget};
use crate::ast::modern::{
    Attribute, AttributeValue, AttributeValueList, Fragment, Node, RegularElement, Root,
};
use camino::Utf8Path;

pub(crate) fn compile_static_markup_js(
    source: &str,
    target: GenerateTarget,
    fragments: FragmentStrategy,
    root: &Root,
    hmr: bool,
    filename: Option<&Utf8Path>,
) -> Option<String> {
    let component_name = component_name_from_filename(filename);
    let mount_binding = static_mount_binding_name(root);

    if let Some(output) =
        compile_imports_only_component_js(source, target, root, hmr, &component_name)
    {
        return Some(output);
    }

    let markup = if matches!(fragments, FragmentStrategy::Tree) {
        serialize_static_markup_fragment(&root.fragment)?
    } else {
        extract_static_markup_template_from_root(source, root)?
    };
    let escaped_markup = escape_js_template_literal(&markup);

    match target {
        GenerateTarget::None => Some(String::new()),
        GenerateTarget::Client => {
            if matches!(fragments, FragmentStrategy::Tree) {
                return compile_static_tree_client_js(root, hmr, &component_name);
            }

            if !hmr {
                return Some(format!(
                    "import 'svelte/internal/disclose-version';\nimport 'svelte/internal/flags/legacy';\nimport * as $ from 'svelte/internal/client';\n\nvar root = $.from_html(`{escaped_markup}`);\n\nexport default function {component_name}($$anchor) {{\n\tvar {mount_binding} = root();\n\n\t$.append($$anchor, {mount_binding});\n}}\n"
                ));
            }

            let mut output = format!(
                "import 'svelte/internal/disclose-version';\nimport 'svelte/internal/flags/legacy';\nimport * as $ from 'svelte/internal/client';\n\nvar root = $.from_html(`{escaped_markup}`);\n\nfunction {component_name}($$anchor) {{\n\tvar {mount_binding} = root();\n\n\t$.append($$anchor, {mount_binding});\n}}\n"
            );
            output.push_str(&format!(
                "\nif (import.meta.hot) {{\n\t{component_name} = $.hmr({component_name});\n\n\timport.meta.hot.accept((module) => {{\n\t\t{component_name}[$.HMR].update(module.default);\n\t}});\n}}\n"
            ));
            output.push_str(&format!("\nexport default {component_name};\n"));
            Some(output)
        }
        GenerateTarget::Server => {
            if let Some(option_text) = extract_static_option_inner_text(root) {
                let escaped_option_text = escape_js_template_literal(&option_text);
                return Some(format!(
                    "import * as $ from 'svelte/internal/server';\n\nexport default function {component_name}($$renderer) {{\n\t$$renderer.option({{}}, ($$renderer) => {{\n\t\t$$renderer.push(`{escaped_option_text}`);\n\t}});\n}}\n"
                ));
            }

            Some(format!(
                "import * as $ from 'svelte/internal/server';\n\nexport default function {component_name}($$renderer) {{\n\t$$renderer.push(`{escaped_markup}`);\n}}\n"
            ))
        }
    }
}

fn compile_imports_only_component_js(
    source: &str,
    target: GenerateTarget,
    root: &Root,
    hmr: bool,
    component_name: &str,
) -> Option<String> {
    if root.module.is_some() || root.options.is_some() || root.css.is_some() {
        return None;
    }
    if root.instance.is_none() || !modern_fragment_is_whitespace_only(&root.fragment) {
        return None;
    }

    let imports = extract_instance_script_imports_only(source, root)?;

    match target {
        GenerateTarget::None => Some(String::new()),
        GenerateTarget::Server => {
            let mut output = String::from("import * as $ from 'svelte/internal/server';\n");
            for import in imports.iter() {
                output.push_str(import);
                output.push('\n');
            }
            output.push_str(&format!(
                "\nexport default function {component_name}($$renderer) {{}}\n"
            ));
            Some(output)
        }
        GenerateTarget::Client => {
            let mut output = String::from(
                "import 'svelte/internal/disclose-version';\nimport 'svelte/internal/flags/legacy';\nimport * as $ from 'svelte/internal/client';\n",
            );
            for import in imports.iter() {
                output.push_str(import);
                output.push('\n');
            }

            if hmr {
                output.push_str(&format!(
                    "\nfunction {component_name}($$anchor) {{}}\n\nif (import.meta.hot) {{\n\t{component_name} = $.hmr({component_name});\n\n\timport.meta.hot.accept((module) => {{\n\t\t{component_name}[$.HMR].update(module.default);\n\t}});\n}}\n\nexport default {component_name};\n"
                ));
            } else {
                output.push_str(&format!(
                    "\nexport default function {component_name}($$anchor) {{}}\n"
                ));
            }

            Some(output)
        }
    }
}

fn modern_fragment_is_whitespace_only(fragment: &Fragment) -> bool {
    fragment.nodes.iter().all(|node| match node {
        Node::Text(text) => text.data.chars().all(char::is_whitespace),
        Node::Comment(_) => true,
        _ => false,
    })
}

fn extract_instance_script_imports_only(source: &str, root: &Root) -> Option<Vec<String>> {
    let ranges = crate::compiler::phases::parse::non_module_script_content_ranges(root);
    if ranges.len() != 1 {
        return None;
    }

    let (start, end) = ranges[0];
    let script = source.get(start..end)?;
    let import_ranges = crate::compiler::phases::parse::parse_js_import_ranges_for_compile(script)?;
    let mut imports = Vec::with_capacity(import_ranges.len());
    for (import_start, import_end) in import_ranges {
        let import_source = script.get(import_start..import_end).unwrap_or_default();
        if import_source.is_empty() {
            return None;
        }
        imports.push(import_source.to_string());
    }

    Some(imports)
}

fn extract_static_markup_template_from_root(source: &str, root: &Root) -> Option<String> {
    if root.module.is_some() || root.instance.is_some() || root.options.is_some() {
        return None;
    }

    let nodes = root.fragment.nodes.as_ref();
    if nodes.is_empty() || !nodes.iter().all(modern_node_is_static_markup) {
        return None;
    }

    let mut markup = String::new();
    for node in nodes {
        let start = crate::api::modern_node_start(node);
        let end = crate::api::modern_node_end(node);
        markup.push_str(source.get(start..end)?);
    }
    Some(markup)
}

fn modern_node_is_static_markup(node: &Node) -> bool {
    match node {
        Node::Text(_) | Node::Comment(_) => true,
        Node::RegularElement(element) => modern_element_is_static_markup(element),
        _ => false,
    }
}

fn modern_element_is_static_markup(element: &RegularElement) -> bool {
    if element.name.contains(':') {
        return false;
    }

    if !element
        .attributes
        .iter()
        .all(modern_attribute_is_static_markup)
    {
        return false;
    }

    element
        .fragment
        .nodes
        .iter()
        .all(modern_node_is_static_markup)
}

fn modern_attribute_is_static_markup(attribute: &Attribute) -> bool {
    match attribute {
        Attribute::Attribute(attribute) => match &attribute.value {
            AttributeValueList::Boolean(_) => true,
            AttributeValueList::Values(values) => values
                .iter()
                .all(|value| matches!(value, AttributeValue::Text(_))),
            AttributeValueList::ExpressionTag(_) => false,
        },
        Attribute::SpreadAttribute(_)
        | Attribute::BindDirective(_)
        | Attribute::OnDirective(_)
        | Attribute::ClassDirective(_)
        | Attribute::LetDirective(_)
        | Attribute::StyleDirective(_)
        | Attribute::TransitionDirective(_)
        | Attribute::AnimateDirective(_)
        | Attribute::UseDirective(_)
        | Attribute::AttachTag(_) => false,
    }
}

fn compile_static_tree_client_js(root: &Root, hmr: bool, component_name: &str) -> Option<String> {
    let tree_literal = serialize_tree_fragment(&root.fragment)?;
    let fragment_count = count_normalized_fragment_items(&root.fragment);
    let tree_next_count = fragment_count.saturating_sub(1);

    if !hmr {
        let mut output = String::new();
        output.push_str(
            "import 'svelte/internal/disclose-version';\nimport 'svelte/internal/flags/legacy';\nimport * as $ from 'svelte/internal/client';\n\n",
        );
        output.push_str("var root = $.from_tree(\n");
        output.push_str(&tree_literal);
        output.push_str(",\n\t1\n);\n\n");
        output.push_str(&format!(
            "export default function {component_name}($$anchor) {{\n\tvar fragment = root();\n"
        ));
        if tree_next_count > 0 {
            output.push_str(&format!("\n\t$.next({tree_next_count});\n"));
        }
        output.push_str("\t$.append($$anchor, fragment);\n}\n");
        return Some(output);
    }

    let mut output = String::new();
    output.push_str(
        "import 'svelte/internal/disclose-version';\nimport 'svelte/internal/flags/legacy';\nimport * as $ from 'svelte/internal/client';\n\n",
    );
    output.push_str("var root = $.from_tree(\n");
    output.push_str(&tree_literal);
    output.push_str(",\n\t1\n);\n\n");
    output.push_str(&format!(
        "function {component_name}($$anchor) {{\n\tvar fragment = root();\n"
    ));
    if tree_next_count > 0 {
        output.push_str(&format!("\n\t$.next({tree_next_count});\n"));
    }
    output.push_str("\t$.append($$anchor, fragment);\n}\n");
    output.push_str(&format!(
        "\nif (import.meta.hot) {{\n\t{component_name} = $.hmr({component_name});\n\n\timport.meta.hot.accept((module) => {{\n\t\t{component_name}[$.HMR].update(module.default);\n\t}});\n}}\n\nexport default {component_name};\n"
    ));
    Some(output)
}

fn serialize_static_markup_fragment(fragment: &Fragment) -> Option<String> {
    let mut output = String::new();

    for item in normalized_fragment_items(fragment).into_iter() {
        match item {
            NormalizedFragmentItem::Space => output.push(' '),
            NormalizedFragmentItem::Node(node) => {
                serialize_html_node(node, &mut output)?;
            }
        }
    }

    Some(output)
}

fn serialize_html_node(node: &Node, output: &mut String) -> Option<()> {
    match node {
        Node::Text(text) => {
            output.push_str(text.data.as_ref());
            Some(())
        }
        Node::Comment(comment) => {
            output.push_str("<!--");
            output.push_str(comment.data.as_ref());
            output.push_str("-->");
            Some(())
        }
        Node::RegularElement(element) => {
            output.push('<');
            output.push_str(element.name.as_ref());
            for attribute in element.attributes.iter() {
                serialize_html_attribute(attribute, output)?;
            }

            if element.self_closing && !element.has_end_tag {
                output.push_str("/>");
                return Some(());
            }

            output.push('>');
            for item in normalized_fragment_items(&element.fragment).into_iter() {
                match item {
                    NormalizedFragmentItem::Space => output.push(' '),
                    NormalizedFragmentItem::Node(node) => {
                        serialize_html_node(node, output)?;
                    }
                }
            }
            output.push_str("</");
            output.push_str(element.name.as_ref());
            output.push('>');
            Some(())
        }
        _ => None,
    }
}

fn serialize_html_attribute(attribute: &Attribute, output: &mut String) -> Option<()> {
    let Attribute::Attribute(attribute) = attribute else {
        return None;
    };

    output.push(' ');
    output.push_str(attribute.name.as_ref());

    match &attribute.value {
        AttributeValueList::Boolean(true) => {}
        AttributeValueList::Boolean(false) => return None,
        AttributeValueList::Values(values) => {
            output.push_str("=\"");
            for value in values.iter() {
                let AttributeValue::Text(text) = value else {
                    return None;
                };
                output.push_str(text.data.as_ref());
            }
            output.push('"');
        }
        AttributeValueList::ExpressionTag(_) => return None,
    }

    Some(())
}

fn serialize_tree_fragment(fragment: &Fragment) -> Option<String> {
    let values = build_tree_values(fragment)?;
    Some(format_tree_array(&values, 1))
}

fn build_tree_values(fragment: &Fragment) -> Option<Vec<TreeValue>> {
    let mut values = Vec::new();

    for item in normalized_fragment_items(fragment).into_iter() {
        match item {
            NormalizedFragmentItem::Space => {
                values.push(TreeValue::String(String::from(" ")));
            }
            NormalizedFragmentItem::Node(node) => values.push(build_tree_value(node)?),
        }
    }

    Some(values)
}

fn build_tree_value(node: &Node) -> Option<TreeValue> {
    match node {
        Node::Text(text) => Some(TreeValue::String(text.data.as_ref().to_string())),
        Node::Comment(comment) => {
            let mut html_comment = String::from("<!--");
            html_comment.push_str(comment.data.as_ref());
            html_comment.push_str("-->");
            Some(TreeValue::String(html_comment))
        }
        Node::RegularElement(element) => {
            let attributes = serialize_tree_attributes(element.attributes.as_ref())?;
            let children = build_tree_values(&element.fragment)?;
            Some(TreeValue::Element(TreeElementValue {
                tag: element.name.as_ref().to_string(),
                attributes,
                children,
            }))
        }
        _ => None,
    }
}

fn serialize_tree_attributes(attributes: &[Attribute]) -> Option<String> {
    if attributes.is_empty() {
        return Some(String::from("null"));
    }

    let mut entries = Vec::new();
    for attribute in attributes.iter() {
        let Attribute::Attribute(attribute) = attribute else {
            return None;
        };

        let key = if is_js_identifier(attribute.name.as_ref()) {
            attribute.name.as_ref().to_string()
        } else {
            js_single_quoted_string(attribute.name.as_ref())
        };

        let value = match &attribute.value {
            AttributeValueList::Boolean(true) => String::from("true"),
            AttributeValueList::Boolean(false) => return None,
            AttributeValueList::Values(values) => {
                let mut value_text = String::new();
                for value in values.iter() {
                    let AttributeValue::Text(text) = value else {
                        return None;
                    };
                    value_text.push_str(text.data.as_ref());
                }
                js_single_quoted_string(&value_text)
            }
            AttributeValueList::ExpressionTag(_) => return None,
        };

        entries.push(format!("{key}: {value}"));
    }

    Some(format!("{{ {} }}", entries.join(", ")))
}

#[derive(Clone)]
enum TreeValue {
    String(String),
    Element(TreeElementValue),
}

#[derive(Clone)]
struct TreeElementValue {
    tag: String,
    attributes: String,
    children: Vec<TreeValue>,
}

fn format_tree_array(values: &[TreeValue], indent: usize) -> String {
    let mut output = String::new();
    output.push_str(&tabs(indent));
    output.push('[');
    if values.is_empty() {
        output.push(']');
        return output;
    }
    output.push('\n');

    for (index, value) in values.iter().enumerate() {
        output.push_str(&format_tree_value(value, indent + 1));
        if index + 1 < values.len() {
            output.push(',');
        }
        output.push('\n');
    }

    output.push_str(&tabs(indent));
    output.push(']');
    output
}

fn format_tree_value(value: &TreeValue, indent: usize) -> String {
    match value {
        TreeValue::String(text) => format!("{}{}", tabs(indent), js_single_quoted_string(text)),
        TreeValue::Element(element) => format_tree_element(element, indent),
    }
}

fn format_tree_element(element: &TreeElementValue, indent: usize) -> String {
    if tree_element_is_inline(element) {
        return format!("{}{}", tabs(indent), format_tree_element_inline(element));
    }

    let mut output = String::new();
    output.push_str(&tabs(indent));
    output.push('[');
    output.push('\n');
    output.push_str(&format!(
        "{}{},\n",
        tabs(indent + 1),
        js_single_quoted_string(&element.tag)
    ));
    output.push_str(&format!("{}{},\n", tabs(indent + 1), element.attributes));

    for (index, child) in element.children.iter().enumerate() {
        output.push_str(&format_tree_value(child, indent + 1));
        if index + 1 < element.children.len() {
            output.push(',');
        }
        output.push('\n');
    }

    output.push_str(&tabs(indent));
    output.push(']');
    output
}

fn format_tree_element_inline(element: &TreeElementValue) -> String {
    let mut parts = vec![
        js_single_quoted_string(&element.tag),
        element.attributes.clone(),
    ];
    for child in element.children.iter() {
        parts.push(format_tree_value_inline(child));
    }
    format!("[{}]", parts.join(", "))
}

fn format_tree_value_inline(value: &TreeValue) -> String {
    match value {
        TreeValue::String(text) => js_single_quoted_string(text),
        TreeValue::Element(element) => format_tree_element_inline(element),
    }
}

fn tree_element_is_inline(element: &TreeElementValue) -> bool {
    element.children.len() <= 1 && element.children.iter().all(tree_value_is_inline)
}

fn tree_value_is_inline(value: &TreeValue) -> bool {
    match value {
        TreeValue::String(_) => true,
        TreeValue::Element(element) => tree_element_is_inline(element),
    }
}

fn tabs(count: usize) -> String {
    "\t".repeat(count)
}

fn count_normalized_fragment_items(fragment: &Fragment) -> usize {
    normalized_fragment_items(fragment).len()
}

#[derive(Clone, Copy)]
enum NormalizedFragmentItem<'a> {
    Space,
    Node(&'a Node),
}

fn normalized_fragment_items(fragment: &Fragment) -> Vec<NormalizedFragmentItem<'_>> {
    let mut items = Vec::new();
    let nodes = fragment.nodes.as_ref();

    for (index, node) in nodes.iter().enumerate() {
        if let Node::Text(text) = node
            && text.data.chars().all(char::is_whitespace)
        {
            if has_non_whitespace_sibling(nodes, index, false)
                && has_non_whitespace_sibling(nodes, index, true)
            {
                items.push(NormalizedFragmentItem::Space);
            }
            continue;
        }
        items.push(NormalizedFragmentItem::Node(node));
    }

    items
}

fn has_non_whitespace_sibling(nodes: &[Node], index: usize, forward: bool) -> bool {
    if forward {
        for sibling in nodes.iter().skip(index + 1) {
            match sibling {
                Node::Text(text) if text.data.chars().all(char::is_whitespace) => continue,
                _ => return true,
            }
        }
        return false;
    }

    for sibling in nodes[..index].iter().rev() {
        match sibling {
            Node::Text(text) if text.data.chars().all(char::is_whitespace) => continue,
            _ => return true,
        }
    }
    false
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

fn static_mount_binding_name(root: &Root) -> String {
    let mut candidate_tag_name: Option<&str> = None;

    for node in root.fragment.nodes.iter() {
        match node {
            Node::RegularElement(element) => {
                if candidate_tag_name.is_some() {
                    return String::from("fragment");
                }
                candidate_tag_name = Some(element.name.as_ref());
            }
            Node::Text(text) => {
                if text.data.chars().all(char::is_whitespace) {
                    continue;
                }
                return String::from("text");
            }
            Node::Comment(_) => {}
            _ => return String::from("fragment"),
        }
    }

    match candidate_tag_name {
        Some(tag_name) => sanitize_identifier(tag_name.to_string()),
        None => String::from("fragment"),
    }
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

fn extract_static_option_inner_text(root: &Root) -> Option<String> {
    let mut option_element: Option<&RegularElement> = None;

    for node in root.fragment.nodes.iter() {
        match node {
            Node::Text(text) => {
                if !text.data.chars().all(char::is_whitespace) {
                    return None;
                }
            }
            Node::Comment(_) => {}
            Node::RegularElement(element) => {
                if option_element.is_some() {
                    return None;
                }
                option_element = Some(element);
            }
            _ => return None,
        }
    }

    let option_element = option_element?;
    if option_element.name.as_ref() != "option" || !option_element.attributes.is_empty() {
        return None;
    }

    let mut text = String::new();
    for child in option_element.fragment.nodes.iter() {
        match child {
            Node::Text(inner_text) => text.push_str(inner_text.data.as_ref()),
            Node::Comment(_) => {}
            _ => return None,
        }
    }

    Some(text)
}
