use crate::api::validation::{
    extend_name_set_with_expression_pattern_bindings, extend_name_set_with_oxc_pattern_bindings,
};
use crate::api::{CompileOptions, Warning};
use crate::ast::modern::{
    Alternate, Attribute, AttributeValue, AttributeValueList, DirectiveAttribute, EachBlock,
    Expression, Fragment, Node, RegularElement, Root, SnippetBlock,
};
use crate::names::{Name, NameSet, OrderedNames};
use crate::source::SourceText;
use aria_query::{
    AriaAbstractRole as QueryAriaAbstractRole, AriaProperty as QueryAriaProperty,
    AriaRoleDefinition as QueryRoleDefinition, AriaRoleDefinitionKey as QueryRoleKey,
    AriaRoleDefinitionSuperClass as QueryRoleSuperClass,
    AriaRoleRelationConcept as QueryRoleRelationConcept, ROLE_ELEMENTS as QUERY_ROLE_ELEMENTS,
    ROLES as QUERY_ROLES,
};
use biome_aria::properties::AriaPropertyDefinition;
use biome_aria::{AriaProperties, AriaRoles};
use biome_aria_metadata::{AriaAbstractRolesEnum, AriaPropertyTypeEnum};
use oxc_ast::ast::{
    AssignmentTarget, BindingPattern, Declaration, Expression as OxcExpression,
    IdentifierReference, Statement, VariableDeclarationKind,
};
use oxc_ast_visit::{Visit, walk};
use oxc_span::GetSpan;
use rustc_hash::{FxHashMap, FxHashSet};
use std::str::FromStr;
use std::sync::{Arc, LazyLock};
use svelte_syntax::ParsedJsProgram;

const A11Y_INVISIBLE_ELEMENTS: &[&str] = &["meta", "html", "script", "style"];
const A11Y_PRESENTATION_ROLES: &[&str] = &["presentation", "none"];
const SCRIPT_ALLOWED_ATTRIBUTES: &[&str] = &["context", "generics", "lang", "module"];
const BIDI_CONTROL_RANGES: &[(u32, u32)] = &[(0x202A, 0x202E), (0x2066, 0x2069)];

#[derive(Clone)]
struct RestBindingWarning {
    name: Arc<str>,
    start: usize,
    end: usize,
}

#[derive(Clone, Copy)]
struct ScriptWalkContext {
    function_depth: usize,
    is_module: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstanceBindingKind {
    Normal,
    State,
    RawState,
    Derived,
    Prop,
    RestProp,
}

#[derive(Debug, Clone)]
struct InstanceBindingInfo {
    kind: InstanceBindingKind,
    start: usize,
    end: usize,
    state_argument_proxyable: bool,
    ignore_codes: Box<[Arc<str>]>,
}

#[derive(Debug, Clone)]
struct ExportedMutableBinding {
    name: Arc<str>,
    start: usize,
    end: usize,
    /// OXC byte offset of the export statement this binding came from.
    statement_start: usize,
    ignore_codes: Box<[Arc<str>]>,
}

#[derive(Debug, Clone)]
struct PatternBinding {
    name: Arc<str>,
    start: usize,
    end: usize,
    is_rest: bool,
}

#[derive(Clone, Default)]
struct NameScope {
    names: NameSet,
}

impl NameScope {
    fn contains(&self, name: &str) -> bool {
        self.names.contains(name)
    }

    fn with_optional_name(&self, name: Option<&Name>) -> Self {
        let mut scope = self.clone();
        scope.extend_optional_name(name);
        scope
    }

    fn with_let_directives(&self, attributes: &[Attribute]) -> Self {
        let mut scope = self.clone();
        scope.extend_let_directives(attributes);
        scope
    }

    fn with_expression_bindings(&self, binding: Option<&Expression>) -> Self {
        let mut scope = self.clone();
        if let Some(binding) = binding {
            extend_name_set_with_expression_pattern_bindings(&mut scope.names, binding);
        }
        scope
    }

    fn with_each_block(&self, block: &EachBlock) -> Self {
        self.with_expression_bindings(block.context.as_ref())
            .with_optional_name(block.index.as_ref())
    }

    fn with_snippet_block(&self, block: &SnippetBlock) -> Self {
        block
            .parameters
            .iter()
            .fold(self.clone(), |mut scope, parameter| {
                extend_name_set_with_expression_pattern_bindings(&mut scope.names, parameter);
                scope
            })
    }

    fn child_scope_for(&self, node: &Node) -> Self {
        match node {
            Node::EachBlock(block) => self.with_each_block(block),
            Node::SnippetBlock(block) => self.with_snippet_block(block),
            _ => {
                if let Some(element) = node.as_element() {
                    self.with_let_directives(element.attributes())
                } else {
                    self.clone()
                }
            }
        }
    }

    fn extend_optional_name(&mut self, name: Option<&Name>) {
        if let Some(name) = name {
            self.names.insert(name.clone());
        }
    }

    fn extend_let_directives(&mut self, attributes: &[Attribute]) {
        for attribute in attributes {
            if let Attribute::LetDirective(directive) = attribute {
                self.extend_let_directive(directive);
            }
        }
    }

    fn extend_let_directive(&mut self, directive: &DirectiveAttribute) {
        let before = self.names.len();
        if let Some(pattern) = directive.expression.oxc_pattern() {
            extend_name_set_with_oxc_pattern_bindings(&mut self.names, pattern);
        } else if let Some(expression) = directive.expression.oxc_expression() {
            // For `let:a={binding}`, the expression may be parsed as an expression
            // rather than a pattern. Extract binding names from identifiers and
            // destructuring patterns in the expression.
            collect_binding_names_from_oxc_expression(expression, &mut self.names);
        }
        if self.names.len() == before {
            self.names.insert(directive.name.clone());
        }
    }
}

fn collect_binding_names_from_oxc_expression(
    expression: &OxcExpression<'_>,
    names: &mut NameSet,
) {
    match expression {
        OxcExpression::Identifier(id) => {
            names.insert(Arc::from(id.name.as_str()));
        }
        OxcExpression::ObjectExpression(obj) => {
            for prop in &obj.properties {
                match prop {
                    oxc_ast::ast::ObjectPropertyKind::ObjectProperty(prop) => {
                        collect_binding_names_from_oxc_expression(&prop.value, names);
                    }
                    oxc_ast::ast::ObjectPropertyKind::SpreadProperty(spread) => {
                        collect_binding_names_from_oxc_expression(&spread.argument, names);
                    }
                }
            }
        }
        OxcExpression::ArrayExpression(arr) => {
            for element in &arr.elements {
                match element {
                    oxc_ast::ast::ArrayExpressionElement::SpreadElement(spread) => {
                        collect_binding_names_from_oxc_expression(&spread.argument, names);
                    }
                    _ => {
                        if let Some(expression) = element.as_expression() {
                            collect_binding_names_from_oxc_expression(expression, names);
                        }
                    }
                }
            }
        }
        OxcExpression::AssignmentExpression(assign) => {
            if let Some(id) = assign.left.as_simple_assignment_target() {
                if let oxc_ast::ast::SimpleAssignmentTarget::AssignmentTargetIdentifier(id) = id {
                    names.insert(Arc::from(id.name.as_str()));
                }
            }
        }
        _ => {}
    }
}

#[derive(Clone, Default)]
struct IgnoreCodes(OrderedNames);

impl IgnoreCodes {
    fn from_slice(ignore_codes: &[Arc<str>]) -> Self {
        let mut codes = OrderedNames::default();
        codes.extend(ignore_codes.iter().cloned());
        Self(codes)
    }

    fn push_unique(&mut self, code: &str) {
        self.0.extend([Arc::from(code)]);
    }

    fn extend_unique<I>(&mut self, codes: I)
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        for code in codes {
            self.push_unique(code.as_ref());
        }
    }

    fn append(&mut self, other: &mut Self) {
        self.0.extend(std::mem::take(&mut other.0));
    }

    fn as_slice(&self) -> &[Arc<str>] {
        self.0.as_slice()
    }

    fn to_boxed_slice(&self) -> Box<[Arc<str>]> {
        self.0.clone().into_boxed_slice()
    }
}
const SVELTE_WARNING_CODES: &[&str] = &[
    "a11y_accesskey",
    "a11y_aria_activedescendant_has_tabindex",
    "a11y_aria_attributes",
    "a11y_autocomplete_valid",
    "a11y_autofocus",
    "a11y_click_events_have_key_events",
    "a11y_consider_explicit_label",
    "a11y_distracting_elements",
    "a11y_figcaption_index",
    "a11y_figcaption_parent",
    "a11y_hidden",
    "a11y_img_redundant_alt",
    "a11y_incorrect_aria_attribute_type",
    "a11y_incorrect_aria_attribute_type_boolean",
    "a11y_incorrect_aria_attribute_type_id",
    "a11y_incorrect_aria_attribute_type_idlist",
    "a11y_incorrect_aria_attribute_type_integer",
    "a11y_incorrect_aria_attribute_type_token",
    "a11y_incorrect_aria_attribute_type_tokenlist",
    "a11y_incorrect_aria_attribute_type_tristate",
    "a11y_interactive_supports_focus",
    "a11y_invalid_attribute",
    "a11y_label_has_associated_control",
    "a11y_media_has_caption",
    "a11y_misplaced_role",
    "a11y_misplaced_scope",
    "a11y_missing_attribute",
    "a11y_missing_content",
    "a11y_mouse_events_have_key_events",
    "a11y_no_abstract_role",
    "a11y_no_interactive_element_to_noninteractive_role",
    "a11y_no_noninteractive_element_interactions",
    "a11y_no_noninteractive_element_to_interactive_role",
    "a11y_no_noninteractive_tabindex",
    "a11y_no_redundant_roles",
    "a11y_no_static_element_interactions",
    "a11y_positive_tabindex",
    "a11y_role_has_required_aria_props",
    "a11y_role_supports_aria_props",
    "a11y_role_supports_aria_props_implicit",
    "a11y_unknown_aria_attribute",
    "a11y_unknown_role",
    "bidirectional_control_characters",
    "legacy_code",
    "unknown_code",
    "options_deprecated_accessors",
    "options_deprecated_immutable",
    "options_missing_custom_element",
    "options_removed_enable_sourcemap",
    "options_removed_hydratable",
    "options_removed_loop_guard_timeout",
    "options_renamed_ssr_dom",
    "custom_element_props_identifier",
    "export_let_unused",
    "legacy_component_creation",
    "non_reactive_update",
    "perf_avoid_inline_class",
    "perf_avoid_nested_class",
    "reactive_declaration_invalid_placement",
    "reactive_declaration_module_script_dependency",
    "state_referenced_locally",
    "store_rune_conflict",
    "css_unused_selector",
    "attribute_avoid_is",
    "attribute_global_event_reference",
    "attribute_illegal_colon",
    "attribute_invalid_property_name",
    "attribute_quoted",
    "bind_invalid_each_rest",
    "bind_invalid_value",
    "block_empty",
    "component_name_lowercase",
    "element_implicitly_closed",
    "element_invalid_self_closing_tag",
    "event_directive_deprecated",
    "node_invalid_placement_ssr",
    "script_context_deprecated",
    "script_unknown_attribute",
    "slot_element_deprecated",
    "svelte_component_deprecated",
    "svelte_element_invalid_this",
    "svelte_self_deprecated",
    "await_waterfall",
    "await_reactivity_loss",
    "state_snapshot_uncloneable",
    "binding_property_non_reactive",
    "hydration_attribute_changed",
    "hydration_html_changed",
    "ownership_invalid_binding",
    "ownership_invalid_mutation",
    "invalid_const_assignment",
];
const ROLE_SUGGESTIONS: &[&str] = &[
    "alert",
    "alertdialog",
    "application",
    "article",
    "banner",
    "blockquote",
    "button",
    "caption",
    "cell",
    "checkbox",
    "code",
    "columnheader",
    "combobox",
    "complementary",
    "contentinfo",
    "definition",
    "deletion",
    "dialog",
    "directory",
    "doc-abstract",
    "doc-acknowledgments",
    "doc-afterword",
    "doc-appendix",
    "doc-backlink",
    "doc-biblioentry",
    "doc-bibliography",
    "doc-biblioref",
    "doc-chapter",
    "doc-colophon",
    "doc-conclusion",
    "doc-cover",
    "doc-credit",
    "doc-credits",
    "doc-dedication",
    "doc-endnote",
    "doc-endnotes",
    "doc-epigraph",
    "doc-epilogue",
    "doc-errata",
    "doc-example",
    "doc-footnote",
    "doc-foreword",
    "doc-glossary",
    "doc-glossref",
    "doc-index",
    "doc-introduction",
    "doc-noteref",
    "doc-notice",
    "doc-pagebreak",
    "doc-pagelist",
    "doc-part",
    "doc-preface",
    "doc-prologue",
    "doc-pullquote",
    "doc-qna",
    "doc-subtitle",
    "doc-tip",
    "doc-toc",
    "document",
    "emphasis",
    "feed",
    "figure",
    "form",
    "generic",
    "grid",
    "gridcell",
    "group",
    "heading",
    "img",
    "insertion",
    "link",
    "list",
    "listbox",
    "listitem",
    "log",
    "main",
    "marquee",
    "math",
    "menu",
    "menubar",
    "menuitem",
    "menuitemcheckbox",
    "menuitemradio",
    "meter",
    "navigation",
    "none",
    "note",
    "option",
    "paragraph",
    "presentation",
    "progressbar",
    "radio",
    "radiogroup",
    "region",
    "row",
    "rowgroup",
    "rowheader",
    "scrollbar",
    "search",
    "searchbox",
    "separator",
    "slider",
    "spinbutton",
    "status",
    "strong",
    "subscript",
    "superscript",
    "switch",
    "tab",
    "table",
    "tablist",
    "tabpanel",
    "term",
    "textbox",
    "time",
    "timer",
    "toolbar",
    "tooltip",
    "tree",
    "treegrid",
    "treeitem",
];
const ARIA_ATTRIBUTE_SUFFIX_SUGGESTIONS: &[&str] = &[
    "activedescendant",
    "atomic",
    "autocomplete",
    "busy",
    "checked",
    "colcount",
    "colindex",
    "colspan",
    "controls",
    "current",
    "describedby",
    "description",
    "details",
    "disabled",
    "dropeffect",
    "errormessage",
    "expanded",
    "flowto",
    "grabbed",
    "haspopup",
    "hidden",
    "invalid",
    "keyshortcuts",
    "label",
    "labelledby",
    "level",
    "live",
    "modal",
    "multiline",
    "multiselectable",
    "orientation",
    "owns",
    "placeholder",
    "posinset",
    "pressed",
    "readonly",
    "relevant",
    "required",
    "roledescription",
    "rowcount",
    "rowindex",
    "rowspan",
    "selected",
    "setsize",
    "sort",
    "valuemax",
    "valuemin",
    "valuenow",
    "valuetext",
];
const AUTOCOMPLETE_ADDRESS_TOKENS: &[&str] = &["shipping", "billing"];
const AUTOCOMPLETE_FIELD_TOKENS: &[&str] = &[
    "",
    "on",
    "off",
    "name",
    "honorific-prefix",
    "given-name",
    "additional-name",
    "family-name",
    "honorific-suffix",
    "nickname",
    "username",
    "new-password",
    "current-password",
    "one-time-code",
    "organization-title",
    "organization",
    "street-address",
    "address-line1",
    "address-line2",
    "address-line3",
    "address-level4",
    "address-level3",
    "address-level2",
    "address-level1",
    "country",
    "country-name",
    "postal-code",
    "cc-name",
    "cc-given-name",
    "cc-additional-name",
    "cc-family-name",
    "cc-number",
    "cc-exp",
    "cc-exp-month",
    "cc-exp-year",
    "cc-csc",
    "cc-type",
    "transaction-currency",
    "transaction-amount",
    "language",
    "bday",
    "bday-day",
    "bday-month",
    "bday-year",
    "sex",
    "url",
    "photo",
];
const AUTOCOMPLETE_CONTACT_TYPE_TOKENS: &[&str] = &["home", "work", "mobile", "fax", "pager"];
const AUTOCOMPLETE_CONTACT_FIELD_TOKENS: &[&str] = &[
    "tel",
    "tel-country-code",
    "tel-national",
    "tel-area-code",
    "tel-local",
    "tel-local-prefix",
    "tel-local-suffix",
    "tel-extension",
    "email",
    "impp",
];
const A11Y_INTERACTIVE_HANDLERS: &[&str] = &[
    "keypress",
    "keydown",
    "keyup",
    "click",
    "contextmenu",
    "dblclick",
    "drag",
    "dragend",
    "dragenter",
    "dragexit",
    "dragleave",
    "dragover",
    "dragstart",
    "drop",
    "mousedown",
    "mouseenter",
    "mouseleave",
    "mousemove",
    "mouseout",
    "mouseover",
    "mouseup",
    "pointerdown",
    "pointerup",
    "pointermove",
    "pointerenter",
    "pointerleave",
    "pointerover",
    "pointerout",
    "pointercancel",
    "touchstart",
    "touchend",
    "touchmove",
    "touchcancel",
];
const A11Y_RECOMMENDED_INTERACTIVE_HANDLERS: &[&str] = &[
    "click",
    "mousedown",
    "mouseup",
    "keypress",
    "keydown",
    "keyup",
];

static QUERY_NON_INTERACTIVE_ROLES: LazyLock<FxHashSet<QueryRoleKey>> = LazyLock::new(|| {
    let mut roles = FxHashSet::default();
    for (role_key, definition) in QUERY_ROLES.iter() {
        if definition.r#abstract {
            continue;
        }
        if matches!(
            role_key,
            QueryRoleKey::Toolbar
                | QueryRoleKey::Tabpanel
                | QueryRoleKey::Generic
                | QueryRoleKey::Cell
        ) {
            continue;
        }
        if !role_has_widget_or_window_superclass(definition) {
            roles.insert(*role_key);
        }
    }
    roles.insert(QueryRoleKey::Progressbar);
    roles
});

static QUERY_INTERACTIVE_ROLES: LazyLock<FxHashSet<QueryRoleKey>> = LazyLock::new(|| {
    let mut roles = FxHashSet::default();
    for (role_key, definition) in QUERY_ROLES.iter() {
        if definition.r#abstract || *role_key == QueryRoleKey::Generic {
            continue;
        }
        if !QUERY_NON_INTERACTIVE_ROLES.contains(role_key) {
            roles.insert(*role_key);
        }
    }
    roles
});

static QUERY_ELEMENT_ROLE_RELATIONS: LazyLock<
    FxHashMap<QueryRoleRelationConcept, Vec<QueryRoleKey>>,
> = LazyLock::new(|| {
    let mut relations = FxHashMap::default();
    for (role_key, definition) in QUERY_ROLES.iter() {
        let concepts = definition
            .base_concepts
            .iter()
            .chain(definition.related_concepts.iter());

        for relation in concepts {
            if relation.module.as_deref() != Some("HTML") {
                continue;
            }
            let Some(concept) = relation.concept.as_ref() else {
                continue;
            };
            let entry = relations.entry(concept.clone()).or_insert_with(Vec::new);
            if !entry.contains(role_key) {
                entry.push(*role_key);
            }
        }
    }
    relations
});

static QUERY_NON_INTERACTIVE_ELEMENT_ROLE_SCHEMAS: LazyLock<Vec<QueryRoleRelationConcept>> =
    LazyLock::new(|| {
        let mut schemas = Vec::new();
        for (schema, roles) in QUERY_ELEMENT_ROLE_RELATIONS.iter() {
            let all_non_interactive = roles.iter().all(|role| {
                *role != QueryRoleKey::Generic && QUERY_NON_INTERACTIVE_ROLES.contains(role)
            });
            if all_non_interactive {
                schemas.push(schema.clone());
            }
        }
        schemas
    });

static QUERY_INTERACTIVE_ELEMENT_ROLE_SCHEMAS: LazyLock<Vec<QueryRoleRelationConcept>> =
    LazyLock::new(|| {
        let mut schemas = Vec::new();
        for (schema, roles) in QUERY_ELEMENT_ROLE_RELATIONS.iter() {
            let all_interactive = roles
                .iter()
                .all(|role| QUERY_INTERACTIVE_ROLES.contains(role));
            if all_interactive {
                schemas.push(schema.clone());
            }
        }
        schemas
    });

#[derive(Debug, Clone)]
enum StaticAttributeValue {
    BooleanTrue,
    Text(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ElementInteractivity {
    Interactive,
    NonInteractive,
    Static,
}

#[derive(Debug, Default)]
struct BiomeA11ySemantics {
    roles: AriaRoles,
    properties: AriaProperties,
}

impl BiomeA11ySemantics {
    fn role_definition(
        &self,
        role: &str,
    ) -> Option<&'static dyn biome_aria::roles::AriaRoleDefinition> {
        self.roles.get_role(role)
    }

    fn implicit_role_name(&self, element: &RegularElement, tag: &str) -> Option<String> {
        implicit_role_name_for_element(element, tag)
    }

    fn redundant_role_implicit_name(
        &self,
        element: &RegularElement,
        tag: &str,
    ) -> Option<&'static str> {
        if tag == "menuitem" {
            return menuitem_redundant_implicit_role(element);
        }
        if tag == "input" {
            return input_redundant_implicit_role(element);
        }
        match tag {
            "a" | "area" => Some("link"),
            "article" => Some("article"),
            "aside" => Some("complementary"),
            "body" => Some("document"),
            "button" => Some("button"),
            "datalist" => Some("listbox"),
            "dd" => Some("definition"),
            "dfn" => Some("term"),
            "details" => Some("group"),
            "dialog" => Some("dialog"),
            "dt" => Some("term"),
            "fieldset" => Some("group"),
            "figure" => Some("figure"),
            "form" => Some("form"),
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => Some("heading"),
            "hr" => Some("separator"),
            "img" => Some("img"),
            "li" => Some("listitem"),
            "link" => Some("link"),
            "main" => Some("main"),
            "menu" => Some("list"),
            "meter" => Some("progressbar"),
            "nav" => Some("navigation"),
            "ol" | "ul" => Some("list"),
            "optgroup" => Some("group"),
            "option" => Some("option"),
            "output" => Some("status"),
            "progress" => Some("progressbar"),
            "section" => Some("region"),
            "summary" => Some("button"),
            "table" => Some("table"),
            "tbody" | "tfoot" | "thead" => Some("rowgroup"),
            "textarea" => Some("textbox"),
            "tr" => Some("row"),
            _ => None,
        }
    }

    fn role_is_interactive(&self, role: &str) -> bool {
        query_role_key(role).is_some_and(|key| QUERY_INTERACTIVE_ROLES.contains(&key))
    }

    fn role_is_non_interactive(&self, role: &str) -> bool {
        query_role_key(role).is_some_and(|key| QUERY_NON_INTERACTIVE_ROLES.contains(&key))
    }

    fn is_noninteractive_to_interactive_role_exception(&self, tag: &str, role: &str) -> bool {
        match tag {
            "ul" | "ol" | "menu" => matches!(
                role,
                "listbox" | "menu" | "menubar" | "radiogroup" | "tablist" | "tree" | "treegrid"
            ),
            "li" => matches!(role, "menuitem" | "option" | "row" | "tab" | "treeitem"),
            "table" => role == "grid",
            "td" => role == "gridcell",
            "fieldset" => matches!(role, "radiogroup" | "presentation"),
            _ => false,
        }
    }

