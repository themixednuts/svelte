use std::sync::Arc;

use camino::Utf8Path;

use super::{css, sourcemap};
use crate::api::{CompileMetadata, CompileResult, OutputArtifact, Warning};
use crate::ast::{Document, modern::Root};
use crate::source::SourceText;

#[derive(Debug, Clone, Copy)]
pub(crate) struct OutputContext<'a> {
    pub source: SourceText<'a>,
    pub output_filename: Option<&'a Utf8Path>,
    pub input_map: Option<&'a crate::api::SourceMap>,
}

impl<'a> OutputContext<'a> {
    pub(crate) fn new(
        source: SourceText<'a>,
        output_filename: Option<&'a Utf8Path>,
        input_map: Option<&'a crate::api::SourceMap>,
    ) -> Self {
        Self {
            source,
            output_filename,
            input_map,
        }
    }

    pub(crate) fn include_map(self, extra_output_filename: Option<&Utf8Path>) -> bool {
        self.input_map.is_some()
            || self.output_filename.is_some()
            || extra_output_filename.is_some()
    }

    pub(crate) fn with_output_filename(self, output_filename: Option<&'a Utf8Path>) -> Self {
        Self {
            output_filename,
            ..self
        }
    }

    pub(crate) fn build_sparse_sourcemap(
        self,
        output: &'a str,
        default_source_filename: &'static str,
        hints: Vec<sourcemap::SparseMappingHint<'a>>,
    ) -> crate::api::SourceMap {
        self.build_sparse_sourcemap_for(
            output,
            self.output_filename,
            default_source_filename,
            hints,
        )
    }

    pub(crate) fn build_sparse_sourcemap_for(
        self,
        output: &'a str,
        output_filename: Option<&'a Utf8Path>,
        default_source_filename: &'static str,
        hints: Vec<sourcemap::SparseMappingHint<'a>>,
    ) -> crate::api::SourceMap {
        sourcemap::build_sparse_sourcemap(sourcemap::SparseMappingOptions {
            output,
            output_filename,
            sources: vec![self.source_map_source(default_source_filename)],
            hints,
        })
    }

    pub(crate) fn compose_input_map(
        self,
        map: Option<crate::api::SourceMap>,
    ) -> Option<crate::api::SourceMap> {
        match (map, self.input_map) {
            (Some(map), Some(input))
                if !input.mappings.is_empty()
                    || !input.sources.is_empty()
                    || !input.names.is_empty() =>
            {
                Some(sourcemap::compose_sourcemaps(&map, input))
            }
            (map, _) => map,
        }
    }

    fn source_map_source(
        self,
        default_source_filename: &'static str,
    ) -> sourcemap::SourceMapSource<'a> {
        sourcemap::SourceMapSource {
            filename: Arc::from(
                self.source
                    .filename
                    .map(Utf8Path::as_str)
                    .unwrap_or(default_source_filename),
            ),
            code: self.source.text,
        }
    }
}

pub(crate) struct BuildCompileResultArgs<'a> {
    pub ctx: OutputContext<'a>,
    pub root: &'a Root,
    pub ast: Option<Document>,
    pub js_code: Arc<str>,
    pub css_hash: Option<&'a str>,
    pub dev: bool,
    pub runes: bool,
    pub css_output_filename: Option<&'a Utf8Path>,
    pub warnings: Box<[Warning]>,
}

pub(crate) fn build_compile_result(args: BuildCompileResultArgs<'_>) -> CompileResult {
    let include_map = args.ctx.include_map(args.css_output_filename);
    let js_map = include_map.then(|| {
        args.ctx.build_sparse_sourcemap(
            &args.js_code,
            "input.svelte",
            vec![
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
        )
    });
    let js_map = args.ctx.compose_input_map(js_map);

    let css = css::generate_component_css_output(
        args.ctx,
        args.root,
        args.css_hash,
        args.dev,
        args.css_output_filename,
    )
    .map(|styles| {
        let map = args.ctx.compose_input_map(styles.map);
        OutputArtifact {
            code: Arc::from(styles.code),
            map,
            has_global: Some(false),
        }
    });

    CompileResult {
        js: OutputArtifact {
            code: args.js_code,
            map: js_map,
            has_global: None,
        },
        css,
        warnings: args.warnings,
        metadata: CompileMetadata { runes: args.runes },
        ast: args.ast,
    }
}
