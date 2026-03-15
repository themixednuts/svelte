# Rust Compiler Port Plan (Strict-Fail)

## Rule
- Never use passthrough/fake placeholder behavior to make tests green.
- Unsupported behavior must fail explicitly with typed errors.

## Current Baseline
- Bridge crate tests pass.
- Compiler fixture suites fail primarily due to missing general component codegen.
- Strict-fail behavior is active (unsupported shapes return `unimplemented`).

## Status (2026-03-05)
- `snapshot_js_suite_ported`, `sourcemaps_suite_ported`, `css_suite_ported` pass.
- `validator_suite_ported` and `validator_suite_ported_smoke` pass after fixing `GenerateTarget::None` module state-declarator handling in AST module codegen.
- `emit_component` now calls a single AST dispatcher entrypoint (`compile_component_js_code`) with strict-fail on unsupported shapes.
- Added typed server structural codegen module for a subset (`Text`, `Comment`, `ExpressionTag`, `HtmlTag`, `RegularElement` with plain attributes, `IfBlock`, `EachBlock`) and externalized server wrapper template with `include_str!`.
- `js_unported_suites_compile_smoke` moved from `4877` to `4843` failures; remaining failures are still dominated by unimplemented client paths and richer server node/directive/script handling.

## Implementation Order

1. Diagnostic precedence fixes
- Ensure semantic diagnostics are emitted before codegen failures where JS parity expects them.
- Prioritize:
  - `rune_missing_parentheses` parity (`compiler-errors`).
  - `attribute_invalid_name` parity (`validator`).

2. `generate: none` / analyze-only compile behavior
- For `GenerateTarget::None`, do not run template codegen.
- Return compile result with warnings and no fake JS/CSS output behavior.
- This unblocks validator-style suites from failing on unrelated codegen gaps.

3. Replace matcher-only emit path with real lowered component IR
- Expand lower phase from pass-through state to a transform IR that covers component structure.
- Keep strict explicit failure for unhandled IR node kinds.

4. Server codegen first
- Port core server transform behavior first (simpler runtime model).
- Cover minimal common nodes and blocks:
  - text/element/attributes
  - `if`, `each`, `await`
  - snippets/slots

5. Client codegen second
- Port equivalent client transform for the same structural set.
- Then layer hydration/event/bind behavior.

6. Analysis parity required by transform
- Port required analysis state used by JS transform:
  - scope/binding classification
  - runes/props/binding-group metadata
  - slot/snippet tracking used in emission

7. CSS transform parity
- Align selector scoping, keyframe rewrites, global handling, and output behavior.

8. Source map parity
- Implement real JS/CSS source maps and preprocessor-map merge behavior.

9. Burn-down via smoke suites
- Use `js_unported_suites_compile_smoke` as rolling coverage metric.
- Expect large drop once generic element/block codegen lands.

## JS Source Mapping (Upstream -> Rust)

1. Transform entrypoints
- Upstream: `packages/svelte/src/compiler/phases/3-transform/index.js`
- Upstream client: `packages/svelte/src/compiler/phases/3-transform/client/transform-client.js`
- Upstream server: `packages/svelte/src/compiler/phases/3-transform/server/transform-server.js`
- Rust targets: `crates/svelte-compiler/src/compiler/phases/transform/mod.rs`, `.../codegen.rs`, `.../codegen/dynamic_markup.rs`

2. Server-first visitor parity
- Upstream: `packages/svelte/src/compiler/phases/3-transform/server/visitors/*`
- First set to port as typed AST paths:
  - `RegularElement.js`
  - `Fragment.js`
  - `IfBlock.js`
  - `EachBlock.js`
  - `AwaitBlock.js`
  - `Component.js`
  - `SlotElement.js`
  - `SnippetBlock.js`
- Rust target: introduce a typed server-lowered IR and emit layer under `crates/svelte-compiler/src/compiler/phases/transform/`.

3. Client visitor parity
- Upstream: `packages/svelte/src/compiler/phases/3-transform/client/visitors/*`
- Port only after server structural parity exists for the same node set.
- Focus sequence:
  - element/fragment/text
  - control blocks (`if`/`each`/`await`)
  - component/snippet/slot
  - binds/events/hydration-specific behavior

4. Shared template assembly logic
- Upstream:
  - `packages/svelte/src/compiler/phases/3-transform/client/transform-template/*`
  - `packages/svelte/src/compiler/phases/3-transform/server/visitors/shared/utils.js`
- Rust direction:
  - keep template sources in `include_str!` files
  - require strict placeholder validation
  - avoid source fallbacks; unsupported typed IR paths must return explicit typed errors

## Test Gates Per Step
- Step 1:
  - `cargo test -p svelte-compiler --test compiler_fixtures compiler_errors_suite_ported -- --nocapture`
  - `cargo test -p svelte-compiler --test compiler_fixtures validator_suite_ported_smoke -- --nocapture`
- Step 2:
  - `cargo test -p svelte-compiler --test compiler_fixtures validator_suite_ported -- --nocapture`
- Step 3+:
  - `cargo test -p svelte-compiler --test compiler_fixtures snapshot_suite_ported -- --nocapture`
  - `cargo test -p svelte-compiler --test compiler_fixtures snapshot_js_suite_ported -- --nocapture`
  - `cargo test -p svelte-compiler --test compiler_fixtures css_suite_ported -- --nocapture`
  - `cargo test -p svelte-compiler --test compiler_fixtures sourcemaps_suite_ported -- --nocapture`
  - `cargo test -p svelte-compiler --test compiler_fixtures js_unported_suites_compile_smoke -- --nocapture`