    fn element_interactivity(&self, element: &RegularElement, tag: &str) -> ElementInteractivity {
        if QUERY_INTERACTIVE_ELEMENT_ROLE_SCHEMAS
            .iter()
            .any(|schema| match_query_role_concept(schema, element, tag))
            || is_intrinsically_interactive(element, tag)
        {
            ElementInteractivity::Interactive
        } else if (tag != "header"
            && QUERY_NON_INTERACTIVE_ELEMENT_ROLE_SCHEMAS
                .iter()
                .any(|schema| match_query_role_concept(schema, element, tag)))
            || matches!(
                tag,
                "br" | "dir"
                    | "dl"
                    | "figcaption"
                    | "form"
                    | "label"
                    | "legend"
                    | "marquee"
                    | "pre"
                    | "ruby"
            )
        {
            ElementInteractivity::NonInteractive
        } else {
            ElementInteractivity::Static
        }
    }
}

fn input_redundant_implicit_role(element: &RegularElement) -> Option<&'static str> {
    let input_type = named_attribute_from_element(element, "type")
        .and_then(attribute_static_text)?
        .to_ascii_lowercase();
    if has_attribute_present(element, "list")
        && matches!(
            input_type.as_str(),
            "email" | "search" | "tel" | "text" | "url"
        )
    {
        return Some("combobox");
    }
    match input_type.as_str() {
        "button" | "image" | "reset" | "submit" => Some("button"),
        "checkbox" => Some("checkbox"),
        "radio" => Some("radio"),
        "range" => Some("slider"),
        "number" => Some("spinbutton"),
        "email" | "tel" | "text" | "url" => Some("textbox"),
        "search" => Some("searchbox"),
        _ => None,
    }
}

fn menuitem_redundant_implicit_role(element: &RegularElement) -> Option<&'static str> {
    let menuitem_type = named_attribute_from_element(element, "type")
        .and_then(attribute_static_text)?
        .to_ascii_lowercase();
    match menuitem_type.as_str() {
        "command" => Some("menuitem"),
        "checkbox" => Some("menuitemcheckbox"),
        "radio" => Some("menuitemradio"),
        _ => None,
    }
}

pub(crate) fn collect_compile_warnings(
    source: SourceText<'_>,
    options: &CompileOptions,
    root: &Root,
) -> Vec<Warning> {
    let source_text = source;
    let source = source_text.text;

    let mut warnings = Vec::new();
    for diagnostic in
        crate::api::validation::collect_error_mode_downgraded_warnings(source, options, root)
    {
        warnings.push(warning_from_compile_error(source_text, diagnostic));
    }

    let runes_mode = crate::api::infer_runes_mode(options, root);
    if !options.custom_element
        && let Some(root_options) = root.options.as_ref()
        && let Some(custom_element_attribute) =
            root_options
                .attributes
                .iter()
                .find_map(|attribute| match attribute {
                    Attribute::Attribute(attribute)
                        if attribute
                            .name
                            .as_ref()
                            .eq_ignore_ascii_case("customElement") =>
                    {
                        Some(attribute)
                    }
                    _ => None,
                })
    {
        warnings.push(make_warning(
            source,
            options,
            "options_missing_custom_element",
            "The `customElement` option is used when generating a custom element. Did you forget the `customElement: true` compile option?",
            custom_element_attribute.start,
            custom_element_attribute.end,
        ));
    }
    let script_declared_names = collect_script_declared_names(root);
    let (root_in_svg_context, root_in_mathml_context) = root_namespace_context(root);
    collect_script_warnings(
        source,
        options,
        root,
        runes_mode,
        &script_declared_names,
        &mut warnings,
    );
    collect_fragment_warnings(
        WarningEnv {
            source,
            options,
            root,
            runes_mode,
            script_declared_names: &script_declared_names,
            each_rest_bindings: &[],
        },
        &root.fragment,
        WarningFragmentContext {
            in_dialog: false,
            parent_regular_tag: None,
            parent_regular_has_end_tag: false,
            inside_control_block: false,
            in_svg_context: root_in_svg_context,
            in_mathml_context: root_in_mathml_context,
            inherited_ignores: &[],
        },
        &mut warnings,
    );
    emit_css_slot_fallback_unused_selector_warnings(source, options, root, &mut warnings);
    dedupe_warnings_in_place(&mut warnings);
    if !options.warning_filter_ignore_codes.is_empty() {
        warnings.retain(|warning| {
            !options
                .warning_filter_ignore_codes
                .iter()
                .any(|ignored| ignored.as_ref() == warning.code.as_ref())
        });
    }
    if let Some(filter) = &options.warning_filter {
        warnings.retain(|warning| filter.call(warning));
    }
    warnings
}

pub(crate) fn collect_module_warnings(
    parsed: &crate::compiler::phases::parse::ParsedModuleProgram<'_>,
    options: &CompileOptions,
) -> Vec<Warning> {
    let mut warnings = Vec::new();
    emit_program_estree_warnings(
        parsed.source_text().text,
        options,
        parsed.program(),
        ScriptWalkContext {
            function_depth: 0,
            is_module: true,
        },
        true,
        0,
        &mut warnings,
    );
    dedupe_warnings_in_place(&mut warnings);
    if !options.warning_filter_ignore_codes.is_empty() {
        warnings.retain(|warning| {
            !options
                .warning_filter_ignore_codes
                .iter()
                .any(|ignored| ignored.as_ref() == warning.code.as_ref())
        });
    }
    if let Some(filter) = &options.warning_filter {
        warnings.retain(|warning| filter.call(warning));
    }
    warnings
}

fn root_namespace_context(root: &Root) -> (bool, bool) {
    let namespace = root.options.as_ref().and_then(|options| {
        options.attributes.iter().find_map(|attribute| {
            let Attribute::Attribute(attribute) = attribute else {
                return None;
            };
            if !attribute.name.as_ref().eq_ignore_ascii_case("namespace") {
                return None;
            }
            match &attribute.value {
                AttributeValueList::Values(values) => values.iter().find_map(|value| match value {
                    AttributeValue::Text(text) => Some(text.data.as_ref().to_ascii_lowercase()),
                    _ => None,
                }),
                AttributeValueList::ExpressionTag(tag) => {
                    expression_string_value(&tag.expression).map(|value| value.to_ascii_lowercase())
                }
                AttributeValueList::Boolean(_) => None,
            }
        })
    });

    match namespace.as_deref() {
        Some("svg") => (true, false),
        Some("mathml") | Some("math") => (false, true),
        _ => (false, false),
    }
}

fn dedupe_warnings_in_place(warnings: &mut Vec<Warning>) {
    let mut seen = FxHashSet::<(Arc<str>, Arc<str>, Option<[usize; 2]>)>::default();
    warnings.retain(|warning| {
        seen.insert((
            warning.code.clone(),
            warning.message.clone(),
            warning.position,
        ))
    });
}

fn emit_css_slot_fallback_unused_selector_warnings(
    source: &str,
    options: &CompileOptions,
    root: &Root,
    warnings: &mut Vec<Warning>,
) {
    if !component_uses_custom_element(root, options) {
        return;
    }
    let Some(css) = root.css.as_ref() else {
        return;
    };
    if css
        .content
        .comment
        .as_ref()
        .is_some_and(|comment| css_comment_ignores_unused_selector(comment))
    {
        return;
    }

    collect_css_slot_fallback_unused_from_nodes(source, options, &css.children, warnings);
}

fn css_comment_ignores_unused_selector(comment: &str) -> bool {
    let stripped = comment
        .trim()
        .trim_start_matches("/*")
        .trim_end_matches("*/")
        .trim();
    [comment, stripped].iter().any(|candidate| {
        crate::api::scan::parse_svelte_ignores(candidate)
            .iter()
            .any(|code| code.as_ref() == "css_unused_selector")
    })
}

fn collect_css_slot_fallback_unused_from_nodes(
    source: &str,
    options: &CompileOptions,
    nodes: &[crate::ast::modern::CssNode],
    warnings: &mut Vec<Warning>,
) {
    for node in nodes.iter() {
        match node {
            crate::ast::modern::CssNode::Rule(rule) => {
                for complex in rule.prelude.children.iter() {
                    if !complex_selector_targets_slot_fallback_under_sibling(complex) {
                        continue;
                    }
                    let selector_text = source
                        .get(complex.start..complex.end)
                        .unwrap_or_default()
                        .trim();
                    if selector_text.is_empty() {
                        continue;
                    }
                    warnings.push(make_warning(
                        source,
                        options,
                        "css_unused_selector",
                        &format!("Unused CSS selector \"{}\"", selector_text),
                        complex.start,
                        complex.end,
                    ));
                }
                collect_css_slot_fallback_unused_from_block(source, options, &rule.block, warnings);
            }
            crate::ast::modern::CssNode::Atrule(atrule) => {
                if let Some(block) = atrule.block.as_ref() {
                    collect_css_slot_fallback_unused_from_block(source, options, block, warnings);
                }
            }
        }
    }
}

fn collect_css_slot_fallback_unused_from_block(
    source: &str,
    options: &CompileOptions,
    block: &crate::ast::modern::CssBlock,
    warnings: &mut Vec<Warning>,
) {
    for child in block.children.iter() {
        match child {
            crate::ast::modern::CssBlockChild::Rule(rule) => {
                for complex in rule.prelude.children.iter() {
                    if !complex_selector_targets_slot_fallback_under_sibling(complex) {
                        continue;
                    }
                    let selector_text = source
                        .get(complex.start..complex.end)
                        .unwrap_or_default()
                        .trim();
                    if selector_text.is_empty() {
                        continue;
                    }
                    warnings.push(make_warning(
                        source,
                        options,
                        "css_unused_selector",
                        &format!("Unused CSS selector \"{}\"", selector_text),
                        complex.start,
                        complex.end,
                    ));
                }
                collect_css_slot_fallback_unused_from_block(source, options, &rule.block, warnings);
            }
            crate::ast::modern::CssBlockChild::Atrule(atrule) => {
                if let Some(block) = atrule.block.as_ref() {
                    collect_css_slot_fallback_unused_from_block(source, options, block, warnings);
                }
            }
            crate::ast::modern::CssBlockChild::Declaration(_) => {}
        }
    }
}

fn complex_selector_targets_slot_fallback_under_sibling(
    complex: &crate::ast::modern::CssComplexSelector,
) -> bool {
    for (index, relative) in complex.children.iter().enumerate() {
        if index == 0 {
            continue;
        }
        let Some(combinator) = relative.combinator.as_ref() else {
            continue;
        };
        if combinator.name.as_ref() != "+" && combinator.name.as_ref() != "~" {
            continue;
        }
        if !relative_selector_targets_slot(relative) {
            continue;
        }
        let Some(next_relative) = complex.children.get(index + 1) else {
            continue;
        };
        let Some(next_combinator) = next_relative.combinator.as_ref() else {
            continue;
        };
        if next_combinator.name.as_ref() == ">" {
            return true;
        }
    }
    false
}

fn relative_selector_targets_slot(relative: &crate::ast::modern::CssRelativeSelector) -> bool {
    relative.selectors.iter().any(|selector| {
        matches!(
            selector,
            crate::ast::modern::CssSimpleSelector::TypeSelector(type_selector)
                if type_selector.name.as_ref().eq_ignore_ascii_case("slot")
        )
    })
}

fn collect_script_warnings(
    source: &str,
    options: &CompileOptions,
    root: &Root,
    runes_mode: bool,
    _script_declared_names: &NameSet,
    warnings: &mut Vec<Warning>,
) {
    for script in root_scripts(root) {
        for attribute in &script.attributes {
            match attribute {
                Attribute::Attribute(attribute)
                    if SCRIPT_ALLOWED_ATTRIBUTES
                        .iter()
                        .any(|allowed| allowed.eq_ignore_ascii_case(attribute.name.as_ref())) => {}
                Attribute::Attribute(attribute) => warnings.push(make_warning(
                    source,
                    options,
                    "script_unknown_attribute",
                    "Unrecognized attribute — should be one of `generics`, `lang` or `module`. If this exists for a preprocessor, ensure that the preprocessor removes it",
                    attribute.start,
                    attribute.end,
                )),
                Attribute::SpreadAttribute(spread) => warnings.push(make_warning(
                    source,
                    options,
                    "script_unknown_attribute",
                    "Unrecognized attribute — should be one of `generics`, `lang` or `module`. If this exists for a preprocessor, ensure that the preprocessor removes it",
                    spread.start,
                    spread.end,
                )),
                Attribute::BindDirective(directive)
                | Attribute::OnDirective(directive)
                | Attribute::ClassDirective(directive)
                | Attribute::LetDirective(directive)
                | Attribute::AnimateDirective(directive)
                | Attribute::UseDirective(directive) => warnings.push(make_warning(
                    source,
                    options,
                    "script_unknown_attribute",
                    "Unrecognized attribute — should be one of `generics`, `lang` or `module`. If this exists for a preprocessor, ensure that the preprocessor removes it",
                    directive.start,
                    directive.end,
                )),
                Attribute::StyleDirective(style) => warnings.push(make_warning(
                    source,
                    options,
                    "script_unknown_attribute",
                    "Unrecognized attribute — should be one of `generics`, `lang` or `module`. If this exists for a preprocessor, ensure that the preprocessor removes it",
                    style.start,
                    style.end,
                )),
                Attribute::TransitionDirective(transition) => warnings.push(make_warning(
                    source,
                    options,
                    "script_unknown_attribute",
                    "Unrecognized attribute — should be one of `generics`, `lang` or `module`. If this exists for a preprocessor, ensure that the preprocessor removes it",
                    transition.start,
                    transition.end,
                )),
                Attribute::AttachTag(tag) => warnings.push(make_warning(
                    source,
                    options,
                    "script_unknown_attribute",
                    "Unrecognized attribute — should be one of `generics`, `lang` or `module`. If this exists for a preprocessor, ensure that the preprocessor removes it",
                    tag.start,
                    tag.end,
                )),
            }
        }

        if runes_mode
            && script.context == crate::ast::modern::ScriptContext::Module
            && let Some(context_attr) =
                script
                    .attributes
                    .iter()
                    .find_map(|attribute| match attribute {
                        Attribute::Attribute(attribute)
                            if attribute.name.as_ref().eq_ignore_ascii_case("context") =>
                        {
                            Some(attribute)
                        }
                        _ => None,
                    })
        {
            warnings.push(make_warning(
                source,
                options,
                "script_context_deprecated",
                "`context=\"module\"` is deprecated, use the `module` attribute instead",
                context_attr.start,
                context_attr.end,
            ));
        }

        emit_script_estree_warnings(source, options, script, runes_mode, warnings);
    }

    if !runes_mode {
        emit_reactive_module_script_dependency_warnings(source, options, root, warnings);
    }
    if options.runes != Some(false) {
        emit_store_rune_conflict_warnings(source, options, root, warnings);
    }
    if runes_mode {
        emit_state_referenced_locally_warnings(source, options, root, warnings);
        emit_non_reactive_update_warnings(source, options, root, warnings);
    } else {
        emit_export_let_unused_warnings(source, options, root, warnings);
    }

    if runes_mode
        && component_uses_custom_element(root, options)
        && !custom_element_has_props_option(root)
    {
        for script in root_scripts(root) {
            emit_custom_element_props_identifier_warnings(
                source,
                options,
                &script.content,
                script.content_start,
                warnings,
            );
        }
    }
}

