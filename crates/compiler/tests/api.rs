use std::sync::Arc;

use camino::Utf8PathBuf;
use futures::executor::block_on;
use svelte_compiler::{
    CompileOptions, CssHashGetterCallback, GenerateTarget, MigrateOptions, ModernPrintTarget,
    ParseOptions, PreprocessAttributeValue, PreprocessOptions, PreprocessOutput, PreprocessorGroup,
    PrintOptions, SourceMap, VERSION, WarningFilterCallback, compile, compile_module, migrate,
    parse, preprocess, preprocess_async, print, print_modern, walk,
};

#[test]
fn preprocess_passthrough_without_custom_steps() {
    let source = include_str!("fixtures/api/preprocess_input.svelte");
    let result =
        preprocess(source, PreprocessOptions::default()).expect("preprocess should succeed");

    assert_eq!(result.code.as_ref(), source);
    assert!(result.dependencies.is_empty());
    assert!(result.map.is_none());
}

#[test]
fn preprocess_runs_markup_and_tag_steps() {
    let source = "<h1>Hello __NAME__!</h1>\n<style color=\"red\"/>\n";
    let result = preprocess(
        source,
        PreprocessOptions {
            filename: Some(Utf8PathBuf::from("file.svelte")),
            groups: vec![PreprocessorGroup {
                markup: Some(Arc::new(|markup| {
                    Ok(Some(PreprocessOutput {
                        code: Arc::from(markup.content.replace("__NAME__", "world")),
                        ..PreprocessOutput::default()
                    }))
                })),
                style: Some(Arc::new(|style| {
                    let color = match style.attributes.get("color") {
                        Some(PreprocessAttributeValue::String(value)) => value.as_ref(),
                        _ => "",
                    };
                    Ok(Some(PreprocessOutput {
                        code: Arc::from(format!("div {{ color: {color}; }}")),
                        ..PreprocessOutput::default()
                    }))
                })),
                ..PreprocessorGroup::default()
            }]
            .into_boxed_slice(),
        },
    )
    .expect("preprocess should succeed");

    assert_eq!(
        result.code.as_ref(),
        "<h1>Hello world!</h1>\n<style color=\"red\">div { color: red; }</style>\n"
    );
}

#[test]
fn preprocess_passes_current_markup_to_tag_steps() {
    let source = "<h1>Hello __NAME__!</h1>\n<script>console.log('__NAME__');</script>\n";
    let result = preprocess(
        source,
        PreprocessOptions {
            groups: vec![PreprocessorGroup {
                markup: Some(Arc::new(|markup| {
                    Ok(Some(PreprocessOutput {
                        code: Arc::from(markup.content.replace("__NAME__", "world")),
                        ..PreprocessOutput::default()
                    }))
                })),
                script: Some(Arc::new(|script| {
                    assert!(script.markup.contains("Hello world!"));
                    Ok(Some(PreprocessOutput {
                        code: Arc::from(script.content),
                        ..PreprocessOutput::default()
                    }))
                })),
                ..PreprocessorGroup::default()
            }]
            .into_boxed_slice(),
            ..PreprocessOptions::default()
        },
    )
    .expect("preprocess should succeed");

    assert!(result.code.contains("Hello world!"));
}

#[test]
fn preprocess_collects_dependencies() {
    let source = "<style>\n\t@import './foo.css';\n</style>\n";
    let result = preprocess(
        source,
        PreprocessOptions {
            groups: vec![PreprocessorGroup {
                style: Some(Arc::new(|style| {
                    Ok(Some(PreprocessOutput {
                        code: Arc::from(
                            style
                                .content
                                .replace("@import './foo.css';", "/* removed */"),
                        ),
                        dependencies: vec![Utf8PathBuf::from("./foo.css")].into_boxed_slice(),
                        ..PreprocessOutput::default()
                    }))
                })),
                ..PreprocessorGroup::default()
            }]
            .into_boxed_slice(),
            ..PreprocessOptions::default()
        },
    )
    .expect("preprocess should succeed");

    assert_eq!(result.code.as_ref(), "<style>\n\t/* removed */\n</style>\n");
    assert_eq!(
        result.dependencies.as_ref(),
        &[Utf8PathBuf::from("./foo.css")]
    );
}

