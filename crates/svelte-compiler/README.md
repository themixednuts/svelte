# svelte-compiler

`svelte-compiler` provides the public compiler surface for working with Svelte in Rust.

It supports:

- parsing Svelte components into the public AST
- printing modern AST nodes back to source
- compiling components into JavaScript and CSS artifacts
- compiling rune-enabled modules
- preprocessing source with custom hooks
- best-effort source migration helpers

## Install

```toml
[dependencies]
svelte-compiler = "0.1.1"
```

## Example

```rust
use svelte_compiler::{CompileOptions, compile};

let result = compile(
    "<script>let name = 'world';</script><h1>Hello {name}</h1>",
    CompileOptions::default(),
)?;

assert!(result.js.code.contains("Hello"));
# Ok::<(), svelte_compiler::CompileError>(())
```

## Main APIs

- `compile` compiles a `.svelte` component
- `compile_module` compiles a rune-enabled JavaScript or TypeScript module
- `parse` parses a component into the public AST
- `print` and `print_modern` turn AST nodes back into Svelte source
- `preprocess` runs one or more preprocessors over source text
- `migrate` performs a best-effort migration to modern Svelte syntax

## License

MIT