fn collect_script_declared_names(root: &Root) -> NameSet {
    let mut names = NameSet::default();
    for script in root_scripts(root) {
        for statement in &script.content.program().body {
            match statement {
                Statement::ImportDeclaration(declaration) => {
                    if let Some(specifiers) = declaration.specifiers.as_ref() {
                        for specifier in specifiers {
                            let local = match specifier {
                                oxc_ast::ast::ImportDeclarationSpecifier::ImportSpecifier(
                                    specifier,
                                ) => &specifier.local,
                                oxc_ast::ast::ImportDeclarationSpecifier::ImportDefaultSpecifier(
                                    specifier,
                                ) => &specifier.local,
                                oxc_ast::ast::ImportDeclarationSpecifier::ImportNamespaceSpecifier(
                                    specifier,
                                ) => &specifier.local,
                            };
                            names.insert(Arc::from(local.name.as_str()));
                        }
                    }
                }
                Statement::VariableDeclaration(declaration) => {
                    collect_declared_names_from_oxc_variable_declaration(declaration, &mut names);
                }
                Statement::FunctionDeclaration(declaration) => {
                    if let Some(id) = declaration.id.as_ref() {
                        names.insert(Arc::from(id.name.as_str()));
                    }
                }
                Statement::ClassDeclaration(declaration) => {
                    if let Some(id) = declaration.id.as_ref() {
                        names.insert(Arc::from(id.name.as_str()));
                    }
                }
                Statement::ExportNamedDeclaration(declaration) => {
                    if let Some(declaration) = declaration.declaration.as_ref() {
                        match declaration {
                            Declaration::VariableDeclaration(declaration) => {
                                collect_declared_names_from_oxc_variable_declaration(
                                    declaration,
                                    &mut names,
                                );
                            }
                            Declaration::FunctionDeclaration(declaration) => {
                                if let Some(id) = declaration.id.as_ref() {
                                    names.insert(Arc::from(id.name.as_str()));
                                }
                            }
                            Declaration::ClassDeclaration(declaration) => {
                                if let Some(id) = declaration.id.as_ref() {
                                    names.insert(Arc::from(id.name.as_str()));
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
    }
    names
}

fn root_scripts(root: &Root) -> Vec<&crate::ast::modern::Script> {
    if !root.scripts.is_empty() {
        return root.scripts.iter().collect();
    }

    let mut scripts = Vec::with_capacity(2);
    if let Some(module) = root.module.as_ref() {
        scripts.push(module);
    }
    if let Some(instance) = root.instance.as_ref() {
        scripts.push(instance);
    }
    scripts
}

#[derive(Clone, Copy)]
struct WarningEnv<'a> {
    source: &'a str,
    options: &'a CompileOptions,
    root: &'a Root,
    runes_mode: bool,
    script_declared_names: &'a NameSet,
    each_rest_bindings: &'a [RestBindingWarning],
}

#[derive(Clone, Copy)]
struct WarningFragmentContext<'a> {
    in_dialog: bool,
    parent_regular_tag: Option<&'a str>,
    parent_regular_has_end_tag: bool,
    inside_control_block: bool,
    in_svg_context: bool,
    in_mathml_context: bool,
    inherited_ignores: &'a [Arc<str>],
}

fn collect_fragment_warnings(
    env: WarningEnv<'_>,
    fragment: &Fragment,
    context: WarningFragmentContext<'_>,
    warnings: &mut Vec<Warning>,
) {
    let WarningEnv {
        source,
        options,
        root,
        runes_mode,
        each_rest_bindings,
        ..
    } = env;
    let WarningFragmentContext {
        in_dialog,
        parent_regular_tag,
        parent_regular_has_end_tag,
        in_svg_context,
        in_mathml_context,
        inherited_ignores,
        ..
    } = context;
    let mut pending_ignores = IgnoreCodes::default();
    for (node_index, node) in fragment.nodes.iter().enumerate() {
        if let Node::Comment(comment) = node {
            let parsed = parse_svelte_ignore_directive(
                comment.start.saturating_add(4),
                &comment.data,
                runes_mode,
            );
            pending_ignores.extend_unique(parsed.ignores.iter());
            for diagnostic in parsed.diagnostics {
                warnings.push(make_warning(
                    source,
                    options,
                    diagnostic.code,
                    &diagnostic.message,
                    diagnostic.start,
                    diagnostic.end,
                ));
            }
            continue;
        }

        if let Node::Text(text) = node
            && !string_contains_bidirectional_controls(text.data.as_ref())
        {
            continue;
        }

        let mut node_ignores = IgnoreCodes::from_slice(inherited_ignores);
        node_ignores.append(&mut pending_ignores);

        match node {
            Node::Text(text) => {
                let warning_start = warnings.len();
                emit_bidirectional_warnings_in_text(source, options, text, warnings);
                filter_recent_ignored_warnings(warnings, warning_start, node_ignores.as_slice());
            }
            Node::RegularElement(element) => {
                collect_element_warnings(env, element, context, node_ignores.as_slice(), warnings);
                let implicit_warning_start = warnings.len();
                if !element.self_closing
                    && !element.has_end_tag
                    && !is_void_element_tag(element.name.as_ref())
                {
                    let implicit_end = opening_tag_end_from_ast(element);
                    if let Some(next_tag) = next_regular_sibling_tag(fragment, node_index)
                        && element_implicitly_closes_with_sibling(
                            element.name.as_ref(),
                            next_tag.as_ref(),
                        )
                    {
                        warnings.push(make_warning(
                            source,
                            options,
                            "element_implicitly_closed",
                            &format!(
                                "This element is implicitly closed by the following `<{}>`, which can cause an unexpected DOM structure. Add an explicit `</{}>` to avoid surprises.",
                                next_tag, element.name
                            ),
                            element.start,
                            implicit_end,
                        ));
                    } else if let Some(parent_tag) = parent_regular_tag
                        && parent_regular_has_end_tag
                    {
                        warnings.push(make_warning(
                            source,
                            options,
                            "element_implicitly_closed",
                            &format!(
                                "This element is implicitly closed by the following `</{}>`, which can cause an unexpected DOM structure. Add an explicit `</{}>` to avoid surprises.",
                                parent_tag, element.name
                            ),
                            element.start,
                            implicit_end,
                        ));
                    }
                }
                filter_recent_ignored_warnings(
                    warnings,
                    implicit_warning_start,
                    node_ignores.as_slice(),
                );
                let child_in_dialog =
                    in_dialog || element.name.as_ref().eq_ignore_ascii_case("dialog");
                let child_parent_regular_tag = if is_void_element_tag(element.name.as_ref()) {
                    parent_regular_tag
                } else {
                    Some(element.name.as_ref())
                };
                let child_in_svg_context =
                    in_svg_context || element.name.as_ref().eq_ignore_ascii_case("svg");
                let child_in_mathml_context =
                    in_mathml_context || element.name.as_ref().eq_ignore_ascii_case("math");
                collect_fragment_warnings(
                    env,
                    &element.fragment,
                    WarningFragmentContext {
                        in_dialog: child_in_dialog,
                        parent_regular_tag: child_parent_regular_tag,
                        parent_regular_has_end_tag: element.has_end_tag,
                        inside_control_block: false,
                        in_svg_context: child_in_svg_context,
                        in_mathml_context: child_in_mathml_context,
                        inherited_ignores: node_ignores.as_slice(),
                    },
                    warnings,
                );
            }
            Node::Component(component) => {
                let warning_start = warnings.len();
                collect_component_attribute_warnings(
                    source,
                    options,
                    &component.attributes,
                    runes_mode,
                    each_rest_bindings,
                    warnings,
                );
                filter_recent_ignored_warnings(warnings, warning_start, node_ignores.as_slice());
                collect_fragment_warnings(
                    env,
                    &component.fragment,
                    WarningFragmentContext {
                        inherited_ignores: node_ignores.as_slice(),
                        ..context
                    },
                    warnings,
                );
            }
            Node::SlotElement(slot) => {
                let warning_start = warnings.len();
                if runes_mode && !component_uses_custom_element(root, options) {
                    warnings.push(make_warning(
                        source,
                        options,
                        "slot_element_deprecated",
                        "Using `<slot>` to render parent content is deprecated. Use `{@render ...}` tags instead",
                        slot.start,
                        slot.end,
                    ));
                }
                filter_recent_ignored_warnings(warnings, warning_start, node_ignores.as_slice());
                collect_fragment_warnings(
                    env,
                    &slot.fragment,
                    WarningFragmentContext {
                        inherited_ignores: node_ignores.as_slice(),
                        ..context
                    },
                    warnings,
                );
            }
            Node::IfBlock(block) => {
                let warning_start = warnings.len();
                warn_if_block_empty_fragment(source, options, Some(&block.consequent), warnings);
                if let Some(alternate) = &block.alternate
                    && let crate::ast::modern::Alternate::Fragment(fragment) = alternate.as_ref()
                {
                    warn_if_block_empty_fragment(source, options, Some(fragment), warnings);
                }
                filter_recent_ignored_warnings(warnings, warning_start, node_ignores.as_slice());
                let branch_context = WarningFragmentContext {
                    inside_control_block: true,
                    inherited_ignores: node_ignores.as_slice(),
                    ..context
                };
                collect_fragment_warnings(env, &block.consequent, branch_context, warnings);
                if let Some(alternate) = &block.alternate {
                    match alternate.as_ref() {
                        crate::ast::modern::Alternate::Fragment(fragment) => {
                            collect_fragment_warnings(env, fragment, branch_context, warnings)
                        }
                        crate::ast::modern::Alternate::IfBlock(elseif) => {
                            collect_fragment_warnings(
                                env,
                                &elseif.consequent,
                                branch_context,
                                warnings,
                            )
                        }
                    }
                }
            }
            Node::EachBlock(block) => {
                let warning_start = warnings.len();
                warn_if_block_empty_fragment(source, options, Some(&block.body), warnings);
                warn_if_block_empty_fragment(source, options, block.fallback.as_ref(), warnings);
                filter_recent_ignored_warnings(warnings, warning_start, node_ignores.as_slice());

                let mut child_rest_bindings = each_rest_bindings.to_vec();
                if let Some(context_pattern) = block.context.as_ref() {
                    collect_rest_pattern_identifiers(context_pattern, &mut child_rest_bindings);
                }
                let branch_context = WarningFragmentContext {
                    inside_control_block: true,
                    inherited_ignores: node_ignores.as_slice(),
                    ..context
                };

                collect_fragment_warnings(
                    WarningEnv {
                        each_rest_bindings: &child_rest_bindings,
                        ..env
                    },
                    &block.body,
                    branch_context,
                    warnings,
                );
                if let Some(fallback) = &block.fallback {
                    collect_fragment_warnings(env, fallback, branch_context, warnings);
                }
            }
            Node::KeyBlock(block) => {
                let warning_start = warnings.len();
                warn_if_block_empty_fragment(source, options, Some(&block.fragment), warnings);
                filter_recent_ignored_warnings(warnings, warning_start, node_ignores.as_slice());
                collect_fragment_warnings(
                    env,
                    &block.fragment,
                    WarningFragmentContext {
                        inside_control_block: true,
                        inherited_ignores: node_ignores.as_slice(),
                        ..context
                    },
                    warnings,
                )
            }
            Node::AwaitBlock(block) => {
                let warning_start = warnings.len();
                warn_if_block_empty_fragment(source, options, block.pending.as_ref(), warnings);
                warn_if_block_empty_fragment(source, options, block.then.as_ref(), warnings);
                warn_if_block_empty_fragment(source, options, block.catch.as_ref(), warnings);
                filter_recent_ignored_warnings(warnings, warning_start, node_ignores.as_slice());
                let branch_context = WarningFragmentContext {
                    inside_control_block: true,
                    inherited_ignores: node_ignores.as_slice(),
                    ..context
                };
                if let Some(pending) = &block.pending {
                    collect_fragment_warnings(env, pending, branch_context, warnings);
                }
                if let Some(then) = &block.then {
                    collect_fragment_warnings(env, then, branch_context, warnings);
                }
                if let Some(catch) = &block.catch {
                    collect_fragment_warnings(env, catch, branch_context, warnings);
                }
            }
            Node::SnippetBlock(block) => {
                let warning_start = warnings.len();
                warn_if_block_empty_fragment(source, options, Some(&block.body), warnings);
                filter_recent_ignored_warnings(warnings, warning_start, node_ignores.as_slice());
                collect_fragment_warnings(
                    env,
                    &block.body,
                    WarningFragmentContext {
                        inside_control_block: true,
                        inherited_ignores: node_ignores.as_slice(),
                        ..context
                    },
                    warnings,
                )
            }
            Node::SvelteSelf(el) => {
                let warning_start = warnings.len();
                collect_component_attribute_warnings(
                    source,
                    options,
                    &el.attributes,
                    runes_mode,
                    each_rest_bindings,
                    warnings,
                );
                warnings.push(make_warning(
                    source,
                    options,
                    "svelte_self_deprecated",
                    "`<svelte:self>` is deprecated — use self-imports (e.g. `import Self from './Self.svelte'`) instead",
                    el.start,
                    el.end,
                ));
                filter_recent_ignored_warnings(warnings, warning_start, node_ignores.as_slice());
                collect_fragment_warnings(
                    env,
                    &el.fragment,
                    WarningFragmentContext {
                        inherited_ignores: node_ignores.as_slice(),
                        ..context
                    },
                    warnings,
                );
            }
            Node::SvelteComponent(el) => {
                let warning_start = warnings.len();
                if runes_mode {
                    warnings.push(make_warning(
                        source,
                        options,
                        "svelte_component_deprecated",
                        "`<svelte:component>` is deprecated in runes mode — components are dynamic by default",
                        el.start,
                        el.end,
                    ));
                }
                collect_component_attribute_warnings(
                    source,
                    options,
                    &el.attributes,
                    runes_mode,
                    each_rest_bindings,
                    warnings,
                );
                filter_recent_ignored_warnings(warnings, warning_start, node_ignores.as_slice());
                collect_fragment_warnings(
                    env,
                    &el.fragment,
                    WarningFragmentContext {
                        inherited_ignores: node_ignores.as_slice(),
                        ..context
                    },
                    warnings,
                );
            }
            Node::SvelteHead(_)
            | Node::SvelteBody(_)
            | Node::SvelteWindow(_)
            | Node::SvelteDocument(_)
            | Node::SvelteElement(_)
            | Node::SvelteFragment(_)
            | Node::SvelteBoundary(_)
            | Node::TitleElement(_) => {
                let fragment = node.as_element().unwrap().fragment();
                collect_fragment_warnings(
                    env,
                    fragment,
                    WarningFragmentContext {
                        inherited_ignores: node_ignores.as_slice(),
                        ..context
                    },
                    warnings,
                );
            }
            _ => {}
        }
    }
}

fn collect_element_warnings(
    env: WarningEnv<'_>,
    element: &RegularElement,
    context: WarningFragmentContext<'_>,
    active_ignores: &[Arc<str>],
    warnings: &mut Vec<Warning>,
) {
    let WarningEnv {
        source,
        options,
        runes_mode,
        script_declared_names,
        each_rest_bindings,
        ..
    } = env;
    let WarningFragmentContext {
        in_dialog,
        parent_regular_tag,
        inside_control_block,
        in_svg_context,
        in_mathml_context,
        ..
    } = context;
    let warning_start = warnings.len();
    let raw_tag = element.name.as_ref();
    let tag = raw_tag.to_ascii_lowercase();
    let a11y = BiomeA11ySemantics::default();
    let has_spread = element
        .attributes
        .iter()
        .any(|attribute| matches!(attribute, Attribute::SpreadAttribute(_)));
    let has_mouse_over_handler = has_event_handler(element, "mouseover");
    let has_mouse_out_handler = has_event_handler(element, "mouseout");
    let has_focus_handler = has_event_handler(element, "focus");
    let has_blur_handler = has_event_handler(element, "blur");
    let has_click_handler = has_event_handler(element, "click");
    let has_keyboard_handler = has_event_handler(element, "keydown")
        || has_event_handler(element, "keyup")
        || has_event_handler(element, "keypress");
    let has_contenteditable_attr = has_attribute_present(element, "contenteditable");
    let has_interactive_handlers = has_any_event_handler(element, A11Y_INTERACTIVE_HANDLERS);
    let has_recommended_interactive_handlers =
        has_any_event_handler(element, A11Y_RECOMMENDED_INTERACTIVE_HANDLERS);
    let element_interactivity = a11y.element_interactivity(element, &tag);
    let is_non_interactive_element = element_interactivity == ElementInteractivity::NonInteractive;
    let is_interactive_element = element_interactivity == ElementInteractivity::Interactive;
    let is_static_element = element_interactivity == ElementInteractivity::Static;

    if element.self_closing && !tag.contains(':') && !in_svg_context && !in_mathml_context {
        let local_tag = strip_namespace_prefix(&tag);
        if !is_void_element_tag(local_tag) {
            warnings.push(make_warning(
                source,
                options,
                "element_invalid_self_closing_tag",
                &format!(
                    "Self-closing HTML tags for non-void elements are ambiguous — use `<{} ...></{}>` rather than `<{} ... />`",
                    element.name, element.name, element.name
                ),
                element.start,
                element.end,
            ));
        }
    }

    if inside_control_block
        && tag == "form"
        && parent_regular_tag.is_some_and(|parent| parent.eq_ignore_ascii_case("form"))
    {
        warnings.push(make_warning(
            source,
            options,
            "node_invalid_placement_ssr",
            "`<form>` cannot be a child of `<form>`. When rendering this component on the server, the resulting HTML will be modified by the browser (by moving, removing, or inserting elements), likely resulting in a `hydration_mismatch` warning",
            element.start,
            element.end,
        ));
    }

    for attribute in &element.attributes {
        match attribute {
            Attribute::OnDirective(directive) if runes_mode => {
                warnings.push(make_warning(
                        source,
                        options,
                        "event_directive_deprecated",
                        &format!(
                            "Using `on:{}` to listen to the {} event is deprecated. Use the event attribute `on{}` instead",
                            directive.name, directive.name, directive.name
                        ),
                        directive.start,
                        directive.end,
                    ));
            }
            Attribute::Attribute(attribute) => {
                let name = attribute.name.as_ref();

                if name.contains(':')
                    && !name.starts_with("xmlns:")
                    && !name.starts_with("xlink:")
                    && !name.starts_with("xml:")
                {
                    warnings.push(make_warning(
                        source,
                        options,
                        "attribute_illegal_colon",
                        "Attributes should not contain ':' characters to prevent ambiguity with Svelte directives",
                        attribute.start,
                        attribute.end,
                    ));
                }

                if let Some(correct_name) = react_attribute_replacement(name) {
                    warnings.push(make_warning(
                        source,
                        options,
                        "attribute_invalid_property_name",
                        &format!(
                            "'{}' is not a valid HTML attribute. Did you mean '{}'?",
                            name, correct_name
                        ),
                        attribute.start,
                        attribute.end,
                    ));
                }

                if let Some((event_name, identifier_name)) =
                    attribute_global_event_reference_name(attribute)
                    && event_name == identifier_name
                    && !script_declared_names
                        .iter()
                        .any(|declared| declared.as_ref() == identifier_name.as_str())
                {
                    warnings.push(make_warning(
                        source,
                        options,
                        "attribute_global_event_reference",
                        &format!(
                            "You are referencing `globalThis.{}`. Did you forget to declare a variable with that name?",
                            identifier_name
                        ),
                        attribute.start,
                        attribute.end,
                    ));
                }
            }
            _ => {}
        }
    }

    for attribute in &element.attributes {
        let Attribute::BindDirective(bind) = attribute else {
            continue;
        };

        let Some(binding_name) = binding_base_identifier_name(&bind.expression) else {
            continue;
        };

        for rest_binding in each_rest_bindings {
            if rest_binding.name.as_ref() != binding_name.as_ref() {
                continue;
            }
            warnings.push(make_warning(
                source,
                options,
                "bind_invalid_each_rest",
                &format!(
                    "The rest operator (...) will create a new object and binding '{}' with the original object will not work",
                    binding_name
                ),
                rest_binding.start,
                rest_binding.end,
            ));
        }
    }

    if is_lowercase_component_like_tag(raw_tag) {
        warnings.push(make_warning(
            source,
            options,
            "component_name_lowercase",
            &format!(
                "`<{raw_tag}>` will be treated as an HTML element unless it begins with a capital letter"
            ),
            element.start,
            element.end,
        ));
    }

    if runes_mode && is_custom_element_tag(raw_tag) {
        for attribute in &element.attributes {
            let Attribute::Attribute(attribute) = attribute else {
                continue;
            };
            if !attribute_is_quoted_expression(attribute) {
                continue;
            }
            warnings.push(make_warning(
                source,
                options,
                "attribute_quoted",
                "Quoted attributes on components and custom elements will be stringified in a future version of Svelte. If this isn't what you want, remove the quotes",
                attribute.start,
                attribute.end,
            ));
        }
    }

    if let Some(attribute) = named_attribute_from_element(element, "accesskey") {
        warnings.push(make_warning(
            source,
            options,
            "a11y_accesskey",
            "Avoid using accesskey",
            attribute.start,
            attribute.end,
        ));
    }

    if let Some(attribute) = named_attribute_from_element(element, "autofocus")
        && tag != "dialog"
        && !in_dialog
    {
        warnings.push(make_warning(
            source,
            options,
            "a11y_autofocus",
            "Avoid using autofocus",
            attribute.start,
            attribute.end,
        ));
    }

    if let Some(active_descendant) = named_attribute_from_element(element, "aria-activedescendant")
        && !has_spread
        && tag != "svelte:element"
        && !is_interactive_element
        && !has_attribute_present(element, "tabindex")
    {
        warnings.push(make_warning(
            source,
            options,
            "a11y_aria_activedescendant_has_tabindex",
            "An element with an aria-activedescendant attribute should have a tabindex value",
            active_descendant.start,
            active_descendant.end,
        ));
    }

    if tag == "figcaption"
        && !parent_regular_tag.is_some_and(|name| name.eq_ignore_ascii_case("figure"))
    {
        warnings.push(make_warning(
            source,
            options,
            "a11y_figcaption_parent",
            "`<figcaption>` must be an immediate child of `<figure>`",
            element.start,
            element.end,
        ));
    }

    if tag == "figure" {
        let mut visible_children: Vec<&Node> = Vec::new();
        for child in &element.fragment.nodes {
            match child {
                Node::Comment(_) => {}
                Node::Text(text) if text.data.chars().all(char::is_whitespace) => {}
                _ => visible_children.push(child),
            }
        }

        if let Some((index, figcaption)) =
            visible_children
                .iter()
                .enumerate()
                .find_map(|(index, child)| match child {
                    Node::RegularElement(child_element)
                        if child_element
                            .name
                            .as_ref()
                            .eq_ignore_ascii_case("figcaption") =>
                    {
                        Some((index, child_element))
                    }
                    _ => None,
                })
            && index != 0
            && index != visible_children.len().saturating_sub(1)
        {
            warnings.push(make_warning(
                source,
                options,
                "a11y_figcaption_index",
                "`<figcaption>` must be first or last child of `<figure>`",
                figcaption.start,
                figcaption.end,
            ));
        }
    }

    for attribute in &element.attributes {
        let Attribute::Attribute(attribute) = attribute else {
            continue;
        };

        let name = attribute.name.as_ref().to_ascii_lowercase();
        if let Some(aria_name) = name.strip_prefix("aria-") {
            if A11Y_INVISIBLE_ELEMENTS.contains(&tag.as_str()) {
                warnings.push(make_warning(
                    source,
                    options,
                    "a11y_aria_attributes",
                    &format!("`<{}>` should not have aria-* attributes", element.name),
                    attribute.start,
                    attribute.end,
                ));
            }

            let property = a11y.properties.get_property(&name);
            if property.is_none() {
                let suggestion = fuzzy_match(aria_name, ARIA_ATTRIBUTE_SUFFIX_SUGGESTIONS);
                let message = if let Some(suggestion) = suggestion {
                    format!(
                        "Unknown aria attribute 'aria-{}'. Did you mean '{}'?",
                        aria_name, suggestion
                    )
                } else {
                    format!("Unknown aria attribute 'aria-{}'", aria_name)
                };

                warnings.push(make_warning(
                    source,
                    options,
                    "a11y_unknown_aria_attribute",
                    &message,
                    attribute.start,
                    attribute.end,
                ));
            }

            if let Some(property) = property
                && let Some(static_value) = attribute_static_value(attribute)
                && let Some((code, message)) =
                    validate_aria_attribute_value(&name, property, &static_value)
            {
                warnings.push(make_warning(
                    source,
                    options,
                    code,
                    &message,
                    attribute.start,
                    attribute.end,
                ));
            }
        }

        if name == "role" {
            if A11Y_INVISIBLE_ELEMENTS.contains(&tag.as_str()) {
                warnings.push(make_warning(
                    source,
                    options,
                    "a11y_misplaced_role",
                    &format!("`<{}>` should not have role attribute", element.name),
                    attribute.start,
                    attribute.end,
                ));
            }

            if let Some(static_role_value) = attribute_static_text(attribute) {
                for role in static_role_value.split_whitespace() {
                    let normalized_role = if role.eq_ignore_ascii_case("none") {
                        "presentation"
                    } else {
                        role
                    };

                    if AriaAbstractRolesEnum::from_str(normalized_role).is_ok() {
                        warnings.push(make_warning(
                            source,
                            options,
                            "a11y_no_abstract_role",
                            &format!("Abstract role '{}' is forbidden", role),
                            attribute.start,
                            attribute.end,
                        ));
                    } else if a11y.role_definition(normalized_role).is_none()
                        && !is_known_role_name(normalized_role)
                    {
                        let suggestion = fuzzy_match(role, ROLE_SUGGESTIONS);
                        let message = if let Some(suggestion) = suggestion
                            && suggestion != role
                        {
                            format!("Unknown role '{}'. Did you mean '{}'?", role, suggestion)
                        } else {
                            format!("Unknown role '{}'", role)
                        };

                        warnings.push(make_warning(
                            source,
                            options,
                            "a11y_unknown_role",
                            &message,
                            attribute.start,
                            attribute.end,
                        ));
                    }

                    if !has_spread
                        && is_interactive_element
                        && (a11y.role_is_non_interactive(normalized_role)
                            || A11Y_PRESENTATION_ROLES.contains(&normalized_role))
                    {
                        warnings.push(make_warning(
                            source,
                            options,
                            "a11y_no_interactive_element_to_noninteractive_role",
                            &format!("`<{}>` cannot have role '{}'", tag, role),
                            element.start,
                            element.end,
                        ));
                    }

                    if !has_spread
                        && is_non_interactive_element
                        && a11y.role_is_interactive(normalized_role)
                        && !a11y
                            .is_noninteractive_to_interactive_role_exception(&tag, normalized_role)
                    {
                        warnings.push(make_warning(
                            source,
                            options,
                            "a11y_no_noninteractive_element_to_interactive_role",
                            &format!(
                                "Non-interactive element `<{}>` cannot have interactive role '{}'",
                                tag, role
                            ),
                            element.start,
                            element.end,
                        ));
                    }

                    if !has_spread
                        && !has_disabled_attribute(element)
                        && !is_hidden_from_screen_reader(element, &tag)
                        && !A11Y_PRESENTATION_ROLES.contains(&normalized_role)
                        && a11y.role_is_interactive(normalized_role)
                        && is_static_element
                        && !has_attribute_present(element, "tabindex")
                        && has_interactive_handlers
                    {
                        warnings.push(make_warning(
                            source,
                            options,
                            "a11y_interactive_supports_focus",
                            &format!(
                                "Elements with the '{}' interactive role must have a tabindex value",
                                role
                            ),
                            element.start,
                            element.end,
                        ));
                    }

                    if let Some(implicit_role) = a11y.redundant_role_implicit_name(element, &tag) {
                        let list_role_exception =
                            matches!(tag.as_str(), "ul" | "ol" | "li" | "menu");
                        let anchor_without_href_exception =
                            tag == "a" && !has_attribute_present(element, "href");
                        if normalized_role == implicit_role
                            && !list_role_exception
                            && !anchor_without_href_exception
                        {
                            warnings.push(make_warning(
                                source,
                                options,
                                "a11y_no_redundant_roles",
                                &format!("Redundant role '{}'", role),
                                attribute.start,
                                attribute.end,
                            ));
                        }
                    }

                    let nested_implicit_role = match tag.as_str() {
                        "header" => Some("banner"),
                        "footer" => Some("contentinfo"),
                        _ => None,
                    };
                    let parent_is_section_or_article = parent_regular_tag.is_some_and(|name| {
                        name.eq_ignore_ascii_case("section") || name.eq_ignore_ascii_case("article")
                    });
                    if nested_implicit_role.is_some_and(|nested| nested == normalized_role)
                        && !parent_is_section_or_article
                    {
                        warnings.push(make_warning(
                            source,
                            options,
                            "a11y_no_redundant_roles",
                            &format!("Redundant role '{}'", role),
                            attribute.start,
                            attribute.end,
                        ));
                    }

                    if !has_spread
                        && tag != "svelte:element"
                        && !is_semantic_role_element(normalized_role, element, &tag)
                    {
                        let required_props = role_required_properties(normalized_role);
                        let missing_required_props = required_props
                            .into_iter()
                            .filter(|property| !has_attribute_present(element, property))
                            .map(|property| format!("\"{}\"", property))
                            .collect::<Vec<_>>();

                        if !missing_required_props.is_empty() {
                            warnings.push(make_warning(
                                source,
                                options,
                                "a11y_role_has_required_aria_props",
                                &format!(
                                    "Elements with the ARIA role \"{}\" must have the following attributes defined: {}",
                                    role,
                                    join_with_conjunction(&missing_required_props, "and")
                                ),
                                attribute.start,
                                attribute.end,
                            ));
                        }
                    }
                }
            }
        }

        if name == "scope" && tag != "svelte:element" && tag != "th" {
            warnings.push(make_warning(
                source,
                options,
                "a11y_misplaced_scope",
                "The scope attribute should only be used with `<th>` elements",
                attribute.start,
                attribute.end,
            ));
        }

        if name == "tabindex"
            && attribute_static_text(attribute)
                .is_some_and(|value| value.parse::<f64>().is_ok_and(|v| v > 0.0))
        {
            warnings.push(make_warning(
                source,
                options,
                "a11y_positive_tabindex",
                "Avoid tabindex values above zero",
                attribute.start,
                attribute.end,
            ));
        }
    }

    let role_attribute = named_attribute_from_element(element, "role");
    let explicit_role_value = role_attribute.and_then(attribute_static_text);
    let explicit_role_for_lookup = explicit_role_value.as_deref().map(str::to_ascii_lowercase);
    let implicit_role_for_lookup = if role_attribute.is_none() {
        a11y.implicit_role_name(element, &tag)
    } else {
        None
    };

    let role_for_aria_support = explicit_role_for_lookup
        .clone()
        .or_else(|| implicit_role_for_lookup.clone());
    if let Some(role_name) = role_for_aria_support.as_deref()
        && let Some(role_key) = query_role_key(role_name)
    {
        for attribute in &element.attributes {
            let Attribute::Attribute(attribute) = attribute else {
                continue;
            };
            let attribute_name = attribute.name.as_ref().to_ascii_lowercase();
            if !attribute_name.starts_with("aria-")
                || a11y.properties.get_property(&attribute_name).is_none()
            {
                continue;
            }

            let supports_attribute = query_property_key(&attribute_name)
                .is_some_and(|property_key| query_role_supports_property(role_key, property_key));
            if supports_attribute {
                continue;
            }

            let (code, message) = if explicit_role_for_lookup.is_some() {
                (
                    "a11y_role_supports_aria_props",
                    format!(
                        "The attribute '{}' is not supported by the role '{}'",
                        attribute_name, role_name
                    ),
                )
            } else {
                (
                    "a11y_role_supports_aria_props_implicit",
                    format!(
                        "The attribute '{}' is not supported by the role '{}'. This role is implicit on the element `<{}>`",
                        attribute_name, role_name, tag
                    ),
                )
            };

            warnings.push(make_warning(
                source,
                options,
                code,
                &message,
                attribute.start,
                attribute.end,
            ));
        }
    }

    if tag != "svelte:element"
        && !is_interactive_element
        && !explicit_role_for_lookup
            .as_deref()
            .is_some_and(|role| a11y.role_is_interactive(role))
        && let Some(tabindex_attribute) = named_attribute_from_element(element, "tabindex")
    {
        let warn_for_tabindex = match attribute_static_text(tabindex_attribute) {
            None => true,
            Some(value) => is_nonnegative_tabindex_value(&value),
        };
        if warn_for_tabindex {
            warnings.push(make_warning(
                source,
                options,
                "a11y_no_noninteractive_tabindex",
                "noninteractive element cannot have nonnegative tabIndex value",
                element.start,
                element.end,
            ));
        }
    }

    if !has_spread
        && !has_contenteditable_attr
        && !is_hidden_from_screen_reader(element, &tag)
        && !explicit_role_for_lookup
            .as_deref()
            .is_some_and(|role| A11Y_PRESENTATION_ROLES.contains(&role))
    {
        let role_is_non_interactive = explicit_role_for_lookup
            .as_deref()
            .is_some_and(|role| a11y.role_is_non_interactive(role));
        if ((!is_interactive_element && role_is_non_interactive)
            || (is_non_interactive_element && explicit_role_for_lookup.is_none()))
            && has_recommended_interactive_handlers
        {
            warnings.push(make_warning(
                source,
                options,
                "a11y_no_noninteractive_element_interactions",
                &format!(
                    "Non-interactive element `<{}>` should not be assigned mouse or keyboard event listeners",
                    tag
                ),
                element.start,
                element.end,
            ));
        }
    }

    if !has_spread
        && (role_attribute.is_none() || explicit_role_for_lookup.is_some())
        && !is_hidden_from_screen_reader(element, &tag)
        && !explicit_role_for_lookup
            .as_deref()
            .is_some_and(|role| A11Y_PRESENTATION_ROLES.contains(&role))
        && !is_interactive_element
        && !explicit_role_for_lookup
            .as_deref()
            .is_some_and(|role| a11y.role_is_interactive(role))
        && !is_non_interactive_element
        && !explicit_role_for_lookup
            .as_deref()
            .is_some_and(|role| a11y.role_is_non_interactive(role))
        && explicit_role_for_lookup
            .as_deref()
            .is_none_or(|role| AriaAbstractRolesEnum::from_str(role).is_err())
    {
        let interactive_handlers = collect_present_interactive_handlers(element);
        if !interactive_handlers.is_empty() {
            warnings.push(make_warning(
                source,
                options,
                "a11y_no_static_element_interactions",
                &format!(
                    "`<{}>` with a {} handler must have an ARIA role",
                    tag,
                    join_with_conjunction(&interactive_handlers, "or")
                ),
                element.start,
                element.end,
            ));
        }
    }

    if has_click_handler {
        let role_is_non_presentation = role_attribute
            .and_then(attribute_static_text)
            .is_some_and(|role| !A11Y_PRESENTATION_ROLES.contains(&role.as_str()));

        if tag != "svelte:element"
            && !is_hidden_from_screen_reader(element, &tag)
            && (role_attribute.is_none() || role_is_non_presentation)
            && !is_interactive_element
            && !has_spread
            && !has_keyboard_handler
        {
            warnings.push(make_warning(
                source,
                options,
                "a11y_click_events_have_key_events",
                "Visible, non-interactive elements with a click event must be accompanied by a keyboard event handler. Consider whether an interactive element such as `<button type=\"button\">` or `<a>` might be more appropriate",
                element.start,
                element.end,
            ));
        }
    }

    if !has_spread && has_mouse_over_handler && !has_focus_handler {
        warnings.push(make_warning(
            source,
            options,
            "a11y_mouse_events_have_key_events",
            "'mouseover' event must be accompanied by 'focus' event",
            element.start,
            element.end,
        ));
    }

    if !has_spread && has_mouse_out_handler && !has_blur_handler {
        warnings.push(make_warning(
            source,
            options,
            "a11y_mouse_events_have_key_events",
            "'mouseout' event must be accompanied by 'blur' event",
            element.start,
            element.end,
        ));
    }

    if tag == "svelte:self" {
        warnings.push(make_warning(
            source,
            options,
            "svelte_self_deprecated",
            "`<svelte:self>` is deprecated — use self-imports (e.g. `import Self from './Self.svelte'`) instead",
            element.start,
            element.end,
        ));
    }

    if tag == "html" && !has_attribute_present(element, "lang") {
        warnings.push(make_warning(
            source,
            options,
            "a11y_missing_attribute",
            "`<html>` element should have a lang attribute",
            element.start,
            element.end,
        ));
    }

    if tag == "img" && !has_attribute_present(element, "alt") {
        let end = opening_tag_end_from_ast(element);
        warnings.push(make_warning(
            source,
            options,
            "a11y_missing_attribute",
            "`<img>` element should have an alt attribute",
            element.start,
            end,
        ));
    }

    if tag == "area"
        && !has_attribute_present(element, "alt")
        && !has_attribute_present(element, "aria-label")
        && !has_attribute_present(element, "aria-labelledby")
    {
        let end = opening_tag_end_from_ast(element);
        warnings.push(make_warning(
            source,
            options,
            "a11y_missing_attribute",
            "`<area>` element should have an alt, aria-label or aria-labelledby attribute",
            element.start,
            end,
        ));
    }

    if tag == "object"
        && !has_attribute_present(element, "title")
        && !has_attribute_present(element, "aria-label")
        && !has_attribute_present(element, "aria-labelledby")
    {
        warnings.push(make_warning(
            source,
            options,
            "a11y_missing_attribute",
            "`<object>` element should have a title, aria-label or aria-labelledby attribute",
            element.start,
            element.end,
        ));
    }

    if tag == "input"
        && attribute_value_equals_ascii_ci(element, "type", "image")
        && !has_attribute_present(element, "alt")
        && !has_attribute_present(element, "aria-label")
        && !has_attribute_present(element, "aria-labelledby")
    {
        let end = opening_tag_end_from_ast(element);
        warnings.push(make_warning(
            source,
            options,
            "a11y_missing_attribute",
            "`<input type=\"image\">` element should have an alt, aria-label or aria-labelledby attribute",
            element.start,
            end,
        ));
    }

    if matches!(tag.as_str(), "marquee" | "blink") {
        warnings.push(make_warning(
            source,
            options,
            "a11y_distracting_elements",
            &format!("Avoid `<{}>` elements", tag),
            element.start,
            element.end,
        ));
    }

    if is_heading_tag(&tag) && !fragment_has_accessible_content(&element.fragment) {
        warnings.push(make_warning(
            source,
            options,
            "a11y_missing_content",
            &format!("`<{}>` element should contain text", element.name),
            element.start,
            element.end,
        ));
    }

    if is_heading_tag(&tag)
        && let Some(attribute) =
            named_attribute_value_equals_ascii_ci(element, "aria-hidden", "true")
    {
        warnings.push(make_warning(
            source,
            options,
            "a11y_hidden",
            &format!("`<{}>` element should not be hidden", element.name),
            attribute.start,
            attribute.end,
        ));
    }

    if tag == "iframe" && !has_attribute_present(element, "title") {
        warnings.push(make_warning(
            source,
            options,
            "a11y_missing_attribute",
            "`<iframe>` element should have a title attribute",
            element.start,
            element.end,
        ));
    }

    if tag == "img"
        && !attribute_value_equals_ascii_ci(element, "aria-hidden", "true")
        && attribute_text_value_from_element(element, "alt")
            .is_some_and(|alt| contains_redundant_image_word(&alt))
    {
        warnings.push(make_warning(
            source,
            options,
            "a11y_img_redundant_alt",
            "Screenreaders already announce `<img>` elements as an image",
            element.start,
            element.end,
        ));
    }

    if tag == "a" {
        if let Some((attribute_name, attribute)) = anchor_href_attribute(source, element) {
            if let Some(value) = attribute_text_value(attribute) {
                let trimmed = value.trim();
                if trimmed.is_empty()
                    || trimmed == "#"
                    || trimmed.eq_ignore_ascii_case("javascript:void(0)")
                {
                    warnings.push(make_warning(
                        source,
                        options,
                        "a11y_invalid_attribute",
                        &format!("'{}' is not a valid {attribute_name} attribute", trimmed),
                        attribute.start,
                        attribute.end,
                    ));
                }
            }
        } else if !has_non_empty_anchor_fragment_target(element)
            && !attribute_value_equals_ascii_ci(element, "aria-disabled", "true")
        {
            warnings.push(make_warning(
                source,
                options,
                "a11y_missing_attribute",
                "`<a>` element should have an href attribute",
                element.start,
                element.end,
            ));
        }
    }

    if tag == "a"
        && has_attribute_present(element, "href")
        && !fragment_has_accessible_content(&element.fragment)
        && !has_attribute_present(element, "aria-label")
        && !has_attribute_present(element, "aria-labelledby")
        && !has_attribute_present(element, "title")
        && !attribute_value_equals_ascii_ci(element, "aria-hidden", "true")
        && !has_attribute_present(element, "inert")
    {
        warnings.push(make_warning(
            source,
            options,
            "a11y_consider_explicit_label",
            "Buttons and links should either contain text or have an `aria-label`, `aria-labelledby` or `title` attribute",
            element.start,
            element.end,
        ));
    }

    if tag == "button"
        && !fragment_has_accessible_content(&element.fragment)
        && !has_attribute_present(element, "aria-label")
        && !has_attribute_present(element, "aria-labelledby")
        && !has_attribute_present(element, "title")
        && !attribute_value_equals_ascii_ci(element, "aria-hidden", "true")
        && !has_attribute_present(element, "inert")
    {
        warnings.push(make_warning(
            source,
            options,
            "a11y_consider_explicit_label",
            "Buttons and links should either contain text or have an `aria-label`, `aria-labelledby` or `title` attribute",
            element.start,
            element.end,
        ));
    }

    if tag == "label"
        && !has_spread
        && !has_attribute_present(element, "for")
        && !label_has_associated_control_in_fragment(&element.fragment)
    {
        warnings.push(make_warning(
            source,
            options,
            "a11y_label_has_associated_control",
            "A form label must be associated with a control",
            element.start,
            element.end,
        ));
    }

    if tag == "video"
        && !has_attribute_present(element, "muted")
        && !attribute_value_equals_ascii_ci(element, "aria-hidden", "true")
        && !has_spread
        && has_attribute_present(element, "src")
    {
        let mut has_caption_track = false;
        for child in &element.fragment.nodes {
            let Node::RegularElement(child_element) = child else {
                continue;
            };
            if !child_element.name.as_ref().eq_ignore_ascii_case("track") {
                continue;
            }

            has_caption_track = child_element
                .attributes
                .iter()
                .any(|attribute| match attribute {
                    Attribute::SpreadAttribute(_) => true,
                    Attribute::Attribute(attribute)
                        if attribute.name.as_ref().eq_ignore_ascii_case("kind") =>
                    {
                        attribute_static_text(attribute)
                            .is_some_and(|value| value.eq_ignore_ascii_case("captions"))
                    }
                    _ => false,
                });
            break;
        }

        if !has_caption_track {
            warnings.push(make_warning(
                source,
                options,
                "a11y_media_has_caption",
                "`<video>` elements must have a `<track kind=\"captions\">`",
                element.start,
                element.end,
            ));
        }
    }

    if tag == "input"
        && let Some(autocomplete_attribute) = named_attribute_from_element(element, "autocomplete")
        && named_attribute_from_element(element, "type").is_some()
    {
        let autocomplete_value = attribute_static_value(autocomplete_attribute);
        if !is_valid_autocomplete(autocomplete_value.as_ref()) {
            let invalid_value = autocomplete_value
                .as_ref()
                .map(static_value_for_message)
                .unwrap_or_else(|| "...".to_string());
            let input_type = named_attribute_from_element(element, "type")
                .and_then(attribute_static_text)
                .unwrap_or_else(|| "...".to_string());

            warnings.push(make_warning(
                source,
                options,
                "a11y_autocomplete_valid",
                &format!(
                    "'{}' is an invalid value for 'autocomplete' on `<input type=\"{}\">`",
                    invalid_value, input_type
                ),
                autocomplete_attribute.start,
                autocomplete_attribute.end,
            ));
        }
    }

    filter_recent_ignored_warnings(warnings, warning_start, active_ignores);
}

fn collect_component_attribute_warnings(
    source: &str,
    options: &CompileOptions,
    attributes: &[Attribute],
    runes_mode: bool,
    each_rest_bindings: &[RestBindingWarning],
    warnings: &mut Vec<Warning>,
) {
    for attribute in attributes {
        let Attribute::Attribute(attribute) = attribute else {
            continue;
        };
        let name = attribute.name.as_ref();
        if name.contains(':')
            && !name.starts_with("xmlns:")
            && !name.starts_with("xlink:")
            && !name.starts_with("xml:")
        {
            warnings.push(make_warning(
                source,
                options,
                "attribute_illegal_colon",
                "Attributes should not contain ':' characters to prevent ambiguity with Svelte directives",
                attribute.start,
                attribute.end,
            ));
        }
    }

    if runes_mode {
        for attribute in attributes {
            let Attribute::Attribute(attribute) = attribute else {
                continue;
            };
            if !attribute_is_quoted_expression(attribute) {
                continue;
            }
            warnings.push(make_warning(
                source,
                options,
                "attribute_quoted",
                "Quoted attributes on components and custom elements will be stringified in a future version of Svelte. If this isn't what you want, remove the quotes",
                attribute.start,
                attribute.end,
            ));
        }
    }

    for attribute in attributes {
        let Attribute::BindDirective(bind) = attribute else {
            continue;
        };
        let Some(binding_name) = binding_base_identifier_name(&bind.expression) else {
            continue;
        };
        for rest_binding in each_rest_bindings {
            if rest_binding.name.as_ref() != binding_name.as_ref() {
                continue;
            }
            warnings.push(make_warning(
                source,
                options,
                "bind_invalid_each_rest",
                &format!(
                    "The rest operator (...) will create a new object and binding '{}' with the original object will not work",
                    binding_name
                ),
                rest_binding.start,
                rest_binding.end,
            ));
        }
    }
}

fn emit_script_estree_warnings(
    source: &str,
    options: &CompileOptions,
    script: &crate::ast::modern::Script,
    runes_mode: bool,
    warnings: &mut Vec<Warning>,
) {
    let script_context = ScriptWalkContext {
        function_depth: if script.context == crate::ast::modern::ScriptContext::Module {
            0
        } else {
            1
        },
        is_module: script.context == crate::ast::modern::ScriptContext::Module,
    };
    emit_program_estree_warnings(
        source,
        options,
        &script.content,
        script_context,
        runes_mode,
        script.content_start,
        warnings,
    );
}

/// Scan OXC program comments for `// svelte-ignore` directives.
/// Returns a map from `attached_to` (the OXC byte offset of the statement the
/// comment is attached to) → boxed slice of ignored warning codes.
fn collect_script_svelte_ignores(
    program: &ParsedJsProgram,
    runes_mode: bool,
) -> FxHashMap<u32, Box<[Arc<str>]>> {
    let mut out = FxHashMap::<u32, Box<[Arc<str>]>>::default();
    let source = program.source();
    for comment in &program.program().comments {
        let text = source
            .get(comment.span.start as usize..comment.span.end as usize)
            .unwrap_or_default();
        // Strip comment delimiters
        let data = match comment.kind {
            oxc_ast::ast::CommentKind::Line => text.strip_prefix("//").unwrap_or(text),
            _ => text
                .strip_prefix("/*")
                .and_then(|s| s.strip_suffix("*/"))
                .unwrap_or(text),
        };
        let parsed = parse_svelte_ignore_directive(0, data, runes_mode);
        if !parsed.ignores.is_empty() {
            out.entry(comment.attached_to)
                .or_insert_with(|| parsed.ignores);
        }
    }
    out
}

fn emit_program_estree_warnings(
    source: &str,
    options: &CompileOptions,
    program: &ParsedJsProgram,
    script_context: ScriptWalkContext,
    runes_mode: bool,
    base_offset: usize,
    warnings: &mut Vec<Warning>,
) {
    let imported_default_svelte_components = collect_default_svelte_imports(program);
    let script_ignores = collect_script_svelte_ignores(program, runes_mode);
    struct ProgramWarningVisitor<'a> {
        source: &'a str,
        options: &'a CompileOptions,
        runes_mode: bool,
        imported_default_svelte_components: &'a NameSet,
        warnings: &'a mut Vec<Warning>,
        function_depth: usize,
        is_module: bool,
        base_offset: usize,
        script_ignores: &'a FxHashMap<u32, Box<[Arc<str>]>>,
    }

    impl ProgramWarningVisitor<'_> {
        fn is_ignored(&self, statement_start: u32, code: &str) -> bool {
            self.script_ignores
                .get(&statement_start)
                .is_some_and(|codes| codes.iter().any(|c| c.as_ref() == code))
        }
    }

    impl<'a> Visit<'a> for ProgramWarningVisitor<'_> {
        fn visit_statement(&mut self, statement: &Statement<'a>) {
            let stmt_start = statement.span().start;
            match statement {
                Statement::ClassDeclaration(declaration) => {
                    let allowed_depth = if self.is_module { 0 } else { 1 };
                    if self.function_depth > allowed_depth
                        && !self.is_ignored(stmt_start, "perf_avoid_nested_class")
                    {
                        let span = declaration.span();
                        self.warnings.push(make_warning(
                            self.source,
                            self.options,
                            "perf_avoid_nested_class",
                            "Avoid declaring classes below the top level scope",
                            span.start as usize + self.base_offset,
                            span.end as usize + self.base_offset,
                        ));
                    }
                }
                Statement::ExpressionStatement(statement) => {
                    if expression_is_legacy_component_creation(
                        &statement.expression,
                        self.imported_default_svelte_components,
                    ) && !self.is_ignored(stmt_start, "legacy_component_creation")
                    {
                        let span = statement.expression.span();
                        self.warnings.push(make_warning(
                            self.source,
                            self.options,
                            "legacy_component_creation",
                            "Svelte 5 components are no longer classes. Instantiate them using `mount` or `hydrate` (imported from 'svelte') instead.",
                            span.start as usize + self.base_offset,
                            span.end as usize + self.base_offset,
                        ));
                    }
                }
                Statement::LabeledStatement(statement) => {
                    if !self.runes_mode
                        && statement.label.name.as_str() == "$"
                        && (self.is_module || self.function_depth > 1)
                        && !self.is_ignored(stmt_start, "reactive_declaration_invalid_placement")
                    {
                        let span = statement.span();
                        self.warnings.push(make_warning(
                            self.source,
                            self.options,
                            "reactive_declaration_invalid_placement",
                            "Reactive declarations only exist at the top level of the instance script",
                            span.start as usize + self.base_offset,
                            span.end as usize + self.base_offset,
                        ));
                    }
                }
                _ => {}
            }

            walk::walk_statement(self, statement);
        }

        fn visit_new_expression(&mut self, expression: &oxc_ast::ast::NewExpression<'a>) {
            if self.function_depth > 0
                && matches!(expression.callee, OxcExpression::ClassExpression(_))
            {
                let span = expression.span();
                self.warnings.push(make_warning(
                    self.source,
                    self.options,
                    "perf_avoid_inline_class",
                    "Avoid 'new class' — instead, declare the class at the top level scope",
                    span.start as usize + self.base_offset,
                    span.end as usize + self.base_offset,
                ));
            }
            walk::walk_new_expression(self, expression);
        }

        fn visit_arrow_function_expression(
            &mut self,
            expression: &oxc_ast::ast::ArrowFunctionExpression<'a>,
        ) {
            self.function_depth = self.function_depth.saturating_add(1);
            walk::walk_arrow_function_expression(self, expression);
            self.function_depth = self.function_depth.saturating_sub(1);
        }

        fn visit_function_body(&mut self, body: &oxc_ast::ast::FunctionBody<'a>) {
            self.function_depth = self.function_depth.saturating_add(1);
            walk::walk_function_body(self, body);
            self.function_depth = self.function_depth.saturating_sub(1);
        }

        fn visit_string_literal(&mut self, literal: &oxc_ast::ast::StringLiteral<'a>) {
            if string_contains_bidirectional_controls(literal.value.as_str()) {
                let span = literal.span();
                self.warnings.push(make_warning(
                    self.source,
                    self.options,
                    "bidirectional_control_characters",
                    "A bidirectional control character was detected in your code. These characters can be used to alter the visual direction of your code and could have unintended consequences",
                    span.start as usize + self.base_offset,
                    span.end as usize + self.base_offset,
                ));
            }
        }
    }

    let mut visitor = ProgramWarningVisitor {
        source,
        options,
        runes_mode,
        imported_default_svelte_components: &imported_default_svelte_components,
        warnings,
        function_depth: script_context.function_depth,
        is_module: script_context.is_module,
        base_offset,
        script_ignores: &script_ignores,
    };
    visitor.visit_program(program.program());
}

fn expression_is_legacy_component_creation(
    expression: &OxcExpression<'_>,
    imported_default_svelte_components: &NameSet,
) -> bool {
    let OxcExpression::NewExpression(expression) = expression else {
        return false;
    };
    let Some(callee_name) = expression
        .callee
        .get_identifier_reference()
        .map(|identifier| identifier.name.as_str())
    else {
        return false;
    };
    if !imported_default_svelte_components.contains(callee_name) {
        return false;
    }

    if expression.arguments.len() != 1 {
        return false;
    }

    expression.arguments[0]
        .as_expression()
        .is_some_and(|argument| object_expression_has_identifier_property(argument, "target"))
}

fn is_reactive_labeled_statement(node: &Statement<'_>) -> bool {
    matches!(
        node,
        Statement::LabeledStatement(statement)
            if statement.label.name.as_str() == "$"
    )
}

fn emit_reactive_module_script_dependency_warnings(
    source: &str,
    options: &CompileOptions,
    root: &Root,
    warnings: &mut Vec<Warning>,
) {
    let Some(module_script) = root_scripts(root)
        .into_iter()
        .find(|script| script.context == crate::ast::modern::ScriptContext::Module)
    else {
        return;
    };
    let Some(instance_script) = root_scripts(root)
        .into_iter()
        .find(|script| script.context != crate::ast::modern::ScriptContext::Module)
    else {
        return;
    };

    let module_declared = collect_declared_names_in_program(&module_script.content);
    if module_declared.is_empty() {
        return;
    }
    let module_reassigned = collect_reassigned_identifier_names(&module_script.content);
    let reassigned_module_bindings = module_declared
        .into_iter()
        .filter(|name| module_reassigned.contains(name))
        .collect::<FxHashSet<_>>();
    if reassigned_module_bindings.is_empty() {
        return;
    }

    let instance_offset = instance_script.content_start;
    for statement in &instance_script.content.program().body {
        if !is_reactive_labeled_statement(statement) {
            continue;
        }
        let Statement::LabeledStatement(labeled) = statement else {
            continue;
        };
        struct ReactiveDependencyVisitor<'a> {
            names: &'a FxHashSet<Arc<str>>,
            warnings: &'a mut Vec<Warning>,
            source: &'a str,
            options: &'a CompileOptions,
            base_offset: usize,
        }

        impl<'a> Visit<'a> for ReactiveDependencyVisitor<'_> {
            fn visit_identifier_reference(&mut self, identifier: &IdentifierReference<'a>) {
                if !self.names.contains(identifier.name.as_str()) {
                    return;
                }
                let span = identifier.span();
                self.warnings.push(make_warning(
                    self.source,
                    self.options,
                    "reactive_declaration_module_script_dependency",
                    "Reassignments of module-level declarations will not cause reactive statements to update",
                    span.start as usize + self.base_offset,
                    span.end as usize + self.base_offset,
                ));
            }
        }

        let mut visitor = ReactiveDependencyVisitor {
            names: &reassigned_module_bindings,
            warnings,
            source,
            options,
            base_offset: instance_offset,
        };
        visitor.visit_statement(&labeled.body);
    }
}

