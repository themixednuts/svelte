# Compiler Alignment Plan

This document tracks alignment of the Rust compiler crate with the JavaScript compiler layout at
`packages/svelte/src/compiler`.

Architecture blueprint companion:

- `crates/svelte-compiler/docs/rust-port-architecture.md`

## Current target layout

- JS root: `packages/svelte/src/compiler/index.js`
- JS phases:
  - `phases/1-parse`
  - `phases/2-analyze`
  - `phases/3-transform`
- JS compatibility layer:
  - `legacy.js`

Rust scaffolding now mirrors this shape:

- `crates/svelte-compiler/src/compiler/mod.rs`
- `crates/svelte-compiler/src/compiler/legacy.rs`
- `crates/svelte-compiler/src/compiler/phases/parse/mod.rs`
- `crates/svelte-compiler/src/compiler/phases/analyze/mod.rs`
- `crates/svelte-compiler/src/compiler/phases/transform/mod.rs`
- `crates/svelte-compiler/src/compiler/phases/transform/codegen.rs`
- `crates/svelte-compiler/src/compiler/phases/transform/codegen/static_markup.rs`

Parse phase now has dedicated submodules:

- `crates/svelte-compiler/src/compiler/phases/parse/cst.rs` for CST-driven component parsing
- `crates/svelte-compiler/src/compiler/phases/parse/oxc.rs` for Oxc program parsing helpers
- `crates/svelte-compiler/src/compiler/phases/parse/css.rs` for stylesheet parsing entrypoint

Bundler bridge scaffolding:

- `crates/svelte-vite-rolldown/` for native Rust-facing Vite/Rolldown bridge types
- includes `RustCompilerBridge` that calls `svelte-compiler::compile` and returns JS/CSS/map payloads
- includes request classification helpers for `.svelte`, `.svelte.js`, and virtual CSS ids
- includes JSON protocol helper (`transform_json`) for host/runtime adapters
- `crates/svelte-vite-rolldown-napi/` N-API host crate exposing sync/json transform entrypoints
- `packages/svelte-vite-rolldown-bridge/` runtime-agnostic JS bridge helpers + Vite plugin glue + compiler-compat surface
  - runtime-neutral core exports
  - Node adapter isolated under `/node` subpath

The public `lib.rs` entrypoints now route through this compiler facade.

## Migration stages

1. Move orchestration from `api.rs` into `compiler/mod.rs`.
2. Move parsing-specific logic into `compiler/phases/parse`.
3. Move validation and semantic analysis into `compiler/phases/analyze`.
4. Move JS/CSS output generation into `compiler/phases/transform`.
5. Keep `api.rs` focused on public types and thin compatibility wrappers.

### Stage status

- Stage 1: completed (public entrypoints route through `compiler/mod.rs` facade)
- Stage 2: completed for compile flow (`parse` split into `parse/cst.rs` and `parse/oxc.rs`; root parsing and script/style regions are CST/root-derived with no range fallback)
- Stage 3: completed for compile flow (`analyze` owns compile/module validation entrypoints and compile path now receives required parsed root)
- Stage 4: completed for compile flow (`transform` owns compile/compile_module/print orchestration and output assembly; static-markup source fallback removed)
- Stage 5: in progress (`api.rs` still contains detector/source-scan logic that should continue moving into phase modules)

Recent extraction progress:

