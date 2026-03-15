use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
};

use camino::{Utf8Path, Utf8PathBuf};
use oxc_allocator::Allocator;
use oxc_ast::{
    CommentContent,
    ast::Comment,
    ast::{
        BindingPattern, Declaration, Expression, FormalParameter, ImportDeclarationSpecifier,
        ImportOrExportKind, ModuleExportName, ObjectPropertyKind, Statement,
    },
};
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType, Span};
use rayon::prelude::*;
use regex::Regex;
use serde_json::{Map, Value, json};

use crate::LoadedKitProject;
use crate::config::{
    CspMode, PreloadStrategy, RouterType, ValidatedConfig, ValidatedCspConfig,
    ValidatedCspDirectives, ValidatedKitConfig,
};
use crate::error::Result;
use crate::manifest::{KitManifest, ManifestNode};
use crate::runtime::server::RuntimePageNodes;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedClientManifest {
    pub app: String,
    pub matchers: Option<String>,
    pub nodes: Vec<GeneratedNodeModule>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedNodeModule {
    pub index: usize,
    pub contents: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedServerInternal {
    pub contents: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedRoot {
    pub svelte: String,
    pub js: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedNonAmbient {
    pub contents: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedAmbient {
    pub contents: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedTsConfig {
    pub contents: String,
    pub custom_hook_source: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncWriteResult {
    pub written_files: Vec<Utf8PathBuf>,
    pub tsconfig_warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GeneratedTypeFile {
    path: Utf8PathBuf,
    contents: String,
}

#[derive(Debug, Clone)]
struct RouteTypeFileContext<'a> {
    route: &'a crate::manifest::DiscoveredRoute,
    path: Utf8PathBuf,
    outdir: Utf8PathBuf,
}

pub fn generate_client_manifest(
    kit: &ValidatedKitConfig,
    manifest: &KitManifest,
    output: &Utf8Path,
) -> Result<GeneratedClientManifest> {
    let nodes_dir = output.join("nodes");
    let node_modules = manifest
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| GeneratedNodeModule {
            index,
            contents: generate_node_module(&nodes_dir, node),
        })
        .collect::<Vec<_>>();

    let client_routing = kit.router.resolution == crate::config::RouterResolution::Client;
    let server_loads = collect_layouts_with_server_load(manifest);
    let app = generate_app_module(kit, manifest, output, client_routing, &server_loads)?;
    let matchers = if client_routing {
        Some(generate_matchers_module(manifest, output))
    } else {
        None
    };

    Ok(GeneratedClientManifest {
        app,
        matchers,
        nodes: node_modules,
    })
}

pub fn generate_root(manifest: &KitManifest) -> GeneratedRoot {
    let max_depth = manifest
        .manifest_routes
        .iter()
        .filter_map(|route| route.page.as_ref())
        .map(|page| {
            page.layouts
                .iter()
                .filter(|layout| layout.is_some())
                .count()
                + 1
        })
        .max()
        .unwrap_or(1);

    let levels = (0..=max_depth).collect::<Vec<_>>();
    let svelte = generate_root_svelte(&levels, max_depth);
    let js = "import { asClassComponent } from 'svelte/legacy';\nimport Root from './root.svelte';\nexport default asClassComponent(Root);".to_string();

    GeneratedRoot { svelte, js }
}

pub fn generate_non_ambient(manifest: &KitManifest) -> GeneratedNonAmbient {
    let app_types = generate_app_types(manifest);
    let contents = format!(
        "{}\n\ndeclare module \"svelte/elements\" {{\n\texport interface HTMLAttributes<T> {{\n\t\t'data-sveltekit-keepfocus'?: true | '' | 'off' | undefined | null;\n\t\t'data-sveltekit-noscroll'?: true | '' | 'off' | undefined | null;\n\t\t'data-sveltekit-preload-code'?:\n\t\t\t| true\n\t\t\t| ''\n\t\t\t| 'eager'\n\t\t\t| 'viewport'\n\t\t\t| 'hover'\n\t\t\t| 'tap'\n\t\t\t| 'off'\n\t\t\t| undefined\n\t\t\t| null;\n\t\t'data-sveltekit-preload-data'?: true | '' | 'hover' | 'tap' | 'off' | undefined | null;\n\t\t'data-sveltekit-reload'?: true | '' | 'off' | undefined | null;\n\t\t'data-sveltekit-replacestate'?: true | '' | 'off' | undefined | null;\n\t}}\n}}\n\nexport {{}};\n\n{}",
        "// this file is generated — do not edit it", app_types
    );

    GeneratedNonAmbient { contents }
}

pub fn generate_ambient(config: &ValidatedConfig, mode: &str) -> GeneratedAmbient {
    let env = collect_env(&config.kit.env.dir, mode);
    let private_env = filter_env(
        &env,
        &config.kit.env.private_prefix,
        &config.kit.env.public_prefix,
    );
    let public_env = filter_env(
        &env,
        &config.kit.env.public_prefix,
        &config.kit.env.private_prefix,
    );

    let contents = format!(
        "// this file is generated — do not edit it\n\n/// <reference types=\"@sveltejs/kit\" />\n\n{}\n{}\n\n{}\n{}\n\n{}\n{}\n\n{}\n{}",
        doc_comment(include_str!(
            "../../../kit/packages/kit/src/types/synthetic/$env+static+private.md"
        )),
        create_static_types("$env/static/private", &private_env),
        doc_comment(include_str!(
            "../../../kit/packages/kit/src/types/synthetic/$env+static+public.md"
        )),
        create_static_types("$env/static/public", &public_env),
        doc_comment(include_str!(
            "../../../kit/packages/kit/src/types/synthetic/$env+dynamic+private.md"
        )),
        create_dynamic_types(
            "$env/dynamic/private",
            &private_env,
            &config.kit.env.public_prefix,
            &config.kit.env.private_prefix,
            true,
        ),
        doc_comment(include_str!(
            "../../../kit/packages/kit/src/types/synthetic/$env+dynamic+public.md"
        )),
        create_dynamic_types(
            "$env/dynamic/public",
            &public_env,
            &config.kit.env.public_prefix,
            &config.kit.env.private_prefix,
            false,
        ),
    );

    GeneratedAmbient { contents }
}

pub fn generate_tsconfig(cwd: &Utf8Path, kit: &ValidatedKitConfig) -> GeneratedTsConfig {
    let config_relative = |file: &Utf8Path| relative_path(&kit.out_dir, file);

    let mut include = BTreeSet::new();
    include.insert("ambient.d.ts".to_string());
    include.insert("non-ambient.d.ts".to_string());
    include.insert("./types/**/$types.d.ts".to_string());
    include.insert(config_relative(&cwd.join("vite.config.js")));
    include.insert(config_relative(&cwd.join("vite.config.ts")));

    let src_includes = [&kit.files.routes, &kit.files.lib, &kit.files.src]
        .into_iter()
        .filter(|dir| {
            pathdiff::diff_paths(dir, &kit.files.src)
                .map(|relative| relative.as_os_str().is_empty() || relative.starts_with(".."))
                .unwrap_or(true)
        })
        .collect::<Vec<_>>();

    for dir in src_includes {
        include.insert(config_relative(&dir.join("**/*.js")));
        include.insert(config_relative(&dir.join("**/*.ts")));
        include.insert(config_relative(&dir.join("**/*.svelte")));
    }

    for test_dir in ["test", "tests"] {
        let dir = cwd.join(test_dir);
        include.insert(config_relative(&dir.join("**/*.js")));
        include.insert(config_relative(&dir.join("**/*.ts")));
        include.insert(config_relative(&dir.join("**/*.svelte")));
    }

    let mut exclude = vec![config_relative(&cwd.join("node_modules").join("**"))];
    if kit.files.service_worker.extension().is_some() {
        exclude.push(config_relative(&kit.files.service_worker));
    } else {
        for suffix in [".js", "/**/*.js", ".ts", "/**/*.ts", ".d.ts", "/**/*.d.ts"] {
            exclude.push(config_relative(&Utf8PathBuf::from(format!(
                "{}{}",
                kit.files.service_worker, suffix
            ))));
        }
    }

    let mut paths = generate_tsconfig_paths(cwd, kit);
    paths.insert("$app/types".to_string(), json!(["./types/index.d.ts"]));

    let mut compiler_options = Map::new();
    compiler_options.insert("paths".to_string(), Value::Object(paths));
    compiler_options.insert(
        "rootDirs".to_string(),
        json!([config_relative(cwd), "./types"]),
    );
    compiler_options.insert("verbatimModuleSyntax".to_string(), Value::Bool(true));
    compiler_options.insert("isolatedModules".to_string(), Value::Bool(true));
    compiler_options.insert("lib".to_string(), json!(["esnext", "DOM", "DOM.Iterable"]));
    compiler_options.insert(
        "moduleResolution".to_string(),
        Value::String("bundler".to_string()),
    );
    compiler_options.insert("module".to_string(), Value::String("esnext".to_string()));
    compiler_options.insert("noEmit".to_string(), Value::Bool(true));
    compiler_options.insert("target".to_string(), Value::String("esnext".to_string()));

    let mut config = Map::new();
    config.insert(
        "compilerOptions".to_string(),
        Value::Object(compiler_options),
    );
    config.insert(
        "include".to_string(),
        Value::Array(include.into_iter().map(Value::String).collect()),
    );
    config.insert(
        "exclude".to_string(),
        Value::Array(exclude.into_iter().map(Value::String).collect()),
    );

    GeneratedTsConfig {
        contents: serde_json::to_string_pretty(&Value::Object(config)).expect("serialize tsconfig"),
        custom_hook_source: kit
            .typescript
            .custom_config_source()
            .map(ToString::to_string),
        warnings: validate_user_tsconfig(cwd, &kit.out_dir.join("tsconfig.json"))
            .unwrap_or_default(),
    }
}

pub fn generate_server_internal(
    config: &ValidatedConfig,
    output: &Utf8Path,
    runtime_directory: &Utf8Path,
    root_module: &str,
) -> Result<GeneratedServerInternal> {
    let server_dir = output.join("server");
    let server_hooks = resolve_entry(
        &config.kit.files.hooks.server,
        &config.kit.module_extensions,
    );
    let universal_hooks = resolve_entry(
        &config.kit.files.hooks.universal,
        &config.kit.module_extensions,
    );
    let template = crate::load_template(&config.kit.files.src, config)?;
    let error_page = crate::load_error_page(config)?;
    let template_contains_nonce = template.contains("%sveltekit.nonce%");
    let has_service_worker = config.kit.service_worker.register
        && resolve_entry(
            &config.kit.files.service_worker,
            &config.kit.module_extensions,
        )
        .is_some();

    let server_hooks_import = server_hooks
        .as_ref()
        .map(|path| relative_path(&server_dir, path));
    let universal_hooks_import = universal_hooks
        .as_ref()
        .map(|path| relative_path(&server_dir, path));
    let runtime_import = relative_path(&server_dir, runtime_directory);
    let app_template = js_template_renderer(&template, &config.kit.version.name, true);
    let error_template = js_error_renderer(&error_page);
    let service_worker_options = if config.kit.service_worker.register {
        config
            .kit
            .service_worker
            .options
            .as_ref()
            .map(json_string)
            .unwrap_or_else(|| "undefined".to_string())
    } else {
        "null".to_string()
    };

    let mut module = String::new();
    module.push_str(&format!("import root from '../{root_module}';\n"));
    module.push_str("import { set_building, set_prerendering } from '__sveltekit/environment';\n");
    module.push_str("import { set_assets } from '$app/paths/internal/server';\n");
    module
        .push_str("import { set_manifest, set_read_implementation } from '__sveltekit/server';\n");
    module.push_str(&format!(
        "import {{ set_private_env, set_public_env }} from '{runtime_import}/shared-server.js';\n\n"
    ));
    module.push_str("export const options = {\n");
    module.push_str(&format!(
        "\tapp_template_contains_nonce: {template_contains_nonce},\n"
    ));
    module.push_str(&format!(
        "\tasync: {},\n",
        json_bool(config.compiler_options.experimental.async_)
    ));
    module.push_str(&format!(
        "\tcsp: {},\n",
        json_string(&csp_json(&config.kit.csp))
    ));
    module.push_str(&format!(
        "\tcsrf_check_origin: {},\n",
        json_bool(
            config.kit.csrf.check_origin
                && !config
                    .kit
                    .csrf
                    .trusted_origins
                    .iter()
                    .any(|origin| origin == "*")
        )
    ));
    module.push_str(&format!(
        "\tcsrf_trusted_origins: {},\n",
        json_string(&config.kit.csrf.trusted_origins)
    ));
    module.push_str(&format!("\tembedded: {},\n", config.kit.embedded));
    module.push_str(&format!(
        "\tenv_public_prefix: {},\n",
        json_string(&config.kit.env.public_prefix)
    ));
    module.push_str(&format!(
        "\tenv_private_prefix: {},\n",
        json_string(&config.kit.env.private_prefix)
    ));
    module.push_str(&format!(
        "\thash_routing: {},\n",
        json_bool(config.kit.router.type_ == RouterType::Hash)
    ));
    module.push_str("\thooks: null, // added lazily, via `get_hooks`\n");
    module.push_str(&format!(
        "\tpreload_strategy: {},\n",
        json_string(&preload_strategy_name(&config.kit.output.preload_strategy))
    ));
    module.push_str("\troot,\n");
    module.push_str(&format!(
        "\tservice_worker: {},\n",
        json_bool(has_service_worker)
    ));
    module.push_str(&format!(
        "\tservice_worker_options: {service_worker_options},\n"
    ));
    module.push_str("\ttemplates: {\n");
    module.push_str("\t\tapp: ({ head, body, assets, nonce, env }) => ");
    module.push_str(&app_template);
    module.push_str(",\n");
    module.push_str("\t\terror: ({ status, message }) => ");
    module.push_str(&error_template);
    module.push_str("\n\t},\n");
    module.push_str(&format!(
        "\tversion_hash: {}\n",
        json_string(&djb2_hash(&config.kit.version.name))
    ));
    module.push_str("};\n\n");
    module.push_str("export async function get_hooks() {\n");
    module.push_str("\tlet handle;\n\tlet handleFetch;\n\tlet handleError;\n\tlet handleValidationError;\n\tlet init;\n");
    if let Some(server_hooks_import) = server_hooks_import {
        module.push_str(&format!(
            "\t({{ handle, handleFetch, handleError, handleValidationError, init }} = await import({}));\n",
            json_string(&server_hooks_import)
        ));
    }
    module.push_str("\n\tlet reroute;\n\tlet transport;\n");
    if let Some(universal_hooks_import) = universal_hooks_import {
        module.push_str(&format!(
            "\t({{ reroute, transport }} = await import({}));\n",
            json_string(&universal_hooks_import)
        ));
    }
    module.push_str(
        "\n\treturn {\n\t\thandle,\n\t\thandleFetch,\n\t\thandleError,\n\t\thandleValidationError,\n\t\tinit,\n\t\treroute,\n\t\ttransport\n\t};\n}\n\n",
    );
    module.push_str("export { set_assets, set_building, set_manifest, set_prerendering, set_private_env, set_public_env, set_read_implementation };");

    Ok(GeneratedServerInternal { contents: module })
}

pub fn write_sync_project(
    project: &LoadedKitProject,
    mode: &str,
    runtime_directory: &Utf8Path,
) -> Result<SyncWriteResult> {
    let init = init_sync_project(project, mode)?;
    let create = create_sync_project(project, runtime_directory)?;

    let mut written_files = init.written_files;
    written_files.extend(create.written_files);

    Ok(SyncWriteResult {
        written_files,
        tsconfig_warnings: init.tsconfig_warnings,
    })
}

pub fn init_sync_project(project: &LoadedKitProject, mode: &str) -> Result<SyncWriteResult> {
    let mut written_files = Vec::new();

    let tsconfig = generate_tsconfig(&project.cwd, &project.config.kit);
    write_if_changed(
        &project.config.kit.out_dir.join("tsconfig.json"),
        &tsconfig.contents,
        &mut written_files,
    )?;

    let ambient = generate_ambient(&project.config, mode);
    write_if_changed(
        &project.config.kit.out_dir.join("ambient.d.ts"),
        &ambient.contents,
        &mut written_files,
    )?;

    Ok(SyncWriteResult {
        written_files,
        tsconfig_warnings: tsconfig.warnings,
    })
}

pub fn create_sync_project(
    project: &LoadedKitProject,
    runtime_directory: &Utf8Path,
) -> Result<SyncWriteResult> {
    let mut written_files = Vec::new();
    let generated_dir = project.config.kit.out_dir.join("generated");
    let client_dir = generated_dir.join("client");
    let nodes_dir = client_dir.join("nodes");

    let non_ambient = generate_non_ambient(&project.manifest);
    write_if_changed(
        &project.config.kit.out_dir.join("non-ambient.d.ts"),
        &non_ambient.contents,
        &mut written_files,
    )?;
    written_files.extend(write_all_types(project)?);

    let root = generate_root(&project.manifest);
    write_if_changed(
        &generated_dir.join("root.svelte"),
        &root.svelte,
        &mut written_files,
    )?;
    write_if_changed(&generated_dir.join("root.js"), &root.js, &mut written_files)?;

    let client_manifest =
        generate_client_manifest(&project.config.kit, &project.manifest, &client_dir)?;
    write_if_changed(
        &client_dir.join("app.js"),
        &client_manifest.app,
        &mut written_files,
    )?;
    if let Some(matchers) = &client_manifest.matchers {
        write_if_changed(
            &client_dir.join("matchers.js"),
            matchers,
            &mut written_files,
        )?;
    }
    for node in &client_manifest.nodes {
        write_if_changed(
            &nodes_dir.join(format!("{}.js", node.index)),
            &node.contents,
            &mut written_files,
        )?;
    }

    let server = write_server_project(project, runtime_directory)?;
    written_files.extend(server.written_files);

    Ok(SyncWriteResult {
        written_files,
        tsconfig_warnings: Vec::new(),
    })
}

pub fn write_all_sync_types(project: &LoadedKitProject, mode: &str) -> Result<SyncWriteResult> {
    let init = init_sync_project(project, mode)?;
    let mut written_files = init.written_files;

    let non_ambient = generate_non_ambient(&project.manifest);
    write_if_changed(
        &project.config.kit.out_dir.join("non-ambient.d.ts"),
        &non_ambient.contents,
        &mut written_files,
    )?;
    written_files.extend(write_all_types(project)?);

    Ok(SyncWriteResult {
        written_files,
        tsconfig_warnings: init.tsconfig_warnings,
    })
}

pub fn update_sync_project_for_file(
    project: &LoadedKitProject,
    file: &Utf8Path,
) -> Result<SyncWriteResult> {
    let resolved = if file.is_absolute() {
        file.to_path_buf()
    } else {
        project.cwd.join(file)
    };

    let Some(file_name) = resolved.file_name() else {
        return Ok(SyncWriteResult {
            written_files: Vec::new(),
            tsconfig_warnings: Vec::new(),
        });
    };
    if !file_name.starts_with('+') || !resolved.starts_with(&project.config.kit.files.routes) {
        return Ok(SyncWriteResult {
            written_files: Vec::new(),
            tsconfig_warnings: Vec::new(),
        });
    }

    let refreshed = crate::config::load_project(&project.cwd)?;
    let mut written_files = write_all_types(&refreshed)?;
    let non_ambient = generate_non_ambient(&refreshed.manifest);
    write_if_changed(
        &refreshed.config.kit.out_dir.join("non-ambient.d.ts"),
        &non_ambient.contents,
        &mut written_files,
    )?;

    Ok(SyncWriteResult {
        written_files,
        tsconfig_warnings: Vec::new(),
    })
}

pub fn write_server_project(
    project: &LoadedKitProject,
    runtime_directory: &Utf8Path,
) -> Result<SyncWriteResult> {
    let mut written_files = Vec::new();
    let generated_dir = project.config.kit.out_dir.join("generated");
    let server_dir = generated_dir.join("server");

    let server = generate_server_internal(
        &project.config,
        &generated_dir,
        runtime_directory,
        "root.js",
    )?;
    write_if_changed(
        &server_dir.join("internal.js"),
        &server.contents,
        &mut written_files,
    )?;

    Ok(SyncWriteResult {
        written_files,
        tsconfig_warnings: Vec::new(),
    })
}

pub fn write_all_types(project: &LoadedKitProject) -> Result<Vec<Utf8PathBuf>> {
    let mut written_files = Vec::new();
    let generated = generate_route_type_files(project);
    let expected = generated
        .iter()
        .map(|file| file.path.clone())
        .collect::<BTreeSet<_>>();

    remove_stale_type_files(&route_types_root(project), &expected, &mut written_files)?;

    for file in generated {
        write_if_changed(&file.path, &file.contents, &mut written_files)?;
    }

    Ok(written_files)
}

fn remove_stale_type_files(
    root: &Utf8Path,
    expected: &BTreeSet<Utf8PathBuf>,
    written_files: &mut Vec<Utf8PathBuf>,
) -> Result<()> {
    if !root.is_dir() {
        return Ok(());
    }

    remove_stale_type_files_recursive(root, root, expected, written_files)
}

fn remove_stale_type_files_recursive(
    root: &Utf8Path,
    dir: &Utf8Path,
    expected: &BTreeSet<Utf8PathBuf>,
    written_files: &mut Vec<Utf8PathBuf>,
) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = Utf8PathBuf::from_path_buf(entry.path())
            .expect("generated type path should be valid utf-8");
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            remove_stale_type_files_recursive(root, &path, expected, written_files)?;
            if path != root && fs::read_dir(&path)?.next().is_none() {
                fs::remove_dir(&path)?;
            }
            continue;
        }

        if !expected.contains(&path) {
            fs::remove_file(&path)?;
            written_files.push(path);
        }
    }

    Ok(())
}

fn generate_route_type_files(project: &LoadedKitProject) -> Vec<GeneratedTypeFile> {
    let types_root = route_types_root(project);
    let mut files = Vec::new();
    let mut proxy_imports = BTreeMap::new();
    let route_contexts = collect_route_type_contexts(project, &types_root);

    for route_context in &route_contexts {
        for (path, is_server) in route_module_sources(route_context.route) {
            if let Some((file, import_specifier)) =
                create_route_module_proxy(project, &route_context.outdir, path, is_server)
            {
                proxy_imports.insert(path.as_str().to_string(), import_specifier);
                files.push(file);
            }
        }
    }

    let route_files = route_contexts
        .par_iter()
        .map(|route_context| {
            generate_route_type_declaration(project, route_context, &proxy_imports)
        })
        .collect::<Vec<_>>();
    files.extend(route_files);

    files
}

fn collect_route_type_contexts<'a>(
    project: &'a LoadedKitProject,
    types_root: &Utf8Path,
) -> Vec<RouteTypeFileContext<'a>> {
    project
        .manifest
        .routes
        .iter()
        .filter(|route| route.page.is_some() || route.layout.is_some() || route.endpoint.is_some())
        .map(|route| {
            let path = route_type_path(types_root, &route.id);
            let outdir = path.parent().expect("route types parent").to_path_buf();
            RouteTypeFileContext {
                route,
                path,
                outdir,
            }
        })
        .collect()
}

fn generate_route_type_declaration(
    project: &LoadedKitProject,
    route_context: &RouteTypeFileContext<'_>,
    proxy_imports: &BTreeMap<String, String>,
) -> GeneratedTypeFile {
    GeneratedTypeFile {
        path: route_context.path.clone(),
        contents: generate_route_type_file(project, route_context.route, proxy_imports),
    }
}

fn route_types_root(project: &LoadedKitProject) -> Utf8PathBuf {
    let relative = pathdiff::diff_paths(&project.config.kit.files.routes, &project.cwd)
        .and_then(|path| Utf8PathBuf::from_path_buf(path).ok())
        .unwrap_or_else(|| project.config.kit.files.routes.clone());
    let cleaned = strip_relative_parent_traversals(relative.as_str());
    if cleaned.is_empty() || cleaned == "." {
        project.config.kit.out_dir.join("types")
    } else {
        project.config.kit.out_dir.join("types").join(cleaned)
    }
}

fn route_type_path(types_root: &Utf8Path, route_id: &str) -> Utf8PathBuf {
    let trimmed = route_id.trim_start_matches('/');
    if trimmed.is_empty() {
        types_root.join("$types.d.ts")
    } else {
        types_root.join(trimmed).join("$types.d.ts")
    }
}

fn generate_route_type_file(
    project: &LoadedKitProject,
    route: &crate::manifest::DiscoveredRoute,
    proxy_imports: &BTreeMap<String, String>,
) -> String {
    let outdir = route_type_path(&route_types_root(project), &route.id)
        .parent()
        .expect("route types parent")
        .to_path_buf();
    let mut imports = vec!["import type * as Kit from '@sveltejs/kit';".to_string()];
    let mut declarations = vec![
        "type Expand<T> = T extends infer O ? { [K in keyof O]: O[K] } : never;".to_string(),
        "// @ts-ignore".to_string(),
        "type MatcherParam<M> = M extends (param: string) => param is infer U ? U extends string ? U : string : string;".to_string(),
        "type MaybeWithVoid<T> = {} extends T ? T | void : T;".to_string(),
        "export type RequiredKeys<T> = { [K in keyof T]-?: {} extends { [P in K]: T[K] } ? never : K; }[keyof T];".to_string(),
        "type OutputDataShape<T> = MaybeWithVoid<Omit<App.PageData, RequiredKeys<T>> & Partial<Pick<App.PageData, keyof T & keyof App.PageData>> & Record<string, any>>;".to_string(),
        "type EnsureDefined<T> = T extends null | undefined ? {} : T;".to_string(),
        "type OptionalUnion<U extends Record<string, any>, A extends keyof U = U extends U ? keyof U : never> = U extends unknown ? { [P in Exclude<A, keyof U>]?: never } & U : never;".to_string(),
        "type ModuleLoadData<T> = T extends { load: (...args: any) => infer R } ? Kit.LoadProperties<Extract<Awaited<R>, Record<string, any> | void>> : null;".to_string(),
        "type ModuleActions<T> = T extends { actions: Record<string, (...args: any) => any> } ? Expand<Kit.AwaitedActions<T['actions']>> | null : null;".to_string(),
        "export type Snapshot<T = any> = Kit.Snapshot<T>;".to_string(),
    ];
    let mut exports = Vec::new();

    let matcher_imports = route_matcher_imports(project, route, &outdir);
    imports.extend(matcher_imports.imports);
    declarations.push(format!(
        "type RouteParams = {};",
        params_type(route, &matcher_imports.aliases)
    ));
    declarations.push(format!("type RouteId = {};", json_string(&route.id)));

    if !route.params.is_empty() {
        exports.push(
            "export type EntryGenerator = () => Promise<Array<RouteParams>> | Array<RouteParams>;"
                .to_string(),
        );
    }

    if let Some(page) = &route.page {
        exports.push(page_parent_server_data_type(project, route, &outdir));
        exports.push(page_parent_data_type(project, route, &outdir));
        let page_server_output_shape = if page.universal.is_some() {
            "Partial<App.PageData> & Record<string, any> | void".to_string()
        } else {
            "OutputDataShape<PageServerParentData>".to_string()
        };
        exports.push(format!("export type PageServerLoad<OutputData extends {page_server_output_shape} = {page_server_output_shape}> = Kit.ServerLoad<RouteParams, PageServerParentData, OutputData, RouteId>;"));
        exports
            .push("export type PageServerLoadEvent = Parameters<PageServerLoad>[0];".to_string());
        exports.push("export type PageLoad<OutputData extends OutputDataShape<PageParentData> = OutputDataShape<PageParentData>> = Kit.Load<RouteParams, PageServerData, PageParentData, OutputData, RouteId>;".to_string());
        exports.push("export type PageLoadEvent = Parameters<PageLoad>[0];".to_string());
        let has_page_server_load = page
            .server
            .as_ref()
            .is_some_and(|server| module_export_names(project, server).contains("load"));
        if has_page_server_load {
            let server = page
                .server
                .as_ref()
                .expect("page server module for load types");
            declarations.push(format!(
                "type PageServerModule = typeof import({});",
                json_string(&source_module_specifier(
                    project,
                    &outdir,
                    server,
                    proxy_imports
                ))
            ));
            exports.push(
                "export type PageServerData = Expand<EnsureDefined<ModuleLoadData<PageServerModule>>>;"
                    .to_string(),
            );
        } else {
            exports.push("export type PageServerData = null;".to_string());
        }
        if let Some(universal) = &page.universal {
            declarations.push(format!(
                "type PageModule = typeof import({});",
                json_string(&source_module_specifier(
                    project,
                    &outdir,
                    universal,
                    proxy_imports
                ))
            ));
            exports.push("export type PageData = Expand<Omit<PageParentData, keyof ModuleLoadData<PageModule>> & OptionalUnion<EnsureDefined<ModuleLoadData<PageModule>>>>;".to_string());
        } else if page.server.is_some() {
            exports.push("export type PageData = Expand<Omit<PageParentData, keyof EnsureDefined<PageServerData>> & EnsureDefined<PageServerData>>;".to_string());
        } else {
            exports.push("export type PageData = Expand<PageParentData>;".to_string());
        }
        if page.server.is_some() {
            exports.push("export type Action<OutputData extends Record<string, any> | void = Record<string, any> | void> = Kit.Action<RouteParams, OutputData, RouteId>;".to_string());
            exports.push("export type Actions<OutputData extends Record<string, any> | void = Record<string, any> | void> = Kit.Actions<RouteParams, OutputData, RouteId>;".to_string());
            let has_actions = page
                .server
                .as_ref()
                .is_some_and(|server| module_export_names(project, server).contains("actions"));
            if has_actions {
                let server = page
                    .server
                    .as_ref()
                    .expect("page server module for action types");
                let server_module_specifier = json_string(&source_module_specifier(
                    project,
                    &outdir,
                    server,
                    proxy_imports,
                ));
                declarations.push("type ExcludeActionFailure<T> = T extends Kit.ActionFailure<any> ? never : T extends void ? never : T;".to_string());
                declarations.push("type ActionsSuccess<T extends Record<string, (...args: any) => any>> = { [Key in keyof T]: ExcludeActionFailure<Awaited<ReturnType<T[Key]>>>; }[keyof T];".to_string());
                declarations.push("type ExtractActionFailure<T> = T extends Kit.ActionFailure<infer X> ? X extends void ? never : X : never;".to_string());
                declarations.push("type ActionsFailure<T extends Record<string, (...args: any) => any>> = { [Key in keyof T]: Exclude<ExtractActionFailure<Awaited<ReturnType<T[Key]>>>, void>; }[keyof T];".to_string());
                declarations.push(format!(
                    "type ActionsExport = typeof import({server_module_specifier})['actions'];"
                ));
                exports.push("export type SubmitFunction = ActionsExport extends Record<string, (...args: any) => any> ? Kit.SubmitFunction<Expand<ActionsSuccess<ActionsExport>>, Expand<ActionsFailure<ActionsExport>>> : never;".to_string());
                exports.push("export type ActionData = ActionsExport extends Record<string, (...args: any) => any> ? Expand<Kit.AwaitedActions<ActionsExport>> | null : unknown;".to_string());
            } else {
                exports.push("export type ActionData = unknown;".to_string());
            }
            exports.push(
                "export type PageProps = { params: RouteParams; data: PageData; form: ActionData };"
                    .to_string(),
            );
        } else {
            exports.push("export type ActionData = null;".to_string());
            exports.push(
                "export type PageProps = { params: RouteParams; data: PageData };".to_string(),
            );
        }
    }

    if route.layout.is_some() {
        let layout_matcher_imports = layout_matcher_imports(project, route, &outdir);
        for import in layout_matcher_imports.imports {
            if !imports.contains(&import) {
                imports.push(import);
            }
        }
        let all_child_pages_have_load = all_child_pages_have_load(project, route);
        declarations.push(format!(
            "type LayoutRouteId = {};",
            layout_route_id_union(project, route)
        ));
        declarations.push(format!(
            "type LayoutParams = {};",
            layout_params_type(project, route, &layout_matcher_imports.aliases)
        ));
        exports.push(layout_parent_server_data_type(project, route, &outdir));
        exports.push(layout_parent_data_type(project, route, &outdir));
        let layout_server_output_shape = if route
            .layout
            .as_ref()
            .is_some_and(|layout| layout.universal.is_some())
            || all_child_pages_have_load
        {
            "Partial<App.PageData> & Record<string, any> | void".to_string()
        } else {
            "OutputDataShape<LayoutServerParentData>".to_string()
        };
        exports.push(format!("export type LayoutServerLoad<OutputData extends {layout_server_output_shape} = {layout_server_output_shape}> = Kit.ServerLoad<LayoutParams, LayoutServerParentData, OutputData, LayoutRouteId>;"));
        exports.push(
            "export type LayoutServerLoadEvent = Parameters<LayoutServerLoad>[0];".to_string(),
        );
        let layout_output_shape = if all_child_pages_have_load {
            "Partial<App.PageData> & Record<string, any> | void".to_string()
        } else {
            "OutputDataShape<LayoutParentData>".to_string()
        };
        exports.push(format!("export type LayoutLoad<OutputData extends {layout_output_shape} = {layout_output_shape}> = Kit.Load<LayoutParams, LayoutServerData, LayoutParentData, OutputData, LayoutRouteId>;"));
        exports.push("export type LayoutLoadEvent = Parameters<LayoutLoad>[0];".to_string());
        if let Some(layout) = &route.layout {
            let has_layout_server_load = layout
                .server
                .as_ref()
                .is_some_and(|server| module_export_names(project, server).contains("load"));
            if has_layout_server_load {
                let server = layout
                    .server
                    .as_ref()
                    .expect("layout server module for load types");
                declarations.push(format!(
                    "type LayoutServerModule = typeof import({});",
                    json_string(&source_module_specifier(
                        project,
                        &outdir,
                        server,
                        proxy_imports
                    ))
                ));
                exports.push("export type LayoutServerData = Expand<EnsureDefined<ModuleLoadData<LayoutServerModule>>>;".to_string());
            } else {
                exports.push("export type LayoutServerData = null;".to_string());
            }
            if let Some(universal) = &layout.universal {
                declarations.push(format!(
                    "type LayoutModule = typeof import({});",
                    json_string(&source_module_specifier(
                        project,
                        &outdir,
                        universal,
                        proxy_imports
                    ))
                ));
                exports.push("export type LayoutData = Expand<Omit<LayoutParentData, keyof ModuleLoadData<LayoutModule>> & OptionalUnion<EnsureDefined<ModuleLoadData<LayoutModule>>>>;".to_string());
            } else if layout.server.is_some() {
                exports.push("export type LayoutData = Expand<Omit<LayoutParentData, keyof EnsureDefined<LayoutServerData>> & EnsureDefined<LayoutServerData>>;".to_string());
            } else {
                exports.push("export type LayoutData = Expand<LayoutParentData>;".to_string());
            }
        }
        exports.push("export type LayoutProps = { params: LayoutParams; data: LayoutData; children: import('svelte').Snippet };".to_string());
    }

    if route.endpoint.is_some() {
        exports.push(
            "export type RequestHandler = Kit.RequestHandler<RouteParams, RouteId>;".to_string(),
        );
    }
    if route.endpoint.is_some()
        || route
            .layout
            .as_ref()
            .is_some_and(|layout| layout.server.is_some())
        || route
            .page
            .as_ref()
            .is_some_and(|page| page.server.is_some())
    {
        exports
            .push("export type RequestEvent = Kit.RequestEvent<RouteParams, RouteId>;".to_string());
    }

    [
        imports.join("\n"),
        declarations.join("\n"),
        exports.join("\n"),
    ]
    .into_iter()
    .filter(|section| !section.is_empty())
    .collect::<Vec<_>>()
    .join("\n\n")
}

fn route_module_sources(route: &crate::manifest::DiscoveredRoute) -> Vec<(&Utf8PathBuf, bool)> {
    let mut sources = Vec::new();
    if let Some(page) = &route.page {
        if let Some(server) = &page.server {
            sources.push((server, true));
        }
        if let Some(universal) = &page.universal {
            sources.push((universal, false));
        }
    }
    if let Some(layout) = &route.layout {
        if let Some(server) = &layout.server {
            sources.push((server, true));
        }
        if let Some(universal) = &layout.universal {
            sources.push((universal, false));
        }
    }
    sources
}

fn create_route_module_proxy(
    project: &LoadedKitProject,
    outdir: &Utf8Path,
    path: &Utf8Path,
    _is_server: bool,
) -> Option<(GeneratedTypeFile, String)> {
    let source_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        project.cwd.join(path)
    };
    let source = fs::read_to_string(&source_path).ok()?;
    let rewritten = maybe_rewrite_types_proxy(&source, &source_path)?;
    let file_name = format!("proxy{}", source_path.file_name()?);
    let proxy_path = outdir.join(&file_name);
    let import_specifier = format!("./{}", replace_ext_with_js(&file_name));
    Some((
        GeneratedTypeFile {
            path: proxy_path,
            contents: rewritten,
        },
        import_specifier,
    ))
}

