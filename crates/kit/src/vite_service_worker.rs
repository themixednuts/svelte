use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

use crate::{
    Result, ValidatedKitConfig, ViteAlias, ViteGuardError, create_static_module,
    env_static_public_module_id, get_config_aliases, normalize_vite_id, service_worker_module_id,
    strip_virtual_prefix,
};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServiceWorkerBuildEntry {
    pub file: String,
    pub css: Vec<String>,
    pub assets: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceWorkerBuildPlan {
    pub out_dir: String,
    pub entry_file_name: String,
    pub asset_file_name_pattern: String,
    pub inline_dynamic_imports: bool,
    pub aliases: Vec<ViteAlias>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceWorkerBuildInvocationPlan {
    pub build_plan: ServiceWorkerBuildPlan,
    pub service_worker_module: String,
    pub public_env_module: String,
    pub rename_from: Option<String>,
    pub rename_to: Option<String>,
}

pub fn collect_service_worker_build_files(
    manifest: &BTreeMap<String, ServiceWorkerBuildEntry>,
) -> Vec<String> {
    let mut files = BTreeSet::new();
    for entry in manifest.values() {
        files.insert(entry.file.clone());
        files.extend(entry.css.iter().cloned());
        files.extend(entry.assets.iter().cloned());
    }
    files.into_iter().collect()
}

pub fn render_service_worker_module(
    base_expression: &str,
    build: &[String],
    files: &[String],
    prerendered: &[String],
    version: &str,
) -> String {
    fn render_array(values: &[String], already_absolute: bool) -> String {
        if values.is_empty() {
            return "[]".to_string();
        }

        let body = values
            .iter()
            .map(|value| {
                let value = if already_absolute || value.starts_with('/') {
                    value.clone()
                } else {
                    format!("/{value}")
                };
                format!("\tbase + {value:?}")
            })
            .collect::<Vec<_>>()
            .join(",\n");
        format!("[\n{body}\n]")
    }

    format!(
        "export const base = /*@__PURE__*/ {base_expression};\n\n\
export const build = {build};\n\n\
export const files = {files};\n\n\
export const prerendered = {prerendered};\n\n\
export const version = {version:?};\n",
        build = render_array(build, false),
        files = render_array(files, false),
        prerendered = render_array(prerendered, true),
    )
}

pub fn create_service_worker_module(kit: &ValidatedKitConfig, assets: &[String]) -> String {
    let files = assets
        .iter()
        .filter(|asset| kit.service_worker.includes(asset))
        .map(|asset| format!("{}/{}", kit.paths.base, asset).replace("//", "/"))
        .collect::<Vec<_>>();

    format!(
        "if (typeof self === 'undefined' || self instanceof ServiceWorkerGlobalScope === false) {{\n\
\tthrow new Error('This module can only be imported inside a service worker');\n\
}}\n\n\
export const build = [];\n\
export const files = {};\n\
export const prerendered = [];\n\
export const version = {:?};\n",
        serde_json::to_string_pretty(&files).expect("json array"),
        kit.version.name
    )
}

pub fn resolve_service_worker_virtual_module(
    id: &str,
    service_worker_code: &str,
    public_env: &Map<String, Value>,
    lib: &str,
    cwd: &str,
) -> Result<String> {
    if id == service_worker_module_id() {
        return Ok(service_worker_code.to_string());
    }

    if id == env_static_public_module_id() {
        return Ok(create_static_module("$env/static/public", public_env));
    }

    let normalized = normalize_vite_id(id, lib, cwd);
    let stripped = strip_virtual_prefix(&normalized);
    Err(ViteGuardError::ServiceWorkerImport {
        normalized: stripped,
    }
    .into())
}

pub const fn service_worker_entry_output_filename(is_rolldown: bool) -> &'static str {
    if is_rolldown {
        "service-worker.js"
    } else {
        "service-worker.mjs"
    }
}

pub const fn should_rename_service_worker_output(is_rolldown: bool) -> bool {
    !is_rolldown
}

pub fn service_worker_runtime_asset_url(filename: &str) -> String {
    format!("new URL({filename:?}, location.href).pathname")
}

pub fn service_worker_build_plan(
    kit: &ValidatedKitConfig,
    out: &str,
    is_rolldown: bool,
) -> Result<ServiceWorkerBuildPlan> {
    Ok(ServiceWorkerBuildPlan {
        out_dir: format!("{out}/client"),
        entry_file_name: service_worker_entry_output_filename(is_rolldown).to_string(),
        asset_file_name_pattern: format!("{}/immutable/assets/[name].[hash][extname]", kit.app_dir),
        inline_dynamic_imports: true,
        aliases: get_config_aliases(kit)?,
    })
}

pub fn service_worker_build_invocation_plan(
    kit: &ValidatedKitConfig,
    out: &str,
    build_entries: &BTreeMap<String, ServiceWorkerBuildEntry>,
    public_files: &[String],
    prerendered_paths: &[String],
    version: &str,
    public_env: &Map<String, Value>,
    is_rolldown: bool,
) -> Result<ServiceWorkerBuildInvocationPlan> {
    let build_plan = service_worker_build_plan(kit, out, is_rolldown)?;
    let service_worker_module = render_service_worker_module(
        "location.pathname.split('/').slice(0, -1).join('/')",
        &collect_service_worker_build_files(build_entries),
        public_files,
        &prerendered_paths
            .iter()
            .map(|path| {
                path.strip_prefix(&kit.paths.base)
                    .unwrap_or(path)
                    .to_string()
            })
            .collect::<Vec<_>>(),
        version,
    );
    let public_env_module = create_static_module("$env/static/public", public_env);
    let (rename_from, rename_to) = if should_rename_service_worker_output(is_rolldown) {
        (
            Some(format!("{out}/client/service-worker.mjs")),
            Some(format!("{out}/client/service-worker.js")),
        )
    } else {
        (None, None)
    };

    Ok(ServiceWorkerBuildInvocationPlan {
        build_plan,
        service_worker_module,
        public_env_module,
        rename_from,
        rename_to,
    })
}
