use std::collections::BTreeMap;
use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use regex::Regex;

use crate::error::{Error, ManifestError, Result};
use crate::page_options::{PageOptions, read_page_options};
use crate::routing::{
    FoundRoute, RouteParam, exec_route_match, find_route, parse_route_id, sort_routes,
};

#[derive(Debug, Clone)]
pub struct ManifestConfig {
    pub routes_dir: Utf8PathBuf,
    pub cwd: Utf8PathBuf,
    pub fallback_dir: Utf8PathBuf,
    pub params_dir: Utf8PathBuf,
    pub assets_dir: Utf8PathBuf,
    pub hooks_client: Utf8PathBuf,
    pub hooks_server: Utf8PathBuf,
    pub hooks_universal: Utf8PathBuf,
    pub component_extensions: Vec<String>,
    pub module_extensions: Vec<String>,
}

impl ManifestConfig {
    pub fn new(routes_dir: Utf8PathBuf, cwd: Utf8PathBuf) -> Self {
        Self {
            assets_dir: cwd.join("static"),
            fallback_dir: cwd.clone(),
            hooks_client: cwd.join("hooks.client"),
            hooks_server: cwd.join("hooks.server"),
            hooks_universal: cwd.join("hooks"),
            params_dir: cwd.join("params"),
            routes_dir,
            cwd,
            component_extensions: vec![".svelte".to_string()],
            module_extensions: vec![".js".to_string(), ".ts".to_string()],
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NodeFiles {
    pub component: Option<Utf8PathBuf>,
    pub universal: Option<Utf8PathBuf>,
    pub server: Option<Utf8PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PageFiles {
    pub component: Option<Utf8PathBuf>,
    pub universal: Option<Utf8PathBuf>,
    pub server: Option<Utf8PathBuf>,
    pub layouts: Vec<Option<NodeFiles>>,
    pub errors: Vec<Option<Utf8PathBuf>>,
}

#[derive(Debug, Clone)]
pub struct DiscoveredRoute {
    pub id: String,
    pub pattern: Regex,
    pub params: Vec<RouteParam>,
    pub page: Option<PageFiles>,
    leaf_parent_id: Option<NamedLayoutReference>,
    pub layout: Option<NodeFiles>,
    layout_parent_id: Option<NamedLayoutReference>,
    pub error: Option<Utf8PathBuf>,
    pub endpoint: Option<Utf8PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub file: Utf8PathBuf,
    pub size: u64,
    pub type_: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestNodeKind {
    Error,
    Layout,
    Page,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestNode {
    pub kind: ManifestNodeKind,
    pub component: Option<Utf8PathBuf>,
    pub universal: Option<Utf8PathBuf>,
    pub server: Option<Utf8PathBuf>,
    pub parent_id: Option<String>,
    pub universal_page_options: Option<PageOptions>,
    pub server_page_options: Option<PageOptions>,
    pub page_options: Option<PageOptions>,
}

#[derive(Debug, Clone)]
pub struct KitManifest {
    pub assets: Vec<Asset>,
    pub hooks: Hooks,
    pub matchers: BTreeMap<String, Utf8PathBuf>,
    pub manifest_routes: Vec<ManifestRoute>,
    pub nodes: Vec<ManifestNode>,
    pub routes: Vec<DiscoveredRoute>,
}

impl KitManifest {
    pub fn discover(config: &ManifestConfig) -> Result<Self> {
        let mut routes = match discover_routes(config) {
            Ok(routes) => routes,
            Err(Error::Manifest(ManifestError::NoRoutesFound)) => {
                vec![fallback_root_route()?]
            }
            Err(error) => return Err(error),
        };
        apply_manifest_fallbacks(config, &mut routes)?;
        let nodes = collect_nodes(config, &routes);
        Ok(Self {
            assets: discover_assets(config)?,
            hooks: discover_hooks(config)?,
            matchers: discover_matchers(config)?,
            manifest_routes: build_manifest_routes(config, &routes, &nodes),
            nodes,
            routes,
        })
    }

    pub fn find_matching_route<F>(
        &self,
        path: &str,
        matches: F,
    ) -> Option<FoundRoute<'_, ManifestRoute>>
    where
        F: FnMut(&str, &str) -> bool,
    {
        find_route(
            path,
            &self.manifest_routes,
            |route| (&route.pattern, &route.params),
            matches,
        )
    }

    pub fn build_client_routes(&self) -> Vec<ClientRoute> {
        self.manifest_routes
            .iter()
            .filter_map(|route| {
                let page = route.page.as_ref()?;
                let mut layouts = page
                    .layouts
                    .iter()
                    .map(|layout| {
                        layout.map(|node| ClientLayoutRef {
                            uses_server_data: self
                                .nodes
                                .get(node)
                                .and_then(|node| node.server.as_ref())
                                .is_some(),
                            node,
                        })
                    })
                    .collect::<Vec<_>>();
                let mut errors = page.errors.clone();
                let len = layouts.len().max(errors.len());
                layouts.resize(len, None);
                errors.resize(len, None);

                Some(ClientRoute {
                    id: route.id.clone(),
                    pattern: route.pattern.clone(),
                    params: route.params.clone(),
                    errors,
                    layouts,
                    leaf: ClientLeafRef {
                        uses_server_data: self
                            .nodes
                            .get(page.leaf)
                            .and_then(|node| node.server.as_ref())
                            .is_some(),
                        node: page.leaf,
                    },
                })
            })
            .collect()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Hooks {
    pub client: Option<Utf8PathBuf>,
    pub server: Option<Utf8PathBuf>,
    pub universal: Option<Utf8PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestRoutePage {
    pub layouts: Vec<Option<usize>>,
    pub errors: Vec<Option<usize>>,
    pub leaf: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestEndpoint {
    pub file: Utf8PathBuf,
    pub page_options: Option<PageOptions>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientLayoutRef {
    pub uses_server_data: bool,
    pub node: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientLeafRef {
    pub uses_server_data: bool,
    pub node: usize,
}

#[derive(Debug, Clone)]
pub struct ManifestRoute {
    pub id: String,
    pub pattern: Regex,
    pub params: Vec<RouteParam>,
    pub page: Option<ManifestRoutePage>,
    pub endpoint: Option<ManifestEndpoint>,
}

#[derive(Debug, Clone)]
pub struct ClientRoute {
    pub id: String,
    pattern: Regex,
    params: Vec<RouteParam>,
    pub errors: Vec<Option<usize>>,
    pub layouts: Vec<Option<ClientLayoutRef>>,
    pub leaf: ClientLeafRef,
}

impl ClientRoute {
    pub fn exec<F>(&self, path: &str, matches: F) -> Option<BTreeMap<String, String>>
    where
        F: FnMut(&str, &str) -> bool,
    {
        let captures = self.pattern.captures(path)?;
        exec_route_match(&captures, &self.params, matches)
    }
}

#[derive(Debug, Clone)]
struct AncestorNode {
    segment: String,
    layout: Option<NodeFiles>,
    error: Option<Utf8PathBuf>,
}

#[derive(Debug, Clone)]
struct NamedLayoutReference {
    target_segment: String,
    source_path: Utf8PathBuf,
}

#[derive(Debug, Clone)]
struct InternalManifestNode {
    node: ManifestNode,
    parent_key: Option<String>,
}

#[derive(Debug, Clone)]
enum NodePageOptionsState {
    Static(PageOptions),
    Empty,
    Dynamic,
}

impl NodePageOptionsState {
    fn into_page_options(self) -> Option<PageOptions> {
        match self {
            Self::Static(options) => Some(options),
            Self::Empty | Self::Dynamic => None,
        }
    }
}

pub fn discover_routes(config: &ManifestConfig) -> Result<Vec<DiscoveredRoute>> {
    let mut routes = Vec::new();

    if config.routes_dir.is_dir() {
        walk_directory(
            config,
            &config.routes_dir,
            "/".to_string(),
            &[],
            &mut routes,
        )?;
    } else {
        let parsed = parse_route_id("/")?;
        routes.push(DiscoveredRoute {
            id: "/".to_string(),
            pattern: parsed.pattern,
            params: parsed.params,
            page: None,
            leaf_parent_id: None,
            layout: None,
            layout_parent_id: None,
            error: None,
            endpoint: None,
        });
    }

    let matchers = discover_matchers(config)?;
    for route in &routes {
        for param in &route.params {
            if let Some(matcher) = param.matcher.as_ref() {
                if !matchers.contains_key(matcher) {
                    return Err(ManifestError::MissingMatcher {
                        matcher: matcher.clone(),
                        route_id: route.id.clone(),
                    }
                    .into());
                }
            }
        }
    }

    if routes.len() == 1 {
        let root = &routes[0];
        if root.page.is_none()
            && root.layout.is_none()
            && root.error.is_none()
            && root.endpoint.is_none()
        {
            return Err(ManifestError::NoRoutesFound.into());
        }
    }

    prevent_conflicts(&routes)?;
    sort_routes(&mut routes, |route| &route.id);
    Ok(routes)
}

pub fn discover_matchers(config: &ManifestConfig) -> Result<BTreeMap<String, Utf8PathBuf>> {
    let mut matchers = BTreeMap::new();

    if !config.params_dir.is_dir() {
        return Ok(matchers);
    }

    for entry in fs::read_dir(&config.params_dir)? {
        let entry = entry?;
        let path = Utf8PathBuf::from_path_buf(entry.path()).map_err(|_| Error::InvalidUtf8Path)?;
        let metadata = entry.metadata()?;
        if !metadata.is_file() {
            continue;
        }

        let Some(file_name) = path.file_name() else {
            continue;
        };

        let Some(ext) = config
            .module_extensions
            .iter()
            .find(|ext| file_name.ends_with(ext.as_str()))
        else {
            continue;
        };

        let matcher_name = &file_name[..file_name.len() - ext.len()];
        if matcher_name.ends_with(".test") || matcher_name.ends_with(".spec") {
            continue;
        }

        if !is_matcher_name(matcher_name) {
            return Err(ManifestError::InvalidMatcherName {
                file_name: file_name.to_string(),
            }
            .into());
        }

        let relative = relative_to_cwd(&path, &config.cwd)?;
        if let Some(existing) = matchers.insert(matcher_name.to_string(), relative.clone()) {
            return Err(ManifestError::DuplicateMatchers {
                incoming: relative.to_string(),
                existing: existing.to_string(),
            }
            .into());
        }
    }

    Ok(matchers)
}

pub fn discover_assets(config: &ManifestConfig) -> Result<Vec<Asset>> {
    let mut assets = Vec::new();

    if !config.assets_dir.is_dir() {
        return Ok(assets);
    }

    walk_assets(config, &config.assets_dir, &mut assets)?;
    assets.sort_by(|left, right| left.file.cmp(&right.file));
    Ok(assets)
}

fn fallback_root_route() -> Result<DiscoveredRoute> {
    let parsed = parse_route_id("/")?;
    Ok(DiscoveredRoute {
        id: "/".to_string(),
        pattern: parsed.pattern,
        params: parsed.params,
        page: None,
        leaf_parent_id: None,
        layout: None,
        layout_parent_id: None,
        error: None,
        endpoint: None,
    })
}

pub fn discover_hooks(config: &ManifestConfig) -> Result<Hooks> {
    Ok(Hooks {
        client: resolve_entry(config, &config.hooks_client)?,
        server: resolve_entry(config, &config.hooks_server)?,
        universal: resolve_entry(config, &config.hooks_universal)?,
    })
}

#[derive(Debug)]
enum RouteFile {
    PageComponent { uses_layout: Option<String> },
    PageUniversal,
    PageServer,
    LayoutComponent { uses_layout: Option<String> },
    LayoutUniversal,
    LayoutServer,
    ErrorComponent,
    Endpoint,
}

fn walk_directory(
    config: &ManifestConfig,
    dir: &Utf8Path,
    route_id: String,
    parent_nodes: &[AncestorNode],
    routes: &mut Vec<DiscoveredRoute>,
) -> Result<()> {
    validate_route_id(&route_id)?;
    let current_segment = route_segment(&route_id);
    validate_route_segment(&route_id, &current_segment)?;

    let parsed = parse_route_id(&route_id)?;
    let mut route = DiscoveredRoute {
        id: route_id.clone(),
        pattern: parsed.pattern,
        params: parsed.params,
        page: None,
        leaf_parent_id: None,
        layout: None,
        layout_parent_id: None,
        error: None,
        endpoint: None,
    };

    let mut children = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let file_name = entry.file_name().to_string_lossy().to_string();
        let path = Utf8PathBuf::from_path_buf(entry.path()).map_err(|_| Error::InvalidUtf8Path)?;
        let file_type = entry.file_type()?;
        let metadata = fs::metadata(&path)?;

        let is_dir = file_type.is_dir() || (file_type.is_symlink() && metadata.is_dir());
        let is_file = file_type.is_file() || (file_type.is_symlink() && metadata.is_file());

        if is_dir {
            if should_ignore_directory(&file_name) {
                continue;
            }

            if file_name.starts_with('+') {
                let project_relative = relative_to_cwd(&path, &config.cwd)?;
                return Err(ManifestError::ReservedPlusPath {
                    path: project_relative.to_string(),
                }
                .into());
            }

            children.push((file_name, path));
            continue;
        }

        if !is_file {
            continue;
        }

        let project_relative = relative_to_cwd(&path, &config.cwd)?;
        let Some(kind) = analyze_file(&project_relative, &file_name, config)? else {
            continue;
        };

        match kind {
            RouteFile::PageComponent { uses_layout } => {
                let page = route.page.get_or_insert_with(PageFiles::default);
                route.leaf_parent_id = uses_layout.map(|target_segment| NamedLayoutReference {
                    target_segment,
                    source_path: project_relative.clone(),
                });
                set_slot(
                    "page component",
                    &route.id,
                    &mut page.component,
                    project_relative,
                )?;
            }
            RouteFile::PageUniversal => {
                let page = route.page.get_or_insert_with(PageFiles::default);
                set_slot(
                    "universal page module",
                    &route.id,
                    &mut page.universal,
                    project_relative,
                )?;
            }
            RouteFile::PageServer => {
                let page = route.page.get_or_insert_with(PageFiles::default);
                set_slot(
                    "server page module",
                    &route.id,
                    &mut page.server,
                    project_relative,
                )?;
            }
            RouteFile::LayoutComponent { uses_layout } => {
                let layout = route.layout.get_or_insert_with(NodeFiles::default);
                route.layout_parent_id = uses_layout.map(|target_segment| NamedLayoutReference {
                    target_segment,
                    source_path: project_relative.clone(),
                });
                set_slot(
                    "layout component",
                    &route.id,
                    &mut layout.component,
                    project_relative,
                )?;
            }
            RouteFile::LayoutUniversal => {
                let layout = route.layout.get_or_insert_with(NodeFiles::default);
                set_slot(
                    "universal layout module",
                    &route.id,
                    &mut layout.universal,
                    project_relative,
                )?;
            }
            RouteFile::LayoutServer => {
                let layout = route.layout.get_or_insert_with(NodeFiles::default);
                set_slot(
                    "server layout module",
                    &route.id,
                    &mut layout.server,
                    project_relative,
                )?;
            }
            RouteFile::ErrorComponent => {
                set_slot(
                    "error component",
                    &route.id,
                    &mut route.error,
                    project_relative,
                )?;
            }
            RouteFile::Endpoint => {
                set_slot("endpoint", &route.id, &mut route.endpoint, project_relative)?;
            }
        }
    }

    if route.layout_parent_id.is_some() {
        let _ = select_page_ancestors(parent_nodes, route.layout_parent_id.as_ref())?;
    }

    let mut combined_nodes = parent_nodes.to_vec();
    if route.layout.is_some() || route.error.is_some() {
        combined_nodes.push(AncestorNode {
            segment: current_segment.clone(),
            layout: route.layout.clone(),
            error: route.error.clone(),
        });
    }

    if let Some(page) = route.page.as_mut() {
        let ancestors = select_page_ancestors(&combined_nodes, route.leaf_parent_id.as_ref())?;
        page.layouts = ancestors
            .iter()
            .map(|ancestor| ancestor.layout.clone())
            .collect();
        page.errors = ancestors
            .iter()
            .map(|ancestor| ancestor.error.clone())
            .collect();
    }

    let next_nodes = combined_nodes;

    routes.push(route);

    children.sort_by(|left, right| left.0.cmp(&right.0));
    for (name, path) in children {
        let child_id = if route_id == "/" {
            format!("/{name}")
        } else {
            format!("{route_id}/{name}")
        };
        walk_directory(config, &path, child_id, &next_nodes, routes)?;
    }

    Ok(())
}

fn analyze_file(
    path: &Utf8Path,
    file_name: &str,
    config: &ManifestConfig,
) -> Result<Option<RouteFile>> {
    let ext = config
        .component_extensions
        .iter()
        .chain(config.module_extensions.iter())
        .find(|ext| file_name.ends_with(ext.as_str()));
    let Some(ext) = ext else {
        return Ok(None);
    };

    if !file_name.starts_with('+') {
        return Ok(None);
    }

    if file_name.ends_with(".d.ts") {
        return Ok(None);
    }

    if config
        .component_extensions
        .iter()
        .any(|candidate| candidate == ext)
    {
        let stem = &file_name[..file_name.len() - ext.len()];
        if stem == "+page" || stem.starts_with("+page@") {
            return Ok(Some(RouteFile::PageComponent {
                uses_layout: parse_uses_layout(stem, "+page"),
            }));
        }
        if stem == "+layout" || stem.starts_with("+layout@") {
            return Ok(Some(RouteFile::LayoutComponent {
                uses_layout: parse_uses_layout(stem, "+layout"),
            }));
        }
        if stem == "+error" {
            return Ok(Some(RouteFile::ErrorComponent));
        }
        return Err(ManifestError::ReservedPlusPath {
            path: path.to_string(),
        }
        .into());
    }

    let stem = &file_name[..file_name.len() - ext.len()];
    if stem == "+server" {
        return Ok(Some(RouteFile::Endpoint));
    }
    if stem == "+page" {
        return Ok(Some(RouteFile::PageUniversal));
    }
    if stem == "+page.server" {
        return Ok(Some(RouteFile::PageServer));
    }
    if stem == "+layout" {
        return Ok(Some(RouteFile::LayoutUniversal));
    }
    if stem == "+layout.server" {
        return Ok(Some(RouteFile::LayoutServer));
    }
    if stem.starts_with("+page@") || stem.starts_with("+layout@") {
        let display_name = path.file_name().unwrap_or(path.as_str());
        return Err(ManifestError::NamedLayoutRequiresSvelte {
            display_name: display_name.to_string(),
            path: path.to_string(),
        }
        .into());
    }

    Err(ManifestError::ReservedPlusPath {
        path: path.to_string(),
    }
    .into())
}

fn parse_uses_layout(stem: &str, prefix: &str) -> Option<String> {
    stem.strip_prefix(prefix)
        .and_then(|suffix| suffix.strip_prefix('@'))
        .map(ToString::to_string)
}

fn is_matcher_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
}

fn walk_assets(config: &ManifestConfig, dir: &Utf8Path, assets: &mut Vec<Asset>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = Utf8PathBuf::from_path_buf(entry.path()).map_err(|_| Error::InvalidUtf8Path)?;
        let metadata = entry.metadata()?;

        if metadata.is_dir() {
            walk_assets(config, &path, assets)?;
            continue;
        }

        if !metadata.is_file() {
            continue;
        }

        let file =
            path.strip_prefix(&config.assets_dir)
                .map_err(|_| ManifestError::PathNotUnderRoot {
                    path: path.to_string(),
                    root: config.assets_dir.to_string(),
                })?;
        let file = Utf8PathBuf::from(file.as_str().replace('\\', "/"));
        let type_ = match path.extension() {
            Some("txt") => Some("text/plain".to_string()),
            _ => None,
        };

        assets.push(Asset {
            file,
            size: metadata.len(),
            type_,
        });
    }

    Ok(())
}

fn resolve_entry(config: &ManifestConfig, entry: &Utf8Path) -> Result<Option<Utf8PathBuf>> {
    if entry.is_file() {
        return Ok(Some(relative_to_cwd(entry, &config.cwd)?));
    }

    if entry.is_dir() {
        let index = entry.join("index");
        if let Some(found) = resolve_entry(config, &index)? {
            return Ok(Some(found));
        }
    }

    let Some(parent) = entry.parent() else {
        return Ok(None);
    };
    if !parent.is_dir() {
        return Ok(None);
    }

    let Some(base) = entry.file_name() else {
        return Ok(None);
    };
    for candidate in fs::read_dir(parent)? {
        let candidate = candidate?;
        let candidate_path =
            Utf8PathBuf::from_path_buf(candidate.path()).map_err(|_| Error::InvalidUtf8Path)?;
        let metadata = candidate.metadata()?;
        if !metadata.is_file() {
            continue;
        }

        let Some(file_name) = candidate_path.file_name() else {
            continue;
        };
        if !config
            .module_extensions
            .iter()
            .any(|ext| file_name.ends_with(ext.as_str()))
        {
            continue;
        }

        let stem = &file_name[..file_name.rfind('.').unwrap_or(file_name.len())];
        if stem == base {
            return Ok(Some(relative_to_cwd(&candidate_path, &config.cwd)?));
        }
    }

    Ok(None)
}

fn apply_manifest_fallbacks(config: &ManifestConfig, routes: &mut [DiscoveredRoute]) -> Result<()> {
    let root_index = routes
        .iter()
        .position(|route| route.id == "/")
        .ok_or(ManifestError::MissingRootRoute)?;
    let fallback_layout = fallback_component(config, "layout.svelte")?;
    let fallback_error = fallback_component(config, "error.svelte")?;

    {
        let root = &mut routes[root_index];
        let root_layout = root.layout.get_or_insert_with(NodeFiles::default);
        if root_layout.component.is_none() {
            root_layout.component = Some(fallback_layout.clone());
        }

        if root.error.is_none() {
            root.error = Some(fallback_error);
        }
    }

    for route in routes.iter_mut() {
        if let Some(layout) = route.layout.as_mut() {
            if layout.component.is_none() {
                layout.component = Some(fallback_layout.clone());
            }
        }
    }

    rebuild_page_ancestors(routes)
}

fn fallback_component(config: &ManifestConfig, file_name: &str) -> Result<Utf8PathBuf> {
    relative_to_cwd(&config.fallback_dir.join(file_name), &config.cwd)
}

fn rebuild_page_ancestors(routes: &mut [DiscoveredRoute]) -> Result<()> {
    let route_lookup = routes
        .iter()
        .map(|route| (route.id.clone(), route.clone()))
        .collect::<BTreeMap<_, _>>();

    for route in routes.iter_mut() {
        let Some(page) = route.page.as_mut() else {
            continue;
        };

        let mut lineage = Vec::new();
        let mut current = Some(route.id.as_str());
        while let Some(route_id) = current {
            let current_route = route_lookup
                .get(route_id)
                .unwrap_or_else(|| panic!("missing route {route_id}"));
            if current_route.layout.is_some() || current_route.error.is_some() {
                lineage.push(AncestorNode {
                    segment: candidate_segment(current_route),
                    layout: current_route.layout.clone(),
                    error: current_route.error.clone(),
                });
            }
            current = parent_route_id(route_id);
        }
        lineage.reverse();

        let ancestors = select_page_ancestors(&lineage, route.leaf_parent_id.as_ref())?;
        page.layouts = ancestors
            .iter()
            .map(|ancestor| ancestor.layout.clone())
            .collect();
        page.errors = ancestors
            .iter()
            .map(|ancestor| ancestor.error.clone())
            .collect();
    }

    Ok(())
}

fn collect_nodes(config: &ManifestConfig, routes: &[DiscoveredRoute]) -> Vec<ManifestNode> {
    let mut nodes = Vec::new();
    let route_lookup = routes
        .iter()
        .map(|route| (route.id.as_str(), route))
        .collect::<BTreeMap<_, _>>();

    for route in routes {
        if let Some(layout) = route.layout.as_ref() {
            nodes.push(InternalManifestNode {
                node: ManifestNode {
                    kind: ManifestNodeKind::Layout,
                    component: layout.component.clone(),
                    universal: layout.universal.clone(),
                    server: layout.server.clone(),
                    parent_id: route
                        .layout_parent_id
                        .as_ref()
                        .map(|parent_id| parent_id.target_segment.clone()),
                    universal_page_options: layout
                        .universal
                        .as_ref()
                        .and_then(|file| read_page_options(&config.cwd.join(file))),
                    server_page_options: layout
                        .server
                        .as_ref()
                        .and_then(|file| read_page_options(&config.cwd.join(file))),
                    page_options: None,
                },
                parent_key: find_parent_layout_key(route, &route_lookup),
            });
        }

        if let Some(error) = route.error.as_ref() {
            nodes.push(InternalManifestNode {
                node: ManifestNode {
                    kind: ManifestNodeKind::Error,
                    component: Some(error.clone()),
                    universal: None,
                    server: None,
                    parent_id: None,
                    universal_page_options: None,
                    server_page_options: None,
                    page_options: None,
                },
                parent_key: None,
            });
        }
    }

    for route in routes {
        if let Some(page) = route.page.as_ref() {
            nodes.push(InternalManifestNode {
                node: ManifestNode {
                    kind: ManifestNodeKind::Page,
                    component: page.component.clone(),
                    universal: page.universal.clone(),
                    server: page.server.clone(),
                    parent_id: route
                        .leaf_parent_id
                        .as_ref()
                        .map(|parent_id| parent_id.target_segment.clone()),
                    universal_page_options: page
                        .universal
                        .as_ref()
                        .and_then(|file| read_page_options(&config.cwd.join(file))),
                    server_page_options: page
                        .server
                        .as_ref()
                        .and_then(|file| read_page_options(&config.cwd.join(file))),
                    page_options: None,
                },
                parent_key: page
                    .layouts
                    .iter()
                    .rev()
                    .flatten()
                    .next()
                    .map(layout_node_key),
            });
        }
    }

    populate_node_page_options(nodes)
}

fn build_manifest_routes(
    config: &ManifestConfig,
    routes: &[DiscoveredRoute],
    nodes: &[ManifestNode],
) -> Vec<ManifestRoute> {
    let node_indexes = build_node_indexes(nodes);

    routes
        .iter()
        .map(|route| ManifestRoute {
            id: route.id.clone(),
            pattern: route.pattern.clone(),
            params: route.params.clone(),
            page: route.page.as_ref().map(|page| ManifestRoutePage {
                layouts: page
                    .layouts
                    .iter()
                    .map(|layout| {
                        layout
                            .as_ref()
                            .map(|layout| node_index_for_layout(&node_indexes, layout))
                    })
                    .collect(),
                errors: page
                    .errors
                    .iter()
                    .map(|error| {
                        error
                            .as_ref()
                            .map(|error| node_index_for_error(&node_indexes, error))
                    })
                    .collect(),
                leaf: node_index_for_page(&node_indexes, page),
            }),
            endpoint: route.endpoint.as_ref().map(|file| ManifestEndpoint {
                file: file.clone(),
                page_options: read_page_options(&config.cwd.join(file)),
            }),
        })
        .collect()
}

fn build_node_indexes(nodes: &[ManifestNode]) -> BTreeMap<String, usize> {
    nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (manifest_node_key(node), index))
        .collect()
}

fn manifest_node_key(node: &ManifestNode) -> String {
    format!(
        "{:?}|{}|{}|{}",
        node.kind,
        node.component
            .as_ref()
            .map(|path| path.as_str())
            .unwrap_or(""),
        node.universal
            .as_ref()
            .map(|path| path.as_str())
            .unwrap_or(""),
        node.server.as_ref().map(|path| path.as_str()).unwrap_or("")
    )
}

fn node_index_for_layout(node_indexes: &BTreeMap<String, usize>, layout: &NodeFiles) -> usize {
    let node = ManifestNode {
        kind: ManifestNodeKind::Layout,
        component: layout.component.clone(),
        universal: layout.universal.clone(),
        server: layout.server.clone(),
        parent_id: None,
        universal_page_options: None,
        server_page_options: None,
        page_options: None,
    };
    node_index(node_indexes, &node)
}

fn node_index_for_error(node_indexes: &BTreeMap<String, usize>, error: &Utf8Path) -> usize {
    let node = ManifestNode {
        kind: ManifestNodeKind::Error,
        component: Some(error.to_path_buf()),
        universal: None,
        server: None,
        parent_id: None,
        universal_page_options: None,
        server_page_options: None,
        page_options: None,
    };
    node_index(node_indexes, &node)
}

fn node_index_for_page(node_indexes: &BTreeMap<String, usize>, page: &PageFiles) -> usize {
    let node = ManifestNode {
        kind: ManifestNodeKind::Page,
        component: page.component.clone(),
        universal: page.universal.clone(),
        server: page.server.clone(),
        parent_id: None,
        universal_page_options: None,
        server_page_options: None,
        page_options: None,
    };
    node_index(node_indexes, &node)
}

fn node_index(node_indexes: &BTreeMap<String, usize>, node: &ManifestNode) -> usize {
    let key = manifest_node_key(node);
    *node_indexes
        .get(&key)
        .unwrap_or_else(|| panic!("missing manifest node for key {key}"))
}

fn populate_node_page_options(nodes: Vec<InternalManifestNode>) -> Vec<ManifestNode> {
    let mut page_options_by_key = BTreeMap::<String, NodePageOptionsState>::new();

    nodes
        .into_iter()
        .map(|internal| {
            let key = manifest_node_key(&internal.node);
            let page_options = compute_node_page_options(
                &internal.node,
                internal.parent_key.as_deref(),
                &page_options_by_key,
            );
            page_options_by_key.insert(key, page_options.clone());

            let mut node = internal.node;
            node.page_options = page_options.into_page_options();
            node
        })
        .collect()
}

fn compute_node_page_options(
    node: &ManifestNode,
    parent_key: Option<&str>,
    page_options_by_key: &BTreeMap<String, NodePageOptionsState>,
) -> NodePageOptionsState {
    let mut page_options =
        match parent_key.and_then(|parent_key| page_options_by_key.get(parent_key)) {
            Some(NodePageOptionsState::Static(options)) => options.clone(),
            Some(NodePageOptionsState::Empty) | None => PageOptions::new(),
            Some(NodePageOptionsState::Dynamic) => return NodePageOptionsState::Dynamic,
        };

    if node.server.is_some() {
        match node.server_page_options.as_ref() {
            Some(server_options) => merge_page_options(&mut page_options, server_options),
            None => return NodePageOptionsState::Dynamic,
        }
    }

    if node.universal.is_some() {
        match node.universal_page_options.as_ref() {
            Some(universal_options) => merge_page_options(&mut page_options, universal_options),
            None => return NodePageOptionsState::Dynamic,
        }
    }

    if page_options.is_empty() {
        NodePageOptionsState::Empty
    } else {
        NodePageOptionsState::Static(page_options)
    }
}

fn merge_page_options(into: &mut PageOptions, next: &PageOptions) {
    for (key, value) in next {
        if key == "config" {
            match (into.get_mut(key), value) {
                (Some(serde_json::Value::Object(current)), serde_json::Value::Object(next)) => {
                    for (config_key, config_value) in next {
                        current.insert(config_key.clone(), config_value.clone());
                    }
                }
                _ => {
                    into.insert(key.clone(), value.clone());
                }
            }
        } else {
            into.insert(key.clone(), value.clone());
        }
    }
}

fn find_parent_layout_key(
    route: &DiscoveredRoute,
    route_lookup: &BTreeMap<&str, &DiscoveredRoute>,
) -> Option<String> {
    let mut current = parent_route_id(&route.id);
    while let Some(route_id) = current {
        let candidate = route_lookup.get(route_id)?;
        let matches_target = route
            .layout_parent_id
            .as_ref()
            .map(|parent_id| candidate_segment(candidate) == parent_id.target_segment)
            .unwrap_or(true);

        if matches_target {
            if let Some(layout) = candidate.layout.as_ref() {
                return Some(layout_node_key(layout));
            }
            if route.layout_parent_id.is_some() {
                return None;
            }
        }
        current = parent_route_id(route_id);
    }

    None
}

fn parent_route_id(route_id: &str) -> Option<&str> {
    if route_id == "/" {
        return None;
    }

    route_id
        .rfind('/')
        .map(|index| if index == 0 { "/" } else { &route_id[..index] })
}

fn layout_node_key(layout: &NodeFiles) -> String {
    manifest_node_key(&ManifestNode {
        kind: ManifestNodeKind::Layout,
        component: layout.component.clone(),
        universal: layout.universal.clone(),
        server: layout.server.clone(),
        parent_id: None,
        universal_page_options: None,
        server_page_options: None,
        page_options: None,
    })
}

fn candidate_segment(route: &DiscoveredRoute) -> String {
    route_segment(&route.id)
}

fn should_ignore_directory(name: &str) -> bool {
    name.starts_with('_') || (name.starts_with('.') && name != ".well-known")
}

fn route_segment(route_id: &str) -> String {
    if route_id == "/" {
        return String::new();
    }

    route_id
        .rsplit('/')
        .next()
        .map(ToString::to_string)
        .unwrap_or_default()
}

fn select_page_ancestors(
    ancestors: &[AncestorNode],
    parent_id: Option<&NamedLayoutReference>,
) -> Result<Vec<AncestorNode>> {
    let Some(parent_id) = parent_id else {
        return Ok(ancestors.to_vec());
    };

    let Some(index) = ancestors
        .iter()
        .rposition(|ancestor| ancestor.segment == parent_id.target_segment)
    else {
        return Err(ManifestError::MissingNamedLayoutSegment {
            source_path: parent_id.source_path.to_string(),
            target_segment: parent_id.target_segment.clone(),
        }
        .into());
    };

    Ok(ancestors[..=index].to_vec())
}

fn set_slot(
    kind: &'static str,
    _route_id: &str,
    slot: &mut Option<Utf8PathBuf>,
    incoming: Utf8PathBuf,
) -> Result<()> {
    if let Some(existing) = slot {
        let directory = format_route_directory(&incoming);
        let existing_name = existing
            .file_name()
            .unwrap_or(existing.as_str())
            .to_string();
        let incoming_name = incoming
            .file_name()
            .unwrap_or(incoming.as_str())
            .to_string();
        return Err(Error::DuplicateRouteFile {
            kind,
            directory,
            existing_name,
            incoming_name,
        });
    }

    *slot = Some(incoming);
    Ok(())
}

fn format_route_directory(path: &Utf8Path) -> String {
    let directory = path.parent().map(Utf8Path::as_str).unwrap_or_default();
    if directory.is_empty() {
        "/".to_string()
    } else {
        format!("{directory}/")
    }
}

fn relative_to_cwd(path: &Utf8Path, cwd: &Utf8Path) -> Result<Utf8PathBuf> {
    let relative = path
        .strip_prefix(cwd)
        .map_err(|_| ManifestError::PathNotUnderRoot {
            path: path.to_string(),
            root: cwd.to_string(),
        })?;
    Ok(Utf8PathBuf::from(relative.as_str().replace('\\', "/")))
}

fn validate_route_id(route_id: &str) -> Result<()> {
    if route_id.contains("][") {
        return Err(ManifestError::AdjacentParams {
            route_id: route_id.to_string(),
        }
        .into());
    }

    let open = route_id.chars().filter(|ch| *ch == '[').count();
    let close = route_id.chars().filter(|ch| *ch == ']').count();
    if open != close {
        return Err(ManifestError::UnbalancedBrackets {
            route_id: route_id.to_string(),
        }
        .into());
    }

    if route_id.contains("/[[...") {
        return Err(ManifestError::OptionalRestSegment {
            route_id: route_id.to_string(),
        }
        .into());
    }

    if route_id.contains("/[...") && route_id.contains("/[[") {
        let rest_index = route_id.find("/[...").unwrap_or(usize::MAX);
        let optional_index = route_id.find("/[[").unwrap_or(usize::MAX);
        if optional_index > rest_index {
            return Err(ManifestError::OptionalAfterRest {
                route_id: route_id.to_string(),
            }
            .into());
        }
    }

    Ok(())
}

fn validate_route_segment(route_id: &str, segment: &str) -> Result<()> {
    for capture in Regex::new(r"\[([ux])\+([^\]]+)\]")
        .expect("valid route escape regex")
        .captures_iter(route_id)
    {
        let matched = capture.get(0).expect("matched capture").as_str();
        let kind = capture.get(1).expect("escape kind").as_str();
        let code = capture.get(2).expect("escape code").as_str();

        if matched != matched.to_ascii_lowercase() {
            return Err(ManifestError::UppercaseEscape {
                route_id: route_id.to_string(),
            }
            .into());
        }

        if !code.chars().all(|ch| ch.is_ascii_hexdigit()) {
            return Err(ManifestError::InvalidEscape {
                route_id: route_id.to_string(),
            }
            .into());
        }

        match kind {
            "x" if code.len() != 2 => {
                return Err(ManifestError::InvalidHexEscapeLength {
                    route_id: route_id.to_string(),
                }
                .into());
            }
            "u" if !(4..=6).contains(&code.len()) => {
                return Err(ManifestError::InvalidUnicodeEscapeLength {
                    route_id: route_id.to_string(),
                }
                .into());
            }
            _ => {}
        }
    }

    if segment.contains('#') {
        return Err(ManifestError::HashInSegment {
            route_id: route_id.to_string(),
            suggested: route_id.replace('#', "[x+23]"),
        }
        .into());
    }

    Ok(())
}

fn prevent_conflicts(routes: &[DiscoveredRoute]) -> Result<()> {
    let mut seen = std::collections::BTreeMap::<String, String>::new();

    for route in routes {
        if route.page.is_none() && route.endpoint.is_none() {
            continue;
        }

        let normalized = normalize_route_id(&route.id);
        let mut route_keys = std::collections::BTreeSet::new();
        for key in conflict_keys(&normalized) {
            if !route_keys.insert(key.clone()) {
                continue;
            }

            if let Some(existing) = seen.insert(key, route.id.clone()) {
                return Err(ManifestError::RouteConflict {
                    existing,
                    incoming: route.id.clone(),
                }
                .into());
            }
        }
    }

    Ok(())
}

fn normalize_route_id(route_id: &str) -> String {
    let mut normalized = String::new();

    for segment in route_id.split('/') {
        if segment.is_empty() {
            normalized.push('/');
            continue;
        }

        if segment.starts_with('(') && segment.ends_with(')') {
            continue;
        }

        normalized.push('/');
        normalized.push_str(&normalize_segment(segment));
    }

    normalized
}

fn normalize_segment(segment: &str) -> String {
    let mut normalized = String::new();
    let mut cursor = 0;

    while let Some(offset) = segment[cursor..].find('[') {
        let start = cursor + offset;
        normalized.push_str(&segment[cursor..start]);

        let (replacement, next_cursor) = normalize_bracket_expression(segment, start);
        normalized.push_str(&replacement);
        cursor = next_cursor;
    }

    normalized.push_str(&segment[cursor..]);
    normalized
}

fn normalize_bracket_expression(segment: &str, start: usize) -> (String, usize) {
    let bytes = segment.as_bytes();
    let optional = bytes.get(start + 1) == Some(&b'[');
    let content_start = start + if optional { 2 } else { 1 };
    let closing = segment[content_start..]
        .find(']')
        .map(|offset| content_start + offset)
        .unwrap_or(segment.len());
    let content = &segment[content_start..closing];
    let next_cursor = if optional { closing + 2 } else { closing + 1 };

    if let Some(decoded) = decode_route_escape(content) {
        return (decoded, next_cursor);
    }

    let rest = content.strip_prefix("...");
    let matcher = rest
        .unwrap_or(content)
        .split_once('=')
        .map(|(_, matcher)| matcher)
        .unwrap_or("*");

    let mut replacement = String::from("<");
    if optional {
        replacement.push('?');
    }
    if rest.is_some() {
        replacement.push_str("...");
    }
    replacement.push_str(matcher);
    replacement.push('>');

    (replacement, next_cursor)
}

fn decode_route_escape(content: &str) -> Option<String> {
    let (kind, hex) = content.split_once('+')?;
    if !hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return None;
    }

    let value = match kind {
        "x" if hex.len() == 2 => u32::from_str_radix(hex, 16).ok()?,
        "u" if (4..=6).contains(&hex.len()) => u32::from_str_radix(hex, 16).ok()?,
        _ => return None,
    };

    let decoded = char::from_u32(value)?;
    if decoded == '/' {
        Some("%2f".to_string())
    } else {
        Some(decoded.to_string())
    }
}

fn conflict_keys(normalized: &str) -> Vec<String> {
    expand_optional_segments(normalized)
        .into_iter()
        .map(|candidate| cleanup_conflict_key(&candidate))
        .collect()
}

fn expand_optional_segments(route: &str) -> Vec<String> {
    let Some(start) = route.find("<?") else {
        return vec![route.to_string()];
    };
    let Some(end_offset) = route[start..].find('>') else {
        return vec![route.to_string()];
    };
    let end = start + end_offset;
    let matcher = &route[start + 2..end];
    let prefix = &route[..start];
    let suffix = &route[end + 1..];

    let mut expanded = expand_optional_segments(&format!("{prefix}{suffix}"));
    expanded.extend(expand_optional_segments(&format!(
        "{prefix}<{matcher}>{suffix}"
    )));
    expanded
}

fn cleanup_conflict_key(candidate: &str) -> String {
    let mut normalized = String::new();
    let mut previous_was_slash = false;

    for ch in candidate.chars() {
        if ch == '/' {
            if previous_was_slash {
                continue;
            }
            previous_was_slash = true;
        } else {
            previous_was_slash = false;
        }

        normalized.push(ch);
    }

    normalized.trim_matches('/').to_string()
}
