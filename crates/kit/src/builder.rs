use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use regex::Regex;
use serde_json::Value;

use crate::{
    config::ValidatedConfig,
    env::{EnvKind, create_dynamic_module},
    error::{Error, Result},
    generate_manifest::{BuildData, RemoteChunk, find_server_assets, generate_manifest},
    manifest::ManifestRoute,
    routing::get_route_segments,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteSegment {
    pub content: String,
    pub dynamic: bool,
    pub rest: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuilderPrerenderOption {
    False,
    True,
    Auto,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BuilderRouteApi {
    pub methods: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BuilderRoutePage {
    pub methods: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct BuilderRouteDefinition {
    pub id: String,
    pub api: BuilderRouteApi,
    pub page: BuilderRoutePage,
    pub pattern: Regex,
    pub prerender: BuilderPrerenderOption,
    pub segments: Vec<RouteSegment>,
    pub methods: Vec<String>,
    pub config: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BuilderServerMetadataRoute {
    pub config: Value,
    pub api: BuilderRouteApi,
    pub page: BuilderRoutePage,
    pub methods: Vec<String>,
    pub prerender: Option<BuilderPrerenderOption>,
    pub entries: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BuilderServerMetadata {
    pub routes: BTreeMap<String, BuilderServerMetadataRoute>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PrerenderedPage {
    pub file: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PrerenderedAsset {
    pub type_: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PrerenderedRedirect {
    pub status: u16,
    pub location: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BuilderPrerendered {
    pub pages: BTreeMap<String, PrerenderedPage>,
    pub assets: BTreeMap<String, PrerenderedAsset>,
    pub redirects: BTreeMap<String, PrerenderedRedirect>,
    pub paths: Vec<String>,
}

#[derive(Debug)]
pub struct BuilderFacade<'a> {
    pub config: &'a ValidatedConfig,
    pub prerendered: &'a BuilderPrerendered,
    route_data: &'a [ManifestRoute],
    route_lookup: BTreeMap<String, usize>,
    route_definitions: Vec<BuilderRouteDefinition>,
    build_data: &'a BuildData<'a>,
    remotes: &'a [RemoteChunk],
}

pub struct BuilderAdapterEntry<'a> {
    pub id: String,
    pub filter: BuilderRouteFilter<'a>,
}

pub struct BuilderRouteFilter<'a> {
    inner: Box<dyn Fn(&BuilderRouteDefinition) -> bool + 'a>,
}

impl<'a> BuilderRouteFilter<'a> {
    pub fn new<F>(filter: F) -> Self
    where
        F: Fn(&BuilderRouteDefinition) -> bool + 'a,
    {
        Self {
            inner: Box::new(filter),
        }
    }

    pub fn call(&self, route: &BuilderRouteDefinition) -> bool {
        (self.inner)(route)
    }
}

pub struct BuilderEntryContext<'a> {
    facade: &'a BuilderFacade<'a>,
    pub id: String,
    pub routes: Vec<&'a BuilderRouteDefinition>,
}

impl<'a> BuilderEntryContext<'a> {
    pub fn generate_manifest(&self, relative_path: &str) -> Result<String> {
        let routes = self
            .routes
            .iter()
            .map(|route| (*route).clone())
            .collect::<Vec<_>>();
        self.facade
            .generate_manifest_with_prerendered(relative_path, Some(&routes), &[])
    }
}

impl<'a> BuilderFacade<'a> {
    pub fn new(
        config: &'a ValidatedConfig,
        build_data: &'a BuildData<'a>,
        server_metadata: &BuilderServerMetadata,
        route_data: &'a [ManifestRoute],
        prerendered: &'a BuilderPrerendered,
        remotes: &'a [RemoteChunk],
    ) -> Self {
        let route_definitions = build_route_definitions(route_data, server_metadata, prerendered);
        let route_lookup = route_data
            .iter()
            .enumerate()
            .map(|(index, route)| (route.id.clone(), index))
            .collect();

        Self {
            config,
            prerendered,
            route_data,
            route_lookup,
            route_definitions,
            build_data,
            remotes,
        }
    }

    pub fn routes(&self) -> &[BuilderRouteDefinition] {
        &self.route_definitions
    }

    pub fn find_server_assets(&self, routes: &[BuilderRouteDefinition]) -> Vec<String> {
        let manifest_routes = routes
            .iter()
            .filter_map(|route| {
                self.route_lookup
                    .get(&route.id)
                    .and_then(|index| self.route_data.get(*index))
                    .cloned()
            })
            .collect::<Vec<_>>();
        find_server_assets(self.build_data, &manifest_routes)
    }

    pub fn generate_manifest(
        &self,
        relative_path: &str,
        routes: Option<&[BuilderRouteDefinition]>,
    ) -> Result<String> {
        self.generate_manifest_with_prerendered(relative_path, routes, &self.prerendered.paths)
    }

    pub fn create_entries<P, C>(&'a self, mut planner: P, mut complete: C) -> Result<()>
    where
        P: FnMut(&'a BuilderRouteDefinition) -> BuilderAdapterEntry<'a>,
        C: FnMut(BuilderEntryContext<'a>) -> Result<()>,
    {
        let mut seen = BTreeSet::new();

        for route in &self.route_definitions {
            if self.prerendered.paths.iter().any(|path| path == &route.id) {
                continue;
            }

            let entry = planner(route);
            if !seen.insert(entry.id.clone()) {
                continue;
            }

            let routes = self.group_routes(&route.id, |candidate| entry.filter.call(candidate));
            if routes.is_empty() {
                continue;
            }

            complete(BuilderEntryContext {
                facade: self,
                id: entry.id,
                routes,
            })?;
        }

        Ok(())
    }

    fn generate_manifest_with_prerendered(
        &self,
        relative_path: &str,
        routes: Option<&[BuilderRouteDefinition]>,
        prerendered_paths: &[String],
    ) -> Result<String> {
        let selected_routes = routes
            .map(|routes| {
                routes
                    .iter()
                    .filter_map(|route| {
                        self.route_lookup
                            .get(&route.id)
                            .and_then(|index| self.route_data.get(*index))
                            .cloned()
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| {
                self.route_data
                    .iter()
                    .filter(|route| !prerendered_paths.iter().any(|path| path == &route.id))
                    .cloned()
                    .collect::<Vec<_>>()
            });

        Ok(generate_manifest(
            self.build_data,
            prerendered_paths,
            relative_path,
            &selected_routes,
            self.remotes,
        )?
        .contents)
    }

    pub fn group_routes<F>(
        &self,
        seed_route_id: &str,
        mut filter: F,
    ) -> Vec<&BuilderRouteDefinition>
    where
        F: FnMut(&BuilderRouteDefinition) -> bool,
    {
        let Some(seed_index) = self
            .route_definitions
            .iter()
            .position(|route| route.id == seed_route_id)
        else {
            return Vec::new();
        };

        let mut grouped = vec![&self.route_definitions[seed_index]];

        for route in self.route_definitions.iter().skip(seed_index + 1) {
            if self.prerendered.paths.iter().any(|path| path == &route.id) {
                continue;
            }
            if filter(route) {
                grouped.push(route);
            }
        }

        let mut seen = grouped
            .iter()
            .map(|route| route.id.clone())
            .collect::<BTreeSet<_>>();

        for route in grouped.clone() {
            if route.page.methods.is_empty() {
                continue;
            }
            let endpoint_id = format!("{}.json", route.id);
            if seen.contains(&endpoint_id) {
                continue;
            }
            if let Some(endpoint) = self
                .route_definitions
                .iter()
                .find(|candidate| candidate.id == endpoint_id)
            {
                seen.insert(endpoint.id.clone());
                grouped.push(endpoint);
            }
        }

        grouped
    }

    pub fn get_build_directory(&self, name: &str) -> String {
        self.config.kit.out_dir.join(name).as_str().to_string()
    }

    pub fn get_client_directory(&self) -> String {
        self.config
            .kit
            .out_dir
            .join("output")
            .join("client")
            .as_str()
            .to_string()
    }

    pub fn get_server_directory(&self) -> String {
        self.config
            .kit
            .out_dir
            .join("output")
            .join("server")
            .as_str()
            .to_string()
    }

    pub fn get_app_path(&self) -> &str {
        &self.build_data.app_path
    }

    pub fn copy<F>(
        &self,
        from: &Utf8Path,
        to: &Utf8Path,
        replace: Option<&BTreeMap<String, String>>,
        filter: F,
    ) -> Result<Vec<String>>
    where
        F: Fn(&str) -> bool + Copy,
    {
        copy_recursively(from, to, replace, filter)
    }

    pub fn write_client(&self, dest: &Utf8Path) -> Result<Vec<String>> {
        let source = self.config.kit.out_dir.join("output").join("client");
        self.copy(&source, dest, None, |basename| basename != ".vite")
    }

    pub fn write_server(&self, dest: &Utf8Path) -> Result<Vec<String>> {
        let source = self.config.kit.out_dir.join("output").join("server");
        self.copy(&source, dest, None, |_| true)
    }

    pub fn write_prerendered(&self, dest: &Utf8Path) -> Result<Vec<String>> {
        let source = self.config.kit.out_dir.join("output").join("prerendered");
        let mut written = Vec::new();
        for segment in ["pages", "dependencies", "data"] {
            let source_dir = source.join(segment);
            if !source_dir.exists() {
                continue;
            }
            written.extend(self.copy(&source_dir, dest, None, |_| true)?);
        }
        Ok(written)
    }

    pub fn generate_env_module(
        &self,
        public_env: &serde_json::Map<String, Value>,
    ) -> Result<Utf8PathBuf> {
        let dest = self
            .config
            .kit
            .out_dir
            .join("output")
            .join("prerendered")
            .join("dependencies")
            .join(&self.config.kit.app_dir)
            .join("env.js");
        let contents = create_dynamic_module(EnvKind::Public, Some(public_env), "");
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&dest, contents)?;
        Ok(dest)
    }
}

pub fn build_route_definitions(
    route_data: &[ManifestRoute],
    server_metadata: &BuilderServerMetadata,
    prerendered: &BuilderPrerendered,
) -> Vec<BuilderRouteDefinition> {
    route_data
        .iter()
        .map(|route| {
            let metadata = server_metadata.routes.get(&route.id);
            let prerender = metadata
                .and_then(|metadata| metadata.prerender.clone())
                .unwrap_or_else(|| {
                    if prerendered.paths.iter().any(|path| path == &route.id) {
                        BuilderPrerenderOption::True
                    } else {
                        BuilderPrerenderOption::False
                    }
                });

            BuilderRouteDefinition {
                id: route.id.clone(),
                api: metadata
                    .map(|metadata| metadata.api.clone())
                    .unwrap_or_default(),
                page: metadata
                    .map(|metadata| metadata.page.clone())
                    .unwrap_or_default(),
                pattern: route.pattern.clone(),
                prerender,
                segments: route_segments(&route.id),
                methods: metadata
                    .map(|metadata| metadata.methods.clone())
                    .unwrap_or_default(),
                config: metadata
                    .map(|metadata| metadata.config.clone())
                    .unwrap_or(Value::Null),
            }
        })
        .collect()
}

fn route_segments(route_id: &str) -> Vec<RouteSegment> {
    get_route_segments(route_id)
        .into_iter()
        .map(|segment| RouteSegment {
            dynamic: segment.contains('['),
            rest: segment.contains("[..."),
            content: segment.to_string(),
        })
        .collect()
}

fn copy_recursively<F>(
    source: &Utf8Path,
    target: &Utf8Path,
    replace: Option<&BTreeMap<String, String>>,
    filter: F,
) -> Result<Vec<String>>
where
    F: Fn(&str) -> bool + Copy,
{
    if !source.exists() {
        return Ok(Vec::new());
    }

    let prefix = target.as_str().replace('\\', "/");
    let mut files = Vec::new();
    copy_entry(source, target, &prefix, replace, filter, &mut files)?;
    Ok(files)
}

fn copy_entry<F>(
    from: &Utf8Path,
    to: &Utf8Path,
    prefix: &str,
    replace: Option<&BTreeMap<String, String>>,
    filter: F,
    files: &mut Vec<String>,
) -> Result<()>
where
    F: Fn(&str) -> bool + Copy,
{
    let Some(basename) = from.file_name() else {
        return Ok(());
    };

    if !filter(basename) {
        return Ok(());
    }

    let metadata = fs::metadata(from)?;
    if metadata.is_dir() {
        fs::create_dir_all(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            let source_path =
                Utf8PathBuf::from_path_buf(entry.path()).map_err(|_| Error::InvalidUtf8Path)?;
            let target_path = to.join(entry.file_name().to_string_lossy().as_ref());
            copy_entry(&source_path, &target_path, prefix, replace, filter, files)?;
        }
        return Ok(());
    }

    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }

    if let Some(replace) = replace {
        let mut contents = fs::read_to_string(from)?;
        for (from, to) in replace {
            contents = contents.replace(from, to);
        }
        fs::write(to, contents)?;
    } else {
        fs::copy(from, to)?;
    }

    let target = to.as_str().replace('\\', "/");
    let relative = if target == prefix {
        to.file_name().unwrap_or(to.as_str()).to_string()
    } else {
        let prefix_with_slash = format!("{prefix}/");
        target
            .strip_prefix(&prefix_with_slash)
            .unwrap_or(&target)
            .to_string()
    };
    files.push(relative);
    Ok(())
}
