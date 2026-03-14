# svelte-syntax

A Rust crate for parsing Svelte components into typed AST and CST
representations.

`svelte-syntax` handles the syntax layer only — it parses `.svelte` files and
CSS stylesheets into inspectable tree structures. It does **not** compile
components into JavaScript or CSS. For compilation, use `svelte-compiler`.

## Install

```toml
[dependencies]
svelte-syntax = "0.1.2"
```

## Quick start

Parse a Svelte component into the modern AST:

```rust
use svelte_syntax::{parse, ParseMode, ParseOptions};

let doc = parse(
    "<script>let count = 0;</script><button>{count}</button>",
    ParseOptions {
        mode: ParseMode::Modern,
        ..ParseOptions::default()
    },
)?;

let root = match doc.root {
    svelte_syntax::ast::Root::Modern(root) => root,
    _ => unreachable!(),
};

assert!(root.instance.is_some());
assert!(!root.fragment.nodes.is_empty());
# Ok::<(), svelte_syntax::CompileError>(())
```

Parse raw source into a tree-sitter CST:

```rust
use svelte_syntax::{SourceId, SourceText, parse_svelte};

let source = SourceText::new(SourceId::new(0), "<div>hello</div>", None);
let cst = parse_svelte(source)?;

assert_eq!(cst.root_kind(), "document");
assert!(!cst.has_error());
# Ok::<(), svelte_syntax::CompileError>(())
```

## What it provides

### AST parsing

- `parse` — parse a component into a `Document` containing either a modern or
  legacy AST root.
- `parse_modern_root` — parse directly into a `modern::Root` with typed script,
  template, and style blocks.
- `parse_modern_root_incremental` — reparse using a previous AST and CST,
  reusing unchanged subtrees via `Arc` sharing.
- `parse_css` — parse a standalone CSS stylesheet.

### CST parsing

- `parse_svelte` — parse source into a tree-sitter `Document` for low-level
  syntax tree inspection.
- `parse_svelte_incremental` — incremental CST reparse using a previous tree
  and a `CstEdit`.
- `CstParser` — configurable tree-sitter parser with typestate for language
  selection.

### JavaScript handles

- `ParsedJsProgram` — self-contained OXC program AST that owns its source and
  allocator. Access the parsed `Program` without reparsing.
- `ParsedJsExpression` — same pattern for a single JS/TS expression.

### Arena AST

- `SvelteAst` — arena-allocated AST with stable `NodeId` values, parent
  pointers, and position queries. Designed for language servers, linters, and
  formatters that need fast navigation and incremental updates.

### Utilities

- `SourceText` — borrowed source text with filename, UTF-16 offset conversion,
  and line/column lookups.
- `BytePos`, `Span`, `SourceId` — lightweight position primitives.
- Element and attribute classification helpers: `classify_element_name`,
  `classify_attribute_name`, `is_component_name`, `is_void_element_name`,
  and others.
- `CompileError` and `CompilerDiagnosticKind` — structured error types with
  source positions and diagnostic codes.

## License

MIT
