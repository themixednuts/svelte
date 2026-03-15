# Rust Port Architecture Blueprint

This document defines the target architecture for the Rust Svelte compiler port, grounded in:

- upstream JS compiler shape in `packages/svelte/src/compiler`
- current Rust crate/module state in `crates/svelte-compiler`
- strict CST/AST-first implementation policy (no source-string fallback in normal compile paths)

## 1. Goals

1. Preserve upstream behavior parity while keeping the Rust codebase maintainable.
2. Make parse/analyze/transform boundaries explicit and typed.
3. Eliminate stringly detection in compile/validation/codegen paths when CST/AST data exists.
4. Provide stable, ergonomic public APIs (`compile`, `compile_module`, `parse`, `parse_css`, `preprocess`, `migrate`).
5. Keep fixture-driven parity visible: failing suites must fail loudly until implemented.

## 2. Current State (Snapshot)

### Upstream JS pipeline

- `index.js`: normalize source, reset compiler state, validate options, parse, analyze, transform
- `compile(...)` -> `analyze_component(...)` -> `transform_component(...)`
- `compileModule(...)` -> `analyze_module(...)` -> `transform_module(...)`
- Separate `preprocess` and `migrate` implementations

### Rust pipeline

- Public facade in [lib.rs](/E:/Projects/svelte/crates/svelte-compiler/src/lib.rs)
- Compiler facade in [compiler/mod.rs](/E:/Projects/svelte/crates/svelte-compiler/src/compiler/mod.rs)
- Phases in `compiler/phases/{parse,analyze,transform,preprocess,migrate}`
- Tree-sitter CST entrypoint in [cst.rs](/E:/Projects/svelte/crates/svelte-compiler/src/cst.rs)
- Component AST in [ast.rs](/E:/Projects/svelte/crates/svelte-compiler/src/ast.rs) + `ast/{legacy,modern}`

### Structural hotspots

- `transform/codegen/dynamic_markup.rs` is large and pattern-oriented.
- `analyze/warnings.rs` is large and mixes policy + traversal concerns.
- Fixture compile-smoke for unported suites currently exposes a large parity backlog.

## 3. Target Architecture

### 3.1 Layer model (inside `svelte-compiler`)

1. **Source/CST layer**
   - input normalization, file identity, CST parse
   - source of truth for syntactic boundaries

2. **AST construction layer**
   - CST -> modern/legacy AST conversion
   - Oxc expression/program embedding (typed nodes, not raw string checks)
   - Lightning CSS AST interop for style analysis/rewrites

3. **Semantic analysis layer**
   - scope graph + bindings + references
   - validation and warnings consume semantic model, not ad-hoc source scans
   - output: typed `ComponentAnalysis` + diagnostics

4. **Lowering/IR layer**
   - lower AST+semantic model into explicit render/module IR
   - no JS text templates as primary representation
   - component and module codegen both emit from IR

5. **Emission layer**
   - JS/CSS/sourcemap emission for client/server/none targets
   - deterministic formatting and snapshot normalization

6. **Public API facade**
   - stable types/options/results
   - thin orchestration only

### 3.2 Core abstractions to standardize

1. `ComponentSource`
   - normalized source + filename + source id

2. `ParsedComponent`
   - CST + AST + parse metadata (mode/loose/ts flags)

3. `ComponentAnalysis`
   - scope tree
   - symbol table (`BindingId`, `ScopeId`)
   - template/script link edges
   - typed diagnostic context

4. `TransformState`
   - component fragment IR (nodes, effects, blocks, directives, bindings)
   - module IR (imports/exports/runes transforms)

5. `EmitArtifact`
   - `js`, `css`, `map`, warnings

These types should be explicit structs/enums in phase modules, not implicit tuple/value passing.

## 4. API/DX Direction

### 4.1 Public API contract

Keep these as stable top-level functions:

- `parse`
- `print`
- `compile`
- `compile_module`
- `parse_css`
- `preprocess`
- `migrate`

### 4.2 Behavior policy

1. API methods must be typed-first and deterministic.
2. `preprocess` and `migrate` are first-class APIs (not test-only shims).
3. Any temporary `unimplemented` paths are allowed only behind explicit diagnostics and tracked milestones.

### 4.3 include_str policy

Use `include_str!` for:

- small, static API fixtures
- deterministic unit-level compiler snapshots that should be compile-time embedded

Do **not** use `include_str!` for:

- entire upstream test suite trees (too large, reduces update agility)
- runtime-discovered fixture ecosystems that intentionally follow upstream directory structure

## 5. Crate Topology Plan

### 5.1 Near-term (current workspace)

Keep one compiler crate, but enforce internal boundaries:

- `compiler/phases/parse/*`
- `compiler/phases/analyze/*`
- `compiler/phases/lower/*` (new)
- `compiler/phases/emit/*` (new)
- `compiler/phases/transform/*` gradually narrowed to orchestration + transitional adapters

### 5.2 Mid-term (optional split once stable)

If compile times and ownership pressure warrant it, split into workspace crates:

- `svelte-compiler-core` (types + diagnostics + shared ids)
- `svelte-compiler-parse`
- `svelte-compiler-analyze`
- `svelte-compiler-lower`
- `svelte-compiler-emit`
- `svelte-compiler` facade crate re-exporting public API

Do not split crates before IR and semantic boundaries are stable.

## 6. Migration Strategy (Execution Order)

1. **Semantic model extraction**
   - isolate scope/binding/reference logic into dedicated typed model
   - route validator/warnings through model-backed queries

2. **Directive and block normalization**
   - move parser/analyzer to AST-driven directive/block handling for the large compile-smoke failure buckets

3. **Introduce `lower` IR**
   - start with small subset (text/element/if/each/await/directives)
   - dual path allowed initially: existing transform + IR-backed path for selected fixtures

4. **Replace pattern-based dynamic codegen**
   - progressively retire hand-written pattern matchers in `dynamic_markup.rs`
   - emit from `TransformState`

5. **Preprocess implementation**
   - implement typed preprocess pipeline with sourcemap/dependency accumulation parity

6. **Migrate implementation**
   - implement migration pipeline on top of parse + semantic model + structured edits

7. **Turn on failing unported compile-smoke in default suite and burn down failures**
   - keep failures visible
   - categorize by subsystem and resolve systematically

## 7. Immediate Design Tasks

1. Add phase-local typed artifacts (`ParsedComponent`, `ComponentAnalysis` skeleton, `TransformState` skeleton).
2. Introduce `lower` module and wire one minimal end-to-end path.
3. Break `dynamic_markup.rs` into:
   - `matchers/*` (temporary)
   - `ir_lowering/*`
   - `emit/*`
4. Move warning rule traversal to reusable visitor utilities over typed AST nodes.
5. Add fixture triage tooling to aggregate compile-smoke failures by diagnostic code and subsystem.

## 8. Non-Negotiable Rules

1. No hidden test suppression for parity suites.
2. No source-string parsing for logic that has CST/AST representation (except explicit malformed-recovery zones).
3. No stringly “header/body scan” for snippet/directive semantics.
4. All new behavior paths must be exercised by fixture tests.