#[test]
fn preprocess_dedupes_dependencies_in_first_seen_order() {
    let source = "<style>body { color: red; }</style>";
    let result = preprocess(
        source,
        PreprocessOptions {
            groups: vec![
                PreprocessorGroup {
                    style: Some(Arc::new(|style| {
                        let _ = style;
                        Ok(Some(PreprocessOutput {
                            code: Arc::from("body { color: blue; }"),
                            dependencies: vec![
                                Utf8PathBuf::from("./shared.css"),
                                Utf8PathBuf::from("./theme.css"),
                            ]
                            .into_boxed_slice(),
                            ..PreprocessOutput::default()
                        }))
                    })),
                    ..PreprocessorGroup::default()
                },
                PreprocessorGroup {
                    markup: Some(Arc::new(|markup| {
                        Ok(Some(PreprocessOutput {
                            code: Arc::from(markup.content),
                            dependencies: vec![
                                Utf8PathBuf::from("./theme.css"),
                                Utf8PathBuf::from("./final.css"),
                            ]
                            .into_boxed_slice(),
                            ..PreprocessOutput::default()
                        }))
                    })),
                    ..PreprocessorGroup::default()
                },
            ]
            .into_boxed_slice(),
            ..PreprocessOptions::default()
        },
    )
    .expect("preprocess should succeed");

    assert_eq!(
        result.dependencies.as_ref(),
        &[
            Utf8PathBuf::from("./shared.css"),
            Utf8PathBuf::from("./theme.css"),
            Utf8PathBuf::from("./final.css"),
        ]
    );
}

#[test]
fn preprocess_runs_async_tag_steps() {
    let source = "<style>\n\t.brand-color { color: $brand; }\n</style>\n";
    let result = preprocess(
        source,
        PreprocessOptions {
            groups: vec![PreprocessorGroup {
                style_async: Some(Arc::new(|style| {
                    Box::pin(async move {
                        Ok(Some(PreprocessOutput {
                            code: Arc::from(style.content.replace("$brand", "purple")),
                            ..PreprocessOutput::default()
                        }))
                    })
                })),
                ..PreprocessorGroup::default()
            }]
            .into_boxed_slice(),
            ..PreprocessOptions::default()
        },
    )
    .expect("preprocess should succeed");

    assert_eq!(
        result.code.as_ref(),
        "<style>\n\t.brand-color { color: purple; }\n</style>\n"
    );
}

#[test]
fn preprocess_async_matches_sync_pipeline() {
    let source = "<style>\n\t.brand-color { color: $brand; }\n</style>\n";
    let options = PreprocessOptions {
        groups: vec![PreprocessorGroup {
            style_async: Some(Arc::new(|style| {
                Box::pin(async move {
                    Ok(Some(PreprocessOutput {
                        code: Arc::from(style.content.replace("$brand", "purple")),
                        ..PreprocessOutput::default()
                    }))
                })
            })),
            ..PreprocessorGroup::default()
        }]
        .into_boxed_slice(),
        ..PreprocessOptions::default()
    };

    let sync = preprocess(source, options.clone()).expect("sync preprocess should succeed");
    let async_result =
        block_on(preprocess_async(source, options)).expect("async preprocess should succeed");

    assert_eq!(async_result, sync);
}

#[test]
fn migrate_api_is_exposed() {
    let source = include_str!("fixtures/api/preprocess_input.svelte");
    let result = migrate(source, MigrateOptions::default()).expect("migrate should succeed");
    let normalized = result.code.replace("\r\n", "\n");

    assert_eq!(
        normalized.as_str(),
        "<script>\n  /**\n   * @typedef {Object} Props\n   * @property {string} [name]\n   */\n\n  /** @type {Props} */\n  let { name = \"world\" } = $props();\n</script>\n\n<h1>Hello {name}</h1>\n"
    );
}

#[test]
fn compile_component_has_no_js_map_without_request() {
    let result = compile("<h1>Hello</h1>", CompileOptions::default()).expect("compile succeeds");
    assert!(result.js.map.is_none());
    assert!(!result.metadata.runes);
    assert!(result.ast.is_some());
}