fn maybe_rewrite_types_proxy(source: &str, source_path: &Utf8Path) -> Option<String> {
    if !references_types_module(source) {
        return None;
    }

    let allocator = Allocator::default();
    let source_type = SourceType::from_path(source_path.as_std_path())
        .ok()?
        .with_module(true);
    let parsed = Parser::new(&allocator, source, source_type).parse();
    if !parsed.errors.is_empty() {
        return None;
    }

    let mut edits = Vec::new();
    let mut handled_comment_starts = BTreeSet::new();
    let mut removed_type_names = BTreeSet::new();
    let mut type_imports = Vec::new();
    let load_event_type = load_event_type_name(source_path);

    for statement in &parsed.program.body {
        match statement {
            Statement::ImportDeclaration(declaration) => {
                if import_targets_types_module(declaration) && import_is_type_only(declaration) {
                    type_imports.push(declaration);
                }
            }
            Statement::ExportNamedDeclaration(declaration) => {
                if let Some(Declaration::FunctionDeclaration(function)) =
                    declaration.declaration.as_ref()
                {
                    if function
                        .id
                        .as_ref()
                        .is_some_and(|id| id.name.as_str() == "load")
                        && let Some(comment) = find_leading_types_jsdoc_comment(
                            source,
                            &parsed.program.comments,
                            declaration.span.start,
                        )
                    {
                        handled_comment_starts.insert(comment.span.start);
                        if let Some(param) = first_parameter_doc_name(&function.params.items) {
                            edits.push(replace_range(
                                expand_span_to_line(source, comment.span),
                                format!(
                                    "/** @param {{Parameters<import('./$types').{load_event_type}>[0]}} {param} */\n"
                                ),
                            ));
                        } else {
                            edits.push(replace_range(
                                expand_span_to_line(source, comment.span),
                                String::new(),
                            ));
                        }
                    }
                }
                let Some(Declaration::VariableDeclaration(variable)) =
                    declaration.declaration.as_ref()
                else {
                    continue;
                };
                let leading_comment = find_leading_types_jsdoc_comment(
                    source,
                    &parsed.program.comments,
                    declaration.span.start,
                );
                if let Some(comment) = leading_comment {
                    handled_comment_starts.insert(comment.span.start);
                    let replacement = variable
                        .declarations
                        .first()
                        .and_then(|declarator| binding_name(&declarator.id))
                        .and_then(|name| match (name, declarator_initializer(variable.declarations.first()?)) {
                            ("load", Some(initializer)) => {
                                first_expression_parameter_doc_name(initializer).map(|param| {
                                    format!(
                                        "/** @param {{Parameters<import('./$types').{load_event_type}>[0]}} {param} */\n"
                                    )
                                })
                            }
                            ("actions", Some(_)) => Some("/** */\n".to_string()),
                            _ => None,
                        })
                        .unwrap_or_default();
                    edits.push(replace_range(
                        expand_span_to_line(source, comment.span),
                        replacement,
                    ));
                }
                for declarator in &variable.declarations {
                    let Some(name) = binding_name(&declarator.id) else {
                        continue;
                    };
                    match name {
                        "load" => {
                            if let Some(type_annotation) = declarator.type_annotation.as_ref() {
                                let type_text = normalize_type_annotation_text(
                                    &source[span_range(type_annotation.span)],
                                );
                                if is_identifier_name(&type_text) {
                                    removed_type_names.insert(type_text.clone());
                                }
                                edits.push(replace_range(
                                    span_range(type_annotation.span),
                                    String::new(),
                                ));
                                if let Some(initializer) = declarator.init.as_ref() {
                                    let annotated = annotate_function_parameter(
                                        source,
                                        initializer,
                                        &format!("Parameters<{type_text}>[0]"),
                                        &mut edits,
                                    );
                                    if !annotated {
                                        edits.push(replace_range(
                                            declarator.span.end as usize
                                                ..declarator.span.end as usize,
                                            format!(";null as any as {type_text};"),
                                        ));
                                    }
                                }
                            }
                        }
                        "actions" => {
                            if let Some(type_annotation) = declarator.type_annotation.as_ref() {
                                let type_text = normalize_type_annotation_text(
                                    &source[span_range(type_annotation.span)],
                                );
                                if is_identifier_name(&type_text) {
                                    removed_type_names.insert(type_text);
                                }
                                edits.push(replace_range(
                                    span_range(type_annotation.span),
                                    String::new(),
                                ));
                            }
                            if let Some(Expression::ObjectExpression(object)) =
                                declarator.init.as_ref()
                            {
                                let has_actions_jsdoc =
                                    leading_comment.is_some() && source_type.is_javascript();
                                for property in &object.properties {
                                    let ObjectPropertyKind::ObjectProperty(property) = property
                                    else {
                                        continue;
                                    };
                                    if has_actions_jsdoc {
                                        let property_comment = find_leading_types_jsdoc_comment(
                                            source,
                                            &parsed.program.comments,
                                            property.span.start,
                                        );
                                        if let Some(comment) = property_comment {
                                            handled_comment_starts.insert(comment.span.start);
                                            if let Some(param) =
                                                first_expression_parameter_doc_name(&property.value)
                                            {
                                                edits.push(replace_range(
                                                    expand_span_to_line(source, comment.span),
                                                    format!(
                                                        "/** @param {{Parameters<import('./$types').Action>[0]}} {param} */\n"
                                                    ),
                                                ));
                                            }
                                        } else {
                                            insert_action_param_jsdoc(
                                                &property.value,
                                                "import('./$types').RequestEvent",
                                                &mut edits,
                                            );
                                        }
                                    } else {
                                        annotate_function_parameter(
                                            source,
                                            &property.value,
                                            "import('./$types').RequestEvent",
                                            &mut edits,
                                        );
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    for declaration in type_imports {
        let Some(rewrite) = rewrite_types_import(declaration, &removed_type_names) else {
            continue;
        };
        edits.push(replace_range(
            expand_span_to_line(source, declaration.span),
            rewrite,
        ));
    }

    for comment in &parsed.program.comments {
        if handled_comment_starts.contains(&comment.span.start) {
            continue;
        }
        if comment.content != CommentContent::Jsdoc && comment.content != CommentContent::JsdocLegal
        {
            continue;
        }
        let text = comment.content_span().source_text(source);
        if !references_types_module(text) {
            continue;
        }
        edits.push(replace_range(
            expand_span_to_line(source, comment.span),
            String::new(),
        ));
    }

    if edits.is_empty() {
        return None;
    }

    let mut rewritten = source.to_string();
    edits.sort_by(|left, right| left.range.start.cmp(&right.range.start));
    dedupe_rewrite_edits(&mut edits);
    for edit in edits.into_iter().rev() {
        rewritten.replace_range(edit.range, &edit.replacement);
    }

    let rewritten = if let Some(rest) = rewritten.strip_prefix("// @ts-check") {
        format!("// @ts-check\n// @ts-nocheck{rest}")
    } else {
        format!("// @ts-nocheck\n\n{}", rewritten.trim_start())
    };

    Some(rewritten)
}

fn load_event_type_name(source_path: &Utf8Path) -> &'static str {
    match source_path.file_stem() {
        Some("+layout.server") => "LayoutServerLoad",
        Some("+layout") => "LayoutLoad",
        Some("+page.server") => "PageServerLoad",
        Some("+page") => "PageLoad",
        _ => "PageLoad",
    }
}

#[derive(Debug)]
struct RewriteEdit {
    range: std::ops::Range<usize>,
    replacement: String,
}

fn replace_range(range: std::ops::Range<usize>, replacement: String) -> RewriteEdit {
    RewriteEdit { range, replacement }
}

fn dedupe_rewrite_edits(edits: &mut Vec<RewriteEdit>) {
    let mut deduped = Vec::with_capacity(edits.len());
    for edit in edits.drain(..) {
        if deduped.iter().any(|existing: &RewriteEdit| {
            existing.range == edit.range && existing.replacement == edit.replacement
        }) {
            continue;
        }
        deduped.push(edit);
    }
    *edits = deduped;
}

fn import_targets_types_module(declaration: &oxc_ast::ast::ImportDeclaration<'_>) -> bool {
    is_types_module_specifier(declaration.source.value.as_str())
}

fn import_is_type_only(declaration: &oxc_ast::ast::ImportDeclaration<'_>) -> bool {
    if declaration.import_kind == ImportOrExportKind::Type {
        return true;
    }

    declaration.specifiers.as_ref().is_some_and(|specifiers| {
        !specifiers.is_empty()
            && specifiers.iter().all(|specifier| match specifier {
                ImportDeclarationSpecifier::ImportSpecifier(specifier) => {
                    specifier.import_kind == ImportOrExportKind::Type
                }
                ImportDeclarationSpecifier::ImportDefaultSpecifier(_) => false,
                ImportDeclarationSpecifier::ImportNamespaceSpecifier(_) => false,
            })
    })
}

fn rewrite_types_import(
    declaration: &oxc_ast::ast::ImportDeclaration<'_>,
    removed_type_names: &BTreeSet<String>,
) -> Option<String> {
    if removed_type_names.is_empty() {
        return None;
    }

    let specifiers = declaration.specifiers.as_ref()?;
    let mut surviving = Vec::new();
    let mut removed_any = false;

    for specifier in specifiers {
        match specifier {
            ImportDeclarationSpecifier::ImportSpecifier(specifier) => {
                let local = specifier.local.name.as_str();
                if removed_type_names.contains(local) {
                    removed_any = true;
                    continue;
                }
                surviving.push(local.to_string());
            }
            ImportDeclarationSpecifier::ImportDefaultSpecifier(_) => return None,
            ImportDeclarationSpecifier::ImportNamespaceSpecifier(_) => return None,
        }
    }

    if !removed_any {
        return None;
    }

    if surviving.is_empty() {
        return Some(String::new());
    }

    Some(format!(
        "import type {{ {} }} from '{}';\n",
        surviving.join(", "),
        declaration.source.value.as_str()
    ))
}

fn references_types_module(text: &str) -> bool {
    text.contains("./$types")
        || text.contains("./$types.js")
        || text.contains("/$types")
        || text.contains("/$types.js")
}

fn is_types_module_specifier(specifier: &str) -> bool {
    specifier == "./$types"
        || specifier == "./$types.js"
        || specifier.ends_with("/$types")
        || specifier.ends_with("/$types.js")
}

fn module_export_names(project: &LoadedKitProject, path: &Utf8Path) -> BTreeSet<String> {
    let source_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        project.cwd.join(path)
    };
    let Ok(source) = fs::read_to_string(&source_path) else {
        return BTreeSet::new();
    };
    let Ok(source_type) = SourceType::from_path(source_path.as_std_path()) else {
        return BTreeSet::new();
    };
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, &source, source_type.with_module(true)).parse();
    if !parsed.errors.is_empty() {
        return BTreeSet::new();
    }

    let mut exports = BTreeSet::new();
    for statement in &parsed.program.body {
        match statement {
            Statement::ExportNamedDeclaration(declaration) => {
                if let Some(inner) = declaration.declaration.as_ref() {
                    collect_declaration_export_names(inner, &mut exports);
                }
                for specifier in &declaration.specifiers {
                    if let Some(name) = module_export_name(&specifier.exported) {
                        exports.insert(name);
                    }
                }
            }
            Statement::ExportDefaultDeclaration(_) => {
                exports.insert("default".to_string());
            }
            _ => {}
        }
    }

    exports
}

fn collect_declaration_export_names(declaration: &Declaration<'_>, exports: &mut BTreeSet<String>) {
    match declaration {
        Declaration::VariableDeclaration(declaration) => {
            for declarator in &declaration.declarations {
                if let Some(name) = binding_name(&declarator.id) {
                    exports.insert(name.to_string());
                }
            }
        }
        Declaration::FunctionDeclaration(declaration) => {
            if let Some(id) = declaration.id.as_ref() {
                exports.insert(id.name.as_str().to_string());
            }
        }
        Declaration::ClassDeclaration(declaration) => {
            if let Some(id) = declaration.id.as_ref() {
                exports.insert(id.name.as_str().to_string());
            }
        }
        _ => {}
    }
}

fn module_export_name(name: &ModuleExportName<'_>) -> Option<String> {
    match name {
        ModuleExportName::IdentifierName(name) => Some(name.name.as_str().to_string()),
        ModuleExportName::IdentifierReference(name) => Some(name.name.as_str().to_string()),
        ModuleExportName::StringLiteral(name) => Some(name.value.as_str().to_string()),
    }
}

fn find_leading_types_jsdoc_comment<'a>(
    source: &str,
    comments: &'a [Comment],
    target_start: u32,
) -> Option<&'a Comment> {
    comments.iter().rev().find(|comment| {
        (comment.content == CommentContent::Jsdoc || comment.content == CommentContent::JsdocLegal)
            && references_types_module(comment.content_span().source_text(source))
            && comment.span.end <= target_start
            && source[comment.span.end as usize..target_start as usize]
                .trim()
                .is_empty()
    })
}

fn binding_name<'a>(pattern: &'a BindingPattern<'a>) -> Option<&'a str> {
    match pattern {
        BindingPattern::BindingIdentifier(identifier) => Some(identifier.name.as_str()),
        BindingPattern::AssignmentPattern(pattern) => binding_name(&pattern.left),
        _ => None,
    }
}

fn span_range(span: Span) -> std::ops::Range<usize> {
    span.start as usize..span.end as usize
}

fn declarator_initializer<'a>(
    declarator: &'a oxc_ast::ast::VariableDeclarator<'a>,
) -> Option<&'a Expression<'a>> {
    declarator.init.as_ref()
}

fn first_parameter_doc_name(params: &[FormalParameter<'_>]) -> Option<String> {
    params
        .first()
        .map(|param| binding_name(&param.pattern).unwrap_or("event").to_string())
}

fn first_expression_parameter_doc_name(expression: &Expression<'_>) -> Option<String> {
    match expression {
        Expression::ArrowFunctionExpression(function) => {
            first_parameter_doc_name(&function.params.items)
        }
        Expression::FunctionExpression(function) => {
            first_parameter_doc_name(&function.params.items)
        }
        _ => None,
    }
}

fn annotate_function_parameter(
    source: &str,
    expression: &Expression<'_>,
    type_text: &str,
    edits: &mut Vec<RewriteEdit>,
) -> bool {
    match expression {
        Expression::ArrowFunctionExpression(function) => {
            annotate_first_param(source, &function.params.items, true, type_text, edits)
        }
        Expression::FunctionExpression(function) => {
            annotate_first_param(source, &function.params.items, false, type_text, edits)
        }
        _ => false,
    }
}

fn insert_action_param_jsdoc(
    expression: &Expression<'_>,
    type_text: &str,
    edits: &mut Vec<RewriteEdit>,
) -> bool {
    let Some(param_name) = first_expression_parameter_doc_name(expression) else {
        return false;
    };

    let start = match expression {
        Expression::ArrowFunctionExpression(function) => function.span.start as usize,
        Expression::FunctionExpression(function) => function.span.start as usize,
        _ => return false,
    };

    edits.push(replace_range(
        start..start,
        format!("/** @param {{{type_text}}} {param_name} */ "),
    ));
    true
}

fn annotate_first_param(
    source: &str,
    params: &[FormalParameter<'_>],
    maybe_wrap_arrow_param: bool,
    type_text: &str,
    edits: &mut Vec<RewriteEdit>,
) -> bool {
    let Some(param) = params.first() else {
        return false;
    };
    if param.type_annotation.is_some() {
        return false;
    }
    let Some(name) = binding_name(&param.pattern) else {
        return false;
    };

    let pattern_end = binding_pattern_end(&param.pattern);
    if maybe_wrap_arrow_param {
        let start = binding_pattern_start(&param.pattern);
        let needs_parens = source[..start]
            .chars()
            .next_back()
            .is_some_and(|ch| ch != '(');
        if needs_parens {
            edits.push(replace_range(start..start, "(".to_string()));
            edits.push(replace_range(
                pattern_end..pattern_end,
                format!(": {type_text})"),
            ));
            return true;
        }
    }

    let _ = name;
    edits.push(replace_range(
        pattern_end..pattern_end,
        format!(": {type_text}"),
    ));
    true
}

fn binding_pattern_end(pattern: &BindingPattern<'_>) -> usize {
    match pattern {
        BindingPattern::BindingIdentifier(identifier) => identifier.span.end as usize,
        BindingPattern::AssignmentPattern(pattern) => binding_pattern_end(&pattern.left),
        _ => pattern.span().end as usize,
    }
}

fn binding_pattern_start(pattern: &BindingPattern<'_>) -> usize {
    match pattern {
        BindingPattern::BindingIdentifier(identifier) => identifier.span.start as usize,
        BindingPattern::AssignmentPattern(pattern) => binding_pattern_start(&pattern.left),
        _ => pattern.span().start as usize,
    }
}

fn normalize_type_annotation_text(text: &str) -> String {
    text.trim().trim_start_matches(':').trim().to_string()
}

fn is_identifier_name(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first == '$' || first.is_ascii_alphabetic()) {
        return false;
    }
    chars.all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric())
}

fn expand_span_to_line(source: &str, span: Span) -> std::ops::Range<usize> {
    let mut range = span_range(span);
    let line_start = source[..range.start]
        .rfind('\n')
        .map_or(0, |index| index + 1);
    if source[line_start..range.start].trim().is_empty() {
        range.start = line_start;
    }

    let line_end = source[range.end..]
        .find('\n')
        .map_or(source.len(), |index| range.end + index + 1);
    if source[range.end..line_end].trim().is_empty() {
        range.end = line_end;
    }

    range
}

struct MatcherImports {
    imports: Vec<String>,
    aliases: BTreeMap<String, String>,
}

fn route_matcher_imports(
    project: &LoadedKitProject,
    route: &crate::manifest::DiscoveredRoute,
    outdir: &Utf8Path,
) -> MatcherImports {
    matcher_imports_for_params(project, route.params.iter(), outdir)
}

fn layout_matcher_imports(
    project: &LoadedKitProject,
    route: &crate::manifest::DiscoveredRoute,
    outdir: &Utf8Path,
) -> MatcherImports {
    let prefix = if route.id == "/" {
        "/".to_string()
    } else {
        format!("{}/", route.id)
    };

    let params = project
        .manifest
        .routes
        .iter()
        .filter(|child| child.id == route.id || child.id.starts_with(&prefix))
        .flat_map(|child| child.params.iter());

    matcher_imports_for_params(project, params, outdir)
}

fn matcher_imports_for_params<'a>(
    project: &LoadedKitProject,
    params: impl Iterator<Item = &'a crate::routing::RouteParam>,
    outdir: &Utf8Path,
) -> MatcherImports {
    let mut imports = Vec::new();
    let mut aliases = BTreeMap::new();
    let mut seen = BTreeSet::new();

    for param in params {
        let Some(matcher) = &param.matcher else {
            continue;
        };
        let Some(path) = project.manifest.matchers.get(matcher) else {
            continue;
        };
        let alias = format!("matcher_{matcher}");
        aliases.insert(matcher.clone(), alias.clone());
        if !seen.insert(matcher.clone()) {
            continue;
        }
        imports.push(format!(
            "import {{ match as {alias} }} from {};",
            json_string(&relative_path(outdir, &project.cwd.join(path)))
        ));
    }

    MatcherImports { imports, aliases }
}

fn params_type(
    route: &crate::manifest::DiscoveredRoute,
    matcher_aliases: &BTreeMap<String, String>,
) -> String {
    if route.params.is_empty() {
        return "{}".to_string();
    }

    let fields = route
        .params
        .iter()
        .map(|param| {
            let value_type = param
                .matcher
                .as_ref()
                .and_then(|matcher| matcher_aliases.get(matcher))
                .map(|alias| format!("MatcherParam<typeof {alias}>"))
                .unwrap_or_else(|| "string".to_string());
            format!(
                "{}{} {}",
                param.name,
                if param.optional { "?:" } else { ":" },
                value_type
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    format!("{{ {fields} }}")
}

fn layout_route_id_union(
    project: &LoadedKitProject,
    route: &crate::manifest::DiscoveredRoute,
) -> String {
    let mut ids = BTreeSet::new();
    ids.insert(json_string(&route.id));
    let prefix = if route.id == "/" {
        "/".to_string()
    } else {
        format!("{}/", route.id)
    };

    for child in &project.manifest.routes {
        if child.id == route.id || child.id.starts_with(&prefix) {
            if child.page.is_some() {
                ids.insert(json_string(&child.id));
            }
        }
    }
    if route.id == "/" {
        ids.insert("null".to_string());
    }

    ids.into_iter().collect::<Vec<_>>().join(" | ")
}

fn layout_params_type(
    project: &LoadedKitProject,
    route: &crate::manifest::DiscoveredRoute,
    matcher_aliases: &BTreeMap<String, String>,
) -> String {
    let mut params = BTreeMap::new();
    for param in &route.params {
        params.insert(param.name.clone(), (param.optional, param.matcher.clone()));
    }

    let Some(layout) = route.layout.as_ref() else {
        return "RouteParams & {}".to_string();
    };

    for child in &project.manifest.routes {
        let Some(page) = child.page.as_ref() else {
            continue;
        };
        if !page_uses_layout(page, layout) {
            continue;
        }
        for param in &child.params {
            params
                .entry(param.name.clone())
                .or_insert((true, param.matcher.clone()));
        }
    }

    if params.is_empty() {
        return "RouteParams & {}".to_string();
    }

    let fields = params
        .into_iter()
        .map(|(name, (optional, matcher))| {
            let value_type = matcher
                .as_ref()
                .and_then(|matcher| matcher_aliases.get(matcher))
                .map(|alias| format!("MatcherParam<typeof {alias}>"))
                .unwrap_or_else(|| "string".to_string());
            format!("{name}{} {value_type}", if optional { "?:" } else { ":" })
        })
        .collect::<Vec<_>>()
        .join("; ");
    format!("RouteParams & {{ {fields} }}")
}

fn page_uses_layout(
    page: &crate::manifest::PageFiles,
    layout: &crate::manifest::NodeFiles,
) -> bool {
    page.layouts
        .iter()
        .flatten()
        .any(|candidate| candidate == layout)
}

fn page_parent_server_data_type(
    project: &LoadedKitProject,
    route: &crate::manifest::DiscoveredRoute,
    outdir: &Utf8Path,
) -> String {
    format!(
        "export type PageServerParentData = {};",
        parent_layout_type_chain(project, route, outdir, "LayoutServerData", true)
    )
}

fn page_parent_data_type(
    project: &LoadedKitProject,
    route: &crate::manifest::DiscoveredRoute,
    outdir: &Utf8Path,
) -> String {
    format!(
        "export type PageParentData = {};",
        parent_layout_type_chain(project, route, outdir, "LayoutData", true)
    )
}

fn layout_parent_server_data_type(
    project: &LoadedKitProject,
    route: &crate::manifest::DiscoveredRoute,
    outdir: &Utf8Path,
) -> String {
    format!(
        "export type LayoutServerParentData = {};",
        parent_layout_type_chain(project, route, outdir, "LayoutServerData", false)
    )
}

fn layout_parent_data_type(
    project: &LoadedKitProject,
    route: &crate::manifest::DiscoveredRoute,
    outdir: &Utf8Path,
) -> String {
    format!(
        "export type LayoutParentData = {};",
        parent_layout_type_chain(project, route, outdir, "LayoutData", false)
    )
}

fn parent_layout_type_chain(
    project: &LoadedKitProject,
    route: &crate::manifest::DiscoveredRoute,
    outdir: &Utf8Path,
    type_name: &str,
    include_self: bool,
) -> String {
    let imports = ancestor_layout_imports(project, &route.id, outdir, type_name, include_self);
    let mut merged = format!(
        "EnsureDefined<{}>",
        imports.first().cloned().unwrap_or_else(|| "{}".to_string())
    );
    for import in imports.into_iter().skip(1) {
        merged = format!("Omit<{merged}, keyof {import}> & EnsureDefined<{import}>");
    }
    merged
}

fn ancestor_layout_imports(
    project: &LoadedKitProject,
    route_id: &str,
    outdir: &Utf8Path,
    type_name: &str,
    include_self: bool,
) -> Vec<String> {
    let mut current = if include_self {
        Some(route_id.to_string())
    } else {
        parent_route_id(route_id)
    };
    let mut route_ids = Vec::new();

    while let Some(candidate) = current {
        if project
            .manifest
            .routes
            .iter()
            .any(|route| route.id == candidate && route.layout.is_some())
        {
            route_ids.push(candidate.clone());
        }
        current = parent_route_id(&candidate);
    }

    route_ids.reverse();
    route_ids
        .into_iter()
        .map(|route_id| {
            format!(
                "import({}).{type_name}",
                json_string(&route_types_import_specifier(project, outdir, &route_id))
            )
        })
        .collect()
}

fn parent_route_id(route_id: &str) -> Option<String> {
    if route_id == "/" {
        return None;
    }

    let trimmed = route_id.trim_end_matches('/');
    let index = trimmed.rfind('/')?;
    if index == 0 {
        Some("/".to_string())
    } else {
        Some(trimmed[..index].to_string())
    }
}

fn all_child_pages_have_load(
    project: &LoadedKitProject,
    layout_route: &crate::manifest::DiscoveredRoute,
) -> bool {
    let prefix = if layout_route.id == "/" {
        "/".to_string()
    } else {
        format!("{}/", layout_route.id)
    };

    let mut saw_page = false;

    for route in &project.manifest.routes {
        let Some(page) = route.page.as_ref() else {
            continue;
        };
        if route.id != layout_route.id && !route.id.starts_with(&prefix) {
            continue;
        }
        saw_page = true;

        let has_server_load = page
            .server
            .as_ref()
            .is_some_and(|path| module_export_names(project, path).contains("load"));
        let has_universal_load = page
            .universal
            .as_ref()
            .is_some_and(|path| module_export_names(project, path).contains("load"));

        if !has_server_load && !has_universal_load {
            return false;
        }
    }

    saw_page
}

fn route_types_import_specifier(
    project: &LoadedKitProject,
    outdir: &Utf8Path,
    route_id: &str,
) -> String {
    let file = route_type_path(&route_types_root(project), route_id);
    let relative = relative_path(outdir, &file);
    relative
        .strip_suffix(".d.ts")
        .unwrap_or(relative.as_str())
        .to_string()
}

fn generate_node_module(nodes_dir: &Utf8Path, node: &ManifestNode) -> String {
    let mut lines = Vec::new();

    if let Some(universal) = &node.universal {
        lines.push(format!(
            "import * as universal from {};",
            json_string(&relative_path(nodes_dir, universal))
        ));
        lines.push("export { universal };".to_string());
    }

    if let Some(component) = &node.component {
        lines.push(format!(
            "export {{ default as component }} from {};",
            json_string(&relative_path(nodes_dir, component))
        ));
    }

    lines.join("\n")
}

fn generate_app_module(
    kit: &ValidatedKitConfig,
    manifest: &KitManifest,
    output: &Utf8Path,
    client_routing: bool,
    server_loads: &[usize],
) -> Result<String> {
    let client_hooks = resolve_entry(&kit.files.hooks.client, &kit.module_extensions);
    let universal_hooks = resolve_entry(&kit.files.hooks.universal, &kit.module_extensions);
    let nodes_count = if client_routing {
        manifest.nodes.len()
    } else {
        manifest.nodes.len().min(2)
    };

    let mut imports = Vec::new();
    if let Some(client_hooks) = &client_hooks {
        imports.push(format!(
            "import * as client_hooks from {};",
            json_string(&relative_path(output, client_hooks))
        ));
    }
    if let Some(universal_hooks) = &universal_hooks {
        imports.push(format!(
            "import * as universal_hooks from {};",
            json_string(&relative_path(output, universal_hooks))
        ));
    }

    let nodes = (0..nodes_count)
        .map(|index| format!("() => import('./nodes/{index}')"))
        .collect::<Vec<_>>()
        .join(",\n\t");

    let dictionary = if client_routing {
        generate_dictionary(manifest)
    } else {
        "{}".to_string()
    };

    let matchers_export = if client_routing {
        "export { matchers } from './matchers.js';".to_string()
    } else {
        "export const matchers = {};".to_string()
    };

    let client_handle_error = if client_hooks.is_some() {
        "client_hooks.handleError || "
    } else {
        ""
    };
    let client_init = if client_hooks.is_some() {
        "init: client_hooks.init,\n\t"
    } else {
        ""
    };
    let universal_reroute = if universal_hooks.is_some() {
        "universal_hooks.reroute || "
    } else {
        ""
    };
    let universal_transport = if universal_hooks.is_some() {
        "universal_hooks.transport || "
    } else {
        ""
    };

    let mut module = String::new();
    if !imports.is_empty() {
        module.push_str(&imports.join("\n"));
        module.push_str("\n\n");
    }
    module.push_str(&matchers_export);
    module.push_str("\n\nexport const nodes = [\n\t");
    module.push_str(&nodes);
    module.push_str("\n];\n\n");
    module.push_str("export const server_loads = [");
    module.push_str(
        &server_loads
            .iter()
            .map(|index| index.to_string())
            .collect::<Vec<_>>()
            .join(","),
    );
    module.push_str("];\n\n");
    module.push_str("export const dictionary = ");
    module.push_str(&dictionary);
    module.push_str(";\n\n");
    module.push_str("export const hooks = {\n\t");
    module.push_str("handleError: ");
    module.push_str(client_handle_error);
    module.push_str("(({ error }) => { console.error(error) }),\n\t");
    module.push_str(client_init);
    module.push_str("reroute: ");
    module.push_str(universal_reroute);
    module.push_str("(() => {}),\n\ttransport: ");
    module.push_str(universal_transport);
    module.push_str("{}\n};\n\n");
    module.push_str(
        "export const decoders = Object.fromEntries(Object.entries(hooks.transport).map(([k, v]) => [k, v.decode]));\n",
    );
    module.push_str(
        "export const encoders = Object.fromEntries(Object.entries(hooks.transport).map(([k, v]) => [k, v.encode]));\n\n",
    );
    module.push_str(&format!(
        "export const hash = {};\n\n",
        json_string(&(kit.router.type_ == crate::config::RouterType::Hash))
    ));
    module.push_str("export const decode = (type, value) => decoders[type](value);\n\n");
    module.push_str("export { default as root } from '../root.js';");

    Ok(module)
}

fn generate_dictionary(manifest: &KitManifest) -> String {
    let mut entries = Vec::new();

    for route in &manifest.manifest_routes {
        let Some(page) = &route.page else {
            continue;
        };

        let mut layouts = page
            .layouts
            .iter()
            .skip(1)
            .map(option_index)
            .collect::<Vec<_>>();
        let mut errors = page
            .errors
            .iter()
            .skip(1)
            .map(option_index)
            .collect::<Vec<_>>();

        while layouts.last().is_some_and(|entry| entry.is_empty()) {
            layouts.pop();
        }
        while errors.last().is_some_and(|entry| entry.is_empty()) {
            errors.pop();
        }

        let leaf_has_server = manifest
            .nodes
            .get(page.leaf)
            .and_then(|node| node.server.as_ref())
            .is_some();
        let mut array = vec![if leaf_has_server {
            format!("~{}", page.leaf)
        } else {
            page.leaf.to_string()
        }];

        if !layouts.is_empty() || !errors.is_empty() {
            array.push(format!("[{}]", layouts.join(",")));
        }
        if !errors.is_empty() {
            array.push(format!("[{}]", errors.join(",")));
        }

        entries.push(format!("{}: [{}]", json_string(&route.id), array.join(",")));
    }

    format!("{{\n\t{}\n}}", entries.join(",\n\t"))
}

fn generate_matchers_module(manifest: &KitManifest, output: &Utf8Path) -> String {
    let mut imports = Vec::new();
    let mut names = Vec::new();

    for (key, src) in &manifest.matchers {
        imports.push(format!(
            "import {{ match as {key} }} from {};",
            json_string(&relative_path(output, src))
        ));
        names.push(key.clone());
    }

    if imports.is_empty() {
        return "export const matchers = {};".to_string();
    }

    format!(
        "{}\n\nexport const matchers = {{ {} }};",
        imports.join("\n"),
        names.join(", ")
    )
}

fn collect_layouts_with_server_load(manifest: &KitManifest) -> Vec<usize> {
    let mut indexes = BTreeSet::new();

    for route in &manifest.manifest_routes {
        let Some(page) = &route.page else {
            continue;
        };
        for layout in &page.layouts {
            let Some(index) = layout else {
                continue;
            };
            if manifest
                .nodes
                .get(*index)
                .and_then(|node| node.server.as_ref())
                .is_some()
            {
                indexes.insert(*index);
            }
        }
    }

    indexes.into_iter().collect()
}

fn resolve_entry(entry: &Utf8Path, module_extensions: &[String]) -> Option<Utf8PathBuf> {
    if entry.is_file() {
        return Some(entry.to_path_buf());
    }

    if entry.is_dir() {
        let index = entry.join("index");
        if let Some(found) = resolve_entry(&index, module_extensions) {
            return Some(found);
        }
    }

    let parent = entry.parent()?;
    if !parent.is_dir() {
        return None;
    }

    let base = entry.file_name()?;
    for candidate in std::fs::read_dir(parent).ok()? {
        let candidate = candidate.ok()?;
        let path = Utf8PathBuf::from_path_buf(candidate.path()).ok()?;
        if !path.is_file() {
            continue;
        }
        let file_name = path.file_name()?;
        if !module_extensions
            .iter()
            .any(|ext| file_name.ends_with(ext.as_str()))
        {
            continue;
        }
        let stem = &file_name[..file_name.rfind('.').unwrap_or(file_name.len())];
        if stem == base {
            return Some(path);
        }
    }

    None
}

fn option_index(value: &Option<usize>) -> String {
    value.map(|value| value.to_string()).unwrap_or_default()
}

fn write_if_changed(
    path: &Utf8Path,
    contents: &str,
    written_files: &mut Vec<Utf8PathBuf>,
) -> Result<()> {
    if path.is_file() && fs::read_to_string(path).is_ok_and(|existing| existing == contents) {
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)?;
    written_files.push(path.to_path_buf());
    Ok(())
}

fn relative_path(from: &Utf8Path, to: &Utf8Path) -> String {
    let relative = pathdiff::diff_paths(to, from)
        .and_then(|path| Utf8PathBuf::from_path_buf(path).ok())
        .unwrap_or_else(|| to.to_path_buf());
    let relative = relative.as_str().replace('\\', "/");
    if relative.starts_with('.') {
        relative
    } else {
        format!("./{relative}")
    }
}

fn generate_tsconfig_paths(cwd: &Utf8Path, kit: &ValidatedKitConfig) -> Map<String, Value> {
    let config_relative = |file: &Utf8Path| relative_path(&kit.out_dir, file);
    let mut aliases = kit.alias.clone();
    if kit.files.lib.exists() {
        aliases.insert("$lib".to_string(), kit.files.lib.as_str().to_string());
    }
    let explicit_globs = aliases.keys().cloned().collect::<BTreeSet<_>>();

    let mut paths = Map::new();
    for (key, value) in aliases {
        let relative = config_relative(&cwd.join(remove_trailing_slashstar(&value)));
        if key.ends_with("/*") {
            paths.insert(key, json!([format!("{relative}/*")]));
            continue;
        }

        let has_explicit_glob = explicit_globs.contains(&format!("{key}/*"));
        let has_extension = Utf8Path::new(remove_trailing_slashstar(&value))
            .extension()
            .is_some();

        paths.insert(key.clone(), json!([relative.clone()]));
        if !has_extension && !has_explicit_glob {
            paths.insert(format!("{key}/*"), json!([format!("{relative}/*")]));
        }
    }

    paths
}

fn remove_trailing_slashstar(value: &str) -> &str {
    value.strip_suffix("/*").unwrap_or(value)
}

fn validate_user_tsconfig(cwd: &Utf8Path, generated_tsconfig: &Utf8Path) -> Option<Vec<String>> {
    let user_config = load_user_tsconfig(cwd)?;
    let mut warnings = Vec::new();

    let extends_framework_config =
        extends_generated_tsconfig(cwd, generated_tsconfig, user_config.options.get("extends"));
    let compiler_options = user_config
        .options
        .get("compilerOptions")
        .and_then(Value::as_object);

    if extends_framework_config {
        let has_paths = compiler_options.is_some_and(|options| {
            options.contains_key("paths") || options.contains_key("baseUrl")
        });
        if has_paths {
            warnings.push(format!(
                "You have specified a baseUrl and/or paths in your {} which interferes with SvelteKit's auto-generated tsconfig.json. Remove it to avoid problems with intellisense. For path aliases, use `kit.alias` instead: https://svelte.dev/docs/kit/configuration#alias",
                user_config.kind
            ));
        }
    } else {
        warnings.push(format!(
            "Your {} should extend the configuration generated by SvelteKit:\n{{\n  \"extends\": \"{}\"\n}}",
            user_config.kind,
            generated_config_reference(cwd, generated_tsconfig)
        ));
    }

    Some(warnings)
}

struct UserTsConfig {
    kind: String,
    options: Value,
}

fn load_user_tsconfig(cwd: &Utf8Path) -> Option<UserTsConfig> {
    for kind in ["tsconfig.json", "jsconfig.json"] {
        let path = cwd.join(kind);
        if !path.is_file() {
            continue;
        }

        let source = fs::read_to_string(&path).ok()?;
        let options = json5::from_str::<Value>(&source).ok()?;
        return Some(UserTsConfig {
            kind: kind.to_string(),
            options,
        });
    }

    None
}

fn extends_generated_tsconfig(
    cwd: &Utf8Path,
    generated_tsconfig: &Utf8Path,
    extends: Option<&Value>,
) -> bool {
    match extends {
        Some(Value::String(path)) => resolve_tsconfig_reference(cwd, path) == generated_tsconfig,
        Some(Value::Array(values)) => values.iter().any(|value| {
            value
                .as_str()
                .is_some_and(|path| resolve_tsconfig_reference(cwd, path) == generated_tsconfig)
        }),
        _ => false,
    }
}

fn resolve_tsconfig_reference(cwd: &Utf8Path, reference: &str) -> Utf8PathBuf {
    let path = Utf8Path::new(reference);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

fn generated_config_reference(cwd: &Utf8Path, generated_tsconfig: &Utf8Path) -> String {
    let relative = pathdiff::diff_paths(generated_tsconfig, cwd)
        .and_then(|path| Utf8PathBuf::from_path_buf(path).ok())
        .unwrap_or_else(|| generated_tsconfig.to_path_buf());
    let relative = relative.as_str().replace('\\', "/");
    if relative.starts_with("./") {
        relative
    } else {
        format!("./{relative}")
    }
}

fn strip_relative_parent_traversals(path: &str) -> String {
    path.replace("../", "").replace("..\\", "")
}

fn source_module_specifier(
    project: &LoadedKitProject,
    outdir: &Utf8Path,
    path: &Utf8Path,
    proxy_imports: &BTreeMap<String, String>,
) -> String {
    if let Some(proxy) = proxy_imports.get(path.as_str()) {
        return proxy.clone();
    }
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        project.cwd.join(path)
    };
    relative_path(outdir, &resolved)
}

fn replace_ext_with_js(path: &str) -> String {
    Utf8Path::new(path)
        .with_extension("js")
        .as_str()
        .to_string()
}

fn json_string<T: serde::Serialize + ?Sized>(value: &T) -> String {
    serde_json::to_string(value).expect("json serialization")
}

fn generate_root_svelte(levels: &[usize], max_depth: usize) -> String {
    let mut pyramid = format!(
        "<!-- svelte-ignore binding_property_non_reactive -->\n<Pyramid_{max_depth} bind:this={{components[{max_depth}]}} data={{data_{max_depth}}} {{form}} params={{page.params}} />"
    );

    for level in (0..max_depth).rev() {
        pyramid = format!(
            "{{#if constructors[{next}]}}\n\t{{@const Pyramid_{level} = constructors[{level}]}}\n\t<!-- svelte-ignore binding_property_non_reactive -->\n\t<Pyramid_{level} bind:this={{components[{level}]}} data={{data_{level}}} {{form}} params={{page.params}}>\n\t\t{pyramid}\n\t</Pyramid_{level}>\n{{:else}}\n\t{{@const Pyramid_{level} = constructors[{level}]}}\n\t<!-- svelte-ignore binding_property_non_reactive -->\n\t<Pyramid_{level} bind:this={{components[{level}]}} data={{data_{level}}} {{form}} params={{page.params}} />\n{{/if}}",
            next = level + 1,
            pyramid = indent_block(&pyramid, 2)
        );
    }

    let props = levels
        .iter()
        .map(|level| format!("data_{level} = null"))
        .collect::<Vec<_>>()
        .join(", ");
    let effect_refs = levels
        .iter()
        .map(|level| format!("data_{level}"))
        .collect::<Vec<_>>()
        .join(";");

    format!(
        "<!-- This file is generated by @sveltejs/kit — do not edit it! -->\n<svelte:options runes={{true}} />\n<script>\n\timport {{ setContext, onMount, tick }} from 'svelte';\n\timport {{ browser }} from '$app/environment';\n\n\t// stores\n\tlet {{ stores, page, constructors, components = [], form, {props} }} = $props();\n\n\tif (!browser) {{\n\t\t// svelte-ignore state_referenced_locally\n\t\tsetContext('__svelte__', stores);\n\t}}\n\n\tif (browser) {{\n\t\t$effect.pre(() => stores.page.set(page));\n\t}} else {{\n\t\t// svelte-ignore state_referenced_locally\n\t\tstores.page.set(page);\n\t}}\n\n\t$effect(() => {{\n\t\tstores;page;constructors;components;form;{effect_refs};\n\t\tstores.page.notify();\n\t}});\n\n\tlet mounted = $state(false);\n\tlet navigated = $state(false);\n\tlet title = $state(null);\n\n\tonMount(() => {{\n\t\tconst unsubscribe = stores.page.subscribe(() => {{\n\t\t\tif (mounted) {{\n\t\t\t\tnavigated = true;\n\t\t\t\ttick().then(() => {{\n\t\t\t\t\ttitle = document.title || 'untitled page';\n\t\t\t\t}});\n\t\t\t}}\n\t\t}});\n\n\t\tmounted = true;\n\t\treturn unsubscribe;\n\t}});\n\n\tconst Pyramid_{max_depth} = $derived(constructors[{max_depth}])\n</script>\n\n{pyramid}\n\n{{#if mounted}}\n\t<div id=\"svelte-announcer\" aria-live=\"assertive\" aria-atomic=\"true\" style=\"position: absolute; left: 0; top: 0; clip: rect(0 0 0 0); clip-path: inset(50%); overflow: hidden; white-space: nowrap; width: 1px; height: 1px\">\n\t\t{{#if navigated}}\n\t\t\t{{title}}\n\t\t{{/if}}\n\t</div>\n{{/if}}"
    )
}

fn indent_block(value: &str, levels: usize) -> String {
    let prefix = "\t".repeat(levels);
    value
        .lines()
        .map(|line| format!("{prefix}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn generate_app_types(manifest: &KitManifest) -> String {
    let mut pathnames = BTreeSet::new();
    let mut dynamic_routes = Vec::new();
    let mut layouts = Vec::new();
    let routable_routes = manifest
        .manifest_routes
        .iter()
        .filter(|route| route.page.is_some() || route.endpoint.is_some())
        .collect::<Vec<_>>();

    for route in &routable_routes {
        if !route.params.is_empty() {
            let params = route
                .params
                .iter()
                .map(|param| {
                    format!(
                        "{}{} string",
                        param.name,
                        if param.optional { "?:" } else { ":" }
                    )
                })
                .collect::<Vec<_>>();
            dynamic_routes.push(format!(
                "{}: {{ {} }}",
                json_string(&route.id),
                params.join("; ")
            ));

            let pathname = remove_group_segments(&route.id);
            let replaced = replace_required_params(&replace_optional_params(&pathname));
            for pathname in pathnames_for_trailing_slash(&replaced, manifest, &route.id) {
                pathnames.insert(format!("`{pathname}` & {{}}"));
            }
        } else {
            let pathname = remove_group_segments(&route.id);
            for pathname in pathnames_for_trailing_slash(&pathname, manifest, &route.id) {
                pathnames.insert(json_string(&pathname));
            }
        }

        let mut child_params = route
            .params
            .iter()
            .map(|param| (param.name.clone(), param.optional))
            .collect::<Vec<_>>();
        for child in routable_routes
            .iter()
            .filter(|child| child.id.starts_with(&route.id))
        {
            for param in &child.params {
                if child_params.iter().all(|(name, _)| name != &param.name) {
                    child_params.push((param.name.clone(), true));
                }
            }
        }

        let layout_params = child_params
            .iter()
            .map(|(name, optional)| format!("{name}{} string", if *optional { "?:" } else { ":" }))
            .collect::<Vec<_>>()
            .join("; ");
        let layout_type = if layout_params.is_empty() {
            "Record<String, never>".replace("String", "string")
        } else {
            format!("{{ {layout_params} }}")
        };
        layouts.push(format!("{}: {}", json_string(&route.id), layout_type));
    }

    let assets = manifest
        .assets
        .iter()
        .map(|asset| json_string(&format!("/{}", asset.file)))
        .collect::<Vec<_>>();
    let route_ids = manifest
        .manifest_routes
        .iter()
        .filter(|route| route.page.is_some() || route.endpoint.is_some())
        .map(|route| json_string(&route.id))
        .collect::<Vec<_>>()
        .join(" | ");
    let route_params = if dynamic_routes.is_empty() {
        "Record<string, never>".to_string()
    } else {
        format!("{{\n\t\t\t{}\n\t\t}}", dynamic_routes.join(";\n\t\t\t"))
    };
    let layout_params = if layouts.is_empty() {
        "Record<string, never>".to_string()
    } else {
        format!("{{\n\t\t\t{}\n\t\t}}", layouts.join(";\n\t\t\t"))
    };
    let pathnames = if pathnames.is_empty() {
        "never".to_string()
    } else {
        pathnames.into_iter().collect::<Vec<_>>().join(" | ")
    };
    let assets = assets
        .into_iter()
        .chain(std::iter::once("string & {}".to_string()))
        .collect::<Vec<_>>()
        .join(" | ");

    format!(
        "declare module \"$app/types\" {{\n\texport interface AppTypes {{\n\t\tRouteId(): {route_ids};\n\t\tRouteParams(): {route_params};\n\t\tLayoutParams(): {layout_params};\n\t\tPathname(): {pathnames};\n\t\tResolvedPathname(): `${{\"\" | `/${{string}}`}}${{ReturnType<AppTypes['Pathname']>}}`;\n\t\tAsset(): {assets};\n\t}}\n}}"
    )
}

fn remove_group_segments(id: &str) -> String {
    let segments = id
        .trim_start_matches('/')
        .split('/')
        .filter(|segment| {
            !segment.is_empty() && !(segment.starts_with('(') && segment.ends_with(')'))
        })
        .collect::<Vec<_>>();
    if segments.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", segments.join("/"))
    }
}

fn replace_optional_params(id: &str) -> String {
    Regex::new(r"/\[\[[^\]]+\]\]")
        .expect("valid optional param regex")
        .replace_all(id, |_: &regex::Captures<'_>| "${string}")
        .into_owned()
}

fn replace_required_params(id: &str) -> String {
    Regex::new(r"/\[[^\]]+\]")
        .expect("valid required param regex")
        .replace_all(id, |_: &regex::Captures<'_>| "/${string}")
        .into_owned()
}

fn pathnames_for_trailing_slash(
    pathname: &str,
    manifest: &KitManifest,
    route_id: &str,
) -> Vec<String> {
    if pathname == "/" {
        return vec![pathname.to_string()];
    }

    let Some(route) = manifest
        .manifest_routes
        .iter()
        .find(|candidate| candidate.id == route_id)
    else {
        return vec![pathname.to_string()];
    };

    let mut pathnames = BTreeSet::new();

    if let Some(page) = &route.page {
        match RuntimePageNodes::from_route(page, manifest)
            .trailing_slash()
            .as_str()
        {
            "ignore" => {
                pathnames.insert(pathname.to_string());
                pathnames.insert(format!("{pathname}/"));
            }
            "always" => {
                pathnames.insert(format!("{pathname}/"));
            }
            _ => {
                pathnames.insert(pathname.to_string());
            }
        }
    }

    if let Some(endpoint) = &route.endpoint {
        match endpoint
            .page_options
            .as_ref()
            .and_then(|options| options.get("trailingSlash"))
            .and_then(Value::as_str)
        {
            Some("ignore") => {
                pathnames.insert(pathname.to_string());
                pathnames.insert(format!("{pathname}/"));
            }
            Some("always") => {
                pathnames.insert(format!("{pathname}/"));
            }
            Some(_) => {
                pathnames.insert(pathname.to_string());
            }
            None if route.page.is_none() => {
                pathnames.insert(pathname.to_string());
                pathnames.insert(format!("{pathname}/"));
            }
            None => {}
        }
    }

    pathnames.into_iter().collect()
}

fn collect_env(dir: &str, mode: &str) -> Map<String, Value> {
    let mut env = Map::new();

    for key in [
        ".env",
        ".env.local",
        &format!(".env.{mode}"),
        &format!(".env.{mode}.local"),
    ] {
        let path = Utf8Path::new(dir).join(key);
        if !path.is_file() {
            continue;
        }
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (key, value) in parse_dotenv(&contents) {
            env.insert(key, Value::String(value));
        }
    }

    for (key, value) in std::env::vars() {
        env.insert(key, Value::String(value));
    }

    env
}

fn parse_dotenv(contents: &str) -> Vec<(String, String)> {
    contents
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }

            let line = line.strip_prefix("export ").unwrap_or(line);
            let (key, value) = line.split_once('=')?;
            Some((key.trim().to_string(), parse_dotenv_value(value.trim())))
        })
        .collect()
}

fn parse_dotenv_value(value: &str) -> String {
    if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        value[1..value.len() - 1].to_string()
    } else {
        value.to_string()
    }
}

fn filter_env(env: &Map<String, Value>, allowed: &str, disallowed: &str) -> Map<String, Value> {
    env.iter()
        .filter(|(key, _)| {
            key.starts_with(allowed) && (disallowed.is_empty() || !key.starts_with(disallowed))
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn doc_comment(content: &str) -> String {
    let lines = content
        .trim()
        .lines()
        .map(|line| format!(" * {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("/**\n{lines}\n */")
}

fn create_static_types(module_id: &str, env: &Map<String, Value>) -> String {
    let declarations = env
        .keys()
        .filter(|key| is_valid_identifier(key) && !is_reserved_identifier(key))
        .map(|key| format!("export const {key}: string;"))
        .collect::<Vec<_>>();

    if declarations.is_empty() {
        format!("declare module '{module_id}' {{\n}}")
    } else {
        format!(
            "declare module '{module_id}' {{\n\t{}\n}}",
            declarations.join("\n\t")
        )
    }
}

fn create_dynamic_types(
    module_id: &str,
    env: &Map<String, Value>,
    public_prefix: &str,
    private_prefix: &str,
    is_private: bool,
) -> String {
    let mut properties = env
        .keys()
        .filter(|key| is_valid_identifier(key))
        .map(|key| format!("{key}: string;"))
        .collect::<Vec<_>>();

    let public_prefixed = format!("[key: `{public_prefix}${{string}}`]");
    let private_prefixed = format!("[key: `{private_prefix}${{string}}`]");

    if is_private {
        if !public_prefix.is_empty() {
            properties.push(format!("{public_prefixed}: undefined;"));
        }
        properties.push(format!("{private_prefixed}: string | undefined;"));
    } else {
        if !private_prefix.is_empty() {
            properties.push(format!("{private_prefixed}: undefined;"));
        }
        properties.push(format!("{public_prefixed}: string | undefined;"));
    }

    format!(
        "declare module '{module_id}' {{\n\texport const env: {{\n\t\t{}\n\t}}\n}}",
        properties.join("\n\t\t")
    )
}

fn is_valid_identifier(key: &str) -> bool {
    Regex::new(r"^[a-zA-Z_$][a-zA-Z0-9_$]*$")
        .expect("valid identifier regex")
        .is_match(key)
}

fn is_reserved_identifier(key: &str) -> bool {
    matches!(
        key,
        "do" | "if"
            | "in"
            | "for"
            | "let"
            | "new"
            | "try"
            | "var"
            | "case"
            | "else"
            | "enum"
            | "eval"
            | "null"
            | "this"
            | "true"
            | "void"
            | "with"
            | "await"
            | "break"
            | "catch"
            | "class"
            | "const"
            | "false"
            | "super"
            | "throw"
            | "while"
            | "yield"
            | "delete"
            | "export"
            | "import"
            | "public"
            | "return"
            | "static"
            | "switch"
            | "typeof"
            | "default"
            | "extends"
            | "finally"
            | "package"
            | "private"
            | "continue"
            | "debugger"
            | "function"
            | "arguments"
            | "interface"
            | "protected"
            | "implements"
            | "instanceof"
    )
}

fn json_bool(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

fn csp_json(csp: &ValidatedCspConfig) -> Value {
    json!({
        "mode": match csp.mode {
            CspMode::Auto => "auto",
            CspMode::Hash => "hash",
            CspMode::Nonce => "nonce",
        },
        "directives": csp_directives_json(&csp.directives),
        "reportOnly": csp_directives_json(&csp.report_only),
    })
}

fn csp_directives_json(directives: &ValidatedCspDirectives) -> Value {
    let mut object = Map::new();
    for (key, value) in &directives.string_lists {
        object.insert(key.clone(), json!(value));
    }
    object.insert(
        "upgrade-insecure-requests".to_string(),
        Value::Bool(directives.upgrade_insecure_requests),
    );
    object.insert(
        "block-all-mixed-content".to_string(),
        Value::Bool(directives.block_all_mixed_content),
    );
    Value::Object(object)
}

fn preload_strategy_name(strategy: &PreloadStrategy) -> &'static str {
    match strategy {
        PreloadStrategy::ModulePreload => "modulepreload",
        PreloadStrategy::PreloadJs => "preload-js",
        PreloadStrategy::PreloadMjs => "preload-mjs",
    }
}

fn js_template_renderer(template: &str, version_name: &str, include_env: bool) -> String {
    let rendered = json_string(&template)
        .replace("%sveltekit.head%", "\" + head + \"")
        .replace("%sveltekit.body%", "\" + body + \"")
        .replace("%sveltekit.assets%", "\" + assets + \"")
        .replace("%sveltekit.nonce%", "\" + nonce + \"")
        .replace("%sveltekit.version%", &escape_html(version_name));

    if !include_env {
        return rendered;
    }

    Regex::new(r"%sveltekit\.env\.([^%]+)%")
        .expect("valid env regex")
        .replace_all(&rendered, |captures: &regex::Captures<'_>| {
            format!("\" + (env[{}] ?? \"\") + \"", json_string(&captures[1]))
        })
        .into_owned()
}

fn js_error_renderer(template: &str) -> String {
    json_string(&template)
        .replace("%sveltekit.status%", "\" + status + \"")
        .replace("%sveltekit.error.message%", "\" + message + \"")
}

fn escape_html(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn djb2_hash(value: &str) -> String {
    let mut hash: u32 = 5381;
    for byte in value.as_bytes().iter().rev() {
        hash = hash.wrapping_mul(33) ^ u32::from(*byte);
    }
    base36(hash)
}

fn base36(mut value: u32) -> String {
    if value == 0 {
        return "0".to_string();
    }

    let mut digits = Vec::new();
    while value > 0 {
        let digit = (value % 36) as u8;
        digits.push(if digit < 10 {
            (b'0' + digit) as char
        } else {
            (b'a' + (digit - 10)) as char
        });
        value /= 36;
    }
    digits.into_iter().rev().collect()
}
