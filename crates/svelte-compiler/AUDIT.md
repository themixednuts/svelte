# Svelte Rust Compiler — Architecture Audit

## Overview

The compiler currently has **~11,300 lines** across the api/ module (modern.rs: 5162, legacy.rs: 5780, scan.rs: 154, api.rs: 230). A significant portion is manual workarounds compensating for gaps in the tree-sitter grammar, duplicated logic between modern/legacy paths, and ad-hoc string-based dispatch that could leverage Rust's type system.

This audit identifies what should move to the grammar, what should be refactored in Rust, and what the target architecture should look like.

---

## Part 1: Tree-sitter Grammar Gaps

### What the grammar handles well

The grammar already provides good structure for:
- **Block syntax**: `{#if}`, `{#each}`, `{#await}`, `{#key}`, `{#snippet}` with field names (`kind`, `expression`, `binding`, `index`, `key`)
- **Expressions**: Balanced brace scanning with JS string/comment awareness via external scanner
- **Attribute expressions**: Separate `attribute_expression` vs body `expression` contexts
- **Directives**: External scanner for `_directive_marker` (bind, on, class, etc.) with `:` detection
- **Spread/shorthand attributes**: Proper node types
- **Member components**: `UI.Button` via `_member_tag_name` with scanner
- **Namespaced elements**: `svelte:head` via `_namespaced_tag_name`
- **Comments in tags**: `//` and `/* */` via scanner
- **Unterminated tags**: Recovery via newline-boundary detection

### 1.1 Grammar should provide: Distinct block kind node types

**Current**: All blocks use generic `block_kind` node (a regex `[a-zA-Z_][a-zA-Z0-9_]*`) aliased the same way. The Rust code reads the text and parses it: `text_for_node(source, kind).parse::<BlockKind>().ok()`.

**Impact**: Every block/tag/branch kind determination requires source text access. Functions like `cst_block_tag_kind`, `cst_block_branch_kind`, `cst_tag_kind` each read source.

**Recommendation**: Keep the current approach. Tree-sitter's `alias(choice("if", "key"), $.block_kind)` produces the same `block_kind` node type regardless of which alternative matched. Making them distinct would require separate rule names per block type, which complicates the grammar significantly. The Rust `.parse()` approach is clean — **this is acceptable**.

### 1.2 Grammar should provide: Expression content as a structured field

**Current**: The grammar gives `expression` nodes with optional `content` field (aliased as `js` or `ts`). The Rust code calls `parse_modern_expression` which manually strips `{` and `}` from the raw text:

```rust
// modern.rs:4037-4043
if node.kind() == "expression" && raw.len() >= 2 && raw.starts_with('{') && raw.ends_with('}') {
    return parse_modern_expression_from_text(
        &raw[1..raw.len().saturating_sub(1)], ...);
}
```

**Recommendation**: The grammar already has a `content` field on expressions. The Rust code should use `node.child_by_field_name("content")` instead of stripping braces from raw text. **This is a Rust-side fix, not a grammar change.**

### 1.3 Grammar should handle: Comment delimiter stripping

**Current**: `parse_modern_comment` manually strips `<!--` and `-->`:
```rust
// modern.rs:853-854
.strip_prefix("<!--").and_then(|inner| inner.strip_suffix("-->"))
```

**Recommendation**: Low priority. HTML comments are inherited from base HTML grammar which treats them as opaque text. The Rust stripping is simple and correct. **Keep as-is.**

### 1.4 Grammar should handle: Spread attribute content extraction

**Current**: `parse_modern_attributes` in legacy.rs manually strips `{...` and `}`:
```rust
// legacy.rs:2141-2143
raw_ref.strip_prefix("{...").and_then(|text| text.strip_suffix('}'))
```

**Recommendation**: The grammar already has `spread_attribute` with a `content` field. The Rust code should use `node.child_by_field_name("content")` instead. **Rust-side fix.**

### 1.5 Grammar should handle better: Unquoted attribute value with single expression

**Current**: For `foo={}`, tree-sitter produces the `expression` as a direct child of `attribute` (not inside `unquoted_attribute_value`). The Rust code handles this in the attribute match with a separate `"expression"` arm.

**Status**: This works correctly after the loose-expression fix. The grammar design is intentional — `unquoted_attribute_value` requires leading text before an expression (for patterns like `class=item-{type}`). A bare `foo={}` uses the `attribute_expression` alias path. **Keep as-is.**

### 1.6 Grammar improvement: Reduce need for ERROR node recovery

