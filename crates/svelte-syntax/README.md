# svelte-syntax

`svelte-syntax` is the syntax layer for working with Svelte source in Rust.

It provides:

- the public Svelte AST types
- tree-sitter based CST parsing
- parser entrypoints for Svelte components and CSS
- lightweight traversal and parser utility helpers

It does not compile components into JavaScript or CSS. For that, use `svelte-compiler`.

## Install

```toml
[dependencies]
svelte-syntax = "0.1.1"
```

## Example

```rust
use svelte_syntax::{ParseMode, ParseOptions, parse};

let document = parse(
    "<script>let count = 0;</script><button>{count}</button>",
    ParseOptions {
        mode: ParseMode::Modern,
        ..ParseOptions::default()
    },
)?;

assert!(matches!(document.root, svelte_syntax::ast::Root::Modern(_)));
# Ok::<(), svelte_syntax::CompileError>(())
```

## Main APIs

- `parse` parses a component into the public AST
- `parse_svelte` parses source into a tree-sitter CST
- `parse_css` parses a stylesheet into the public CSS AST
- `SourceText` provides filename and offset helpers for parser and diagnostic work

## License

MIT
