use std::sync::Arc;

use camino::Utf8Path;

use super::{css, sourcemap};
use crate::api::{CompileMetadata, CompileResult, OutputArtifact, Warning};
use crate::ast::{Document, modern::Root};

pub(crate) fn build_compile_result(
    source: &str,
    root: &Root,
    ast: Option<Document>,
    js_code: Arc<str>,
    source_filename: Option<&Utf8Path>,
    output_filename: Option<&Utf8Path>,
    css_hash: Option<&str>,
    dev: bool,
    runes: bool,
    input_map: Option<&crate::api::SourceMap>,
    include_map: bool,
    css_output_filename: Option<&Utf8Path>,
    warnings: Box<[Warning]>,
) -> CompileResult {
    let js_map = include_map.then(|| {
        sourcemap::build_sparse_sourcemap(sourcemap::SparseMappingOptions {
            output: &js_code,
            output_filename,
            sources: vec![sourcemap::SourceMapSource {
                filename: Arc::from(
                    source_filename
                        .map(Utf8Path::as_str)
                        .unwrap_or("input.svelte"),
                ),
                code: source,
            }],
            hints: vec![
                sourcemap::SparseMappingHint {
                    original: "$effect.pre",
                    generated: "$.user_pre_effect",
                    name: Some("$effect.pre"),
                },
                sourcemap::SparseMappingHint {
                    original: "$effect",
                    generated: "$.user_effect",
                    name: Some("$effect"),
                },
            ],
        })
    });
    let js_map = match (js_map, input_map) {
        (Some(map), Some(input))
            if !input.mappings.is_empty()
                || !input.sources.is_empty()
                || !input.names.is_empty() =>
        {
            Some(sourcemap::compose_sourcemaps(&map, input))
        }
        (map, None) => map,
        (map, Some(_)) => map,
    };

    let css = css::generate_component_css_output(
        source,
        root,
        css_hash,
        dev,
        source_filename,
        css_output_filename,
        include_map,
    )
    .map(|styles| {
        let map = match (styles.map, input_map) {
            (Some(map), Some(input))
                if !input.mappings.is_empty()
                    || !input.sources.is_empty()
                    || !input.names.is_empty() =>
            {
                Some(sourcemap::compose_sourcemaps(&map, input))
            }
            (map, None) => map,
            (map, Some(_)) => map,
        };
        OutputArtifact {
            code: Arc::from(styles.code),
            map,
            has_global: Some(false),
        }
    });

    CompileResult {
        js: OutputArtifact {
            code: js_code,
            map: js_map,
            has_global: None,
        },
        css,
        warnings,
        metadata: CompileMetadata { runes },
        ast,
    }
}
