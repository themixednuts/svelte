use std::collections::{BTreeSet, HashMap, HashSet};

use oxc_ast::ast::{Declaration, Expression as OxcExpression, Statement as OxcStatement};
use oxc_codegen::{Codegen, Context, Gen};
use oxc_span::GetSpan;

use crate::api::GenerateTarget;
use crate::ast::modern::{
    Alternate, Attribute, AttributeValue, AttributeValueKind, Component as ComponentNode,
    EachBlock, Fragment, IfBlock, Node,
    RegularElement, Root, Script, SnippetBlock, SvelteElement,
};
use crate::js::{Render, codegen_options};

use super::{
    SourceReplacement, oxc_codegen_for, oxc_state_call_argument, replace_source_ranges,
};
use super::static_markup::{component_name_from_filename, escape_js_template_literal};

// ---------------------------------------------------------------------------
// Shared codegen types
// ---------------------------------------------------------------------------

/// Info about a `$state(arg)` or `$derived(arg)` rune call.
#[derive(Debug)]
pub(super) struct RuneCallInfo {
    /// The rune name (e.g. `"$state"`, `"$derived"`).
    pub rune: &'static str,
    /// The rendered argument text.
    pub argument: String,
}

/// Info about script-level async run pattern, passed from script to template compilation.
#[derive(Debug)]
pub(super) struct ServerAsyncRunInfo {
    /// Number of run slots (including empty ones).
    pub run_slot_count: usize,
    /// Variable names that are assigned via async run closures.
    pub async_vars: Vec<String>,
    /// Variable names that are reactive state ($state).
    pub state_vars: Vec<String>,
    /// Whether any sync $.derived() calls exist (needs component wrapper).
    pub has_sync_derived: bool,
    /// Promise variable name ("$$promises" for top-level script, "promises" for @const run).
    pub promise_var: String,
}

/// Result of compiling an instance `<script>` block.
#[derive(Debug)]
pub(super) struct InstanceScriptResult {
    /// The compiled script body text.
    pub body: String,
    /// Async run info, if the script uses top-level await.
    pub async_run_info: Option<ServerAsyncRunInfo>,
}

mod state;
use state::*;

mod util;
use util::*;

mod server;
use server::*;

mod instance;
use instance::*;

mod client;
use client::*;

pub(crate) fn compile_dynamic_markup_js(ctx: &super::ComponentCodegenContext<'_>) -> Option<String> {
    let _ = ctx.hmr; // TODO: HMR support
    let component_name = component_name_from_filename(ctx.filename);

    match ctx.target {
        GenerateTarget::Client => compile_client(ctx.source, ctx.root, ctx.runes_mode, &component_name),
        GenerateTarget::Server => compile_server(ctx.source, ctx.root, ctx.runes_mode, &component_name),
        GenerateTarget::None => Some(String::new()),
    }
}