#[test]
fn compile_component_preserves_requested_js_map_slot() {
    let result = compile(
        "<h1>Hello</h1>",
        CompileOptions {
            filename: Some(Utf8PathBuf::from("input.svelte")),
            output_filename: Some(Utf8PathBuf::from("_output/client/input.svelte.js")),
            sourcemap: Some(SourceMap::default()),
            ..CompileOptions::default()
        },
    )
    .expect("compile succeeds");
    let map = result.js.map.expect("requested component sourcemap");
    assert_eq!(map.sources.as_ref(), &[Arc::from("../../input.svelte")]);
    assert!(!map.mappings.is_empty());
}

#[test]
fn compile_component_emits_css_sourcemap_when_requested() {
    let result = compile(
        "<style>.foo { color: red; }</style><div class=\"foo\"></div>",
        CompileOptions {
            filename: Some(Utf8PathBuf::from("input.svelte")),
            css_hash: Some(Arc::from("svelte-abc123")),
            css_output_filename: Some(Utf8PathBuf::from("_output/client/input.svelte.css")),
            sourcemap: Some(SourceMap::default()),
            ..CompileOptions::default()
        },
    )
    .expect("compile succeeds");

    let css = result.css.expect("css output");
    let map = css.map.expect("requested css sourcemap");
    assert_eq!(map.sources.as_ref(), &[Arc::from("../../input.svelte")]);
    assert!(!map.mappings.is_empty());
}

#[test]
fn compile_module_has_no_js_map_without_request() {
    let result = compile_module(
        "export const answer = 42;",
        CompileOptions {
            generate: GenerateTarget::Client,
            ..CompileOptions::default()
        },
    )
    .expect("compile module succeeds");
    assert!(result.js.map.is_none());
    assert!(result.metadata.runes);
    assert!(result.ast.is_none());
}

#[test]
fn compile_module_accepts_typescript_modules() {
    let source =
        "export function loadImage(src: string, onLoad: () => void) { onLoad(); return src; }";
    let result = compile_module(
        source,
        CompileOptions {
            filename: Some(Utf8PathBuf::from("image-loader.svelte.ts")),
            generate: GenerateTarget::Client,
            ..CompileOptions::default()
        },
    )
    .expect("compile module succeeds");

    assert!(result.js.code.contains("export function loadImage"));
    assert!(!result.js.code.contains(": string"));
    assert!(result.metadata.runes);
}

#[test]
fn compile_module_detects_typescript_without_filename() {
    let source = "export const withDefault = (value: string | undefined): string => value ?? 'ok';";
    let result = compile_module(
        source,
        CompileOptions {
            generate: GenerateTarget::Client,
            ..CompileOptions::default()
        },
    )
    .expect("compile module succeeds");

    assert!(result.js.code.contains("import * as $ from 'svelte/internal/client';"));
    assert!(!result.js.code.contains("withDefault"));
    assert!(!result.js.code.contains(": string"));
}

#[test]
fn compile_module_emits_script_warnings() {
    let source = "import App from './App.svelte'; function build() { return new class {}; } new App({ target });";
    let result = compile_module(
        source,
        CompileOptions {
            generate: GenerateTarget::Client,
            ..CompileOptions::default()
        },
    )
    .expect("compile module succeeds");

    assert!(
        result
            .warnings
            .iter()
            .any(|warning| warning.code.as_ref() == "legacy_component_creation"),
        "expected legacy_component_creation warning, got: {:?}",
        result.warnings
    );
    assert!(
        result
            .warnings
            .iter()
            .any(|warning| warning.code.as_ref() == "perf_avoid_inline_class"),
        "expected perf_avoid_inline_class warning, got: {:?}",
        result.warnings
    );
}

#[test]
fn compile_component_scopes_css_by_default() {
    let result = compile(
        "<style>.foo { color: red; }</style><div class=\"foo\"></div>",
        CompileOptions::default(),
    )
    .expect("compile succeeds");

    let css = result.css.expect("css output");
    assert!(css.code.contains(".foo.svelte-"));
}

