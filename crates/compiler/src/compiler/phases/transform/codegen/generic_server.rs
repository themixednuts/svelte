use camino::Utf8Path;

use crate::ast::modern::Root;

use super::generic_renderer::{ServerRenderBackend, compile_generic_markup_js};

pub(crate) fn compile_generic_server_markup_js(
    source: &str,
    root: &Root,
    filename: Option<&Utf8Path>,
) -> Option<String> {
    compile_generic_markup_js::<ServerRenderBackend>(source, root, filename)
}

#[cfg(test)]
mod tests {
    use super::compile_generic_server_markup_js;
    use crate::compiler::phases::parse::parse_component_for_compile;

    fn parse_modern_root(source: &str) -> crate::ast::modern::Root {
        let parsed = parse_component_for_compile(source).expect("parse component");
        parsed.root
    }

    #[test]
    fn generic_server_codegen_supports_expression_if_each_and_components() {
        let source = "{#if ok}<Widget>{name}</Widget>{:else}<span>{@html html}</span>{/if}{#each items as item, i}<div>{item}{i}</div>{:else}<em>empty</em>{/each}";
        let root = parse_modern_root(source);
        let output =
            compile_generic_server_markup_js(source, &root, None).expect("generic server codegen");
        assert!(output.contains("if (ok) {"));
        assert!(output.contains("$.ensure_array_like(items)"));
        assert!(output.contains("$.html(html)"));
        assert!(output.contains("(Widget)($$renderer,"));
    }

    #[test]
    fn generic_server_codegen_hoists_instance_imports_and_strips_instance_export() {
        let source = "<script>import x from './x'; export let answer = 42; const b = 1;</script><p>{answer}</p>";
        let root = parse_modern_root(source);
        let output =
            compile_generic_server_markup_js(source, &root, None).expect("generic server codegen");
        assert!(output.contains("import x from './x';"));
        assert!(output.contains("let answer = 42;"));
        assert!(output.contains("const b = 1;"));
        assert!(!output.contains("export let answer"));
    }

    #[test]
    fn generic_server_codegen_supports_component_and_element_directives() {
        let source = "<Widget on:click={f} bind:value={x} class:active={ok} /><div {...attrs} bind:value={x} class:active={ok} />";
        let root = parse_modern_root(source);
        let output =
            compile_generic_server_markup_js(source, &root, None).expect("generic server codegen");
        assert!(output.contains("onclick: f"));
        assert!(output.contains("$.attributes(attrs, null, null, null)"));
        assert!(output.contains("$.attr('value', x, false)"));
    }

    #[test]
    fn generic_server_codegen_supports_optional_render_calls() {
        let source = "<script>let { snippets, snippet, optional } = $props();</script>{@render snippets[snippet]()} {@render snippets?.[snippet]?.()} {@render snippets.foo?.()} {@render optional?.()}";
        let root = parse_modern_root(source);
        let output =
            compile_generic_server_markup_js(source, &root, None).expect("generic server codegen");
        assert!(output.contains("snippets[snippet]"));
        assert!(output.contains("snippets?.[snippet]"));
        assert!(output.contains("snippets.foo"));
        assert!(output.contains("optional"));
    }

    #[test]
    fn generic_server_codegen_supports_assignment_expressions_in_blocks() {
        let source = "{#if a = 0}{/if}{#each [b = 0] as x}{x,''}{/each}{#key c = 0}{/key}{#await d = 0}{/await}{#snippet snip()}{/snippet}{@render (e = 0, snip)()}{@html f = 0, ''}<div {@attach !!(g = 0)}></div>";
        let root = parse_modern_root(source);
        let output =
            compile_generic_server_markup_js(source, &root, None).expect("generic server codegen");
        assert!(output.contains("if (a = 0)"));
        assert!(output.contains("$.ensure_array_like([b = 0])"));
        assert!(output.contains("$.await($$renderer, d = 0"));
        assert!(output.contains("$.html(f = 0, '')"));
    }
}