**Current Rust recovery functions** (HIGH PRIORITY):
- `recover_modern_error_nodes` — 180+ lines reconstructing structure from ERROR nodes
- `recover_each_header_without_as_key` — manual parsing of malformed each headers
- `recover_malformed_snippet_block` — snippet block recovery
- `recover_snippet_block_missing_right_brace` — manual brace recovery
- `recover_snippet_block_missing_right_paren` — manual paren recovery
- `recover_legacy_unquoted_attribute_value` — manual brace matching for attribute values
- `recover_modern_invalid_attribute_from_error` — manual text inspection for ERROR nodes
- `parse_collapsed_tag_sequence_from_text` — 200+ lines of manual comment/text parsing

**Recommendation**: Many of these exist because tree-sitter's default error recovery is generic. The grammar should add more `prec(-1, ...)` recovery alternatives for common malformations:
- Snippet blocks with missing `)` or `}`
- Each blocks without `as` keyword
- Attributes with unmatched expression braces

This is the **highest-value grammar improvement**.

### 1.7 Grammar should handle: Brace/paren matching

**Current**: `find_matching_brace_close` (80 lines) and `find_matching_paren` (50 lines) in Rust implement JS-aware balanced delimiter matching. The scanner already does this for expressions via `scan_balanced_expr`.

**Problem**: These Rust functions exist because when the grammar produces ERROR nodes, the Rust code needs to manually find boundaries that the grammar failed to detect.

**Recommendation**: If grammar error recovery is improved (1.6), these functions can be eliminated. They're symptoms of insufficient grammar coverage. **Fix the grammar, delete these.**

---

## Part 2: Rust Architecture Anti-Patterns

### 2.1 EstreeNode as BTreeMap — The Core Problem

**Current**: Expressions are represented as `Expression(EstreeNode, ExpressionSyntax)` where `EstreeNode` is `BTreeMap<String, EstreeValue>`. Construction looks like:

```rust
let mut fields = BTreeMap::new();
fields.insert("type".to_string(), EstreeValue::String(Arc::from("Identifier")));
fields.insert("start".to_string(), EstreeValue::UInt(start as u64));
fields.insert("end".to_string(), EstreeValue::UInt(end as u64));
fields.insert("name".to_string(), EstreeValue::String(Arc::from("")));
Expression(EstreeNode { fields }, Default::default())
```

**11+ sites** construct expressions this way. Every field access is a string lookup.

**Why it exists**: The JS AST (estree) is dynamic — oxc parses into JSON-like structures, and the output must serialize to match the JS parser's JSON output exactly. A fully-typed Rust AST for estree would be enormous.

**Recommendation**: Keep `EstreeNode` for oxc interop, BUT:
1. Add builder functions for common patterns:
   ```rust
   impl EstreeNode {
       fn identifier(name: &str, start: usize, end: usize) -> Self { ... }
       fn member_expression(object: EstreeNode, property: EstreeNode) -> Self { ... }
   }
   ```
2. Add typed accessor methods instead of raw field lookups:
   ```rust
   impl EstreeNode {
       fn node_type(&self) -> Option<&str> { ... }
       fn start(&self) -> Option<usize> { ... }
       fn end(&self) -> Option<usize> { ... }
   }
   ```
   Some of these already exist (`estree_node_type`, `estree_value_to_usize`) but they're free functions, not methods.

### 2.2 String-based dispatch everywhere

**Current pattern**: Node kinds determined by string comparison:
```rust
match child.kind() {
    "text" | "entity" => { ... }
    "comment" => { ... }
    "expression" => { ... }
    "block" => { ... }
    "element" => { ... }
    "ERROR" => { ... }
    _ => {}
}
```

This is repeated in: `parse_root`, `parse_modern_regular_element`, `parse_modern_nodes_slice`, `parse_modern_options_fragment`, `push_modern_attribute_value_part`, and `parse_modern_attributes`.

**Assessment**: This is inherent to tree-sitter's API — `node.kind()` returns `&str`. However:

**Recommendation**: Create a `CstKind` enum and a conversion function:
```rust
enum CstKind {
    Text, Entity, Comment, Expression, Block, Tag, Element,
    StartTag, EndTag, SelfClosingTag, Error, Other,
}

impl CstKind {
    fn from_node(node: TsNode) -> Self { ... }
}
```
This centralizes the string-to-enum mapping and makes match arms exhaustive.

### 2.3 Duplicated modern/legacy paths

**Problem**: `modern.rs` (5162 lines) and `legacy.rs` (5780 lines) share significant logic:
- Both parse elements, attributes, text nodes, comments, blocks
- `parse_modern_attributes` lives in legacy.rs despite being used by modern path
- `parse_modern_text`, `parse_modern_comment` used by both
- Block kind enums (`BlockKind`, `BlockBranchKind`, `TagKind`) defined in legacy.rs but used in modern.rs
- `line_column_at_offset` defined in modern.rs but used everywhere