#[test]
fn compile_component_uses_custom_css_hash_getter() {
    let result = compile(
        "<style>.foo { color: red; }</style><div class=\"foo\"></div>",
        CompileOptions {
            css_hash_getter: Some(CssHashGetterCallback::new(|input| {
                Arc::from(format!("custom-{}", (input.hash)(input.filename)))
            })),
            ..CompileOptions::default()
        },
    )
    .expect("compile succeeds");

    let css = result.css.expect("css output");
    assert!(css.code.contains(".foo.custom-"));
}

#[test]
fn compile_component_applies_warning_filter_callback() {
    let result = compile(
        "<svelte:component this={Thing} />",
        CompileOptions {
            generate: GenerateTarget::None,
            runes: Some(true),
            warning_filter: Some(WarningFilterCallback::new(|warning| {
                warning.code.as_ref() != "svelte_component_deprecated"
            })),
            ..CompileOptions::default()
        },
    )
    .expect("compile succeeds");

    assert!(result.warnings.is_empty());
}

#[test]
fn compile_component_emits_a11y_warnings_when_generating_client_output() {
    let result = compile(
        "<img src=\"photo.jpg\">",
        CompileOptions {
            filename: Some("/src/App.svelte".into()),
            generate: GenerateTarget::Client,
            ..CompileOptions::default()
        },
    )
    .expect("compile succeeds");

    assert!(
        result
            .warnings
            .iter()
            .any(|warning| warning.code.as_ref() == "a11y_missing_attribute"),
        "expected a11y_missing_attribute warning, got: {:?}",
        result.warnings
    );
}

#[test]
fn compile_component_emits_deprecation_warnings_when_generating_client_output() {
    let result = compile(
        "<svelte:component this={Thing} />",
        CompileOptions {
            filename: Some("/src/App.svelte".into()),
            generate: GenerateTarget::Client,
            runes: Some(true),
            ..CompileOptions::default()
        },
    )
    .expect("compile succeeds");

    assert!(
        result
            .warnings
            .iter()
            .any(|warning| warning.code.as_ref() == "svelte_component_deprecated"),
        "expected svelte_component_deprecated warning, got: {:?}",
        result.warnings
    );
}

#[test]
fn print_returns_sourcemap() {
    let ast = parse("<h1>Hello</h1>", ParseOptions::default()).expect("parse succeeds");
    let printed = print(&ast, PrintOptions::default()).expect("print succeeds");
    assert_eq!(printed.code.as_ref(), "<h1>Hello</h1>");
    assert_eq!(printed.map.sources.as_ref(), &[Arc::from("input.svelte")]);
    assert!(!printed.map.mappings.is_empty());
}

#[test]
fn parse_accepts_package_style_modern_options() {
    let ast = parse(
        "<h1>Hello</h1>",
        svelte_compiler::ParseOptions {
            filename: Some(Utf8PathBuf::from("Component.svelte")),
            root_dir: Some(Utf8PathBuf::from("src")),
            modern: Some(true),
            loose: false,
            ..Default::default()
        },
    )
    .expect("parse succeeds");

    assert!(matches!(ast.root, svelte_compiler::ast::Root::Modern(_)));
}

#[test]
fn print_modern_accepts_source_backed_subnodes() {
    let source = "<div><h1>Hello</h1></div>";
    let ast = parse(
        source,
        svelte_compiler::ParseOptions {
            modern: Some(true),
            ..Default::default()
        },
    )
    .expect("parse succeeds");
    let svelte_compiler::ast::Root::Modern(root) = &ast.root else {
        panic!("expected modern root");
    };
    let node = root.fragment.nodes.first().expect("top-level node");

    let printed = print_modern(
        ModernPrintTarget::node(source, node),
        PrintOptions::default(),
    )
    .expect("print modern succeeds");

    assert_eq!(printed.code.as_ref(), "<div><h1>Hello</h1></div>");
    assert_eq!(printed.map.sources.as_ref(), &[Arc::from("input.svelte")]);
    assert!(!printed.map.mappings.is_empty());
}

#[test]
fn compiler_version_matches_svelte_package() {
    assert_eq!(VERSION, "5.53.9");
}

#[test]
#[should_panic(
    expected = "'svelte/compiler' no longer exports a `walk` utility — please import it directly from `estree-walker` instead"
)]
fn deprecated_walk_panics_with_upstream_message() {
    walk();
}