fn collect_declared_names_in_program(program: &ParsedJsProgram) -> NameSet {
    let mut names = FxHashSet::<Arc<str>>::default();
    for statement in &program.program().body {
        match statement {
            Statement::VariableDeclaration(declaration) => {
                collect_declared_names_from_oxc_variable_declaration(declaration, &mut names);
            }
            Statement::FunctionDeclaration(declaration) => {
                if let Some(id) = declaration.id.as_ref() {
                    names.insert(Arc::from(id.name.as_str()));
                }
            }
            Statement::ClassDeclaration(declaration) => {
                if let Some(id) = declaration.id.as_ref() {
                    names.insert(Arc::from(id.name.as_str()));
                }
            }
            Statement::ExportNamedDeclaration(declaration) => {
                if let Some(declaration) = declaration.declaration.as_ref() {
                    match declaration {
                        Declaration::VariableDeclaration(declaration) => {
                            collect_declared_names_from_oxc_variable_declaration(
                                declaration,
                                &mut names,
                            );
                        }
                        Declaration::FunctionDeclaration(declaration) => {
                            if let Some(id) = declaration.id.as_ref() {
                                names.insert(Arc::from(id.name.as_str()));
                            }
                        }
                        Declaration::ClassDeclaration(declaration) => {
                            if let Some(id) = declaration.id.as_ref() {
                                names.insert(Arc::from(id.name.as_str()));
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    names
}

fn collect_reassigned_identifier_names(program: &ParsedJsProgram) -> NameSet {
    let mut names = FxHashSet::<Arc<str>>::default();
    struct ReassignedNamesVisitor<'a> {
        names: &'a mut NameSet,
    }
    impl<'a> Visit<'a> for ReassignedNamesVisitor<'_> {
        fn visit_assignment_target(&mut self, target: &AssignmentTarget<'a>) {
            if let AssignmentTarget::AssignmentTargetIdentifier(identifier) = target {
                self.names.insert(Arc::from(identifier.name.as_str()));
            }
            walk::walk_assignment_target(self, target);
        }

        fn visit_simple_assignment_target(
            &mut self,
            target: &oxc_ast::ast::SimpleAssignmentTarget<'a>,
        ) {
            if let oxc_ast::ast::SimpleAssignmentTarget::AssignmentTargetIdentifier(identifier) =
                target
            {
                self.names.insert(Arc::from(identifier.name.as_str()));
            }
            walk::walk_simple_assignment_target(self, target);
        }

        fn visit_update_expression(&mut self, expression: &oxc_ast::ast::UpdateExpression<'a>) {
            if let oxc_ast::ast::SimpleAssignmentTarget::AssignmentTargetIdentifier(identifier) =
                &expression.argument
            {
                self.names.insert(Arc::from(identifier.name.as_str()));
            }
            walk::walk_update_expression(self, expression);
        }
    }
    let mut visitor = ReassignedNamesVisitor { names: &mut names };
    visitor.visit_program(program.program());
    names
}

fn emit_store_rune_conflict_warnings(
    source: &str,
    options: &CompileOptions,
    root: &Root,
    warnings: &mut Vec<Warning>,
) {
    let declared = collect_script_declared_names(root);
    if declared.is_empty() {
        return;
    }

    for script in root_scripts(root) {
        let base_offset = script.content_start;
        struct StoreRuneConflictVisitor<'a> {
            declared: &'a NameSet,
            warnings: &'a mut Vec<Warning>,
            source: &'a str,
            options: &'a CompileOptions,
            base_offset: usize,
        }

        impl<'a> Visit<'a> for StoreRuneConflictVisitor<'_> {
            fn visit_call_expression(&mut self, expression: &oxc_ast::ast::CallExpression<'a>) {
                let Some(identifier) = expression.callee.get_identifier_reference() else {
                    return;
                };
                let name = identifier.name.as_str();
                let Some(alias) = name.strip_prefix('$') else {
                    return;
                };
                if alias.is_empty() || !is_known_rune_name(name) || !self.declared.contains(alias) {
                    return;
                }
                let span = identifier.span();
                self.warnings.push(make_warning(
                    self.source,
                    self.options,
                    "store_rune_conflict",
                    &format!(
                        "It looks like you're using the `${}` rune, but there is a local binding called `{}`. Referencing a local variable with a `$` prefix will create a store subscription. Please rename `{}` to avoid the ambiguity",
                        alias, alias, alias
                    ),
                    span.start as usize + self.base_offset,
                    span.end as usize + self.base_offset,
                ));
            }
        }

        let mut visitor = StoreRuneConflictVisitor {
            declared: &declared,
            warnings,
            source,
            options,
            base_offset,
        };
        visitor.visit_program(script.content.program());
    }
}

fn is_known_rune_name(name: &str) -> bool {
    matches!(
        name,
        "$state"
            | "$state.raw"
            | "$state.snapshot"
            | "$derived"
            | "$derived.by"
            | "$effect"
            | "$effect.pre"
            | "$effect.root"
            | "$effect.tracking"
            | "$inspect"
            | "$inspect.trace"
            | "$host"
    )
}

fn emit_non_reactive_update_warnings(
    source: &str,
    options: &CompileOptions,
    root: &Root,
    warnings: &mut Vec<Warning>,
) {
    let candidates = collect_reassigned_normal_bindings(root, true);
    if candidates.is_empty() {
        return;
    }
    let bindings = collect_instance_bindings(root, true);

    let candidate_names = candidates.keys().cloned().collect::<FxHashSet<_>>();
    let mut referenced = FxHashSet::<Arc<str>>::default();
    collect_non_reactive_template_references(&root.fragment, 0, &candidate_names, &mut referenced);

    for (name, (start, end)) in candidates {
        if !referenced.contains(name.as_ref()) {
            continue;
        }
        if let Some(binding) = bindings.get(&name)
            && warning_is_ignored("non_reactive_update", &binding.ignore_codes)
        {
            continue;
        }
        warnings.push(make_warning(
            source,
            options,
            "non_reactive_update",
            &format!(
                "`{}` is updated, but is not declared with `$state(...)`. Changing its value will not correctly trigger updates",
                name
            ),
            start,
            end,
        ));
    }
}

fn collect_reassigned_normal_bindings(
    root: &Root,
    runes_mode: bool,
) -> FxHashMap<Arc<str>, (usize, usize)> {
    let mut bindings = collect_instance_bindings(root, runes_mode)
        .into_iter()
        .filter_map(|(name, info)| {
            (info.kind == InstanceBindingKind::Normal).then_some((name, (info.start, info.end)))
        })
        .collect::<FxHashMap<_, _>>();

    if bindings.is_empty() {
        return bindings;
    }

    let mut reassigned = FxHashSet::<Arc<str>>::default();
    if let Some(instance_script) = instance_script(root) {
        reassigned.extend(collect_reassigned_identifier_names(
            &instance_script.content,
        ));
    }
    collect_template_reassigned_names(&root.fragment, &mut reassigned);

    bindings.retain(|name, _| reassigned.contains(name.as_ref()));
    bindings
}

fn instance_script(root: &Root) -> Option<&crate::ast::modern::Script> {
    if let Some(instance) = root.instance.as_ref() {
        return Some(instance);
    }
    root_scripts(root)
        .into_iter()
        .find(|script| script.context != crate::ast::modern::ScriptContext::Module)
}

fn collect_instance_bindings(
    root: &Root,
    _runes_mode: bool,
) -> FxHashMap<Arc<str>, InstanceBindingInfo> {
    let Some(instance_script) = instance_script(root) else {
        return FxHashMap::default();
    };
    let base_offset = instance_script.content_start;
    let script_ignores = collect_script_svelte_ignores(&instance_script.content, true);
    let mut out = FxHashMap::<Arc<str>, InstanceBindingInfo>::default();

    for statement in &instance_script.content.program().body {
        let stmt_start = statement.span().start;
        let declarations = match statement {
            Statement::VariableDeclaration(d) => Some(d.as_ref()),
            Statement::ExportNamedDeclaration(e) => {
                if let Some(Declaration::VariableDeclaration(d)) = e.declaration.as_ref() {
                    Some(d.as_ref())
                } else {
                    None
                }
            }
            _ => None,
        };
        let Some(declarations) = declarations else {
            continue;
        };
        let ignore_codes = script_ignores
            .get(&stmt_start)
            .cloned()
            .unwrap_or_default();
        for declarator in &declarations.declarations {
            let (kind, state_argument_proxyable) =
                classify_declarator_init(declarator.init.as_ref());
            let mut bindings = Vec::<PatternBinding>::new();
            collect_pattern_bindings_from_oxc(&declarator.id, &mut bindings);
            for binding in bindings {
                let info_kind = if binding.is_rest && kind == InstanceBindingKind::Prop {
                    InstanceBindingKind::RestProp
                } else {
                    kind
                };
                out.insert(
                    binding.name,
                    InstanceBindingInfo {
                        kind: info_kind,
                        start: binding.start + base_offset,
                        end: binding.end + base_offset,
                        state_argument_proxyable,
                        ignore_codes: ignore_codes.clone(),
                    },
                );
            }
        }
    }
    out
}

fn classify_declarator_init(
    init: Option<&OxcExpression<'_>>,
) -> (InstanceBindingKind, bool) {
    let Some(init) = init else {
        return (InstanceBindingKind::Normal, false);
    };
    let OxcExpression::CallExpression(call) = init else {
        return (InstanceBindingKind::Normal, false);
    };
    let callee_name = call_expression_callee_name(call);
    match callee_name.as_deref() {
        Some("$state") => {
            let proxyable = call
                .arguments
                .first()
                .map(|arg| argument_is_proxyable(arg))
                .unwrap_or(true);
            (InstanceBindingKind::State, proxyable)
        }
        Some("$state.raw") => (InstanceBindingKind::RawState, false),
        Some("$derived" | "$derived.by") => (InstanceBindingKind::Derived, false),
        Some("$props") => (InstanceBindingKind::Prop, false),
        _ => (InstanceBindingKind::Normal, false),
    }
}

fn call_expression_callee_name(call: &oxc_ast::ast::CallExpression<'_>) -> Option<String> {
    match &call.callee {
        OxcExpression::Identifier(id) => Some(id.name.to_string()),
        OxcExpression::StaticMemberExpression(member) => {
            let obj_name = match &member.object {
                OxcExpression::Identifier(id) => id.name.as_str(),
                _ => return None,
            };
            Some(format!("{}.{}", obj_name, member.property.name))
        }
        _ => None,
    }
}

fn argument_is_proxyable(arg: &oxc_ast::ast::Argument<'_>) -> bool {
    match arg {
        oxc_ast::ast::Argument::ObjectExpression(_)
        | oxc_ast::ast::Argument::ArrayExpression(_) => true,
        _ => false,
    }
}

fn collect_template_reassigned_names(fragment: &Fragment, out: &mut NameSet) {
    for node in fragment.nodes.iter() {
        collect_template_reassigned_in_node(node, out);
        if let Some(error_branches) = template_reassigned_alternate(node) {
            collect_template_reassigned_in_alternate(error_branches, out);
            continue;
        }
        node.for_each_child_fragment(|child| collect_template_reassigned_names(child, out));
    }
}

fn collect_template_reassigned_in_node(node: &Node, out: &mut NameSet) {
    match node {
        Node::Text(_) | Node::Comment(_) | Node::DebugTag(_) | Node::SnippetBlock(_) => {}
        Node::ExpressionTag(tag) => {
            collect_template_reassigned_from_expression(&tag.expression, out)
        }
        Node::RenderTag(tag) => collect_template_reassigned_from_expression(&tag.expression, out),
        Node::HtmlTag(tag) => collect_template_reassigned_from_expression(&tag.expression, out),
        Node::ConstTag(tag) => collect_template_reassigned_from_expression(&tag.declaration, out),
        Node::IfBlock(block) => collect_template_reassigned_from_expression(&block.test, out),
        Node::EachBlock(block) => {
            collect_template_reassigned_from_expression(&block.expression, out);
            if let Some(key) = block.key.as_ref() {
                collect_template_reassigned_from_expression(key, out);
            }
        }
        Node::KeyBlock(block) => {
            collect_template_reassigned_from_expression(&block.expression, out);
        }
        Node::AwaitBlock(block) => {
            collect_template_reassigned_from_expression(&block.expression, out);
            if let Some(value) = block.value.as_ref() {
                collect_template_reassigned_from_expression(value, out);
            }
            if let Some(error) = block.error.as_ref() {
                collect_template_reassigned_from_expression(error, out);
            }
        }
        _ => {
            if let Some(element) = node.as_element() {
                collect_template_reassigned_from_attributes(element.attributes(), out);
            }
        }
    }
}

fn template_reassigned_alternate(node: &Node) -> Option<&Alternate> {
    match node {
        Node::IfBlock(block) => block.alternate.as_deref(),
        _ => None,
    }
}

fn collect_template_reassigned_in_alternate(alternate: &Alternate, out: &mut NameSet) {
    match alternate {
        Alternate::Fragment(fragment) => collect_template_reassigned_names(fragment, out),
        Alternate::IfBlock(block) => {
            collect_template_reassigned_from_expression(&block.test, out);
            collect_template_reassigned_names(&block.consequent, out);
            if let Some(alternate) = block.alternate.as_deref() {
                collect_template_reassigned_in_alternate(alternate, out);
            }
        }
    }
}

fn collect_template_reassigned_from_attributes(attributes: &[Attribute], out: &mut NameSet) {
    for attribute in attributes {
        match attribute {
            Attribute::Attribute(attribute) => match &attribute.value {
                AttributeValueList::Boolean(_) => {}
                AttributeValueList::ExpressionTag(tag) => {
                    collect_template_reassigned_from_expression(&tag.expression, out);
                }
                AttributeValueList::Values(values) => {
                    for value in values.iter() {
                        if let AttributeValue::ExpressionTag(tag) = value {
                            collect_template_reassigned_from_expression(&tag.expression, out);
                        }
                    }
                }
            },
            Attribute::BindDirective(directive) => {
                if directive.name.as_ref() == "this"
                    && let Some(name) = binding_base_identifier_name(&directive.expression)
                {
                    out.insert(name);
                }
                collect_template_reassigned_from_expression(&directive.expression, out);
            }
            Attribute::OnDirective(directive)
            | Attribute::ClassDirective(directive)
            | Attribute::LetDirective(directive)
            | Attribute::AnimateDirective(directive)
            | Attribute::UseDirective(directive) => {
                collect_template_reassigned_from_expression(&directive.expression, out);
            }
            Attribute::TransitionDirective(directive) => {
                collect_template_reassigned_from_expression(&directive.expression, out);
            }
            Attribute::AttachTag(tag) => {
                collect_template_reassigned_from_expression(&tag.expression, out);
            }
            Attribute::StyleDirective(style) => match &style.value {
                AttributeValueList::Boolean(_) => {}
                AttributeValueList::ExpressionTag(tag) => {
                    collect_template_reassigned_from_expression(&tag.expression, out);
                }
                AttributeValueList::Values(values) => {
                    for value in values.iter() {
                        if let AttributeValue::ExpressionTag(tag) = value {
                            collect_template_reassigned_from_expression(&tag.expression, out);
                        }
                    }
                }
            },
            Attribute::SpreadAttribute(spread) => {
                collect_template_reassigned_from_expression(&spread.expression, out);
            }
        }
    }
}

fn collect_template_reassigned_from_expression(
    expression: &crate::ast::modern::Expression,
    out: &mut NameSet,
) {
    struct ReassignedTemplateVisitor<'a> {
        out: &'a mut NameSet,
    }

    impl<'a> Visit<'a> for ReassignedTemplateVisitor<'_> {
        fn visit_assignment_target(&mut self, target: &AssignmentTarget<'a>) {
            if let AssignmentTarget::AssignmentTargetIdentifier(identifier) = target {
                self.out.insert(Arc::from(identifier.name.as_str()));
            }
            walk::walk_assignment_target(self, target);
        }

        fn visit_simple_assignment_target(
            &mut self,
            target: &oxc_ast::ast::SimpleAssignmentTarget<'a>,
        ) {
            if let oxc_ast::ast::SimpleAssignmentTarget::AssignmentTargetIdentifier(identifier) =
                target
            {
                self.out.insert(Arc::from(identifier.name.as_str()));
            }
            walk::walk_simple_assignment_target(self, target);
        }

        fn visit_update_expression(&mut self, expression: &oxc_ast::ast::UpdateExpression<'a>) {
            if let oxc_ast::ast::SimpleAssignmentTarget::AssignmentTargetIdentifier(identifier) =
                &expression.argument
            {
                self.out.insert(Arc::from(identifier.name.as_str()));
            }
            walk::walk_update_expression(self, expression);
        }
    }

    if let Some(expression) = expression.oxc_expression() {
        let mut visitor = ReassignedTemplateVisitor { out };
        visitor.visit_expression(expression);
    }
}

fn emit_state_referenced_locally_warnings(
    source: &str,
    options: &CompileOptions,
    root: &Root,
    warnings: &mut Vec<Warning>,
) {
    let Some(instance_script) = instance_script(root) else {
        return;
    };
    let bindings = collect_instance_bindings(root, true)
        .into_iter()
        .filter(|(_, info)| {
            matches!(
                info.kind,
                InstanceBindingKind::State
                    | InstanceBindingKind::RawState
                    | InstanceBindingKind::Derived
                    | InstanceBindingKind::Prop
                    | InstanceBindingKind::RestProp
            )
        })
        .collect::<FxHashMap<_, _>>();
    if bindings.is_empty() {
        return;
    }

    let base_offset = instance_script.content_start;
    let reassigned = collect_reassigned_identifier_names(&instance_script.content);

    struct StateReferenceVisitor<'a> {
        bindings: &'a FxHashMap<Arc<str>, InstanceBindingInfo>,
        reassigned: &'a FxHashSet<Arc<str>>,
        warnings: &'a mut Vec<Warning>,
        source: &'a str,
        options: &'a CompileOptions,
        function_depth: usize,
        base_offset: usize,
        in_assignment_target: bool,
        in_derived_call: bool,
        in_var_init: bool,
    }

    impl<'a> Visit<'a> for StateReferenceVisitor<'_> {
        fn visit_identifier_reference(&mut self, identifier: &IdentifierReference<'a>) {
            if self.function_depth != 0 || self.in_assignment_target || self.in_derived_call {
                return;
            }
            let name = identifier.name.as_str();
            let Some(binding) = self.bindings.get(name) else {
                return;
            };
            let span = identifier.span();
            let start = span.start as usize + self.base_offset;
            let end = span.end as usize + self.base_offset;
            if start == binding.start && end == binding.end {
                return;
            }
            let should_warn = match binding.kind {
                InstanceBindingKind::State => {
                    self.reassigned.contains(name) || !binding.state_argument_proxyable
                }
                InstanceBindingKind::RawState
                | InstanceBindingKind::Derived
                | InstanceBindingKind::Prop
                | InstanceBindingKind::RestProp => true,
                InstanceBindingKind::Normal => false,
            };
            if !should_warn {
                return;
            }
            let suggestion = if self.in_var_init
                && matches!(
                    binding.kind,
                    InstanceBindingKind::State | InstanceBindingKind::RawState
                )
            {
                "a derived"
            } else {
                "a closure"
            };
            self.warnings.push(make_warning(
                self.source,
                self.options,
                "state_referenced_locally",
                &format!(
                    "This reference only captures the initial value of `{}`. Did you mean to reference it inside {} instead?",
                    name, suggestion
                ),
                start,
                end,
            ));
        }

        fn visit_variable_declarator(&mut self, declarator: &oxc_ast::ast::VariableDeclarator<'a>) {
            // Skip the binding pattern (left side) and only visit the init
            if let Some(init) = &declarator.init {
                let prev = self.in_var_init;
                self.in_var_init = self.function_depth == 0;
                self.visit_expression(init);
                self.in_var_init = prev;
            }
        }

        fn visit_arrow_function_expression(
            &mut self,
            expression: &oxc_ast::ast::ArrowFunctionExpression<'a>,
        ) {
            self.function_depth = self.function_depth.saturating_add(1);
            walk::walk_arrow_function_expression(self, expression);
            self.function_depth = self.function_depth.saturating_sub(1);
        }

        fn visit_function_body(&mut self, body: &oxc_ast::ast::FunctionBody<'a>) {
            self.function_depth = self.function_depth.saturating_add(1);
            walk::walk_function_body(self, body);
            self.function_depth = self.function_depth.saturating_sub(1);
        }

        fn visit_call_expression(&mut self, expression: &oxc_ast::ast::CallExpression<'a>) {
            let callee = call_expression_callee_name(expression);
            if matches!(callee.as_deref(), Some("$derived" | "$derived.by")) {
                let prev = self.in_derived_call;
                self.in_derived_call = true;
                walk::walk_call_expression(self, expression);
                self.in_derived_call = prev;
            } else {
                walk::walk_call_expression(self, expression);
            }
        }

        fn visit_assignment_target(&mut self, target: &oxc_ast::ast::AssignmentTarget<'a>) {
            let prev = self.in_assignment_target;
            self.in_assignment_target = true;
            walk::walk_assignment_target(self, target);
            self.in_assignment_target = prev;
        }

        fn visit_update_expression(&mut self, expression: &oxc_ast::ast::UpdateExpression<'a>) {
            let prev = self.in_assignment_target;
            self.in_assignment_target = true;
            walk::walk_update_expression(self, expression);
            self.in_assignment_target = prev;
        }

        fn visit_export_named_declaration(
            &mut self,
            declaration: &oxc_ast::ast::ExportNamedDeclaration<'a>,
        ) {
            // Only walk the declaration part, not the specifiers
            if let Some(decl) = &declaration.declaration {
                self.visit_declaration(decl);
            }
        }
    }

    let mut visitor = StateReferenceVisitor {
        bindings: &bindings,
        reassigned: &reassigned,
        warnings,
        source,
        options,
        function_depth: 0,
        base_offset,
        in_assignment_target: false,
        in_derived_call: false,
        in_var_init: false,
    };
    visitor.visit_program(instance_script.content.program());
}

