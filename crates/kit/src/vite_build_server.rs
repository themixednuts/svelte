use std::collections::{BTreeMap, BTreeSet};

use camino::Utf8Path;
use rayon::prelude::*;

use crate::{
    AssetDependencies, BuildManifestChunk, BundleStrategy, CssUrlRewriteOptions, ManifestNode,
    Result, ValidatedKitConfig, ViteBuildError, create_function_as_string, filter_fonts, find_deps,
    fix_css_urls, resolve_manifest_symlink,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InlineStylesExport {
    Identifier(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServerNodeModulePlan {
    pub index: usize,
    pub component_import: Option<String>,
    pub universal_import: Option<String>,
    pub universal_id: Option<String>,
    pub server_import: Option<String>,
    pub server_id: Option<String>,
    pub imports: Vec<String>,
    pub stylesheets: Vec<String>,
    pub fonts: Vec<String>,
    pub inline_styles: BTreeMap<String, InlineStylesExport>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientBuildAsset {
    pub file_name: String,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedInlineStylesheetModule {
    pub identifier: String,
    pub output_file: String,
    pub contents: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServerNodeBuildArtifacts {
    pub module: ServerNodeModulePlan,
    pub inline_stylesheets: Vec<PreparedInlineStylesheetModule>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedBuildOutputFile {
    pub output_path: String,
    pub contents: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BuildServerNodesPlan {
    pub node_modules: Vec<PlannedBuildOutputFile>,
    pub stylesheet_modules: Vec<PlannedBuildOutputFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlannedNodeOutputs {
    node_module: PlannedBuildOutputFile,
    stylesheet_modules: Vec<PlannedBuildOutputFile>,
}

#[derive(Debug, Clone)]
pub struct ServerNodeBuildInput<'a> {
    pub index: usize,
    pub node: &'a ManifestNode,
    pub kit: &'a ValidatedKitConfig,
    pub server_manifest: &'a BTreeMap<String, BuildManifestChunk>,
    pub client_manifest: Option<&'a BTreeMap<String, BuildManifestChunk>>,
    pub client_chunks: Option<&'a [ClientBuildAsset]>,
    pub client_entry_path: Option<&'a str>,
    pub assets_path: Option<&'a str>,
    pub static_assets: &'a BTreeSet<String>,
}

pub fn render_inline_stylesheet_module(filename: &str, exported: &str) -> String {
    format!("// {filename}\nexport default {exported};")
}

pub fn render_server_node_module(plan: &ServerNodeModulePlan) -> String {
    let mut imports = Vec::new();
    let mut exports = vec![format!("export const index = {};", plan.index)];

    if let Some(component_import) = &plan.component_import {
        imports.push(format!(
            "let component_cache;\nexport const component = async () => component_cache ??= (await import({component_import:?})).default;"
        ));
    }

    if let Some(universal_import) = &plan.universal_import {
        imports.push(format!("import * as universal from {universal_import:?};"));
        exports.push("export { universal };".to_string());
    }
    if let Some(universal_id) = &plan.universal_id {
        exports.push(format!("export const universal_id = {universal_id:?};"));
    }

    if let Some(server_import) = &plan.server_import {
        imports.push(format!("import * as server from {server_import:?};"));
        exports.push("export { server };".to_string());
    }
    if let Some(server_id) = &plan.server_id {
        exports.push(format!("export const server_id = {server_id:?};"));
    }

    exports.push(format!(
        "export const imports = {};",
        serde_json::to_string(&plan.imports).expect("imports json")
    ));
    exports.push(format!(
        "export const stylesheets = {};",
        serde_json::to_string(&plan.stylesheets).expect("stylesheets json")
    ));
    exports.push(format!(
        "export const fonts = {};",
        serde_json::to_string(&plan.fonts).expect("fonts json")
    ));

    if !plan.inline_styles.is_empty() {
        let mut entries = Vec::new();
        for (file, export) in &plan.inline_styles {
            match export {
                InlineStylesExport::Identifier(identifier) => {
                    let filename = Utf8Path::new(file).file_name().unwrap_or(file.as_str());
                    imports.push(format!(
                        "import {identifier} from '../stylesheets/{filename}.js';"
                    ));
                    entries.push(format!("\t{file:?}: {identifier}"));
                }
            }
        }
        exports.push(format!(
            "export const inline_styles = () => ({{\n{}\n}});",
            entries.join(",\n")
        ));
    }

    format!("{}\n\n{}\n", imports.join("\n"), exports.join("\n"))
}

pub fn build_server_node_artifacts(
    input: &ServerNodeBuildInput<'_>,
) -> Result<ServerNodeBuildArtifacts> {
    let mut module = ServerNodeModulePlan {
        index: input.index,
        ..ServerNodeModulePlan::default()
    };

    if let Some(component) = &input.node.component {
        let (_, chunk) = resolve_manifest_symlink(input.server_manifest, component.as_str())?;
        module.component_import = Some(format!("../{}", chunk.file));
    }

    let component_deps = input
        .node
        .component
        .as_ref()
        .and_then(|component| find_deps(input.server_manifest, component.as_str(), true));

    if let Some(universal) = &input.node.universal {
        let (_, chunk) = resolve_manifest_symlink(input.server_manifest, universal.as_str())?;
        module.universal_import = Some(format!("../{}", chunk.file));
        module.universal_id = Some(universal.as_str().to_string());
    }

    let universal_deps = input
        .node
        .universal
        .as_ref()
        .and_then(|universal| find_deps(input.server_manifest, universal.as_str(), true));

    if let Some(server) = &input.node.server {
        let (_, chunk) = resolve_manifest_symlink(input.server_manifest, server.as_str())?;
        module.server_import = Some(format!("../{}", chunk.file));
        module.server_id = Some(server.as_str().to_string());
    }

    let mut eager_assets = BTreeSet::new();
    if let (Some(client_manifest), Some(client_entry_path)) =
        (input.client_manifest, input.client_entry_path)
    {
        if matches!(input.kit.output.bundle_strategy, BundleStrategy::Split)
            && (input.node.universal.is_some() || input.node.component.is_some())
        {
            if let Some(entry_deps) = find_deps(client_manifest, client_entry_path, true) {
                let mut eager_css = BTreeSet::new();
                for (mut filepath, entry) in entry_deps.stylesheet_map {
                    if filepath == client_entry_path {
                        if let Some(component) = &input.node.component {
                            filepath = component.as_str().to_string();
                        }
                    }

                    if has_stylesheet_entry(&component_deps, &filepath)
                        || has_stylesheet_entry(&universal_deps, &filepath)
                    {
                        eager_css.extend(entry.css);
                        eager_assets.extend(entry.assets);
                    }
                }

                module.imports = entry_deps.imports;
                module.stylesheets = eager_css.into_iter().collect();
                module.fonts = filter_fonts(&eager_assets.iter().cloned().collect::<Vec<_>>());
            }
        }
    }

    if let Some(client_chunks) = input.client_chunks {
        if input.kit.inline_style_threshold > 0
            && matches!(input.kit.output.bundle_strategy, BundleStrategy::Split)
            && !module.stylesheets.is_empty()
        {
            let stylesheets_to_inline = client_chunks
                .iter()
                .filter(|chunk| {
                    chunk.file_name.ends_with(".css")
                        && chunk.source.len() < input.kit.inline_style_threshold as usize
                })
                .map(|chunk| (chunk.file_name.clone(), chunk.source.clone()))
                .collect::<BTreeMap<_, _>>();

            let vite_assets = build_vite_assets_set(&eager_assets, input.assets_path);
            let static_asset_prefix = input
                .assets_path
                .map(|assets_path| {
                    let segments = assets_path
                        .split('/')
                        .filter(|segment| !segment.is_empty())
                        .count();
                    "../".repeat(segments)
                })
                .unwrap_or_default();

            for (inline_index, file) in module.stylesheets.clone().into_iter().enumerate() {
                let Some(css) = stylesheets_to_inline.get(&file) else {
                    continue;
                };

                let transformed = if !input.kit.paths.assets.is_empty() || input.kit.paths.relative
                {
                    fix_css_urls(CssUrlRewriteOptions {
                        css,
                        vite_assets: &vite_assets,
                        static_assets: input.static_assets,
                        paths_assets: "${assets}",
                        base: "${base}",
                        static_asset_prefix: &static_asset_prefix,
                    })
                } else {
                    css.clone()
                };

                let _ = if transformed == *css {
                    serde_json::to_string(css).map_err(|error| ViteBuildError::CssStringify {
                        message: error.to_string(),
                    })?
                } else {
                    create_function_as_string("css", &["assets", "base"], &transformed)?
                };

                let _filename = Utf8Path::new(&file).file_name().ok_or_else(|| {
                    ViteBuildError::MissingStylesheetFilename { file: file.clone() }
                })?;
                let identifier = format!("stylesheet_{inline_index}");
                module.inline_styles.insert(
                    file.clone(),
                    InlineStylesExport::Identifier(identifier.clone()),
                );
            }
        }
    }

    let mut inline_stylesheets = Vec::new();
    for (file, export) in &module.inline_styles {
        let identifier = match export {
            InlineStylesExport::Identifier(identifier) => identifier,
        };
        let filename = Utf8Path::new(file)
            .file_name()
            .ok_or_else(|| ViteBuildError::MissingStylesheetFilename { file: file.clone() })?;
        let source = input
            .client_chunks
            .and_then(|chunks| chunks.iter().find(|chunk| chunk.file_name == *file))
            .map(|chunk| chunk.source.clone())
            .ok_or_else(|| ViteBuildError::MissingClientStylesheetSource { file: file.clone() })?;

        let vite_assets = build_vite_assets_set(&eager_assets, input.assets_path);
        let static_asset_prefix = input
            .assets_path
            .map(|assets_path| {
                let segments = assets_path
                    .split('/')
                    .filter(|segment| !segment.is_empty())
                    .count();
                "../".repeat(segments)
            })
            .unwrap_or_default();
        let transformed = if !input.kit.paths.assets.is_empty() || input.kit.paths.relative {
            fix_css_urls(CssUrlRewriteOptions {
                css: &source,
                vite_assets: &vite_assets,
                static_assets: input.static_assets,
                paths_assets: "${assets}",
                base: "${base}",
                static_asset_prefix: &static_asset_prefix,
            })
        } else {
            source
        };
        let exported = if transformed.contains("${assets}") || transformed.contains("${base}") {
            create_function_as_string("css", &["assets", "base"], &transformed)?
        } else {
            serde_json::to_string(&transformed).map_err(|error| ViteBuildError::CssStringify {
                message: error.to_string(),
            })?
        };

        inline_stylesheets.push(PreparedInlineStylesheetModule {
            identifier: identifier.clone(),
            output_file: format!("{filename}.js"),
            contents: render_inline_stylesheet_module(filename, &exported),
        });
    }

    Ok(ServerNodeBuildArtifacts {
        module,
        inline_stylesheets,
    })
}

pub fn build_server_nodes_plan(
    out: &str,
    kit: &ValidatedKitConfig,
    manifest: &crate::KitManifest,
    server_manifest: &BTreeMap<String, BuildManifestChunk>,
    client_generated_nodes_dir: Option<&str>,
    client_manifest: Option<&BTreeMap<String, BuildManifestChunk>>,
    assets_path: Option<&str>,
    client_chunks: Option<&[ClientBuildAsset]>,
) -> Result<BuildServerNodesPlan> {
    let static_assets = manifest
        .assets
        .iter()
        .map(|asset| asset.file.as_str().to_string())
        .collect::<BTreeSet<_>>();

    let planned_outputs = manifest
        .nodes
        .par_iter()
        .enumerate()
        .map(|(index, node)| {
            plan_server_node_outputs(
                out,
                index,
                node,
                kit,
                server_manifest,
                client_generated_nodes_dir,
                client_manifest,
                assets_path,
                client_chunks,
                &static_assets,
            )
        })
        .collect::<Result<Vec<_>>>()?;

    let mut plan = BuildServerNodesPlan::default();
    for planned_output in planned_outputs {
        plan.node_modules.push(planned_output.node_module);
        plan.stylesheet_modules
            .extend(planned_output.stylesheet_modules);
    }

    Ok(plan)
}

fn plan_server_node_outputs(
    out: &str,
    index: usize,
    node: &ManifestNode,
    kit: &ValidatedKitConfig,
    server_manifest: &BTreeMap<String, BuildManifestChunk>,
    client_generated_nodes_dir: Option<&str>,
    client_manifest: Option<&BTreeMap<String, BuildManifestChunk>>,
    assets_path: Option<&str>,
    client_chunks: Option<&[ClientBuildAsset]>,
    static_assets: &BTreeSet<String>,
) -> Result<PlannedNodeOutputs> {
    let client_entry =
        client_generated_nodes_dir.map(|dir| format!("{}/{}.js", dir.trim_end_matches('/'), index));
    let artifacts = build_server_node_artifacts(&ServerNodeBuildInput {
        index,
        node,
        kit,
        server_manifest,
        client_manifest,
        client_chunks,
        client_entry_path: client_entry.as_deref(),
        assets_path,
        static_assets,
    })?;

    Ok(PlannedNodeOutputs {
        node_module: PlannedBuildOutputFile {
            output_path: format!("{out}/server/nodes/{index}.js"),
            contents: render_server_node_module(&artifacts.module),
        },
        stylesheet_modules: artifacts
            .inline_stylesheets
            .into_iter()
            .map(|stylesheet| PlannedBuildOutputFile {
                output_path: format!("{out}/server/stylesheets/{}", stylesheet.output_file),
                contents: stylesheet.contents,
            })
            .collect(),
    })
}

fn has_stylesheet_entry(deps: &Option<AssetDependencies>, path: &str) -> bool {
    deps.as_ref()
        .and_then(|deps| deps.stylesheet_map.get(path))
        .is_some()
}

fn build_vite_assets_set(
    eager_assets: &BTreeSet<String>,
    assets_path: Option<&str>,
) -> BTreeSet<String> {
    let Some(assets_path) = assets_path else {
        return BTreeSet::new();
    };
    let prefix = format!("{}/", assets_path.trim_end_matches('/'));
    eager_assets
        .iter()
        .filter_map(|asset| asset.strip_prefix(&prefix).map(str::to_string))
        .collect()
}