- compile output assembly moved to `compiler/phases/transform/output.rs`
- Oxc parse logic centralized in `parse/oxc.rs` via `SvelteOxcParser`
- static-markup JS generation moved into transform codegen module
- root types are now namespaced under `ast::legacy::Root` and `ast::modern::Root`
- compile component path now fails fast if root parse fails (no optional-root fallback path)
- analyze validation is split into `validation/component/{template,css,imports,snippet,runes}.rs` and `validation/module.rs`
- component validation is strict `&Root` (no optional-root path)
- diagnostics now use typed Oxc parse helpers for import-forbidden, dollar-prefix, and rune-argument-count checks
- non-semantic trim usage removed from analyze and static-markup whitespace checks
- api validation internals are now split into `api/validation/{css,imports,runes,snippet,template}.rs`, reducing `api.rs` monolith surface
- template-source checks (`svelte:options` children, raw mustache spacing, unquoted/unterminated attribute expressions) now live in `api/validation/template.rs`
- import/runes/snippet validation now consumes parsed script AST programs (`ModernScript.content`) instead of direct script-source scanning for import/export binding checks
- module export/import rule checks are now AST-based in `api/validation/imports.rs` and reused by `validate_module_program`
- rune/module diagnostics for props/bindable/host/effect/derived/state placement, invalid rune names, and constant assignments now run through AST walkers in `api/validation/runes.rs`
- removed source-string fallback path for scoped store subscription validation; component scoped checks now use root/fragment traversal only
- component/module store-subscription and dollar-binding diagnostics are now routed through `api/validation/runes.rs` AST passes
- removed now-dead source-based rune/module validator implementations from `api.rs` and cleaned unused parse region exports
- render-tag invalid call validation and `{#each}` header `$state(...)` checks are now root-AST driven in `api/validation/runes.rs`
- template/snippet validator entrypoints are further de-monolithed: snippet structural checks moved into `api/validation/snippet.rs`; `svelte:self` placement, `each` key-without-as, and script `arguments` usage checks are now root/AST-based in `api/validation/template.rs`
- scoped store-subscription diagnostics are now AST-walked from template/script expressions in `api/validation/runes.rs` (removed fragment source slicing helpers from `api.rs`)
- remaining template validation implementation clusters (directive validation, const-tag checks, slot/attribute checks, EOF/continuation checks) were moved out of `api.rs` into `api/validation/template.rs`
- `api.rs` no longer contains `detect_*` validator implementations; validation entrypoints now delegate through split validation modules
- shared source scanner helpers were split into `api/source_scan.rs` (tag tokenization, char-boundary scanning, brace/paren matching)
- shared raw AST traversal helpers were split into `api/raw_ast.rs` (`estree_node_*`, walkers, node-span, identifier extraction)
- runes-mode inference helpers were split into `api/runes_mode.rs`
- renamed source scanner helper module to `api/scan.rs` (dropped prefixed filename style)
- legacy/modern CST-root entrypoints now live in their respective modules and are re-exported from `api.rs` (`legacy::parse_root_from_cst`, `modern::parse_root_from_cst`)
- removed legacy-root alias indirection in `api.rs` in favor of direct namespaced legacy root types
- rune missing-parentheses validation now uses script AST traversal (`ModernScript.content`) instead of source scanning
- modern-specific collapsed-tag recovery and root comment/location builders were moved from `api.rs` to `api/modern.rs`
- canonical `Node`/`Fragment`/`Script` types now live in `ast::legacy` and `ast::modern` (no type aliases in AST modules), with parser/analyze callsites starting to consume namespaced `ast::...::Node` directly
- all `Legacy*`/`Modern*` AST type definitions were moved out of `api.rs` into `ast/legacy.rs` and `ast/modern.rs`; `api.rs` now re-exports them instead of defining them inline
- `ast::legacy` and `ast::modern` type names are now unprefixed within their modules (`Element`, `Expression`, `Css`, etc.); legacy `Legacy*`/`Modern*` names are currently provided only as `api.rs` re-export aliases for transition
- legacy-only UTF-16 remap and legacy-style conversion helpers were moved from `api.rs` into `api/legacy.rs`
- modern tag-comment CST extraction helpers (`collect_modern_tag_comments`, `parse_modern_tag_comment`) were moved from `api.rs` into `api/modern.rs`
- modern→legacy loose node/attribute conversion helpers were moved from `api.rs` into `api/legacy.rs`
- legacy→modern loose node/attribute/expression conversion helpers were moved from `api.rs` into `api/modern.rs`
- legacy loose-recovery predicates and modern→legacy expression fallback helpers were moved from `api.rs` into `api/legacy.rs` (`legacy_nodes_need_loose_recovery`, `legacy_node_needs_loose_recovery`, `legacy_node_has_unclosed_start_tag`, `legacy_expression_from_modern_or_empty`, `modern_expression_bounds`)
- modern element-name extraction and modern error-node recovery helpers were moved from `api.rs` into `api/modern.rs` (`modern_element_name`, `recover_modern_error_nodes`)
- legacy doctype extraction helper moved from `api.rs` into `api/legacy.rs` (`parse_legacy_doctype_node`)
- removed `api/css_output.rs`; compile CSS output now routes directly through transform phase (`compiler/phases/transform/css.rs`), with `transform/output.rs` calling phase-local CSS generation instead of API-module wiring
- modern CSS body node parsing is now routed through parse phase (`parse::parse_modern_css_nodes` / `parse/css.rs`) and consumed by both API CST style parsing and transform CSS output paths
- CSS scoping/pruning/rewrite engine moved from `api.rs` into `compiler/phases/transform/css/` (usage.rs, scoping.rs, rewrite.rs); `build_css_usage_context` now uses AST traversal for element/block collection with source-based class fallbacks for expression-derived candidates
- `ModernCssParser` moved to `compiler/phases/parse/css.rs`; transform phase no longer depends on api.rs CSS surface
- modern script/style/options CST parsers (`parse_modern_script`, `parse_modern_style`, `parse_modern_options`) moved from `api.rs` to `api/modern.rs`
- modern expression parsing now routes through Oxc AST parse + raw-node specialization (Identifier/Literal/Binary/Call), instead of source-string expression fallback splitting
- legacy root CST parse path no longer triggers source-range loose-recovery fallbacks during normal parse flow
- modern ERROR-node recovery no longer falls back to legacy loose source conversion
- removed dead legacy source-range fallback utilities from `api.rs` (`parse_legacy_nodes_from_source_range*`, `recover_legacy_*_from_source`, and related loose source token walkers)
- removed dead modern legacy-loose conversion fallback tail from `api/modern.rs` that depended on legacy source-range recovery helpers
- moved valid closing-tag boundary scan helper (`find_valid_legacy_closing_tag_start`) from `api.rs` to `api/scan.rs`