fn emit_export_let_unused_warnings(
    source: &str,
    options: &CompileOptions,
    root: &Root,
    warnings: &mut Vec<Warning>,
) {
    let Some(instance_script) = instance_script(root) else {
        return;
    };
    let base_offset = instance_script.content_start;
    let script_ignores = collect_script_svelte_ignores(&instance_script.content, true);
    let mut exports = collect_instance_mutable_exports(&instance_script.content, false);
    if exports.is_empty() {
        return;
    }

    // Populate ignore_codes from svelte-ignore comments in the script
    for export in &mut exports {
        if let Some(codes) = script_ignores.get(&(export.statement_start as u32)) {
            export.ignore_codes = codes.clone();
        }
    }

    let export_names = exports
        .iter()
        .map(|entry| entry.name.clone())
        .collect::<FxHashSet<_>>();
    let mut used = FxHashSet::<Arc<str>>::default();
    collect_script_export_uses(&instance_script.content, &export_names, &mut used);
    collect_template_export_uses(
        &root.fragment,
        &export_names,
        &NameScope::default(),
        &mut used,
    );

    exports.sort_by_key(|entry| entry.start);
    for export in exports {
        if used.contains(export.name.as_ref()) {
            continue;
        }
        if warning_is_ignored("export_let_unused", &export.ignore_codes) {
            continue;
        }
        warnings.push(make_warning(
            source,
            options,
            "export_let_unused",
            &format!(
                "Component has unused export property '{}'. If it is for external reference only, please consider using `export const {}`",
                export.name, export.name
            ),
            export.start + base_offset,
            export.end + base_offset,
        ));
    }
}

