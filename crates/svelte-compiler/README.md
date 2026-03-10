# svelte-compiler (Rust port)

This crate ports Svelte compiler parser behavior to typed Rust ASTs.

## Porting rules for this branch

- Use typed Rust AST nodes for public parser output (legacy + modern).
- Prefer `oxc_*` crates for JavaScript/TypeScript parsing, expression parsing, and pattern parsing.
- Match JavaScript Svelte parser fixture output as closely as possible.
- Keep parser paths panic-free in production code; recover or return structured errors instead.
- Preserve existing fixture semantics before introducing new Rust-specific behavior.

## Expression and pattern parsing

- Legacy and modern expression parsing routes through OXC whenever possible.
- For ambiguous legacy block/attribute cases, parser recovery falls back to source-slice parsing while still using OXC parse output.
- Type information on snippet parameters is preserved by converting OXC pattern nodes directly into legacy expression nodes.

## HTML entity decoding

- Entity decoding is implemented with `html-escape` for robust named entity support.
- Svelte-compatible semicolon-less decoding behavior is preserved where applicable (for example, `&quot` before non-alphanumeric boundaries).

## Current known parity gaps

- A small set of legacy parser fixtures still differ in malformed-input recovery and comment attachment edge cases.
- Unicode offset parity in some legacy fixtures still needs JS-equivalent index mapping for non-BMP characters.