**Recommendation**: Extract shared logic into a `shared.rs` or `cst_utils.rs` module:
- `CstKind` enum and conversion
- `BlockKind`, `BlockBranchKind`, `TagKind` enums and their `FromStr` impls
- `line_column_at_offset`, `text_for_node`, `find_first_named_child`
- `parse_modern_text`, `parse_modern_comment`
- `parse_modern_attributes` (rename to `parse_attributes`)
- Balanced delimiter matching utilities

### 2.4 Functions with too many parameters

**Examples**:
- `parse_modern_element_node(source, node, in_shadowroot_template, in_svelte_head, loose)` — 5 params
- `parse_modern_regular_element(source, node, in_shadowroot_template, in_svelte_head, loose)` — 5 params
- `shorthand_directive_identifier_expression(source, name_node, head, name_loc, start, end)` — 6 params
- `modern_identifier_expression_with_loc(name, start, end, line, column)` — 5 params

**Recommendation**: Use context structs:
```rust
struct ParseContext<'src> {
    source: &'src str,
    loose: bool,
    in_shadowroot_template: bool,
    in_svelte_head: bool,
}
```

### 2.5 Ad-hoc directive parsing via string splitting

**Current**: `parse_directive_head` in legacy.rs splits attribute names like `bind:value|modifier` by string operations to extract directive kind, name, and modifiers.

**Assessment**: The grammar's `__attribute_directive` rule already produces structured children:
- `attribute_directive` (the kind: "bind", "on", etc.)
- `attribute_identifier` (the name after `:`)
- `attribute_modifiers` with individual `attribute_modifier` children

**Recommendation**: Use the CST structure instead of re-parsing the attribute name string. Read `attribute_directive`, `attribute_identifier`, and `attribute_modifiers` children from the CST node. **This eliminates ~50 lines of string splitting.**

### 2.6 Missing trait-based polymorphism for node types

**Current**: Element classification uses a `classify_element_name` function that returns an `ElementKind` enum, then a large match dispatches to different node constructors. Each node type (RegularElement, Component, SlotElement, SvelteHead, etc.) is a separate struct with overlapping fields.

**Recommendation**: Consider a trait-based approach:
```rust
trait SvelteNode: Serialize {
    fn start(&self) -> usize;
    fn end(&self) -> usize;
    fn fragment(&self) -> &Fragment;
}
```
But be cautious — the current flat enum approach works well for serialization. Only refactor if it simplifies the code, not just for abstraction's sake.

### 2.7 `Arc<str>` overuse for short-lived strings

**Current**: Almost all string values use `Arc<str>`:
```rust
pub name: Arc<str>,        // element name like "div"
pub data: Arc<str>,        // text content
pub raw: Arc<str>,         // raw text content
```

**Assessment**: `Arc` is for shared ownership. If strings are constructed once and only read, `Box<str>` is cheaper (no atomic refcount overhead). If strings are compared against known values, interning or enum variants are better.

**Recommendation**:
- For element names, directive kinds, block kinds: Already using enums in the right places
- For text content that's never shared: Consider `Box<str>` or even `&'src str` if lifetimes allow
- For strings that are genuinely shared (e.g., scripts stored in both `js` and `module`/`instance`): Keep `Arc<str>`

This is a **low priority** optimization — measure before changing.

### 2.8 `line_column_at_offset` called 50+ times

**Current**: Every time a source location is needed, `line_column_at_offset` scans from the start of source to the offset, counting newlines. This is O(n) per call.

**Recommendation**: Build a line-offset index once during parsing:
```rust
struct LineIndex {
    line_starts: Vec<usize>,  // byte offsets of each line start
}