fn collect_instance_mutable_exports(
    program: &ParsedJsProgram,
    _runes_mode: bool,
) -> Vec<ExportedMutableBinding> {
    let mutable_bindings = collect_program_mutable_bindings(program);
    let mut out = Vec::<ExportedMutableBinding>::new();

    for statement in &program.program().body {
        let Statement::ExportNamedDeclaration(statement) = statement else {
            continue;
        };

        if let Some(Declaration::VariableDeclaration(declaration)) = statement.declaration.as_ref()
        {
            if declaration.kind != VariableDeclarationKind::Const {
                let mut bindings = Vec::<PatternBinding>::new();
                for declarator in &declaration.declarations {
                    collect_pattern_bindings_from_oxc(&declarator.id, &mut bindings);
                }
                let stmt_start = statement.span().start as usize;
                out.extend(bindings.into_iter().map(|binding| ExportedMutableBinding {
                    name: binding.name,
                    start: binding.start,
                    end: binding.end,
                    statement_start: stmt_start,
                    ignore_codes: Box::default(),
                }));
            }
        }

        if statement.source.is_some() {
            continue;
        }

        for specifier in &statement.specifiers {
            let name = specifier.local.name().as_str();
            let Some((start, end)) = mutable_bindings.get(name).copied() else {
                continue;
            };
            out.push(ExportedMutableBinding {
                name: Arc::from(name),
                start,
                end,
                statement_start: statement.span().start as usize,
                ignore_codes: Box::default(),
            });
        }
    }

    let mut deduped = FxHashMap::<Arc<str>, ExportedMutableBinding>::default();
    for binding in out {
        deduped.entry(binding.name.clone()).or_insert(binding);
    }
    deduped.into_values().collect()
}

fn collect_program_mutable_bindings(
    program: &ParsedJsProgram,
) -> FxHashMap<Arc<str>, (usize, usize)> {
    let mut out = FxHashMap::<Arc<str>, (usize, usize)>::default();

    for statement in &program.program().body {
        match statement {
            Statement::VariableDeclaration(declaration)
                if declaration.kind != VariableDeclarationKind::Const =>
            {
                let mut bindings = Vec::<PatternBinding>::new();
                for declarator in &declaration.declarations {
                    collect_pattern_bindings_from_oxc(&declarator.id, &mut bindings);
                }
                for binding in bindings {
                    out.insert(binding.name, (binding.start, binding.end));
                }
            }
            Statement::ExportNamedDeclaration(statement) => {
                let Some(Declaration::VariableDeclaration(declaration)) =
                    statement.declaration.as_ref()
                else {
                    continue;
                };
                if declaration.kind == VariableDeclarationKind::Const {
                    continue;
                }
                let mut bindings = Vec::<PatternBinding>::new();
                for declarator in &declaration.declarations {
                    collect_pattern_bindings_from_oxc(&declarator.id, &mut bindings);
                }
                for binding in bindings {
                    out.insert(binding.name, (binding.start, binding.end));
                }
            }
            _ => {}
        }
    }
    out
}

fn collect_script_export_uses(
    program: &ParsedJsProgram,
    export_names: &NameSet,
    out: &mut NameSet,
) {
    struct ExportUseVisitor<'a> {
        export_names: &'a NameSet,
        out: &'a mut NameSet,
    }

    impl<'a> Visit<'a> for ExportUseVisitor<'_> {
        fn visit_identifier_reference(&mut self, identifier: &IdentifierReference<'a>) {
            if let Some(mapped) = mapped_export_name(identifier.name.as_str(), self.export_names) {
                self.out.insert(mapped);
            }
        }

        fn visit_export_named_declaration(
            &mut self,
            declaration: &oxc_ast::ast::ExportNamedDeclaration<'a>,
        ) {
            // Only visit the inline declaration, not the specifiers.
            // Specifier references (e.g. `export { d2 }`) are not real "uses".
            if let Some(decl) = declaration.declaration.as_ref() {
                self.visit_declaration(decl);
            }
        }
    }

    let mut visitor = ExportUseVisitor { export_names, out };
    visitor.visit_program(program.program());
}

fn collect_template_export_uses(
    fragment: &Fragment,
    export_names: &NameSet,
    scope: &NameScope,
    out: &mut NameSet,
) {
    for node in fragment.nodes.iter() {
        collect_template_export_uses_in_node(node, export_names, scope, out);

        if let Node::AwaitBlock(block) = node {
            if let Some(fragment) = block.pending.as_ref() {
                collect_template_export_uses(fragment, export_names, scope, out);
            }
            if let Some(fragment) = block.then.as_ref() {
                let then_scope = scope.with_expression_bindings(block.value.as_ref());
                collect_template_export_uses(fragment, export_names, &then_scope, out);
            }
            if let Some(fragment) = block.catch.as_ref() {
                let catch_scope = scope.with_expression_bindings(block.error.as_ref());
                collect_template_export_uses(fragment, export_names, &catch_scope, out);
            }
            continue;
        }

        let child_scope = scope.child_scope_for(node);
        if let Some(alternate) = export_use_alternate(node) {
            collect_template_export_uses_in_alternate(alternate, export_names, &child_scope, out);
            continue;
        }
        node.for_each_child_fragment(|child| {
            collect_template_export_uses(child, export_names, &child_scope, out);
        });
    }
}

fn collect_template_export_uses_in_node(
    node: &Node,
    export_names: &NameSet,
    scope: &NameScope,
    out: &mut NameSet,
) {
    match node {
        Node::Text(_) | Node::Comment(_) | Node::DebugTag(_) | Node::SnippetBlock(_) => {}
        Node::ExpressionTag(tag) => {
            collect_export_uses_from_expression(&tag.expression, export_names, scope, out);
        }
        Node::RenderTag(tag) => {
            collect_export_uses_from_expression(&tag.expression, export_names, scope, out);
        }
        Node::HtmlTag(tag) => {
            collect_export_uses_from_expression(&tag.expression, export_names, scope, out);
        }
        Node::ConstTag(tag) => {
            collect_export_uses_from_expression(&tag.declaration, export_names, scope, out);
        }
        Node::IfBlock(block) => {
            collect_export_uses_from_expression(&block.test, export_names, scope, out);
        }
        Node::EachBlock(block) => {
            collect_export_uses_from_expression(&block.expression, export_names, scope, out);
            if let Some(key) = block.key.as_ref() {
                collect_export_uses_from_expression(key, export_names, scope, out);
            }
            if let Some(context) = block.context.as_ref() {
                collect_export_uses_from_pattern_defaults(context, export_names, scope, out);
            }
        }
        Node::KeyBlock(block) => {
            collect_export_uses_from_expression(&block.expression, export_names, scope, out);
        }
        Node::AwaitBlock(block) => {
            collect_export_uses_from_expression(&block.expression, export_names, scope, out);
        }
        _ => {
            if let Some(element) = node.as_element() {
                collect_export_uses_from_attributes(element.attributes(), export_names, scope, out);
                if let Some(expression) = element.expression() {
                    collect_export_uses_from_expression(expression, export_names, scope, out);
                }
            }
        }
    }
}

fn export_use_alternate(node: &Node) -> Option<&Alternate> {
    match node {
        Node::IfBlock(block) => block.alternate.as_deref(),
        _ => None,
    }
}

fn collect_template_export_uses_in_alternate(
    alternate: &Alternate,
    export_names: &NameSet,
    scope: &NameScope,
    out: &mut NameSet,
) {
    match alternate {
        Alternate::Fragment(fragment) => {
            collect_template_export_uses(fragment, export_names, scope, out);
        }
        Alternate::IfBlock(block) => {
            collect_export_uses_from_expression(&block.test, export_names, scope, out);
            collect_template_export_uses(&block.consequent, export_names, scope, out);
            if let Some(alternate) = block.alternate.as_deref() {
                collect_template_export_uses_in_alternate(alternate, export_names, scope, out);
            }
        }
    }
}

fn collect_export_uses_from_attributes(
    attributes: &[Attribute],
    export_names: &NameSet,
    scope: &NameScope,
    out: &mut NameSet,
) {
    for attribute in attributes {
        match attribute {
            Attribute::Attribute(attribute) => match &attribute.value {
                AttributeValueList::Boolean(_) => {}
                AttributeValueList::ExpressionTag(tag) => {
                    collect_export_uses_from_expression(&tag.expression, export_names, scope, out);
                }
                AttributeValueList::Values(values) => {
                    for value in values.iter() {
                        if let AttributeValue::ExpressionTag(tag) = value {
                            collect_export_uses_from_expression(
                                &tag.expression,
                                export_names,
                                scope,
                                out,
                            );
                        }
                    }
                }
            },
            Attribute::BindDirective(directive)
            | Attribute::OnDirective(directive)
            | Attribute::ClassDirective(directive)
            | Attribute::AnimateDirective(directive)
            | Attribute::UseDirective(directive) => {
                collect_export_uses_from_expression(
                    &directive.expression,
                    export_names,
                    scope,
                    out,
                );
            }
            Attribute::LetDirective(_) => {}
            Attribute::TransitionDirective(directive) => {
                collect_export_uses_from_expression(
                    &directive.expression,
                    export_names,
                    scope,
                    out,
                );
            }
            Attribute::AttachTag(tag) => {
                collect_export_uses_from_expression(&tag.expression, export_names, scope, out);
            }
            Attribute::StyleDirective(style) => match &style.value {
                AttributeValueList::Boolean(_) => {
                    if let Some(mapped) = mapped_export_name(style.name.as_ref(), export_names)
                        && !scope.contains(mapped.as_ref())
                    {
                        out.insert(mapped);
                    }
                }
                AttributeValueList::ExpressionTag(tag) => {
                    collect_export_uses_from_expression(&tag.expression, export_names, scope, out);
                }
                AttributeValueList::Values(values) => {
                    for value in values.iter() {
                        if let AttributeValue::ExpressionTag(tag) = value {
                            collect_export_uses_from_expression(
                                &tag.expression,
                                export_names,
                                scope,
                                out,
                            );
                        }
                    }
                }
            },
            Attribute::SpreadAttribute(spread) => {
                collect_export_uses_from_expression(&spread.expression, export_names, scope, out);
            }
        }
    }
}

fn collect_export_uses_from_expression(
    expression: &crate::ast::modern::Expression,
    export_names: &NameSet,
    scope: &NameScope,
    out: &mut NameSet,
) {
    if let Some(expression) = expression.oxc_expression() {
        collect_export_uses_from_oxc_expression(expression, export_names, scope, out);
    }
}

fn collect_export_uses_from_pattern_defaults(
    pattern: &Expression,
    export_names: &NameSet,
    scope: &NameScope,
    out: &mut NameSet,
) {
    let Some(pattern) = pattern.oxc_pattern() else {
        if let Some(expression) = pattern.oxc_expression() {
            collect_export_uses_from_oxc_expression(expression, export_names, scope, out);
        }
        return;
    };
    collect_export_uses_from_pattern_defaults_oxc(pattern, export_names, scope, out);
}

fn collect_export_uses_from_pattern_defaults_oxc(
    pattern: &BindingPattern<'_>,
    export_names: &NameSet,
    scope: &NameScope,
    out: &mut NameSet,
) {
    match pattern {
        BindingPattern::BindingIdentifier(_) => {}
        BindingPattern::AssignmentPattern(pattern) => {
            collect_export_uses_from_pattern_defaults_oxc(&pattern.left, export_names, scope, out);
            collect_export_uses_from_oxc_expression(&pattern.right, export_names, scope, out);
        }
        BindingPattern::ObjectPattern(pattern) => {
            for property in &pattern.properties {
                collect_export_uses_from_pattern_defaults_oxc(
                    &property.value,
                    export_names,
                    scope,
                    out,
                );
            }
            if let Some(rest) = pattern.rest.as_ref() {
                collect_export_uses_from_pattern_defaults_oxc(
                    &rest.argument,
                    export_names,
                    scope,
                    out,
                );
            }
        }
        BindingPattern::ArrayPattern(pattern) => {
            for element in pattern.elements.iter().flatten() {
                collect_export_uses_from_pattern_defaults_oxc(element, export_names, scope, out);
            }
            if let Some(rest) = pattern.rest.as_ref() {
                collect_export_uses_from_pattern_defaults_oxc(
                    &rest.argument,
                    export_names,
                    scope,
                    out,
                );
            }
        }
    }
}

fn collect_export_uses_from_oxc_expression(
    expression: &OxcExpression<'_>,
    export_names: &NameSet,
    scope: &NameScope,
    out: &mut NameSet,
) {
    struct ExportExpressionVisitor<'a> {
        export_names: &'a NameSet,
        scope: &'a NameScope,
        out: &'a mut NameSet,
    }

    impl<'a> Visit<'a> for ExportExpressionVisitor<'_> {
        fn visit_identifier_reference(&mut self, identifier: &IdentifierReference<'a>) {
            let Some(mapped) = mapped_export_name(identifier.name.as_str(), self.export_names)
            else {
                return;
            };
            if self.scope.contains(mapped.as_ref()) {
                return;
            }
            self.out.insert(mapped);
        }
    }

    let mut visitor = ExportExpressionVisitor {
        export_names,
        scope,
        out,
    };
    visitor.visit_expression(expression);
}

fn mapped_export_name(name: &str, export_names: &NameSet) -> Option<Arc<str>> {
    if export_names.contains(name) {
        return Some(name.into());
    }
    let stripped = name.strip_prefix('$')?;
    export_names.contains(stripped).then_some(stripped.into())
}

fn collect_non_reactive_template_references(
    fragment: &Fragment,
    block_depth: usize,
    candidate_names: &NameSet,
    out: &mut NameSet,
) {
    for node in fragment.nodes.iter() {
        collect_non_reactive_in_node(node, block_depth, candidate_names, out);
        let child_block_depth = non_reactive_child_block_depth(node, block_depth);
        if let Some(alternate) = non_reactive_alternate(node) {
            collect_non_reactive_template_references_in_alternate(
                alternate,
                child_block_depth,
                candidate_names,
                out,
            );
            continue;
        }
        node.for_each_child_fragment(|child| {
            collect_non_reactive_template_references(
                child,
                child_block_depth,
                candidate_names,
                out,
            );
        });
    }
}

fn collect_non_reactive_in_node(
    node: &Node,
    block_depth: usize,
    candidate_names: &NameSet,
    out: &mut NameSet,
) {
    match node {
        Node::Text(_) | Node::Comment(_) | Node::DebugTag(_) | Node::SnippetBlock(_) => {}
        Node::ExpressionTag(tag) => {
            collect_non_reactive_from_expression(
                &tag.expression,
                false,
                block_depth,
                candidate_names,
                out,
            );
        }
        Node::RenderTag(tag) => {
            collect_non_reactive_from_expression(
                &tag.expression,
                false,
                block_depth,
                candidate_names,
                out,
            );
        }
        Node::HtmlTag(tag) => {
            collect_non_reactive_from_expression(
                &tag.expression,
                false,
                block_depth,
                candidate_names,
                out,
            );
        }
        Node::ConstTag(tag) => {
            collect_non_reactive_from_expression(
                &tag.declaration,
                false,
                block_depth,
                candidate_names,
                out,
            );
        }
        Node::IfBlock(block) => {
            collect_non_reactive_from_expression(
                &block.test,
                false,
                block_depth,
                candidate_names,
                out,
            );
        }
        Node::EachBlock(block) => {
            collect_non_reactive_from_expression(
                &block.expression,
                false,
                block_depth,
                candidate_names,
                out,
            );
            if let Some(key) = block.key.as_ref() {
                collect_non_reactive_from_expression(key, false, block_depth, candidate_names, out);
            }
        }
        Node::KeyBlock(block) => {
            collect_non_reactive_from_expression(
                &block.expression,
                false,
                block_depth,
                candidate_names,
                out,
            );
        }
        Node::AwaitBlock(block) => {
            collect_non_reactive_from_expression(
                &block.expression,
                false,
                block_depth,
                candidate_names,
                out,
            );
            if let Some(value) = block.value.as_ref() {
                collect_non_reactive_from_expression(
                    value,
                    false,
                    block_depth,
                    candidate_names,
                    out,
                );
            }
            if let Some(error) = block.error.as_ref() {
                collect_non_reactive_from_expression(
                    error,
                    false,
                    block_depth,
                    candidate_names,
                    out,
                );
            }
        }
        _ => {
            if let Some(element) = node.as_element() {
                collect_non_reactive_from_attributes(
                    element.attributes(),
                    block_depth,
                    candidate_names,
                    out,
                );
            }
        }
    }
}

fn non_reactive_child_block_depth(node: &Node, block_depth: usize) -> usize {
    match node {
        Node::IfBlock(_) | Node::EachBlock(_) | Node::KeyBlock(_) | Node::AwaitBlock(_) => {
            block_depth + 1
        }
        _ => block_depth,
    }
}

fn non_reactive_alternate(node: &Node) -> Option<&Alternate> {
    match node {
        Node::IfBlock(block) => block.alternate.as_deref(),
        _ => None,
    }
}

fn collect_non_reactive_template_references_in_alternate(
    alternate: &Alternate,
    block_depth: usize,
    candidate_names: &NameSet,
    out: &mut NameSet,
) {
    match alternate {
        Alternate::Fragment(fragment) => {
            collect_non_reactive_template_references(fragment, block_depth, candidate_names, out);
        }
        Alternate::IfBlock(block) => {
            collect_non_reactive_from_expression(
                &block.test,
                false,
                block_depth,
                candidate_names,
                out,
            );
            collect_non_reactive_template_references(
                &block.consequent,
                block_depth,
                candidate_names,
                out,
            );
            if let Some(alternate) = block.alternate.as_deref() {
                collect_non_reactive_template_references_in_alternate(
                    alternate,
                    block_depth,
                    candidate_names,
                    out,
                );
            }
        }
    }
}

fn collect_non_reactive_from_attributes(
    attributes: &[Attribute],
    block_depth: usize,
    candidate_names: &NameSet,
    out: &mut NameSet,
) {
    for attribute in attributes {
        match attribute {
            Attribute::Attribute(attribute) => match &attribute.value {
                AttributeValueList::Boolean(_) => {}
                AttributeValueList::ExpressionTag(tag) => {
                    collect_non_reactive_from_expression(
                        &tag.expression,
                        false,
                        block_depth,
                        candidate_names,
                        out,
                    );
                }
                AttributeValueList::Values(values) => {
                    for value in values.iter() {
                        if let AttributeValue::ExpressionTag(tag) = value {
                            collect_non_reactive_from_expression(
                                &tag.expression,
                                false,
                                block_depth,
                                candidate_names,
                                out,
                            );
                        }
                    }
                }
            },
            Attribute::BindDirective(directive) => {
                collect_non_reactive_from_expression(
                    &directive.expression,
                    directive.name.as_ref() == "this",
                    block_depth,
                    candidate_names,
                    out,
                );
            }
            Attribute::OnDirective(directive)
            | Attribute::ClassDirective(directive)
            | Attribute::LetDirective(directive)
            | Attribute::AnimateDirective(directive)
            | Attribute::UseDirective(directive) => {
                collect_non_reactive_from_expression(
                    &directive.expression,
                    false,
                    block_depth,
                    candidate_names,
                    out,
                );
            }
            Attribute::TransitionDirective(directive) => {
                collect_non_reactive_from_expression(
                    &directive.expression,
                    false,
                    block_depth,
                    candidate_names,
                    out,
                );
            }
            Attribute::AttachTag(tag) => {
                collect_non_reactive_from_expression(
                    &tag.expression,
                    false,
                    block_depth,
                    candidate_names,
                    out,
                );
            }
            Attribute::StyleDirective(style) => match &style.value {
                AttributeValueList::Boolean(_) => {}
                AttributeValueList::ExpressionTag(tag) => {
                    collect_non_reactive_from_expression(
                        &tag.expression,
                        false,
                        block_depth,
                        candidate_names,
                        out,
                    );
                }
                AttributeValueList::Values(values) => {
                    for value in values.iter() {
                        if let AttributeValue::ExpressionTag(tag) = value {
                            collect_non_reactive_from_expression(
                                &tag.expression,
                                false,
                                block_depth,
                                candidate_names,
                                out,
                            );
                        }
                    }
                }
            },
            Attribute::SpreadAttribute(spread) => {
                collect_non_reactive_from_expression(
                    &spread.expression,
                    false,
                    block_depth,
                    candidate_names,
                    out,
                );
            }
        }
    }
}

fn collect_non_reactive_from_expression(
    expression: &crate::ast::modern::Expression,
    bind_this: bool,
    block_depth: usize,
    candidate_names: &NameSet,
    out: &mut NameSet,
) {
    struct NonReactiveVisitor<'a> {
        bind_this: bool,
        block_depth: usize,
        candidate_names: &'a NameSet,
        out: &'a mut NameSet,
        function_depth: usize,
    }

    impl<'a> Visit<'a> for NonReactiveVisitor<'_> {
        fn visit_identifier_reference(&mut self, identifier: &IdentifierReference<'a>) {
            if self.function_depth != 0 {
                return;
            }
            let name = identifier.name.as_str();
            if !self.candidate_names.contains(name) {
                return;
            }
            if self.bind_this && self.block_depth == 0 {
                return;
            }
            self.out.insert(name.into());
        }

        fn visit_arrow_function_expression(
            &mut self,
            expression: &oxc_ast::ast::ArrowFunctionExpression<'a>,
        ) {
            self.function_depth = self.function_depth.saturating_add(1);
            walk::walk_arrow_function_expression(self, expression);
            self.function_depth = self.function_depth.saturating_sub(1);
        }

        fn visit_function_body(&mut self, body: &oxc_ast::ast::FunctionBody<'a>) {
            self.function_depth = self.function_depth.saturating_add(1);
            walk::walk_function_body(self, body);
            self.function_depth = self.function_depth.saturating_sub(1);
        }
    }

    if let Some(expression) = expression.oxc_expression() {
        let mut visitor = NonReactiveVisitor {
            bind_this,
            block_depth,
            candidate_names,
            out,
            function_depth: 0,
        };
        visitor.visit_expression(expression);
    }
}

fn collect_default_svelte_imports(program: &ParsedJsProgram) -> NameSet {
    let mut imported = FxHashSet::<Arc<str>>::default();
    for statement in &program.program().body {
        let Statement::ImportDeclaration(statement) = statement else {
            continue;
        };
        let is_svelte_file = statement.source.value.as_str().ends_with(".svelte");
        if !is_svelte_file {
            continue;
        }

        if let Some(specifiers) = statement.specifiers.as_ref() {
            for specifier in specifiers {
                if let oxc_ast::ast::ImportDeclarationSpecifier::ImportDefaultSpecifier(specifier) =
                    specifier
                {
                    imported.insert(specifier.local.name.as_str().into());
                }
            }
        }
    }
    imported
}