Current no-fallback validation snapshot:

- `parser-legacy` ported fixture suite currently passes end-to-end, including malformed recovery
  cases such as `textarea-end-tag`, `no-error-if-before-closing`, `loose-unclosed-block`, and
  `loose-unclosed-tag`.
- `parser-modern` ported fixture suite currently passes end-to-end.
- textarea malformed-close handling now uses CST-guided span recovery with valid close-tag
  boundary checks (keeps malformed `</textaread...` as text; only valid `</textarea...>` closes).

### `raw_ast` role and removal target

- `api/raw_ast.rs` is currently a transition shim for `EstreeNode`-based expression/program shapes that are
  not yet represented as fully typed Oxc nodes in all parse/validation paths.
- It should shrink over time as validation and parse adapters consume typed Oxc structures directly.
- End target: keep only minimal bridge helpers (or remove module entirely) once no compile-path logic depends
  on `EstreeNode` field-walking.

## `api.rs` full-scan breakdown (current)

Current remaining function surface in `api.rs` is still large (~100 top-level fns), and can be
grouped into these coherent clusters:

1. Public API facade/types (keep in `api.rs`)
2. ~~CSS scoping/pruning/rewrite engine (move to transform phase module)~~ — done: `compiler/phases/transform/css/`
3. ~~Custom CSS parser/tokenizer internals (move to parse CSS module)~~ — done: `ModernCssParser` in `parse/css.rs`
4. Modern/legacy parse bridges and loose conversion helpers (move to `api/modern.rs` and
   `api/legacy.rs`, then parse phase)
5. Expression parse fallback adapters around Oxc (move to `parse/oxc.rs`)
6. Recovery/range-reparse utilities (keep as explicit recovery modules, not general parse path)
7. Generic scan/location helpers (move to `api/scan.rs` or parse utility module)

