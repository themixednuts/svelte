# Svelte Rust Port Vision

This document is the persistent guide for the Rust port effort. It is not a task list.
It defines the mission, architecture direction, quality bar, and sequencing so every implementation decision can be checked against one source of truth.

## Mission

Build a single high-performance Rust binary named `svelte` that eventually covers:

- compiler
- language server (LSP)
- SvelteKit workflows
- project creation (`sv create` equivalent)
- formatter
- linting
- future ecosystem tooling

The first milestone is compiler parity and correctness against existing Svelte compiler tests.

## Non-Negotiable Product Goals

1. **Parity first**: test behavior must match Svelte compiler fixtures before feature expansion.
2. **Zero-copy by default**: keep source text borrowed as long as possible; allocate only when needed for ownership boundaries.
3. **Performance discipline**: deterministic fixtures, benchmark gates, and no hidden regressions.
4. **API and DX quality**: stable, ergonomic public APIs and clear diagnostics.
5. **Extensibility**: architecture must make new passes and features additive, not invasive.
6. **Single binary end-state**: all major capabilities converge under `svelte` subcommands.

## Architecture Direction

Rust workspace is organized to keep core logic independent from adapters.

- `svelte-compiler`: parser, analysis, transforms, codegen, diagnostics contracts.
- `svelte-cli` (binary package `svelte`): command UX and orchestration.
- `svelte-test-fixtures`: parity harness over existing JS fixture directories.

Planned expansion (after compiler baseline):

- `svelte-lsp`: LSP adapter using `tower-lsp-server`.
- `svelte-kit`: kit-oriented commands and project graph tooling.
- `svelte-fmt` and `svelte-lint`: formatting and linting pipelines.

## Syntax Architecture (CST -> AST)

Use `tree-sitter-htmlx` as the baseline concrete syntax tree (CST) layer.

- CST responsibility: lossless syntax, trivia, incremental edit resilience, and fast recovery.
- AST responsibility: semantic normalization and compiler-friendly typed structures.
- Lowering boundary: one explicit CST -> AST phase with stable node ids and source spans.

This architecture is preferred for long-term maintainability because grammar evolution remains localized in CST, while semantic passes operate on a stable AST contract.

## Core Primitive Policy

Compiler core should converge on explicit native primitives and trait-based ergonomics:

- `BytePos` and `Span` as canonical byte-offset range types.
- `SourceId` for stable source identity across CST/AST/diagnostics.
- line/column `LineColumn` for human-facing diagnostics only.
- avoid ad-hoc `(start, end)` tuples in public contracts where primitives are available.

Design expectations:

- implement native traits (`Copy`, `Eq`, `Ord`, `Hash`, `Display`, serde derives where needed);
- keep primitives small (`repr(transparent)` where appropriate);
- centralize conversion logic instead of repeated per-pass mapping code.

## Lifetime and Allocation Policy

- accept borrowed source (`&str`) at parse boundaries by default;
- keep CST parsing zero-copy over source bytes when possible;
- move to owned allocations only at explicit API boundaries (serialization, caching, cross-thread transport);
- document ownership boundaries in each public module to prevent accidental cloning.

Ownership/container preference:

- prefer `Arc<T>`, `Box<T>`, and `Box<[T]>` over `String`/`Vec<T>` in core types when semantically appropriate;
- use borrowed references and lifetimes in internal pipelines even when signatures become generic-heavy;
- keep clone-cost visible in API review (cheap clone handles preferred over deep copies).

Typestate guidance:

- use typestate patterns for stateful pipelines (for example parser configuration, lowered/validated pass stages);
- allow multiple generic parameters when they improve invalid-state prevention at compile time.

## External Dependencies and Policy

- CLI: `clap` with `derive`.
- LSP: `tower-lsp-server` community fork.
- Diagnostics: `miette`, `thiserror`.
- Formatting and linting (later phase): Oxc/Oxlint crates from VoidZero.
- Type checking (later phase): TSGo-backed pipeline integration.
- Zero-copy parsing primitives: `winnow`, `memchr`, arenas (`bumpalo`), compact containers (`smallvec`).
- Syntax ecosystem: tree-sitter crates from `../tree-sitter-htmlx` used strategically where they improve reliability and iteration speed.

Dependency policy:

- prefer stable crates and explicit versions;
- minimize heavy runtime dependencies in core compiler crate;
- adapter crates may carry async/runtime dependencies.

## Quality Gates

Every meaningful change should satisfy these gates:

1. fixture tests run (compiler suites first),
2. no accidental API break in core crate,
3. diagnostics remain machine-parseable and human-readable,
4. measurable runtime/memory trend is non-regressive for touched paths.
5. `cargo +nightly clippy --workspace --all-targets -- -D warnings` passes.

## TDD Operating Model

We follow test-first vertical slices:

1. port and run fixture tests,
2. implement only the minimum needed to pass the next failing slice,
3. refactor behind stable public interfaces,
4. repeat until parity,
5. only then broaden into LSP/Kit/fmt/lint layers.

No implementation-first broad rewrites in compiler core.

## Immediate Program Roadmap

1. establish Rust workspace and `svelte` binary shell,
2. port compiler fixture harness (parser modern/legacy, compiler-errors, css) against existing JS fixture directories,
3. drive parser + diagnostics parity,
4. drive compile output parity,
5. add benchmarking and profiling gates,
6. expand binary capabilities in modular adapters.

## Decision Heuristics

When trade-offs appear, prefer:

- correctness over novelty,
- explicit APIs over implicit magic,
- composable modules over global state,
- measured optimization over speculative optimization,
- fixture parity over ad-hoc examples.