fn emit_custom_element_props_identifier_warnings(
    source: &str,
    options: &CompileOptions,
    program: &ParsedJsProgram,
    base_offset: usize,
    warnings: &mut Vec<Warning>,
) {
    struct CustomElementPropsVisitor<'a> {
        source: &'a str,
        options: &'a CompileOptions,
        warnings: &'a mut Vec<Warning>,
        base_offset: usize,
    }

    impl<'a> Visit<'a> for CustomElementPropsVisitor<'_> {
        fn visit_variable_declarator(&mut self, declarator: &oxc_ast::ast::VariableDeclarator<'a>) {
            let Some(init) = declarator.init.as_ref() else {
                return;
            };
            if !is_dollar_props_call(init) {
                return;
            }

            match &declarator.id {
                BindingPattern::BindingIdentifier(identifier) => {
                    let span = identifier.span();
                    self.warnings.push(make_warning(
                        self.source,
                        self.options,
                        "custom_element_props_identifier",
                        "Using a rest element or a non-destructured declaration with `$props()` means that Svelte can't infer what properties to expose when creating a custom element. Consider destructuring all the props or explicitly specifying the `customElement.props` option.",
                        span.start as usize + self.base_offset,
                        span.end as usize + self.base_offset,
                    ));
                }
                BindingPattern::ObjectPattern(pattern) => {
                    if let Some(rest) = pattern.rest.as_ref() {
                        let span = rest.span();
                        self.warnings.push(make_warning(
                            self.source,
                            self.options,
                            "custom_element_props_identifier",
                            "Using a rest element or a non-destructured declaration with `$props()` means that Svelte can't infer what properties to expose when creating a custom element. Consider destructuring all the props or explicitly specifying the `customElement.props` option.",
                            span.start as usize + self.base_offset,
                            span.end as usize + self.base_offset,
                        ));
                    }
                }
                _ => {}
            }
        }
    }

    let mut visitor = CustomElementPropsVisitor {
        source,
        options,
        warnings,
        base_offset,
    };
    visitor.visit_program(program.program());
}

fn is_dollar_props_call(node: &OxcExpression<'_>) -> bool {
    let OxcExpression::CallExpression(call) = node else {
        return false;
    };
    call.callee
        .get_identifier_reference()
        .is_some_and(|callee| callee.name.as_str() == "$props")
}

fn component_uses_custom_element(root: &Root, options: &CompileOptions) -> bool {
    if options.custom_element {
        return true;
    }
    root.options.as_ref().is_some_and(|options| {
        options.attributes.iter().any(|attribute| {
            matches!(
                attribute,
                Attribute::Attribute(attribute)
                    if attribute.name.as_ref().eq_ignore_ascii_case("customElement")
            )
        })
    })
}

fn custom_element_has_props_option(root: &Root) -> bool {
    root.options.as_ref().is_some_and(|options| {
        options.attributes.iter().any(|attribute| {
            let Attribute::Attribute(attribute) = attribute else {
                return false;
            };
            if !attribute
                .name
                .as_ref()
                .eq_ignore_ascii_case("customElement")
            {
                return false;
            }
            let AttributeValueList::ExpressionTag(tag) = &attribute.value else {
                return false;
            };
            let Some(expression) = tag.expression.oxc_expression() else {
                return false;
            };
            object_expression_has_identifier_property(expression, "props")
        })
    })
}

fn collect_declared_names_from_oxc_variable_declaration(
    declaration: &oxc_ast::ast::VariableDeclaration<'_>,
    out: &mut NameSet,
) {
    for declarator in &declaration.declarations {
        extend_name_set_with_oxc_pattern_bindings(out, &declarator.id);
    }
}

fn object_expression_has_identifier_property(object: &OxcExpression<'_>, name: &str) -> bool {
    let OxcExpression::ObjectExpression(object) = object else {
        return false;
    };
    object.properties.iter().any(|property| {
        let oxc_ast::ast::ObjectPropertyKind::ObjectProperty(property) = property else {
            return false;
        };
        match &property.key {
            oxc_ast::ast::PropertyKey::StaticIdentifier(identifier) => {
                identifier.name.as_str() == name
            }
            oxc_ast::ast::PropertyKey::StringLiteral(literal) => literal.value.as_str() == name,
            _ => false,
        }
    })
}


fn emit_bidirectional_warnings_in_text(
    source: &str,
    options: &CompileOptions,
    text: &crate::ast::modern::Text,
    warnings: &mut Vec<Warning>,
) {
    let mut run_start: Option<usize> = None;
    let mut run_end: usize = 0;

    for (idx, ch) in text.data.char_indices() {
        if is_bidirectional_control_char(ch) {
            if run_start.is_none() {
                run_start = Some(idx);
            }
            run_end = idx + ch.len_utf8();
        } else if let Some(start_idx) = run_start.take() {
            warnings.push(make_warning(
                source,
                options,
                "bidirectional_control_characters",
                "A bidirectional control character was detected in your code. These characters can be used to alter the visual direction of your code and could have unintended consequences",
                text.start + start_idx,
                text.start + run_end,
            ));
        }
    }

    if let Some(start_idx) = run_start {
        warnings.push(make_warning(
            source,
            options,
            "bidirectional_control_characters",
            "A bidirectional control character was detected in your code. These characters can be used to alter the visual direction of your code and could have unintended consequences",
            text.start + start_idx,
            text.start + run_end,
        ));
    }
}

fn warn_if_block_empty_fragment(
    source: &str,
    options: &CompileOptions,
    fragment: Option<&Fragment>,
    warnings: &mut Vec<Warning>,
) {
    let Some(fragment) = fragment else {
        return;
    };
    let [Node::Text(text)] = fragment.nodes.as_ref() else {
        return;
    };
    if text.raw.trim().is_empty() {
        warnings.push(make_warning(
            source,
            options,
            "block_empty",
            "Empty block",
            text.start,
            text.end,
        ));
    }
}

fn collect_rest_pattern_identifiers(expression: &Expression, out: &mut Vec<RestBindingWarning>) {
    let Some(pattern) = expression.oxc_pattern() else {
        return;
    };
    // OXC pattern is parsed via `(PATTERN)=>{}` wrapper, so OXC spans are
    // shifted by 1 for the leading `(`. Subtract 1 to get pattern-relative,
    // then add expression.start to get file-absolute positions.
    let base_offset = expression.start.saturating_sub(1);
    let start_len = out.len();
    collect_rest_pattern_identifiers_inner(pattern, false, out);
    for item in &mut out[start_len..] {
        item.start += base_offset;
        item.end += base_offset;
    }
}

fn collect_rest_pattern_identifiers_inner(
    pattern: &BindingPattern<'_>,
    inside_rest: bool,
    out: &mut Vec<RestBindingWarning>,
) {
    match pattern {
        BindingPattern::BindingIdentifier(identifier) => {
            if !inside_rest {
                return;
            };
            let span = identifier.span();
            out.push(RestBindingWarning {
                name: Arc::from(identifier.name.as_str()),
                start: span.start as usize,
                end: span.end as usize,
            });
        }
        BindingPattern::AssignmentPattern(pattern) => {
            collect_rest_pattern_identifiers_inner(&pattern.left, inside_rest, out);
        }
        BindingPattern::ArrayPattern(pattern) => {
            for element in pattern.elements.iter().flatten() {
                collect_rest_pattern_identifiers_inner(element, inside_rest, out);
            }
            if let Some(rest) = pattern.rest.as_ref() {
                collect_rest_pattern_identifiers_inner(&rest.argument, true, out);
            }
        }
        BindingPattern::ObjectPattern(pattern) => {
            for property in &pattern.properties {
                collect_rest_pattern_identifiers_inner(&property.value, inside_rest, out);
            }
            if let Some(rest) = pattern.rest.as_ref() {
                collect_rest_pattern_identifiers_inner(&rest.argument, true, out);
            }
        }
    }
}

fn binding_base_identifier_name(expression: &Expression) -> Option<Arc<str>> {
    if let Some(identifier) = expression.binding_identifier() {
        return Some(Arc::from(identifier.name.as_str()));
    }
    base_identifier_name_from_oxc_expression(expression.oxc_expression()?)
}

fn base_identifier_name_from_oxc_expression(expression: &OxcExpression<'_>) -> Option<Arc<str>> {
    match expression {
        OxcExpression::Identifier(identifier) => Some(Arc::from(identifier.name.as_str())),
        OxcExpression::StaticMemberExpression(member) => {
            base_identifier_name_from_oxc_expression(&member.object)
        }
        OxcExpression::ComputedMemberExpression(member) => {
            base_identifier_name_from_oxc_expression(&member.object)
        }
        OxcExpression::PrivateFieldExpression(member) => {
            base_identifier_name_from_oxc_expression(&member.object)
        }
        OxcExpression::TSAsExpression(expression) => {
            base_identifier_name_from_oxc_expression(&expression.expression)
        }
        OxcExpression::TSSatisfiesExpression(expression) => {
            base_identifier_name_from_oxc_expression(&expression.expression)
        }
        OxcExpression::TSInstantiationExpression(expression) => {
            base_identifier_name_from_oxc_expression(&expression.expression)
        }
        OxcExpression::TSNonNullExpression(expression) => {
            base_identifier_name_from_oxc_expression(&expression.expression)
        }
        OxcExpression::ParenthesizedExpression(expression) => {
            base_identifier_name_from_oxc_expression(&expression.expression)
        }
        _ => None,
    }
}

fn expression_string_value(expression: &Expression) -> Option<String> {
    let expression = expression.oxc_expression()?;
    match expression {
        OxcExpression::StringLiteral(literal) => Some(literal.value.as_str().to_owned()),
        _ => None,
    }
}

fn collect_pattern_bindings_from_oxc(pattern: &BindingPattern<'_>, out: &mut Vec<PatternBinding>) {
    match pattern {
        BindingPattern::BindingIdentifier(identifier) => {
            let span = identifier.span();
            out.push(PatternBinding {
                name: Arc::from(identifier.name.as_str()),
                start: span.start as usize,
                end: span.end as usize,
                is_rest: false,
            });
        }
        BindingPattern::AssignmentPattern(pattern) => {
            collect_pattern_bindings_from_oxc(&pattern.left, out);
        }
        BindingPattern::ObjectPattern(pattern) => {
            for property in &pattern.properties {
                collect_pattern_bindings_from_oxc(&property.value, out);
            }
            if let Some(rest) = pattern.rest.as_ref() {
                collect_pattern_bindings_from_oxc_rest(&rest.argument, out);
            }
        }
        BindingPattern::ArrayPattern(pattern) => {
            for element in pattern.elements.iter().flatten() {
                collect_pattern_bindings_from_oxc(element, out);
            }
            if let Some(rest) = pattern.rest.as_ref() {
                collect_pattern_bindings_from_oxc_rest(&rest.argument, out);
            }
        }
    }
}

fn collect_pattern_bindings_from_oxc_rest(
    pattern: &BindingPattern<'_>,
    out: &mut Vec<PatternBinding>,
) {
    match pattern {
        BindingPattern::BindingIdentifier(identifier) => {
            let span = identifier.span();
            out.push(PatternBinding {
                name: Arc::from(identifier.name.as_str()),
                start: span.start as usize,
                end: span.end as usize,
                is_rest: true,
            });
        }
        BindingPattern::AssignmentPattern(pattern) => {
            collect_pattern_bindings_from_oxc_rest(&pattern.left, out);
        }
        BindingPattern::ObjectPattern(pattern) => {
            for property in &pattern.properties {
                collect_pattern_bindings_from_oxc_rest(&property.value, out);
            }
            if let Some(rest) = pattern.rest.as_ref() {
                collect_pattern_bindings_from_oxc_rest(&rest.argument, out);
            }
        }
        BindingPattern::ArrayPattern(pattern) => {
            for element in pattern.elements.iter().flatten() {
                collect_pattern_bindings_from_oxc_rest(element, out);
            }
            if let Some(rest) = pattern.rest.as_ref() {
                collect_pattern_bindings_from_oxc_rest(&rest.argument, out);
            }
        }
    }
}

fn attribute_global_event_reference_name(
    attribute: &crate::ast::modern::NamedAttribute,
) -> Option<(String, String)> {
    let name = attribute.name.as_ref();
    if !name.starts_with("on") || name.len() <= 2 {
        return None;
    }

    let AttributeValueList::ExpressionTag(tag) = &attribute.value else {
        return None;
    };
    let identifier_name = tag.expression.identifier_name()?.to_string();
    Some((name.to_string(), identifier_name))
}

fn attribute_is_quoted_expression(attribute: &crate::ast::modern::NamedAttribute) -> bool {
    attribute.value_syntax == crate::ast::common::AttributeValueSyntax::Quoted
}

fn strip_namespace_prefix(tag: &str) -> &str {
    tag.rsplit(':').next().unwrap_or(tag)
}

fn is_custom_element_tag(tag: &str) -> bool {
    tag.contains('-') && !tag.starts_with("svelte:")
}

fn is_bidirectional_control_char(ch: char) -> bool {
    let code = ch as u32;
    BIDI_CONTROL_RANGES
        .iter()
        .any(|(start, end)| code >= *start && code <= *end)
}

fn string_contains_bidirectional_controls(value: &str) -> bool {
    value.chars().any(is_bidirectional_control_char)
}

fn make_warning(
    source: &str,
    options: &CompileOptions,
    code: &str,
    message: &str,
    start: usize,
    end: usize,
) -> Warning {
    let source_text = SourceText::new(crate::SourceId::new(0), source, options.filename.as_deref());
    let start_location = source_text.location_at_offset(start);
    let end_location = source_text.location_at_offset(end);

    Warning {
        code: code.into(),
        message: warning_message_with_doc_link(code, message),
        filename: options.filename.clone(),
        start: Some(start_location),
        end: Some(end_location),
        frame: None,
        position: Some([start, end]),
    }
}

fn warning_from_compile_error(source: SourceText<'_>, diagnostic: crate::CompileError) -> Warning {
    Warning {
        message: warning_message_with_doc_link(&diagnostic.code, &diagnostic.message),
        code: diagnostic.code,
        filename: diagnostic
            .filename
            .map(|path| path.as_ref().clone())
            .or_else(|| source.filename.map(|path| path.to_path_buf())),
        start: diagnostic.start.map(|location| *location),
        end: diagnostic.end.map(|location| *location),
        frame: None,
        position: diagnostic
            .position
            .map(|position| [position.start, position.end]),
    }
}

fn warning_message_with_doc_link(code: &str, message: &str) -> Arc<str> {
    let doc_link = format!("https://svelte.dev/e/{code}");
    if message.contains(&doc_link) {
        return Arc::from(message);
    }
    Arc::from(format!("{message}\n{doc_link}"))
}

#[derive(Debug, Default)]
struct ParsedSvelteIgnoreDirective {
    ignores: Box<[Arc<str>]>,
    diagnostics: Vec<IgnoreDirectiveDiagnostic>,
}

#[derive(Debug)]
struct IgnoreDirectiveDiagnostic {
    code: &'static str,
    message: String,
    start: usize,
    end: usize,
}

fn parse_svelte_ignore_directive(
    comment_data_start: usize,
    comment_data: &str,
    runes_mode: bool,
) -> ParsedSvelteIgnoreDirective {
    let mut ignores = IgnoreCodes::default();
    let mut diagnostics = Vec::new();
    let Some(payload_start) = svelte_ignore_payload_start(comment_data) else {
        return ParsedSvelteIgnoreDirective::default();
    };

    let payload = &comment_data[payload_start..];
    if runes_mode {
        parse_svelte_ignore_runes_mode(
            comment_data_start,
            payload_start,
            payload,
            &mut ignores,
            &mut diagnostics,
        );
    } else {
        parse_svelte_ignore_legacy_mode(payload, &mut ignores);
    }

    ParsedSvelteIgnoreDirective {
        ignores: ignores.to_boxed_slice(),
        diagnostics,
    }
}

fn svelte_ignore_payload_start(comment_data: &str) -> Option<usize> {
    let bytes = comment_data.as_bytes();
    let mut cursor = 0usize;
    while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
        cursor += 1;
    }

    const DIRECTIVE: &str = "svelte-ignore";
    if !comment_data[cursor..].starts_with(DIRECTIVE) {
        return None;
    }
    cursor += DIRECTIVE.len();

    if cursor >= bytes.len() || !bytes[cursor].is_ascii_whitespace() {
        return None;
    }

    Some(cursor + 1)
}

fn parse_svelte_ignore_runes_mode(
    comment_data_start: usize,
    payload_start: usize,
    payload: &str,
    ignores: &mut IgnoreCodes,
    diagnostics: &mut Vec<IgnoreDirectiveDiagnostic>,
) {
    let bytes = payload.as_bytes();
    let mut cursor = 0usize;

    loop {
        while cursor < bytes.len() && !is_ignore_code_char(bytes[cursor]) {
            cursor += 1;
        }
        if cursor >= bytes.len() {
            break;
        }

        let token_start = cursor;
        while cursor < bytes.len() && is_ignore_code_char(bytes[cursor]) {
            cursor += 1;
        }
        let token = &payload[token_start..cursor];
        let has_comma = if cursor < bytes.len() && bytes[cursor] == b',' {
            cursor += 1;
            true
        } else {
            false
        };

        if is_known_warning_code(token) {
            ignores.push_unique(token);
        } else {
            let replacement = legacy_ignore_replacement(token)
                .map(str::to_string)
                .unwrap_or_else(|| token.replace('-', "_"));
            let start = comment_data_start + payload_start + token_start;
            let end = start + token.len();
            if is_known_warning_code(&replacement) {
                diagnostics.push(IgnoreDirectiveDiagnostic {
                    code: "legacy_code",
                    message: format!(
                        "`{}` is no longer valid — please use `{}` instead",
                        token, replacement
                    ),
                    start,
                    end,
                });
            } else {
                let suggestion = fuzzy_match(token, SVELTE_WARNING_CODES);
                let message = if let Some(suggestion) = suggestion {
                    format!(
                        "`{}` is not a recognised code (did you mean `{}`?)",
                        token, suggestion
                    )
                } else {
                    format!("`{}` is not a recognised code", token)
                };
                diagnostics.push(IgnoreDirectiveDiagnostic {
                    code: "unknown_code",
                    message,
                    start,
                    end,
                });
            }
        }

        if !has_comma {
            break;
        }
    }
}

fn parse_svelte_ignore_legacy_mode(payload: &str, ignores: &mut IgnoreCodes) {
    let bytes = payload.as_bytes();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        while cursor < bytes.len() && !is_ignore_code_char(bytes[cursor]) {
            cursor += 1;
        }
        if cursor >= bytes.len() {
            break;
        }

        let token_start = cursor;
        while cursor < bytes.len() && is_ignore_code_char(bytes[cursor]) {
            cursor += 1;
        }
        let token = &payload[token_start..cursor];
        ignores.push_unique(token);

        if !is_known_warning_code(token) {
            let replacement = legacy_ignore_replacement(token)
                .map(str::to_string)
                .unwrap_or_else(|| token.replace('-', "_"));
            if is_known_warning_code(&replacement) {
                ignores.push_unique(&replacement);
            }
        }
    }
}

fn is_ignore_code_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'$')
}

fn is_known_warning_code(code: &str) -> bool {
    SVELTE_WARNING_CODES.contains(&code)
}

fn legacy_ignore_replacement(code: &str) -> Option<&'static str> {
    match code {
        "non-top-level-reactive-declaration" => Some("reactive_declaration_invalid_placement"),
        "module-script-reactive-declaration" => Some("reactive_declaration_module_script"),
        "empty-block" => Some("block_empty"),
        "avoid-is" => Some("attribute_avoid_is"),
        "invalid-html-attribute" => Some("attribute_invalid_property_name"),
        "a11y-structure" => Some("a11y_figcaption_parent"),
        "illegal-attribute-character" => Some("attribute_illegal_colon"),
        "invalid-rest-eachblock-binding" => Some("bind_invalid_each_rest"),
        "unused-export-let" => Some("export_let_unused"),
        _ => None,
    }
}

fn filter_recent_ignored_warnings(
    warnings: &mut Vec<Warning>,
    start_len: usize,
    ignore_codes: &[Arc<str>],
) {
    if ignore_codes.is_empty() || start_len >= warnings.len() {
        return;
    }

    let mut kept = Vec::with_capacity(warnings.len() - start_len);
    for warning in warnings.drain(start_len..) {
        if warning_is_ignored(&warning.code, ignore_codes) {
            continue;
        }
        kept.push(warning);
    }
    warnings.extend(kept);
}

fn warning_is_ignored(code: &str, ignore_codes: &[Arc<str>]) -> bool {
    ignore_codes.iter().any(|ignored| {
        let ignored = ignored.as_ref();
        ignored == code || ignored.replace('-', "_") == code
    })
}

fn is_lowercase_component_like_tag(tag: &str) -> bool {
    let mut chars = tag.chars();
    let Some(first) = chars.next() else {
        return false;
    };

    first.is_ascii_lowercase()
        && chars.any(|ch| ch.is_ascii_uppercase())
        && !tag.contains(':')
        && !tag.contains('-')
}

fn has_attribute_value(element: &RegularElement, name: &str) -> bool {
    element
        .attributes
        .iter()
        .find_map(|attribute| named_attribute(attribute, name))
        .is_some_and(attribute_has_value)
}

fn has_event_handler(element: &RegularElement, name: &str) -> bool {
    element.attributes.iter().any(|attribute| match attribute {
        Attribute::OnDirective(directive) => directive.name.as_ref().eq_ignore_ascii_case(name),
        Attribute::Attribute(attribute) => attribute_event_name(attribute.name.as_ref())
            .is_some_and(|event_name| event_name.eq_ignore_ascii_case(name)),
        _ => false,
    })
}

fn has_any_event_handler(element: &RegularElement, names: &[&str]) -> bool {
    names.iter().any(|name| has_event_handler(element, name))
}

fn collect_present_interactive_handlers(element: &RegularElement) -> Vec<String> {
    let mut handlers: Vec<String> = Vec::new();
    for attribute in &element.attributes {
        let handler_name = match attribute {
            Attribute::OnDirective(directive) => Some(directive.name.as_ref()),
            Attribute::Attribute(attribute) => attribute_event_name(attribute.name.as_ref()),
            _ => None,
        };
        let Some(handler_name) = handler_name else {
            continue;
        };
        if !A11Y_INTERACTIVE_HANDLERS
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(handler_name))
        {
            continue;
        }
        if handlers
            .iter()
            .all(|existing| !existing.as_str().eq_ignore_ascii_case(handler_name))
        {
            handlers.push(handler_name.to_ascii_lowercase());
        }
    }
    handlers
}

fn attribute_event_name(name: &str) -> Option<&str> {
    if name.len() > 2
        && name.as_bytes()[0].eq_ignore_ascii_case(&b'o')
        && name.as_bytes()[1].eq_ignore_ascii_case(&b'n')
    {
        Some(&name[2..])
    } else {
        None
    }
}

fn react_attribute_replacement(name: &str) -> Option<&'static str> {
    match name {
        "className" => Some("class"),
        "htmlFor" => Some("for"),
        _ => None,
    }
}

fn next_regular_sibling_tag(fragment: &Fragment, index: usize) -> Option<Arc<str>> {
    for sibling in fragment.nodes.iter().skip(index.saturating_add(1)) {
        match sibling {
            Node::Comment(_) => {}
            Node::Text(text) if text.data.chars().all(char::is_whitespace) => {}
            Node::RegularElement(element) => return Some(element.name.clone()),
            _ => break,
        }
    }
    None
}