### CST/AST compliance rule (no fallback policy)

- Normal compile path must consume CST/AST first.
- Source-string scanning is allowed only for explicit malformed-input recovery modules.
- Printing/formatting-only normalization may keep string-level rewrite helpers.
- Any detector/parser that uses source slicing when CST/AST already contains the field is a gap and
  should be rewritten.

### Target module map

- `api.rs`: public facade only (options/results/errors/re-exports)
- `api/legacy.rs`: legacy CST parse + legacy conversion + legacy recovery
- `api/modern.rs`: modern CST parse + modern recovery
- `api/raw_ast.rs`: generic raw AST walkers/extractors
- `api/scan.rs`: shared scanner primitives
- `compiler/phases/parse/oxc.rs`: expression/program parse normalization, comment attachment
- `compiler/phases/parse/css.rs`: CSS parse authority
- `compiler/phases/transform/css/*` (new): selector scoping/pruning/animation rewrite pipeline
- `compiler/phases/analyze/validation/*`: final home for validation logic currently under `api/validation/*`

### Planned migration order (non-incremental batches)

1. ~~Extract CSS transform engine from `api.rs` into `compiler/phases/transform/css/*`.~~
2. ~~Extract custom CSS parser internals to `compiler/phases/parse/css.rs`.~~
3. Move remaining legacy/modern parse/recovery bridges from `api.rs` to `api/{legacy,modern}.rs`.
4. Centralize expression fallback adapters in `compiler/phases/parse/oxc.rs`.
5. Shrink `api.rs` to facade/re-exports only and route all compile behavior through phase modules.

## Notes from vue-oxc-parser

Repository: `E:/Projects/vue-oxc-parser`

Patterns to follow:

- Central parser object carrying allocator/source/state (`VueOxcParser`).
- Parse pipeline outputs a typed return struct (`ParserReturn`) with program + diagnostics.
- Oxc-backed expression parsing with source-offset preservation.
- Distinct semantic pass (`parse_for_semantic`) implemented as AST transform pass.

Additional patterns from `oxc-angular-compiler`:

- Explicit phase modules (`parser`, `transform`, `pipeline`, `output`) with stable public re-exports.
- Build-tool integration separated from core compiler, with dedicated binding/plugin layers.
- Filtered Vite plugin hooks and direct HMR-specific routing paths.

Applicability to Svelte:

- Build a `SvelteOxcParser` phase module for JS expression/program handling.
- Separate parse and semantic passes explicitly.
- Keep source offsets stable through transforms for fixture parity.

## Rolldown and Vite direct-usage plan

1. Add a `@svelte-rs/compiler` JS binding package exposing parse/compile APIs.
2. Provide a Vite plugin that calls Rust for Svelte file transforms.
3. Ensure plugin hooks are filter-based for Rolldown/Vite performance:
   - `transform.filter.id` for `\.svelte($|\?)`
   - `load.filter.id` for virtual CSS/module ids
4. Return explicit module type when generating JS/CSS virtual modules.
5. Keep compatibility with Vite plugin API first; validate with `rolldown-vite` second.
6. Add a fixture-driven integration test matrix:
   - Vite (dev/build)
   - rolldown-vite (dev/build)

Immediate bridge next steps:

1. Add a Node-facing host layer (N-API or Wasm binding) that forwards Vite plugin hooks into `RustCompilerBridge`.
2. Wire `transform` + `load` paths using `classify_request_id` and `should_transform_id`.
3. Preserve sourcemap JSON round-tripping and CSS virtual-module handoff semantics.

Compatibility target:

- Keep JS-facing compiler compatibility via `packages/svelte-vite-rolldown-bridge/src/compat-svelte-compiler.js`
- Support existing Vite-plugin ecosystems by allowing aliasing/adapter layers on top of the bridge client

This sequence keeps us on stable Vite APIs while preparing for native Rolldown performance wins.

