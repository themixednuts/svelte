use std::sync::Arc;

use camino::{Utf8Path, Utf8PathBuf};
use miette::Diagnostic;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::primitives::{SourceId, Span};
use crate::source::SourceText;

/// A byte range in source text, used by [`CompileError`] to indicate where
/// the error occurred.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourcePosition {
    /// UTF-16 character offset of the start of the error.
    pub start: usize,
    /// UTF-16 character offset of the end of the error.
    pub end: usize,
}

/// A line/column location in source text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineColumn {
    /// One-based line number.
    pub line: usize,
    /// Zero-based UTF-16 column.
    pub column: usize,
    /// UTF-16 character offset from the start of the source.
    pub character: usize,
}

/// An error produced during parsing.
///
/// Contains a machine-readable `code`, a human-readable `message`, and
/// optional source position information.
#[derive(Debug, Clone, Error, Serialize, Deserialize)]
#[error("{message}")]
pub struct CompileError {
    /// Machine-readable error code (e.g. `"expected_token"`).
    pub code: Arc<str>,
    /// Human-readable error message.
    pub message: Arc<str>,
    /// Character range where the error occurred.
    pub position: Option<Box<SourcePosition>>,
    /// Line/column location of the start of the error.
    pub start: Option<Box<LineColumn>>,
    /// Line/column location of the end of the error.
    pub end: Option<Box<LineColumn>>,
    /// File path, if known.
    pub filename: Option<Arc<Utf8PathBuf>>,
}

impl CompileError {
    /// Create an error for an unimplemented feature.
    pub fn unimplemented(feature: &'static str) -> Self {
        Self {
            code: Arc::from("unimplemented"),
            message: Arc::from(format!("{feature} is not implemented yet in rust-port")),
            position: None,
            start: None,
            end: None,
            filename: None,
        }
    }

    /// Create an internal error with the given message.
    pub fn internal(message: impl Into<Arc<str>>) -> Self {
        Self {
            code: Arc::from("internal"),
            message: message.into(),
            position: None,
            start: None,
            end: None,
            filename: None,
        }
    }

    /// Attach a source span to this error.
    pub fn with_span(mut self, span: Span) -> Self {
        self.position = Some(Box::new(SourcePosition {
            start: span.start.as_usize(),
            end: span.end.as_usize(),
        }));
        self
    }

    /// Attach a filename to this error (only if one is not already set).
    pub fn with_filename(mut self, filename: Option<&Utf8Path>) -> Self {
        if self.filename.is_none() {
            self.filename = filename.map(|path| Arc::new(path.to_path_buf()));
        }
        self
    }

    /// Attach filename information from a [`SourceText`].
    pub fn with_source_text(self, source: SourceText<'_>) -> Self {
        self.with_filename(source.filename)
    }
}

/// Diagnostic codes for Svelte-specific parse and validation errors.
///
/// Each variant carries its own error message and a `miette` diagnostic code.
/// Use [`DiagnosticKind::to_compile_error`] to convert a variant into
/// a [`CompileError`] with source position information.
#[derive(Debug, Clone, PartialEq, Eq, Error, Diagnostic)]
pub enum DiagnosticKind {
    #[error("Expected attribute value")]
    #[diagnostic(code(svelte::expected_attribute_value))]
    ExpectedAttributeValue,

    #[error("Attributes need to be unique")]
    #[diagnostic(code(svelte::attribute_duplicate))]
    AttributeDuplicate,

    #[error("Duplicate slot name '{slot}' in <{component}>")]
    #[diagnostic(code(svelte::slot_attribute_duplicate))]
    SlotAttributeDuplicate { slot: Arc<str>, component: Arc<str> },

