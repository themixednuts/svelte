use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
};

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    ValidatedKitConfig,
    error::{GenerateManifestError, Result},
    manifest::{Hooks, KitManifest, ManifestEndpoint, ManifestRoute, ManifestRoutePage},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct BuildManifestChunk {
    pub file: String,
    #[serde(default)]
    pub imports: Vec<String>,
    #[serde(default)]
    pub dynamic_imports: Vec<String>,
    #[serde(default)]
    pub css: Vec<String>,
    #[serde(default)]
    pub assets: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AssetDependencies {
    pub assets: Vec<String>,
    pub file: String,
    pub imports: Vec<String>,
    pub stylesheets: Vec<String>,
    pub fonts: Vec<String>,
    #[serde(default)]
    pub stylesheet_map: BTreeMap<String, StylesheetMapEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct StylesheetMapEntry {
    pub css: Vec<String>,
    pub assets: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct BuildData<'a> {
    pub app_dir: String,
    pub app_path: String,
    pub manifest_data: &'a KitManifest,
    pub out_dir: Utf8PathBuf,
    pub service_worker: Option<String>,
    pub client: Option<Value>,
    pub server_manifest: &'a BTreeMap<String, BuildManifestChunk>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RemoteChunk {
    pub hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedServerManifest {
    pub contents: String,
}

pub fn resolve_manifest_symlink<'a>(
    manifest: &'a BTreeMap<String, BuildManifestChunk>,
    file: &str,
) -> Result<(&'a str, &'a BuildManifestChunk)> {
    manifest
        .get_key_value(file)
        .map(|(key, chunk)| (key.as_str(), chunk))
        .ok_or_else(|| {
            GenerateManifestError::MissingViteManifestFile {
                file: file.to_string(),
            }
            .into()
        })
}

pub fn assets_base(config: &ValidatedKitConfig) -> String {
    let value = if !config.paths.assets.is_empty() {
        config.paths.assets.as_str()
    } else if !config.paths.base.is_empty() {
        config.paths.base.as_str()
    } else {
        "."
    };
    format!("{value}/")
}

pub fn find_deps(
    manifest: &BTreeMap<String, BuildManifestChunk>,
    entry: &str,
    add_dynamic_css: bool,
) -> Option<AssetDependencies> {
    let mut seen = BTreeSet::new();
    let mut imports = BTreeSet::new();
    let mut stylesheets = BTreeSet::new();
    let mut imported_assets = BTreeSet::new();
    let mut stylesheet_map = BTreeMap::<String, StylesheetMapEntry>::new();

    fn traverse(
        manifest: &BTreeMap<String, BuildManifestChunk>,
        current: &str,
        add_js: bool,
        initial_importer: &str,
        dynamic_import_depth: usize,
        add_dynamic_css: bool,
        seen: &mut BTreeSet<String>,
        imports: &mut BTreeSet<String>,
        stylesheets: &mut BTreeSet<String>,
        imported_assets: &mut BTreeSet<String>,
        stylesheet_map: &mut BTreeMap<String, StylesheetMapEntry>,
    ) {
        if !seen.insert(current.to_string()) {
            return;
        }

        let Some(chunk) = manifest.get(current) else {
            return;
        };

        if add_js {
            imports.insert(chunk.file.clone());
        }

        for asset in &chunk.assets {
            imported_assets.insert(asset.clone());
        }

        for file in &chunk.css {
            stylesheets.insert(file.clone());
        }

        for file in &chunk.imports {
            traverse(
                manifest,
                file,
                add_js,
                initial_importer,
                dynamic_import_depth,
                add_dynamic_css,
                seen,
                imports,
                stylesheets,
                imported_assets,
                stylesheet_map,
            );
        }

        if add_dynamic_css {
            if (!chunk.css.is_empty() || !chunk.assets.is_empty()) && dynamic_import_depth <= 1 {
                let entry = stylesheet_map
                    .entry(initial_importer.to_string())
                    .or_default();
                entry.css.extend(chunk.css.iter().cloned());
                entry.assets.extend(chunk.assets.iter().cloned());
                entry.css.sort();
                entry.css.dedup();
                entry.assets.sort();
                entry.assets.dedup();
            }

            for file in &chunk.dynamic_imports {
                traverse(
                    manifest,
                    file,
                    false,
                    file,
                    dynamic_import_depth + 1,
                    add_dynamic_css,
                    seen,
                    imports,
                    stylesheets,
                    imported_assets,
                    stylesheet_map,
                );
            }
        }
    }

    let chunk = manifest.get(entry)?;
    traverse(
        manifest,
        entry,
        true,
        entry,
        0,
        add_dynamic_css,
        &mut seen,
        &mut imports,
        &mut stylesheets,
        &mut imported_assets,
        &mut stylesheet_map,
    );

    let assets = imported_assets.into_iter().collect::<Vec<_>>();
    Some(AssetDependencies {
        file: chunk.file.clone(),
        fonts: filter_fonts(&assets),
        imports: imports.into_iter().collect(),
        stylesheets: stylesheets.into_iter().collect(),
        assets,
        stylesheet_map,
    })
}

pub fn find_server_assets(build_data: &BuildData<'_>, routes: &[ManifestRoute]) -> Vec<String> {
    let mut used_nodes = BTreeSet::from([0usize, 1usize]);
    let mut server_assets = BTreeSet::new();

    let mut add_assets = |id: &str| {
        if let Some(deps) = find_deps(build_data.server_manifest, id, false) {
            for asset in deps.assets {
                server_assets.insert(asset);
            }
        }
    };

    for route in routes {
        if let Some(page) = &route.page {
            collect_page_nodes(page, &mut used_nodes);
        }

        if let Some(ManifestEndpoint { file, .. }) = &route.endpoint {
            add_assets(file.as_str());
        }
    }

    for index in used_nodes {
        if let Some(node) = build_data.manifest_data.nodes.get(index) {
            if let Some(universal) = &node.universal {
                add_assets(universal.as_str());
            }
            if let Some(server) = &node.server {
                add_assets(server.as_str());
            }
        }
    }

    add_hook_assets(&build_data.manifest_data.hooks, &mut add_assets);

    server_assets.into_iter().collect()
}

pub fn generate_manifest(
    build_data: &BuildData<'_>,
    prerendered: &[String],
    relative_path: &str,
    routes: &[ManifestRoute],
    remotes: &[RemoteChunk],
) -> Result<GeneratedServerManifest> {
    let mut reindexed = BTreeMap::<usize, usize>::new();
    let mut used_nodes = BTreeSet::from([0usize, 1usize]);
    let server_assets = find_server_assets(build_data, routes);

    for route in routes {
        if let Some(page) = &route.page {
            collect_page_nodes(page, &mut used_nodes);
        }
    }

    let node_paths = build_data
        .manifest_data
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(index, _)| {
            if used_nodes.contains(&index) {
                reindexed.insert(index, reindexed.len());
                Some(join_relative(relative_path, &format!("nodes/{index}.js")))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    let mut assets = build_data
        .manifest_data
        .assets
        .iter()
        .map(|asset| asset.file.as_str().to_string())
        .collect::<Vec<_>>();
    if let Some(service_worker) = build_data.service_worker.as_ref() {
        assets.push(service_worker.clone());
    }

    let mut matchers = if build_data.client.is_some() {
        build_data
            .manifest_data
            .matchers
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>()
    } else {
        BTreeSet::new()
    };

    let route_entries = routes
        .iter()
        .filter_map(|route| {
            render_route(route, &reindexed, relative_path, build_data, &mut matchers)
        })
        .collect::<Result<Vec<_>>>()?;

    let mut mime_types = build_mime_lookup(build_data.manifest_data);
    let server_asset_sizes = build_server_asset_sizes(build_data, &server_assets, &mut mime_types)?;

    let node_loaders = node_paths
        .iter()
        .map(|path| render_loader(path))
        .collect::<Vec<_>>()
        .join(",\n\t\t\t\t\t");

    let remotes_object = remotes
        .iter()
        .map(|remote| {
            format!(
                "{}: {}",
                json_string(&remote.hash),
                render_loader(&join_relative(
                    relative_path,
                    &format!("chunks/remote-{}.js", remote.hash)
                ))
            )
        })
        .collect::<Vec<_>>()
        .join(",\n\t\t\t\t\t");

    let matcher_imports = matchers
        .iter()
        .map(|matcher| {
            format!(
                "\t\t\t\t\tconst {{ match: {matcher} }} = await import({});",
                json_string(&join_relative(
                    relative_path,
                    &format!("entries/matchers/{matcher}.js")
                ))
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let client = json_string(&build_data.client);
    let assets_expr = json_string(&assets);
    let prerendered_expr = json_string(prerendered);
    let mime_types_expr = json_string(&mime_types);
    let files_expr = json_string(&server_asset_sizes);
    let matchers_return = if matchers.is_empty() {
        "{}".to_string()
    } else {
        format!(
            "{{ {} }}",
            matchers.iter().cloned().collect::<Vec<_>>().join(", ")
        )
    };

    let contents = format!(
        "(() => {{\n\tfunction __memo(fn) {{\n\t\tlet value;\n\t\treturn () => value ??= (value = fn());\n\t}}\n\n\treturn {{\n\t\tappDir: {},\n\t\tappPath: {},\n\t\tassets: new Set({}),\n\t\tmimeTypes: {},\n\t\t_: {{\n\t\t\tclient: {},\n\t\t\tnodes: [\n\t\t\t\t\t{}\n\t\t\t],\n\t\t\tremotes: {{\n\t\t\t\t\t{}\n\t\t\t}},\n\t\t\troutes: [\n\t\t\t\t{}\n\t\t\t],\n\t\t\tprerendered_routes: new Set({}),\n\t\t\tmatchers: async () => {{\n{}\n\t\t\t\t\treturn {};\n\t\t\t}},\n\t\t\tserver_assets: {}\n\t\t}}\n\t}};\n}})()",
        json_string(&build_data.app_dir),
        json_string(&build_data.app_path),
        assets_expr,
        mime_types_expr,
        client,
        node_loaders,
        remotes_object,
        route_entries.join(",\n\t\t\t\t"),
        prerendered_expr,
        matcher_imports,
        matchers_return,
        files_expr,
    );

    Ok(GeneratedServerManifest { contents })
}

fn collect_page_nodes(page: &ManifestRoutePage, used_nodes: &mut BTreeSet<usize>) {
    for index in page.layouts.iter().flatten() {
        used_nodes.insert(*index);
    }
    for index in page.errors.iter().flatten() {
        used_nodes.insert(*index);
    }
    used_nodes.insert(page.leaf);
}

fn add_hook_assets(hooks: &Hooks, add_assets: &mut impl FnMut(&str)) {
    if let Some(server) = &hooks.server {
        add_assets(server.as_str());
    }
    if let Some(universal) = &hooks.universal {
        add_assets(universal.as_str());
    }
}

fn render_route(
    route: &ManifestRoute,
    reindexed: &BTreeMap<usize, usize>,
    relative_path: &str,
    build_data: &BuildData<'_>,
    matchers: &mut BTreeSet<String>,
) -> Option<Result<String>> {
    if route.page.is_none() && route.endpoint.is_none() {
        return None;
    }

    for param in &route.params {
        if let Some(matcher) = param.matcher.as_ref() {
            matchers.insert(matcher.clone());
        }
    }

    let page = route
        .page
        .as_ref()
        .map(|page| render_page(page, reindexed))
        .unwrap_or_else(|| "null".to_string());

    let endpoint = match route.endpoint.as_ref() {
        Some(endpoint) => {
            match resolve_manifest_chunk(build_data.server_manifest, endpoint.file.as_str()) {
                Ok(chunk) => render_loader(&join_relative(relative_path, &chunk.file)),
                Err(error) => return Some(Err(error)),
            }
        }
        None => "null".to_string(),
    };

    Some(Ok(format!(
        "{{ id: {}, pattern: {}, params: {}, page: {}, endpoint: {} }}",
        json_string(&route.id),
        render_regex(&route.pattern.as_str().to_string()),
        json_string(&route.params),
        page,
        endpoint
    )))
}

fn render_page(page: &ManifestRoutePage, reindexed: &BTreeMap<usize, usize>) -> String {
    format!(
        "{{ layouts: {}, errors: {}, leaf: {} }}",
        render_node_indexes(&page.layouts, reindexed),
        render_node_indexes(&page.errors, reindexed),
        reindexed
            .get(&page.leaf)
            .copied()
            .expect("used leaf node should be reindexed"),
    )
}

fn render_node_indexes(indexes: &[Option<usize>], reindexed: &BTreeMap<usize, usize>) -> String {
    let rendered = indexes
        .iter()
        .map(|index| {
            index
                .and_then(|index| reindexed.get(&index).copied())
                .map(|index| index.to_string())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("[{rendered},]")
}

fn resolve_manifest_chunk<'a>(
    manifest: &'a BTreeMap<String, BuildManifestChunk>,
    file: &str,
) -> Result<&'a BuildManifestChunk> {
    manifest.get(file).ok_or_else(|| {
        GenerateManifestError::MissingBuildManifestFile {
            file: file.to_string(),
        }
        .into()
    })
}

fn build_mime_lookup(manifest: &KitManifest) -> BTreeMap<String, String> {
    let mut mime = BTreeMap::new();
    for asset in &manifest.assets {
        if let Some(type_) = asset.type_.as_ref() {
            if let Some(extension) = Utf8Path::new(asset.file.as_str()).extension() {
                mime.insert(format!(".{extension}"), type_.clone());
            }
        }
    }
    mime
}

fn build_server_asset_sizes(
    build_data: &BuildData<'_>,
    server_assets: &[String],
    mime_types: &mut BTreeMap<String, String>,
) -> Result<BTreeMap<String, u64>> {
    let mut files = BTreeMap::new();

    for file in server_assets {
        let asset_path = build_data.out_dir.join("server").join(file);
        files.insert(file.clone(), fs::metadata(&asset_path)?.len());

        if let Some(extension) = Utf8Path::new(file).extension() {
            let key = format!(".{extension}");
            mime_types.entry(key).or_insert_with(|| {
                mime_guess::from_ext(extension)
                    .first_raw()
                    .unwrap_or("")
                    .to_string()
            });
        }
    }

    Ok(files)
}

fn render_loader(path: &str) -> String {
    format!("__memo(() => import({}))", json_string(path))
}

fn render_regex(pattern: &str) -> String {
    format!("new RegExp({})", json_string(pattern))
}

fn join_relative(base: &str, target: &str) -> String {
    let base_path = Utf8Path::new(base);
    let target_path = Utf8Path::new(target.trim_start_matches('/'));
    let joined = base_path.join(target_path);
    let mut joined = joined.as_str().replace('\\', "/");
    if !joined.starts_with('.') {
        joined = format!("./{joined}");
    }
    joined
}

fn json_string<T: Serialize + ?Sized>(value: &T) -> String {
    serde_json::to_string(value).expect("json serialization")
}

pub fn filter_fonts(assets: &[String]) -> Vec<String> {
    assets
        .iter()
        .filter(|asset| {
            asset.ends_with(".woff")
                || asset.ends_with(".woff2")
                || asset.ends_with(".ttf")
                || asset.ends_with(".otf")
        })
        .cloned()
        .collect()
}