## AST alignment: JS source vs Rust legacy/modern

Reference: `packages/svelte/src/compiler/types/template.d.ts`

### Attribute types (element.attributes)

| JS (template.d.ts) | Legacy Rust | Modern Rust | Status |
|-------------------|-------------|-------------|--------|
| `Attribute` | `Attribute(NamedAttribute)` | `Attribute(NamedAttribute)` | OK |
| `SpreadAttribute` | `Spread(SpreadAttribute)` | `SpreadAttribute(SpreadAttribute)` | Fixed |
| `BindDirective` | `Binding(DirectiveAttribute)` | `BindDirective(DirectiveAttribute)` | OK |
| `OnDirective` | `EventHandler(DirectiveAttribute)` | `OnDirective(DirectiveAttribute)` | OK |
| `ClassDirective` | `Class(DirectiveAttribute)` | `ClassDirective(DirectiveAttribute)` | OK |
| `StyleDirective` | `StyleDirective(StyleDirective)` | `StyleDirective(StyleDirective)` | Fixed |
| `TransitionDirective` | `Transition(TransitionDirective)` | `TransitionDirective(TransitionDirective)` | Fixed |
| `AnimateDirective` | `Animation(DirectiveAttribute)` | `AnimateDirective(DirectiveAttribute)` | Fixed |
| `UseDirective` | `Action(DirectiveAttribute)` | `UseDirective(DirectiveAttribute)` | Fixed |
| `LetDirective` | `Let(DirectiveAttribute)` | `LetDirective(DirectiveAttribute)` | Fixed |
| `AttachTag` | — | `AttachTag(AttachTag)` | Modern only |

### Element types (fragment.nodes / children)

Rust uses `RegularElement`/`Component` with `name` to distinguish; JS uses dedicated types.

| JS (ElementLike) | Legacy Rust Node | Modern Rust Node | Notes |
|------------------|------------------|------------------|-------|
| `RegularElement` | `Element` | `RegularElement` | OK |
| `Component` | `InlineComponent` | `Component` | OK |
| `SlotElement` | (in Element name=slot) | `SlotElement` | OK |
| `SvelteElement` | `Element` name=svelte:element | `RegularElement` name=svelte:element | Represented by name |
| `SvelteComponent` | `InlineComponent` name=svelte:component | `Component` name=svelte:component | Represented by name |
| `SvelteBody` | — | `RegularElement`? | Would need name=svelte:body |
| `SvelteDocument` | — | — | Not parsed |
| `SvelteFragment` | — | — | Not parsed |
| `SvelteBoundary` | — | — | Not parsed (await block internal) |
| `SvelteHead` | `Head` | — | Legacy only |
| `SvelteWindow` | — | — | Not parsed |
| `SvelteSelf` | — | `Component` name=svelte:self? | Would need name check |
| `TitleElement` | — | `RegularElement` name=title? | Special document title |
| `SvelteOptionsRaw` | — | — | Parse intermediate only |

### Block types

| JS (Block) | Legacy Rust | Modern Rust | Status |
|------------|-------------|-------------|--------|
| `EachBlock` | `EachBlock` | `EachBlock` | OK |
| `IfBlock` | `IfBlock` | `IfBlock` | OK |
| `AwaitBlock` | `AwaitBlock` | `AwaitBlock` | OK |
| `KeyBlock` | `KeyBlock` | `KeyBlock` | OK |
| `SnippetBlock` | `SnippetBlock` | `SnippetBlock` | OK |

### Tag types (inline expressions)

| JS (Tag) | Legacy Rust | Modern Rust | Status |
|----------|-------------|-------------|--------|
| `ExpressionTag` | `MustacheTag` | `ExpressionTag` | OK |
| `HtmlTag` | — | `HtmlTag` | Modern only |
| `RenderTag` | — | `RenderTag` | Modern only |
| `ConstTag` | — | `ConstTag` | Modern only |
| `DebugTag` | `DebugTag` | `DebugTag` | Fixed |
| `AttachTag` | — | `ExpressionTag` (?) | Modern has AttachTag as attribute |