    #[error(
        "Element with a slot='...' attribute must be a child of a component or a descendant of a custom element"
    )]
    #[diagnostic(code(svelte::slot_attribute_invalid_placement))]
    SlotAttributeInvalidPlacement,

    #[error("Found default slot content alongside an explicit slot=\"default\"")]
    #[diagnostic(code(svelte::slot_default_duplicate))]
    SlotDefaultDuplicate,

    #[error("The $ name is reserved, and cannot be used for variables and imports")]
    #[diagnostic(code(svelte::dollar_binding_invalid))]
    DollarBindingInvalid,

    #[error(
        "`{ident}` is an illegal variable name. To reference a global variable called `{ident}`, use `globalThis.{ident}`"
    )]
    #[diagnostic(code(svelte::global_reference_invalid))]
    GlobalReferenceInvalid { ident: Arc<str> },

    #[error(
        "`$state(...)` can only be used as a variable declaration initializer, a class field declaration, or the first assignment to a class field at the top level of the constructor."
    )]
    #[diagnostic(code(svelte::state_invalid_placement))]
    StateInvalidPlacement,

    #[error("`{directive}:` name cannot be empty")]
    #[diagnostic(code(svelte::directive_missing_name))]
    DirectiveMissingName { directive: Arc<str> },

    #[error("Attribute shorthand cannot be empty")]
    #[diagnostic(code(svelte::attribute_empty_shorthand))]
    AttributeEmptyShorthand,

    #[error("`bind:value` can only be used with `<input>`, `<textarea>`, `<select>`")]
    #[diagnostic(code(svelte::bind_invalid_target))]
    BindInvalidTarget,

    #[error("An `{{#each ...}}` block without an `as` clause cannot have a key")]
    #[diagnostic(code(svelte::each_key_without_as))]
    EachKeyWithoutAs,

    #[error("`$effect.active` is now `$effect.tracking`")]
    #[diagnostic(code(svelte::rune_renamed))]
    RuneRenamedEffectActive,

    #[error(
        "Cannot export state from a module if it is reassigned. Either export a function returning the state value or only mutate the state value's properties"
    )]
    #[diagnostic(code(svelte::state_invalid_export))]
    StateInvalidExport,

    #[error(
        "Cannot export derived state from a module. To expose the current derived value, export a function returning its value"
    )]
    #[diagnostic(code(svelte::derived_invalid_export))]
    DerivedInvalidExport,

    #[error("`{name}` is not defined")]
    #[diagnostic(code(svelte::export_undefined))]
    ExportUndefined { name: Arc<str> },

    #[error("`{name}` is not a valid rune")]
    #[diagnostic(code(svelte::rune_invalid_name))]
    RuneInvalidName { name: Arc<str> },

    #[error(
        "The arguments keyword cannot be used within the template or at the top level of a component"
    )]
    #[diagnostic(code(svelte::invalid_arguments_usage))]
    InvalidArgumentsUsage,

    #[error("Assigning to rvalue")]
    #[diagnostic(code(svelte::js_parse_error))]
    JsParseErrorAssigningToRvalue,

    #[error("Cannot assign to constant")]
    #[diagnostic(code(svelte::constant_assignment))]
    ConstantAssignment,

    #[error("A component can have a single top-level `<style>` element")]
    #[diagnostic(code(svelte::style_duplicate))]
    StyleDuplicate,

    #[error("A component cannot have a default export")]
    #[diagnostic(code(svelte::module_illegal_default_export))]
    ModuleIllegalDefaultExport,

    #[error("<svelte:options> cannot have children")]
    #[diagnostic(code(svelte::svelte_meta_invalid_content))]
    SvelteMetaInvalidContent,

    #[error("<svelte:window> cannot have children")]
    #[diagnostic(code(svelte::svelte_meta_invalid_content))]
    SvelteWindowInvalidContent,

    #[error("Cannot reassign or bind to snippet parameter")]
    #[diagnostic(code(svelte::snippet_parameter_assignment))]
    SnippetParameterAssignment,

    #[error("`{name}` has already been declared on this class")]
    #[diagnostic(code(svelte::state_field_duplicate))]
    StateFieldDuplicate { name: Arc<str> },

    #[error("Cannot assign to a state field before its declaration")]
    #[diagnostic(code(svelte::state_field_invalid_assignment))]
    StateFieldInvalidAssignment,

    #[error("`{name}` has already been declared")]
    #[diagnostic(code(svelte::duplicate_class_field))]
    DuplicateClassField { name: Arc<str> },

    #[error("Expected token }}")]
    #[diagnostic(code(svelte::expected_token))]
    ExpectedTokenRightBrace,

    #[error("Expected token )")]
    #[diagnostic(code(svelte::expected_token))]
    ExpectedTokenRightParen,

    #[error("Calling a snippet function using apply, bind or call is not allowed")]
    #[diagnostic(code(svelte::render_tag_invalid_call_expression))]
    RenderTagInvalidCallExpression,

    #[error("`{{@render ...}}` tags can only contain call expressions")]
    #[diagnostic(code(svelte::render_tag_invalid_expression))]
    RenderTagInvalidExpression,

    #[error("cannot use spread arguments in `{{@render ...}}` tags")]
    #[diagnostic(code(svelte::render_tag_invalid_spread_argument))]
    RenderTagInvalidSpreadArgument,

    #[error("{{@debug ...}} arguments must be identifiers, not arbitrary expressions")]
    #[diagnostic(code(svelte::debug_tag_invalid_arguments))]
    DebugTagInvalidArguments,

    #[error("beforeUpdate cannot be used in runes mode")]
    #[diagnostic(code(svelte::runes_mode_invalid_import))]
    RunesModeInvalidImportBeforeUpdate,

    #[error("Cannot use rune without parentheses")]
    #[diagnostic(code(svelte::rune_missing_parentheses))]
    RuneMissingParentheses,

    #[error("Cannot use `$props()` more than once")]
    #[diagnostic(code(svelte::props_duplicate))]
    PropsDuplicate,

    #[error("Cannot use `export let` in runes mode — use `$props()` instead")]
    #[diagnostic(code(svelte::legacy_export_invalid))]
    LegacyExportInvalid,

    #[error(
        "Cannot reassign or bind to each block argument in runes mode. Use the array and index variables instead (e.g. `array[i] = value` instead of `entry = value`, or `bind:value={{array[i]}}` instead of `bind:value={{entry}}`)"
    )]
    #[diagnostic(code(svelte::each_item_invalid_assignment))]
    EachItemInvalidAssignment,

    #[error("The `{{@const foo = ...}}` declaration is not available in this snippet")]
    #[diagnostic(code(svelte::const_tag_invalid_reference))]
    ConstTagInvalidReference,

    #[error("Cannot reference store value outside a `.svelte` file")]
    #[diagnostic(code(svelte::store_invalid_subscription_module))]
    StoreInvalidSubscriptionModule,

    #[error(
        "Declaring or accessing a prop starting with `$$` is illegal (they are reserved for Svelte internals)"
    )]
    #[diagnostic(code(svelte::props_illegal_name))]
    PropsIllegalName,

    #[error("`$bindable` must be called with zero or one arguments")]
    #[diagnostic(code(svelte::rune_invalid_arguments_length))]
    RuneInvalidArgumentsLengthBindable,

    #[error("Expected a valid CSS identifier")]
    #[diagnostic(code(svelte::css_expected_identifier))]
    CssExpectedIdentifier,

    #[error("Invalid selector")]
    #[diagnostic(code(svelte::css_selector_invalid))]
    CssSelectorInvalid,

    #[error("A `:global` selector cannot follow a `>` combinator")]
    #[diagnostic(code(svelte::css_global_block_invalid_combinator))]
    CssGlobalBlockInvalidCombinator,

    #[error("A top-level `:global {{...}}` block can only contain rules, not declarations")]
    #[diagnostic(code(svelte::css_global_block_invalid_declaration))]
    CssGlobalBlockInvalidDeclaration,

    #[error("A `:global` selector cannot be inside a pseudoclass")]
    #[diagnostic(code(svelte::css_global_block_invalid_placement))]
    CssGlobalBlockInvalidPlacement,

    #[error(
        "A `:global` selector cannot be part of a selector list with entries that don't contain `:global`"
    )]
    #[diagnostic(code(svelte::css_global_block_invalid_list))]
    CssGlobalBlockInvalidList,

    #[error("A `:global` selector cannot modify an existing selector")]
    #[diagnostic(code(svelte::css_global_block_invalid_modifier))]
    CssGlobalBlockInvalidModifier,

    #[error("A `:global` selector can only be modified if it is a descendant of other selectors")]
    #[diagnostic(code(svelte::css_global_block_invalid_modifier_start))]
    CssGlobalBlockInvalidModifierStart,

    #[error(
        "`:global(...)` can be at the start or end of a selector sequence, but not in the middle"
    )]
    #[diagnostic(code(svelte::css_global_invalid_placement))]
    CssGlobalInvalidPlacement,

    #[error("`:global(...)` must contain exactly one selector")]
    #[diagnostic(code(svelte::css_global_invalid_selector))]
    CssGlobalInvalidSelector,

    #[error(
        "`:global(...)` must not contain type or universal selectors when used in a compound selector"
    )]
    #[diagnostic(code(svelte::css_global_invalid_selector_list))]
    CssGlobalInvalidSelectorList,

    #[error(
        "Nesting selectors can only be used inside a rule or as the first selector inside a lone `:global(...)`"
    )]
    #[diagnostic(code(svelte::css_nesting_selector_invalid_placement))]
    CssNestingSelectorInvalidPlacement,

    #[error("`:global(...)` must not be followed by a type selector")]
    #[diagnostic(code(svelte::css_type_selector_invalid_placement))]
    CssTypeSelectorInvalidPlacement,

    #[error("`$bindable()` can only be used inside a `$props()` declaration")]
    #[diagnostic(code(svelte::bindable_invalid_location))]
    BindableInvalidLocation,

    #[error("`$derived` must be called with exactly one argument")]
    #[diagnostic(code(svelte::rune_invalid_arguments_length))]
    RuneInvalidArgumentsLengthDerived,

    #[error("`$effect` must be called with exactly one argument")]
    #[diagnostic(code(svelte::rune_invalid_arguments_length))]
    RuneInvalidArgumentsLengthEffect,

    #[error("`$state` must be called with zero or one arguments")]
    #[diagnostic(code(svelte::rune_invalid_arguments_length))]
    RuneInvalidArgumentsLengthState,

    #[error("`$state.raw` must be called with zero or one arguments")]
    #[diagnostic(code(svelte::rune_invalid_arguments_length))]
    RuneInvalidArgumentsLengthStateRaw,

    #[error("`$state.snapshot` must be called with exactly one argument")]
    #[diagnostic(code(svelte::rune_invalid_arguments_length))]
    RuneInvalidArgumentsLengthStateSnapshot,

    #[error("`$props` cannot be called with arguments")]
    #[diagnostic(code(svelte::rune_invalid_arguments))]
    RuneInvalidArgumentsProps,

    #[error(
        "`$props()` can only be used at the top level of components as a variable declaration initializer"
    )]
    #[diagnostic(code(svelte::props_invalid_placement))]
    PropsInvalidPlacement,

    #[error(
        "`$derived(...)` can only be used as a variable declaration initializer, a class field declaration, or the first assignment to a class field at the top level of the constructor."
    )]
    #[diagnostic(code(svelte::state_invalid_placement))]
    StateInvalidPlacementDerived,

    #[error("`$effect()` can only be used as an expression statement")]
    #[diagnostic(code(svelte::effect_invalid_placement))]
    EffectInvalidPlacement,

    #[error("`$host()` can only be used inside custom element component instances")]
    #[diagnostic(code(svelte::host_invalid_placement))]
    HostInvalidPlacement,

    #[error("`<script>` was left open")]
    #[diagnostic(code(svelte::element_unclosed))]
    ElementUnclosedScript,

    #[error("`<div>` was left open")]
    #[diagnostic(code(svelte::element_unclosed))]
    ElementUnclosedDiv,

    #[error(
        "`<svelte:self>` components can only exist inside `{{#if}}` blocks, `{{#each}}` blocks, `{{#snippet}}` blocks or slots passed to components"
    )]
    #[diagnostic(code(svelte::svelte_self_invalid_placement))]
    SvelteSelfInvalidPlacement,

    #[error(
        "Cannot use `<slot>` syntax and `{{@render ...}}` tags in the same component. Migrate towards `{{@render ...}}` tags completely"
    )]
    #[diagnostic(code(svelte::slot_snippet_conflict))]
    SlotSnippetConflict,

    #[error(
        "Cannot use explicit children snippet at the same time as implicit children content. Remove either the non-whitespace content or the children snippet block"
    )]
    #[diagnostic(code(svelte::snippet_conflict))]
    SnippetConflict,

    #[error(
        "An exported snippet can only reference things declared in a `<script module>`, or other exportable snippets"
    )]
    #[diagnostic(code(svelte::snippet_invalid_export))]
    SnippetInvalidExport,

    #[error("Snippets do not support rest parameters; use an array instead")]
    #[diagnostic(code(svelte::snippet_invalid_rest_parameter))]
    SnippetInvalidRestParameter,

    #[error("Cannot reference store value inside `<script module>`")]
    #[diagnostic(code(svelte::store_invalid_subscription))]
    StoreInvalidSubscription,

    #[error("Cannot subscribe to stores that are not declared at the top level of the component")]
    #[diagnostic(code(svelte::store_invalid_scoped_subscription))]
    StoreInvalidScopedSubscription,

    #[error("The $ prefix is reserved, and cannot be used for variables and imports")]
    #[diagnostic(code(svelte::dollar_prefix_invalid))]
    DollarPrefixInvalid,

    #[error(
        "Expected a valid element or component name. Components must have a valid variable name or dot notation expression"
    )]
    #[diagnostic(code(svelte::tag_invalid_name))]
    TagInvalidName,

    #[error("Expected whitespace")]
    #[diagnostic(code(svelte::expected_whitespace))]
    ExpectedWhitespace,

    #[error("{{@const ...}} must consist of a single variable declaration")]
    #[diagnostic(code(svelte::const_tag_invalid_expression))]
    ConstTagInvalidExpression,

    #[error("Cyclical dependency detected: a → b → a")]
    #[diagnostic(code(svelte::const_tag_cycle))]
    ConstTagCycle,

    #[error(
        "Comma-separated expressions are not allowed as attribute/directive values in runes mode, unless wrapped in parentheses"
    )]
    #[diagnostic(code(svelte::attribute_invalid_sequence_expression))]
    AttributeInvalidSequenceExpression,

    #[error(
        "{{:...}} block is invalid at this position (did you forget to close the preceding element or block?)"
    )]
    #[diagnostic(code(svelte::block_invalid_continuation_placement))]
    BlockInvalidContinuationPlacement,

    #[error("Expected token -->")]
    #[diagnostic(code(svelte::expected_token))]
    ExpectedTokenCommentClose,

    #[error("Expected token </style")]
    #[diagnostic(code(svelte::expected_token))]
    ExpectedTokenStyleClose,

    #[error("Expected token {{:else}}")]
    #[diagnostic(code(svelte::expected_token))]
    ExpectedTokenElse,

    #[error("Expected token {{:then ...}} or {{:catch ...}}")]
    #[diagnostic(code(svelte::expected_token))]
    ExpectedTokenAwaitBranch,

    #[error(
        "Valid `<svelte:...>` tag names are svelte:head, svelte:options, svelte:window, svelte:document, svelte:body, svelte:element, svelte:component, svelte:self, svelte:fragment or svelte:boundary"
    )]
    #[diagnostic(code(svelte::svelte_meta_invalid_tag))]
    SvelteMetaInvalidTag,

    #[error(
        "Attribute values containing `{{...}}` must be enclosed in quote marks, unless the value only contains the expression"
    )]
    #[diagnostic(code(svelte::attribute_unquoted_sequence))]
    AttributeUnquotedSequence,

    #[error("Block was left open")]
    #[diagnostic(code(svelte::block_unclosed))]
    BlockUnclosed,

    #[error("`</div>` attempted to close an element that was not open")]
    #[diagnostic(code(svelte::element_invalid_closing_tag))]
    ElementInvalidClosingTag,

    #[error("`</p>` attempted to close an element that was not open")]
    #[diagnostic(code(svelte::element_invalid_closing_tag))]
    ElementInvalidClosingTagP,

    #[error(
        "`</p>` attempted to close element that was already automatically closed by `<pre>` (cannot nest `<pre>` inside `<p>`)"
    )]
    #[diagnostic(code(svelte::element_invalid_closing_tag_autoclosed))]
    ElementInvalidClosingTagAutoclosed,

    #[error("Void elements cannot have children or closing tags")]
    #[diagnostic(code(svelte::void_element_invalid_content))]
    VoidElementInvalidContent,

    #[error("A component can only have one `<svelte:window>` element")]
    #[diagnostic(code(svelte::svelte_meta_duplicate))]
    SvelteMetaDuplicate,

    #[error("`<svelte:window>` tags cannot be inside elements or blocks")]
    #[diagnostic(code(svelte::svelte_meta_invalid_placement))]
    SvelteMetaInvalidPlacement,

    #[error("Unexpected end of input")]
    #[diagnostic(code(svelte::unexpected_eof))]
    UnexpectedEof,

    #[error(
        "Imports of `svelte/internal/*` are forbidden. It contains private runtime code which is subject to change without notice. If you're importing from `svelte/internal/*` to work around a limitation of Svelte, please open an issue at https://github.com/sveltejs/svelte and explain your use case"
    )]
    #[diagnostic(code(svelte::import_svelte_internal_forbidden))]
    ImportSvelteInternalForbidden,
}