impl LineIndex {
    fn new(source: &str) -> Self { ... }
    fn line_col(&self, offset: usize) -> (usize, usize) {
        // Binary search in line_starts — O(log n)
    }
}
```
This is a common pattern (used by rust-analyzer, oxc, etc.). **Medium priority** — improves performance for large files.

---

## Part 3: Refactoring Priority

### Tier 1: Grammar Improvements (highest value)

1. **Add error recovery rules** for common malformations in the Svelte grammar:
   - Snippet blocks with missing delimiters
   - Each blocks without `as`
   - Unclosed expression braces in attributes

   This eliminates the need for 400+ lines of Rust recovery code.

2. **Fix expression content access**: Use `child_by_field_name("content")` instead of stripping `{`/`}` from raw text. Pure Rust fix, no grammar change needed.

3. **Fix spread content access**: Same — use the `content` field the grammar already provides.

### Tier 2: Rust Architecture (high value)

4. **Extract shared module**: Move shared types and utilities out of modern.rs/legacy.rs into a dedicated module. This reduces the 11K-line monster into focused modules.

5. **Parse context struct**: Replace 5-6 parameter functions with a `ParseContext` struct threaded through parsing.

6. **EstreeNode builders**: Add `EstreeNode::identifier()`, `EstreeNode::with_loc()`, etc. Eliminates 11+ manual BTreeMap construction sites.

7. **CstKind enum**: Centralize node kind string-to-enum mapping. Makes match arms exhaustive.

8. **Directive parsing from CST**: Use the grammar's structured directive children instead of re-parsing attribute name strings.

### Tier 3: Polish (medium value)

9. **LineIndex**: Build once, binary-search for line/column. Eliminates O(n) per-call overhead.

10. **EstreeNode accessor methods**: Move free functions like `estree_node_type`, `estree_value_to_usize` into impl blocks.

11. **Deduplicate pattern binding collectors**: 4 near-identical functions (`collect_pattern_binding_names` in warnings.rs × 2, validation/template.rs, and `collect_rest_pattern_identifiers_inner`) differ only in return type (Vec vs FxHashSet, Arc<str> vs String). Use a generic `PatternCollector` trait:
    ```rust
    trait PatternCollector {
        fn push(&mut self, name: &str);
    }
    fn collect_pattern_bindings<C: PatternCollector>(node: &EstreeNode, c: &mut C) { ... }
    ```

12. **Estree type string matching in codegen/analysis**: `js.rs:748-848` has a 40+ arm match on estree node type strings ("TSTypeAnnotation", "TSTypeReference", etc.), and `codegen/dynamic_markup.rs` does similar matching in 4+ places. Create an `EstreeKind` enum with `FromStr`.

13. **Review Arc<str> vs Box<str>**: Profile first, then convert non-shared strings.

### Tier 4: Future (lower value)

14. **Consider removing legacy path entirely**: If Svelte 5 only uses modern mode, the legacy AST format may be removable. This would cut ~5800 lines.

15. **Typestate for parse phases**: `Parsed<Cst>` → `Parsed<Ast>` → `Parsed<Validated>` to enforce phase ordering at the type level.

16. **From/Into for attribute conversions**: `legacy_attributes_from_modern` does a 10-arm match to convert modern::Attribute to legacy::Attribute — implement `From` trait per variant instead.

---

## Part 4: What the Grammar Gets Right (Do Not Change)

Based on the grammar audit, these design choices are intentional and correct:

1. **Generic `block_kind` node with text parsing** — tree-sitter's `alias(choice(...), $.block_kind)` produces one node type. Distinct rule names per block type would complicate the grammar for minimal gain. The Rust `.parse::<BlockKind>()` is clean.

2. **Generic `element` for all HTML-like structures** — Components, slots, svelte:* elements are all `element` nodes. Classification by name string is how Svelte's JS parser works too. The grammar correctly delegates semantic classification to the compiler.

3. **No directive variants in CST** — All directives are `attribute` nodes. The grammar provides structured children (`attribute_directive`, `attribute_identifier`, `attribute_modifiers`) which is sufficient. The Rust code should use these children (see 2.5) rather than re-parsing the name string.

4. **Await block complexity** — The grammar's `await_pending`, `await_branch`, `await_branch_children` containers correctly model the varying syntactic patterns of await blocks.

5. **Scanner-based expression boundary detection** — `scan_balanced_expr` and `scan_iterator` in the C scanner correctly handle JS string/comment awareness. This is the right layer for this work.

---

## File-by-File Summary

| File | Lines | Main Issues |
|------|-------|-------------|
| `modern.rs` | 5162 | Manual BTreeMap expression construction (11 sites), brace matching (80 lines), ERROR recovery (180+ lines), comment parsing (200+ lines), expression field stripping instead of using CST content field |
| `legacy.rs` | 5780 | Hosts `parse_modern_attributes` (wrong module), directive string re-parsing (should use CST children), duplicated block kind enums, attribute recovery |
| `scan.rs` | 154 | Manual brace/paren matching (should be grammar), closing tag search |
| `api.rs` | 230 | Re-exports, could be the home for shared types |
| `js.rs` | ~850 | 40+ arm estree type string matching |
| `warnings.rs` | large | 4 duplicated pattern binding collector functions |
| `codegen/dynamic_markup.rs` | large | Estree type string matching in 4+ places, attribute name string comparisons |
| Grammar (svelte) | 275 | Good block/tag structure, needs better error recovery |
| Scanner (htmlx) | 1006 | Well-structured, already handles balanced expressions |
| Scanner (svelte) | 424 | Good iterator/binding/key scanning |