fn element_implicitly_closes_with_sibling(open_tag: &str, next_tag: &str) -> bool {
    open_tag.eq_ignore_ascii_case("p") && next_tag.eq_ignore_ascii_case("p")
}

fn has_attribute_present(element: &RegularElement, name: &str) -> bool {
    element
        .attributes
        .iter()
        .any(|attribute| named_attribute(attribute, name).is_some())
}

fn attribute_value_equals_ascii_ci(element: &RegularElement, name: &str, expected: &str) -> bool {
    element
        .attributes
        .iter()
        .find_map(|attribute| named_attribute(attribute, name))
        .and_then(attribute_text_value)
        .is_some_and(|value| value.eq_ignore_ascii_case(expected))
}

fn named_attribute_value_equals_ascii_ci<'a>(
    element: &'a RegularElement,
    name: &str,
    expected: &str,
) -> Option<&'a crate::ast::modern::NamedAttribute> {
    element
        .attributes
        .iter()
        .find_map(|attribute| named_attribute(attribute, name))
        .filter(|attribute| {
            attribute_text_value(attribute)
                .is_some_and(|value| value.eq_ignore_ascii_case(expected))
        })
}

fn attribute_text_value_from_element(element: &RegularElement, name: &str) -> Option<String> {
    element
        .attributes
        .iter()
        .find_map(|attribute| named_attribute(attribute, name))
        .and_then(attribute_text_value)
}

fn named_attribute<'a>(
    attribute: &'a Attribute,
    name: &str,
) -> Option<&'a crate::ast::modern::NamedAttribute> {
    match attribute {
        Attribute::Attribute(attribute) if attribute.name.as_ref().eq_ignore_ascii_case(name) => {
            Some(attribute)
        }
        _ => None,
    }
}

fn named_attribute_from_element<'a>(
    element: &'a RegularElement,
    name: &str,
) -> Option<&'a crate::ast::modern::NamedAttribute> {
    element
        .attributes
        .iter()
        .find_map(|attribute| named_attribute(attribute, name))
}

fn has_disabled_attribute(element: &RegularElement) -> bool {
    if has_attribute_present(element, "disabled") {
        return true;
    }
    attribute_value_equals_ascii_ci(element, "aria-disabled", "true")
}

fn is_nonnegative_tabindex_value(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return true;
    }
    trimmed.parse::<f64>().is_ok_and(|number| number >= 0.0)
}

fn label_has_associated_control_in_fragment(fragment: &Fragment) -> bool {
    fragment.nodes.iter().any(node_has_label_associated_control)
}

fn node_has_label_associated_control(node: &Node) -> bool {
    match node {
        Node::RenderTag(_) | Node::Component(_) | Node::SlotElement(_) => true,
        Node::RegularElement(element) => {
            let tag = element.name.as_ref().to_ascii_lowercase();
            if matches!(
                tag.as_str(),
                "button"
                    | "input"
                    | "keygen"
                    | "meter"
                    | "output"
                    | "progress"
                    | "select"
                    | "textarea"
                    | "slot"
                    | "svelte:element"
            ) {
                return true;
            }
            label_has_associated_control_in_fragment(&element.fragment)
        }
        Node::IfBlock(block) => {
            label_has_associated_control_in_fragment(&block.consequent)
                || block
                    .alternate
                    .as_ref()
                    .is_some_and(|alternate| match alternate.as_ref() {
                        crate::ast::modern::Alternate::Fragment(fragment) => {
                            label_has_associated_control_in_fragment(fragment)
                        }
                        crate::ast::modern::Alternate::IfBlock(elseif) => {
                            label_has_associated_control_in_fragment(&elseif.consequent)
                        }
                    })
        }
        Node::EachBlock(block) => {
            label_has_associated_control_in_fragment(&block.body)
                || block
                    .fallback
                    .as_ref()
                    .is_some_and(label_has_associated_control_in_fragment)
        }
        Node::KeyBlock(block) => label_has_associated_control_in_fragment(&block.fragment),
        Node::AwaitBlock(block) => {
            block
                .pending
                .as_ref()
                .is_some_and(label_has_associated_control_in_fragment)
                || block
                    .then
                    .as_ref()
                    .is_some_and(label_has_associated_control_in_fragment)
                || block
                    .catch
                    .as_ref()
                    .is_some_and(label_has_associated_control_in_fragment)
        }
        Node::SnippetBlock(block) => label_has_associated_control_in_fragment(&block.body),
        _ => false,
    }
}

fn anchor_href_attribute<'a>(
    source: &str,
    element: &'a RegularElement,
) -> Option<(&'static str, &'a crate::ast::modern::NamedAttribute)> {
    if let Some(attribute) = named_attribute_from_element_full_name(source, element, "xlink:href") {
        return Some(("xlink:href", attribute));
    }

    named_attribute_from_element(element, "href").map(|attribute| ("href", attribute))
}

fn has_non_empty_anchor_fragment_target(element: &RegularElement) -> bool {
    ["name", "id"].iter().any(|name| {
        named_attribute_from_element(element, name)
            .and_then(attribute_text_value)
            .is_some_and(|value| !value.trim().is_empty())
    })
}

fn named_attribute_from_element_full_name<'a>(
    source: &str,
    element: &'a RegularElement,
    expected_name: &str,
) -> Option<&'a crate::ast::modern::NamedAttribute> {
    element
        .attributes
        .iter()
        .filter_map(|attribute| match attribute {
            Attribute::Attribute(attribute) => Some(attribute),
            _ => None,
        })
        .find(|attribute| {
            if attribute.name.as_ref() == expected_name {
                return true;
            }

            let start = attribute.name_loc.start.character;
            let end = attribute.name_loc.end.character;
            source
                .get(start..end)
                .is_some_and(|name| name == expected_name)
        })
}

fn attribute_has_value(attribute: &crate::ast::modern::NamedAttribute) -> bool {
    match &attribute.value {
        AttributeValueList::Boolean(value) => *value,
        AttributeValueList::Values(values) => values.iter().any(|value| match value {
            AttributeValue::Text(text) => text.data.chars().any(|ch| !ch.is_whitespace()),
            AttributeValue::ExpressionTag(_) => true,
        }),
        AttributeValueList::ExpressionTag(_) => true,
    }
}

fn attribute_text_value(attribute: &crate::ast::modern::NamedAttribute) -> Option<String> {
    match &attribute.value {
        AttributeValueList::Values(values) => {
            let mut out = String::new();
            for value in values.iter() {
                match value {
                    AttributeValue::Text(text) => out.push_str(text.data.as_ref()),
                    AttributeValue::ExpressionTag(_) => return None,
                }
            }
            Some(out)
        }
        _ => None,
    }
}

fn attribute_static_value(
    attribute: &crate::ast::modern::NamedAttribute,
) -> Option<StaticAttributeValue> {
    match &attribute.value {
        AttributeValueList::Boolean(value) if *value => Some(StaticAttributeValue::BooleanTrue),
        AttributeValueList::Values(values) => {
            let mut out = String::new();
            for value in values.iter() {
                match value {
                    AttributeValue::Text(text) => out.push_str(text.data.as_ref()),
                    AttributeValue::ExpressionTag(_) => return None,
                }
            }
            Some(StaticAttributeValue::Text(out))
        }
        _ => None,
    }
}

fn attribute_static_text(attribute: &crate::ast::modern::NamedAttribute) -> Option<String> {
    match attribute_static_value(attribute) {
        Some(StaticAttributeValue::Text(value)) => Some(value),
        _ => None,
    }
}

fn static_value_for_message(value: &StaticAttributeValue) -> String {
    match value {
        StaticAttributeValue::BooleanTrue => "true".to_string(),
        StaticAttributeValue::Text(value) => value.clone(),
    }
}

fn is_hidden_from_screen_reader(element: &RegularElement, tag: &str) -> bool {
    if tag == "input" && attribute_value_equals_ascii_ci(element, "type", "hidden") {
        return true;
    }

    let Some(aria_hidden) = named_attribute_from_element(element, "aria-hidden") else {
        return false;
    };
    match attribute_static_value(aria_hidden) {
        None => true,
        Some(StaticAttributeValue::BooleanTrue) => true,
        Some(StaticAttributeValue::Text(value)) => value.eq_ignore_ascii_case("true"),
    }
}

fn is_intrinsically_interactive(element: &RegularElement, tag: &str) -> bool {
    match tag {
        "button" | "select" | "textarea" | "option" | "menuitem" | "summary" => true,
        "a" | "area" => has_attribute_value(element, "href"),
        "input" => !attribute_value_equals_ascii_ci(element, "type", "hidden"),
        _ => false,
    }
}

fn is_known_role_name(role: &str) -> bool {
    ROLE_SUGGESTIONS
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(role))
}

fn query_role_key(role: &str) -> Option<QueryRoleKey> {
    QueryRoleKey::from_str(role).ok()
}

fn query_role_definition(role: &str) -> Option<&'static QueryRoleDefinition> {
    let key = query_role_key(role)?;
    QUERY_ROLES.get(&key)
}

fn query_property_key(name: &str) -> Option<QueryAriaProperty> {
    QueryAriaProperty::from_str(name).ok()
}

fn query_role_supports_property(role: QueryRoleKey, property: QueryAriaProperty) -> bool {
    let mut seen = FxHashSet::default();
    query_role_supports_property_inner(role, property, &mut seen)
}

fn query_role_supports_property_inner(
    role: QueryRoleKey,
    property: QueryAriaProperty,
    seen: &mut FxHashSet<QueryRoleKey>,
) -> bool {
    if !seen.insert(role) {
        return false;
    }

    let Some(definition) = QUERY_ROLES.get(&role) else {
        return false;
    };
    if definition.props.contains_key(&property) {
        return true;
    }

    for chain in &definition.super_class {
        for super_class in chain {
            let parent_role = match super_class {
                QueryRoleSuperClass::Role(role) => QueryRoleKey::from(*role),
                QueryRoleSuperClass::AbstractRole(role) => QueryRoleKey::from(*role),
            };
            if query_role_supports_property_inner(parent_role, property, seen) {
                return true;
            }
        }
    }

    false
}

fn role_has_widget_or_window_superclass(definition: &QueryRoleDefinition) -> bool {
    definition
        .super_class
        .iter()
        .flatten()
        .any(|super_class| match super_class {
            QueryRoleSuperClass::AbstractRole(role) => {
                matches!(
                    role,
                    QueryAriaAbstractRole::Widget | QueryAriaAbstractRole::Window
                )
            }
            QueryRoleSuperClass::Role(role) => {
                let role_name = role.to_string();
                role_name == "widget" || role_name == "window"
            }
        })
}

fn role_required_properties(role: &str) -> Vec<String> {
    let Some(definition) = query_role_definition(role) else {
        return Vec::new();
    };
    let mut props: Vec<String> = definition
        .required_props
        .keys()
        .map(ToString::to_string)
        .collect();
    props.sort_unstable();
    props
}

fn is_semantic_role_element(role: &str, element: &RegularElement, tag: &str) -> bool {
    if role == "switch"
        && tag == "input"
        && attribute_value_equals_ascii_ci(element, "type", "checkbox")
    {
        return true;
    }

    let Some(role_key) = query_role_key(role) else {
        return false;
    };
    let Some(concepts) = QUERY_ROLE_ELEMENTS.get(&role_key) else {
        return false;
    };
    concepts
        .iter()
        .any(|concept| match_query_role_concept(concept, element, tag))
}

fn match_query_role_concept(
    concept: &QueryRoleRelationConcept,
    element: &RegularElement,
    tag: &str,
) -> bool {
    if !concept.name.eq_ignore_ascii_case(tag) {
        return false;
    }
    let Some(schema_attributes) = concept.attributes.as_ref() else {
        return true;
    };
    schema_attributes.iter().all(|schema_attribute| {
        let Some(attribute) = named_attribute_from_element(element, &schema_attribute.name) else {
            return false;
        };
        match schema_attribute.value.as_ref() {
            Some(expected_value) => {
                attribute_static_text(attribute).is_some_and(|actual| actual == *expected_value)
            }
            None => true,
        }
    })
}

fn implicit_role_name_for_element(element: &RegularElement, tag: &str) -> Option<String> {
    if tag == "menuitem" {
        return menuitem_redundant_implicit_role(element).map(str::to_string);
    }
    if tag == "input" {
        return input_redundant_implicit_role(element).map(str::to_string);
    }

    match tag {
        "a" | "area" => Some("link".to_string()),
        "article" => Some("article".to_string()),
        "aside" => Some("complementary".to_string()),
        "body" => Some("document".to_string()),
        "button" => Some("button".to_string()),
        "datalist" => Some("listbox".to_string()),
        "dd" => Some("definition".to_string()),
        "dfn" => Some("term".to_string()),
        "details" => Some("group".to_string()),
        "dialog" => Some("dialog".to_string()),
        "dt" => Some("term".to_string()),
        "fieldset" => Some("group".to_string()),
        "figure" => Some("figure".to_string()),
        "form" => Some("form".to_string()),
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => Some("heading".to_string()),
        "hr" => Some("separator".to_string()),
        "img" => Some("img".to_string()),
        "li" => Some("listitem".to_string()),
        "link" => Some("link".to_string()),
        "main" => Some("main".to_string()),
        "menu" => Some("list".to_string()),
        "meter" => Some("progressbar".to_string()),
        "nav" => Some("navigation".to_string()),
        "ol" | "ul" => Some("list".to_string()),
        "optgroup" => Some("group".to_string()),
        "option" => Some("option".to_string()),
        "output" => Some("status".to_string()),
        "progress" => Some("progressbar".to_string()),
        "section" => Some("region".to_string()),
        "summary" => Some("button".to_string()),
        "table" => Some("table".to_string()),
        "tbody" | "tfoot" | "thead" => Some("rowgroup".to_string()),
        "textarea" => Some("textbox".to_string()),
        "tr" => Some("row".to_string()),
        _ => None,
    }
}

fn validate_aria_attribute_value(
    name: &str,
    property: &dyn AriaPropertyDefinition,
    value: &StaticAttributeValue,
) -> Option<(&'static str, String)> {
    let raw = match value {
        StaticAttributeValue::BooleanTrue => String::new(),
        StaticAttributeValue::Text(text) => text.clone(),
    };

    let lowercase = raw.to_ascii_lowercase();
    match property.property_type() {
        AriaPropertyTypeEnum::String => {
            if raw.is_empty() {
                return Some((
                    "a11y_incorrect_aria_attribute_type",
                    format!("The value of '{}' must be a non-empty string", name),
                ));
            }
        }
        AriaPropertyTypeEnum::Id => {
            if raw.is_empty() {
                return Some((
                    "a11y_incorrect_aria_attribute_type_id",
                    format!(
                        "The value of '{}' must be a string that represents a DOM element ID",
                        name
                    ),
                ));
            }
        }
        AriaPropertyTypeEnum::Idlist => {
            if raw.is_empty() {
                return Some((
                    "a11y_incorrect_aria_attribute_type_idlist",
                    format!(
                        "The value of '{}' must be a space-separated list of strings that represent DOM element IDs",
                        name
                    ),
                ));
            }
        }
        AriaPropertyTypeEnum::Boolean => {
            if lowercase != "true" && lowercase != "false" {
                return Some((
                    "a11y_incorrect_aria_attribute_type_boolean",
                    format!(
                        "The value of '{}' must be either 'true' or 'false'. It cannot be empty",
                        name
                    ),
                ));
            }
        }
        AriaPropertyTypeEnum::Integer => {
            if raw.is_empty()
                || raw
                    .parse::<f64>()
                    .map_or(true, |number| number.fract() != 0.0)
            {
                return Some((
                    "a11y_incorrect_aria_attribute_type_integer",
                    format!("The value of '{}' must be an integer", name),
                ));
            }
        }
        AriaPropertyTypeEnum::Number => {
            if raw.is_empty() || raw.parse::<f64>().is_err() {
                return Some((
                    "a11y_incorrect_aria_attribute_type",
                    format!("The value of '{}' must be a number", name),
                ));
            }
        }
        AriaPropertyTypeEnum::Token => {
            let allowed_values = property.values().copied().collect::<Vec<_>>();
            if !allowed_values.iter().any(|value| *value == lowercase) {
                return Some((
                    "a11y_incorrect_aria_attribute_type_token",
                    format!(
                        "The value of '{}' must be exactly one of {}",
                        name,
                        quoted_list(&allowed_values)
                    ),
                ));
            }
        }
        AriaPropertyTypeEnum::Tokenlist => {
            let allowed_values = property.values().copied().collect::<Vec<_>>();
            let values: Vec<&str> = lowercase.split(char::is_whitespace).collect();
            if values
                .iter()
                .any(|value| !allowed_values.iter().any(|allowed| allowed == value))
            {
                return Some((
                    "a11y_incorrect_aria_attribute_type_tokenlist",
                    format!(
                        "The value of '{}' must be a space-separated list of one or more of {}",
                        name,
                        quoted_list(&allowed_values)
                    ),
                ));
            }
        }
        AriaPropertyTypeEnum::Tristate => {
            if lowercase != "true" && lowercase != "false" && lowercase != "mixed" {
                return Some((
                    "a11y_incorrect_aria_attribute_type_tristate",
                    format!(
                        "The value of '{}' must be exactly one of true, false, or mixed",
                        name
                    ),
                ));
            }
        }
    }

    None
}

fn quoted_list(values: &[&str]) -> String {
    match values {
        [] => String::new(),
        [single] => format!("\"{}\"", single),
        [first, second] => format!("\"{}\" or \"{}\"", first, second),
        _ => {
            let prefix = values[..values.len() - 1]
                .iter()
                .map(|value| format!("\"{}\"", value))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{} or \"{}\"", prefix, values[values.len() - 1])
        }
    }
}

fn join_with_conjunction(items: &[String], conjunction: &str) -> String {
    match items.len() {
        0 => String::new(),
        1 => items[0].clone(),
        2 => format!("{} {} {}", items[0], conjunction, items[1]),
        _ => format!(
            "{} {} {}",
            items[..items.len() - 1].join(", "),
            conjunction,
            items[items.len() - 1]
        ),
    }
}

fn is_valid_autocomplete(value: Option<&StaticAttributeValue>) -> bool {
    match value {
        None => true,
        Some(StaticAttributeValue::BooleanTrue) => false,
        Some(StaticAttributeValue::Text(value)) if value.is_empty() => true,
        Some(StaticAttributeValue::Text(value)) => {
            let normalized = value.trim().to_ascii_lowercase();
            let mut tokens: Vec<&str> = if normalized.is_empty() {
                vec![""]
            } else {
                normalized.split_whitespace().collect()
            };

            if tokens
                .first()
                .is_some_and(|token| token.starts_with("section-"))
            {
                tokens.remove(0);
            }
            if tokens
                .first()
                .is_some_and(|token| AUTOCOMPLETE_ADDRESS_TOKENS.contains(token))
            {
                tokens.remove(0);
            }

            let mut accepted_field = tokens
                .first()
                .is_some_and(|token| AUTOCOMPLETE_FIELD_TOKENS.contains(token));
            if accepted_field {
                tokens.remove(0);
            } else {
                if tokens
                    .first()
                    .is_some_and(|token| AUTOCOMPLETE_CONTACT_TYPE_TOKENS.contains(token))
                {
                    tokens.remove(0);
                }
                accepted_field = tokens
                    .first()
                    .is_some_and(|token| AUTOCOMPLETE_CONTACT_FIELD_TOKENS.contains(token));
                if accepted_field {
                    tokens.remove(0);
                } else {
                    return false;
                }
            }

            if tokens
                .first()
                .is_some_and(|token| token.eq_ignore_ascii_case("webauthn"))
            {
                tokens.remove(0);
            }

            tokens.is_empty()
        }
    }
}

fn fuzzy_match<'a>(value: &str, names: &'a [&'a str]) -> Option<&'a str> {
    let mut best_match = None;
    let mut best_score = 0.0_f64;

    for name in names {
        let score = similarity(value, name);
        if score > best_score {
            best_score = score;
            best_match = Some(*name);
        }
    }

    if best_score > 0.7 { best_match } else { None }
}

fn similarity(a: &str, b: &str) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let distance = levenshtein_distance(a, b);
    let max_len = a.len().max(b.len()) as f64;
    1.0 - (distance as f64 / max_len)
}

fn levenshtein_distance(a: &str, b: &str) -> usize {
    if a == b {
        return 0;
    }
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }

    let mut costs: Vec<usize> = (0..=b.len()).collect();
    for (i, a_byte) in a.bytes().enumerate() {
        let mut previous_diagonal = i;
        costs[0] = i + 1;

        for (j, b_byte) in b.bytes().enumerate() {
            let upper = costs[j + 1];
            let cost = if a_byte == b_byte {
                previous_diagonal
            } else {
                1 + previous_diagonal.min(costs[j]).min(upper)
            };
            costs[j + 1] = cost;
            previous_diagonal = upper;
        }
    }
    costs[b.len()]
}

fn fragment_has_accessible_content(fragment: &Fragment) -> bool {
    fragment.nodes.iter().any(|node| match node {
        Node::Text(text) => text.data.chars().any(|ch| !ch.is_whitespace()),
        Node::Comment(_) => false,
        Node::ExpressionTag(_) | Node::RenderTag(_) | Node::HtmlTag(_) => true,
        Node::DebugTag(_) => false,
        Node::RegularElement(element) => element_has_accessible_content(element),
        Node::Component(_) | Node::SlotElement(_) => true,
        _ => false,
    })
}

fn element_has_accessible_content(element: &RegularElement) -> bool {
    let tag = element.name.as_ref().to_ascii_lowercase();
    if tag == "selectedcontent" {
        return true;
    }
    if has_attribute_present(element, "popover") {
        return false;
    }
    if tag == "img" {
        return has_attribute_value(element, "alt")
            || has_attribute_value(element, "aria-label")
            || has_attribute_value(element, "aria-labelledby");
    }
    fragment_has_accessible_content(&element.fragment)
}

fn contains_redundant_image_word(value: &str) -> bool {
    let lowercase = value.to_ascii_lowercase();
    lowercase
        .split(|ch: char| !ch.is_ascii_alphabetic())
        .any(|word| matches!(word, "image" | "images" | "photo" | "picture"))
}

fn opening_tag_end_from_ast(element: &RegularElement) -> usize {
    let mut end = if element.attributes.is_empty() {
        element.start + element.name.len() + 2
    } else {
        element
            .attributes
            .iter()
            .map(attribute_end_offset)
            .max()
            .unwrap_or(element.start + element.name.len() + 2)
            + 1
    };

    if element.end > end {
        end = end.min(element.end);
    }

    end
}

fn attribute_end_offset(attribute: &Attribute) -> usize {
    match attribute {
        Attribute::Attribute(attribute) => attribute.end,
        Attribute::SpreadAttribute(attribute) => attribute.end,
        Attribute::BindDirective(attribute) => attribute.end,
        Attribute::OnDirective(attribute) => attribute.end,
        Attribute::ClassDirective(attribute) => attribute.end,
        Attribute::LetDirective(attribute) => attribute.end,
        Attribute::StyleDirective(attribute) => attribute.end,
        Attribute::TransitionDirective(attribute) => attribute.end,
        Attribute::AnimateDirective(attribute) => attribute.end,
        Attribute::UseDirective(attribute) => attribute.end,
        Attribute::AttachTag(attribute) => attribute.end,
    }
}

fn is_heading_tag(tag: &str) -> bool {
    matches!(tag, "h1" | "h2" | "h3" | "h4" | "h5" | "h6")
}

fn is_void_element_tag(tag: &str) -> bool {
    matches!(
        tag.to_ascii_lowercase().as_str(),
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