impl DiagnosticKind {
    /// Return the machine-readable diagnostic code string for this variant.
    pub fn code(&self) -> &'static str {
        match self {
            Self::ExpectedAttributeValue => "expected_attribute_value",
            Self::AttributeDuplicate => "attribute_duplicate",
            Self::SlotAttributeDuplicate { .. } => "slot_attribute_duplicate",
            Self::SlotAttributeInvalidPlacement => "slot_attribute_invalid_placement",
            Self::SlotDefaultDuplicate => "slot_default_duplicate",
            Self::DollarBindingInvalid => "dollar_binding_invalid",
            Self::GlobalReferenceInvalid { .. } => "global_reference_invalid",
            Self::StateInvalidPlacement => "state_invalid_placement",
            Self::DirectiveMissingName { .. } => "directive_missing_name",
            Self::AttributeEmptyShorthand => "attribute_empty_shorthand",
            Self::BindInvalidTarget => "bind_invalid_target",
            Self::EachKeyWithoutAs => "each_key_without_as",
            Self::RuneRenamedEffectActive => "rune_renamed",
            Self::StateInvalidExport => "state_invalid_export",
            Self::DerivedInvalidExport => "derived_invalid_export",
            Self::ExportUndefined { .. } => "export_undefined",
            Self::RuneInvalidName { .. } => "rune_invalid_name",
            Self::InvalidArgumentsUsage => "invalid_arguments_usage",
            Self::JsParseErrorAssigningToRvalue => "js_parse_error",
            Self::ConstantAssignment => "constant_assignment",
            Self::StyleDuplicate => "style_duplicate",
            Self::ModuleIllegalDefaultExport => "module_illegal_default_export",
            Self::SvelteMetaInvalidContent | Self::SvelteWindowInvalidContent => {
                "svelte_meta_invalid_content"
            }
            Self::SnippetParameterAssignment => "snippet_parameter_assignment",
            Self::StateFieldDuplicate { .. } => "state_field_duplicate",
            Self::StateFieldInvalidAssignment => "state_field_invalid_assignment",
            Self::DuplicateClassField { .. } => "duplicate_class_field",
            Self::ExpectedTokenRightBrace | Self::ExpectedTokenRightParen => "expected_token",
            Self::RenderTagInvalidCallExpression => "render_tag_invalid_call_expression",
            Self::RenderTagInvalidExpression => "render_tag_invalid_expression",
            Self::RenderTagInvalidSpreadArgument => "render_tag_invalid_spread_argument",
            Self::DebugTagInvalidArguments => "debug_tag_invalid_arguments",
            Self::RunesModeInvalidImportBeforeUpdate => "runes_mode_invalid_import",
            Self::RuneMissingParentheses => "rune_missing_parentheses",
            Self::PropsDuplicate => "props_duplicate",
            Self::LegacyExportInvalid => "legacy_export_invalid",
            Self::EachItemInvalidAssignment => "each_item_invalid_assignment",
            Self::ConstTagInvalidReference => "const_tag_invalid_reference",
            Self::StoreInvalidSubscriptionModule => "store_invalid_subscription_module",
            Self::PropsIllegalName => "props_illegal_name",
            Self::RuneInvalidArgumentsLengthBindable => "rune_invalid_arguments_length",
            Self::CssExpectedIdentifier => "css_expected_identifier",
            Self::CssSelectorInvalid => "css_selector_invalid",
            Self::CssGlobalBlockInvalidCombinator => "css_global_block_invalid_combinator",
            Self::CssGlobalBlockInvalidDeclaration => "css_global_block_invalid_declaration",
            Self::CssGlobalBlockInvalidPlacement => "css_global_block_invalid_placement",
            Self::CssGlobalBlockInvalidList => "css_global_block_invalid_list",
            Self::CssGlobalBlockInvalidModifier => "css_global_block_invalid_modifier",
            Self::CssGlobalBlockInvalidModifierStart => "css_global_block_invalid_modifier_start",
            Self::CssGlobalInvalidPlacement => "css_global_invalid_placement",
            Self::CssGlobalInvalidSelector => "css_global_invalid_selector",
            Self::CssGlobalInvalidSelectorList => "css_global_invalid_selector_list",
            Self::CssNestingSelectorInvalidPlacement => "css_nesting_selector_invalid_placement",
            Self::CssTypeSelectorInvalidPlacement => "css_type_selector_invalid_placement",
            Self::BindableInvalidLocation => "bindable_invalid_location",
            Self::RuneInvalidArgumentsLengthDerived
            | Self::RuneInvalidArgumentsLengthEffect
            | Self::RuneInvalidArgumentsLengthState
            | Self::RuneInvalidArgumentsLengthStateRaw
            | Self::RuneInvalidArgumentsLengthStateSnapshot => "rune_invalid_arguments_length",
            Self::RuneInvalidArgumentsProps => "rune_invalid_arguments",
            Self::PropsInvalidPlacement => "props_invalid_placement",
            Self::StateInvalidPlacementDerived => "state_invalid_placement",
            Self::EffectInvalidPlacement => "effect_invalid_placement",
            Self::HostInvalidPlacement => "host_invalid_placement",
            Self::ElementUnclosedScript | Self::ElementUnclosedDiv => "element_unclosed",
            Self::SvelteSelfInvalidPlacement => "svelte_self_invalid_placement",
            Self::SlotSnippetConflict => "slot_snippet_conflict",
            Self::SnippetConflict => "snippet_conflict",
            Self::SnippetInvalidExport => "snippet_invalid_export",
            Self::SnippetInvalidRestParameter => "snippet_invalid_rest_parameter",
            Self::StoreInvalidSubscription => "store_invalid_subscription",
            Self::StoreInvalidScopedSubscription => "store_invalid_scoped_subscription",
            Self::DollarPrefixInvalid => "dollar_prefix_invalid",
            Self::TagInvalidName => "tag_invalid_name",
            Self::ExpectedWhitespace => "expected_whitespace",
            Self::ConstTagInvalidExpression => "const_tag_invalid_expression",
            Self::ConstTagCycle => "const_tag_cycle",
            Self::AttributeInvalidSequenceExpression => "attribute_invalid_sequence_expression",
            Self::BlockInvalidContinuationPlacement => "block_invalid_continuation_placement",
            Self::ExpectedTokenCommentClose
            | Self::ExpectedTokenStyleClose
            | Self::ExpectedTokenElse
            | Self::ExpectedTokenAwaitBranch => "expected_token",
            Self::SvelteMetaInvalidTag => "svelte_meta_invalid_tag",
            Self::AttributeUnquotedSequence => "attribute_unquoted_sequence",
            Self::BlockUnclosed => "block_unclosed",
            Self::ElementInvalidClosingTag | Self::ElementInvalidClosingTagP => {
                "element_invalid_closing_tag"
            }
            Self::ElementInvalidClosingTagAutoclosed => "element_invalid_closing_tag_autoclosed",
            Self::VoidElementInvalidContent => "void_element_invalid_content",
            Self::SvelteMetaDuplicate => "svelte_meta_duplicate",
            Self::SvelteMetaInvalidPlacement => "svelte_meta_invalid_placement",
            Self::UnexpectedEof => "unexpected_eof",
            Self::ImportSvelteInternalForbidden => "import_svelte_internal_forbidden",
        }
    }

    /// Convert this diagnostic into a [`CompileError`] with source positions
    /// computed from byte offsets.
    pub fn to_compile_error(self, source: &str, start: usize, end: usize) -> CompileError {
        self.to_compile_error_in(SourceText::new(SourceId::new(0), source, None), start, end)
    }

    /// Convert this diagnostic into a [`CompileError`] using a [`SourceText`]
    /// for position and filename information.
    pub fn to_compile_error_in(
        self,
        source: SourceText<'_>,
        start: usize,
        end: usize,
    ) -> CompileError {
        let start_location = source.location_at_offset(start);
        let end_location = source.location_at_offset(end);

        CompileError {
            code: Arc::from(self.code()),
            message: Arc::from(self.to_string()),
            position: Some(Box::new(SourcePosition {
                start: start_location.character,
                end: end_location.character,
            })),
            start: Some(Box::new(start_location)),
            end: Some(Box::new(end_location)),
            filename: source.filename.map(|path| Arc::new(path.to_path_buf())),
        }
    }
}

#[cfg(test)]
mod tests {
    use camino::Utf8Path;

    use super::DiagnosticKind;
    use crate::{SourceId, SourceText};

    #[test]
    fn compile_error_uses_source_text_locations_and_filename() {
        let source = SourceText::new(
            SourceId::new(7),
            "a\n😀b",
            Some(Utf8Path::new("input.svelte")),
        );
        let error = DiagnosticKind::ExpectedWhitespace.to_compile_error_in(
            source,
            "a\n😀".len(),
            "a\n😀b".len(),
        );

        assert_eq!(error.start.as_ref().expect("start").line, 2);
        assert_eq!(error.start.as_ref().expect("start").column, 2);
        assert_eq!(error.position.as_ref().expect("position").start, 4);
        assert_eq!(
            error.filename.as_deref().map(|path| path.as_str()),
            Some("input.svelte"),
        );
    }
}