### Attribute value (Attribute.value)

| JS | Legacy Rust AttributeValueList | Modern Rust AttributeValueList | Status |
|----|--------------------------------|-------------------------------|--------|
| `true` (boolean) | `Boolean(bool)` | `Boolean(bool)` | OK |
| `ExpressionTag` | `Values([MustacheTag])` | `ExpressionTag(ExpressionTag)` | OK |
| `Array<Text \| ExpressionTag>` | `Values([Text, MustacheTag, ...])` | `Values([Text, ExpressionTag, ...])` | OK |

### RegularElement metadata (JS)

JS `RegularElement.metadata` includes:
- `has_spread: boolean` — set from `attributes.some(a => a.type === 'SpreadAttribute')`
- `svg`, `mathml`, `scoped`, `path`, `synthetic_value_node`

Rust modern `RegularElement` has no metadata field. `has_spread` is derived via `has_spread_attributes_from_ast` (AST walk).

### SpreadAttribute (fixed)

1. Added `SpreadAttribute(SpreadAttribute)` to `ast::modern::Attribute` enum.
2. Added `SpreadAttribute` struct (start, end, expression) to `ast::modern`.
3. In `parse_modern_attributes`: when attribute node has `spread_attribute` child, parse and push `ModernAttribute::SpreadAttribute`.
4. In `modern_attributes_from_legacy_loose`: map `LegacyAttribute::Spread` → `ModernAttribute::SpreadAttribute`.
5. In `build_css_usage_context`: `has_spread_attributes` derived via `has_spread_attributes_from_ast` (AST walk).
6. Updated printing, warnings, static_markup, runes validation, legacy_attributes_from_modern.

### Other missing nodes (from JS template.d.ts)

Previously missing and now implemented:

- `LetDirective` in legacy and modern AST.
- `StyleDirective`, `TransitionDirective`, `AnimateDirective`, `UseDirective` are now retained in modern AST (not dropped during legacy->modern loose conversion).
- `DebugTag` in legacy and modern AST.

**Elements** (represented by name in Rust, dedicated type in JS):
- `SvelteElement`, `SvelteComponent`, `SvelteBody`, `SvelteDocument`, `SvelteFragment`, `SvelteBoundary`, `SvelteHead`, `SvelteWindow`, `SvelteSelf` — Rust uses `RegularElement`/`Component` with matching `name`. JS has dedicated types with type-specific fields (e.g. `SvelteElement.tag`, `SvelteComponent.expression`).
- `TitleElement` — JS special type for `<title>`. Rust would use `RegularElement` name=title.

**Metadata** (JS analysis-phase fields, not in Rust):
- `RegularElement.metadata` — `svg`, `mathml`, `has_spread`, `scoped`, `path`, `synthetic_value_node`
- `Fragment.metadata` — `transparent`, `dynamic`
- `Component.metadata` — `expression`, `scopes`, `dynamic`, `snippets`, `path`
- Block metadata — `keyed`, `contains_group_binding`, `is_controlled`, etc.

Rust derives equivalent info on demand (e.g. `has_spread` via AST walk) rather than storing in metadata.

### CST / tree-sitter validation for these nodes

- tree-sitter grammar already tokenizes these forms via existing generic shapes:
  - directives through `attribute -> attribute_name` (`let:`, `style:`, `transition:`, `animate:`, `use:`)
  - inline tags through `tag_kind` + `expression_value` (`{@debug ...}`)
- Rust CST checks now assert these shapes are present (`cst_directive_and_debug_tag_shapes`).
- Remaining parity work is at AST/type-model and transform/analyze behavior layers, not grammar coverage.

## Dependency baseline

Workspace currently pins:

- Oxc crates at `0.115.0`
- N-API crates (`napi`, `napi-derive`, `napi-build`) at latest compatible versions
- workspace `rust-version` updated to `1.91`
