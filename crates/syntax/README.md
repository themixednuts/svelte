# svelte-syntax

A Rust crate for parsing Svelte components into typed AST and CST
representations.

`svelte-syntax` handles the syntax layer only — it parses `.svelte` files and
CSS stylesheets into inspectable tree structures. It does **not** compile
components into JavaScript or CSS. For compilation, use `svelte-compiler`.

## Install

```toml
[dependencies]
svelte-syntax = "0.1.4"
```

## Quick start

Parse a Svelte component into the modern AST (Svelte 5 runes mode):

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

- `parse` — parse a component into a `Document` containing a modern
  (Svelte 5 runes) or legacy (Svelte 3/4) AST root.
- `parse_modern_root` — parse directly into a `modern::Root` with typed
  script, template, and style blocks.
- `parse_modern_root_incremental` — reparse using a previous AST and CST,
  reusing unchanged subtrees for speed.
- `parse_css` — parse a standalone CSS stylesheet.

### CST parsing

- `parse_svelte` — parse source into a tree-sitter concrete syntax tree for
  low-level inspection.
- `parse_svelte_incremental` — incremental reparse using a previous tree and
  a `CstEdit`.
- `CstParser` — configurable tree-sitter parser with language selection.

### JavaScript handles

- `JsProgram` — a parsed JavaScript/TypeScript program. Owns its source so
  you can access the AST without reparsing.
- `JsExpression` — same idea for a single JS/TS expression.

### Arena AST

- `SvelteAst` — arena-allocated AST with stable node IDs, parent pointers,
  and position queries. Designed for language servers, linters, and formatters
  that need fast navigation and incremental updates.

### Utilities

- `SourceText` — borrowed source text with filename, UTF-16 offset
  conversion, and line/column lookups.
- `BytePos`, `Span`, `SourceId` — lightweight position primitives.
- Element and attribute classification helpers: `classify_element_name`,
  `classify_attribute_name`, `is_component_name`, `is_void_element_name`,
  and others.
- `CompileError` — structured error type with source positions and diagnostic
  codes.

## License

MIT
