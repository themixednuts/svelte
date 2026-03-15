use std::collections::BTreeMap;

use camino::{Utf8Path, Utf8PathBuf};

use crate::{
    Result, ValidatedKitConfig, ViteAlias, ViteAliasFind, get_config_aliases, posixify,
    resolve_entry,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViteOptimizeRemoteFunctionsPlan {
    Rolldown {
        filter_pattern: String,
        contents: String,
    },
    Esbuild {
        filter_pattern: String,
        contents: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViteSetupPlan {
    pub root: String,
    pub resolve_aliases: Vec<ViteAlias>,
    pub server_allow: Vec<String>,
    pub watch_ignored: Vec<String>,
    pub optimize_entries: Vec<String>,
    pub optimize_exclude: Vec<String>,
    pub ssr_no_external: Vec<String>,
    pub define: BTreeMap<String, String>,
    pub build_ssr: bool,
    pub external_in_dev: Vec<String>,
    pub remote_functions_optimizer: Option<ViteOptimizeRemoteFunctionsPlan>,
}

pub fn build_vite_setup_plan(
    cwd: &Utf8Path,
    kit: &ValidatedKitConfig,
    runtime_directory: &str,
    _mode: &str,
    is_build: bool,
    is_rolldown: bool,
    secondary_build_started: bool,
    has_server_load: Option<bool>,
    has_universal_load: Option<bool>,
) -> Result<ViteSetupPlan> {
    let generated = relative_path(cwd, &kit.out_dir).join("generated");

    let mut resolve_aliases = vec![
        ViteAlias {
            find: ViteAliasFind::Literal("__SERVER__".to_string()),
            replacement: generated.join("server").as_str().to_string(),
        },
        ViteAlias {
            find: ViteAliasFind::Literal("$app".to_string()),
            replacement: format!("{runtime_directory}/app"),
        },
    ];
    resolve_aliases.extend(get_config_aliases(kit)?);

    let mut server_allow = vec![
        relative_path(cwd, &kit.files.lib).as_str().to_string(),
        relative_path(cwd, &kit.files.routes).as_str().to_string(),
        relative_path(cwd, &kit.out_dir).as_str().to_string(),
        "src".to_string(),
        "node_modules".to_string(),
    ];
    if let Some(client_hooks) = resolve_entry(&kit.files.hooks.client)? {
        if let Some(parent) = client_hooks.parent() {
            server_allow.push(relative_path(cwd, parent).as_str().to_string());
        }
    }
    server_allow.sort();
    server_allow.dedup();

    let watch_ignored = vec![format!(
        "{}/!(generated)",
        posixify(relative_path(cwd, &kit.out_dir).as_str())
    )];
    let routes_dir = posixify(relative_path(cwd, &kit.files.routes).as_str());
    let optimize_entries = vec![
        format!("{routes_dir}/**/+*.{{svelte,js,ts}}"),
        format!("!{routes_dir}/**/+*server.*"),
    ];
    let optimize_exclude = vec![
        "@sveltejs/kit".to_string(),
        "$app".to_string(),
        "$env".to_string(),
    ];
    let ssr_no_external = vec![
        "esm-env".to_string(),
        "@sveltejs/kit/src/runtime".to_string(),
    ];

    let mut define = BTreeMap::from([
        (
            "__SVELTEKIT_APP_DIR__".to_string(),
            json_string(&kit.app_dir),
        ),
        (
            "__SVELTEKIT_EMBEDDED__".to_string(),
            bool_string(kit.embedded),
        ),
        (
            "__SVELTEKIT_EXPERIMENTAL__REMOTE_FUNCTIONS__".to_string(),
            bool_string(kit.experimental.remote_functions),
        ),
        (
            "__SVELTEKIT_FORK_PRELOADS__".to_string(),
            bool_string(kit.experimental.fork_preloads),
        ),
        (
            "__SVELTEKIT_PATHS_ASSETS__".to_string(),
            json_string(&kit.paths.assets),
        ),
        (
            "__SVELTEKIT_PATHS_BASE__".to_string(),
            json_string(&kit.paths.base),
        ),
        (
            "__SVELTEKIT_PATHS_RELATIVE__".to_string(),
            bool_string(kit.paths.relative),
        ),
        (
            "__SVELTEKIT_CLIENT_ROUTING__".to_string(),
            bool_string(matches!(
                kit.router.resolution,
                crate::RouterResolution::Client
            )),
        ),
        (
            "__SVELTEKIT_HASH_ROUTING__".to_string(),
            bool_string(matches!(kit.router.type_, crate::RouterType::Hash)),
        ),
        (
            "__SVELTEKIT_SERVER_TRACING_ENABLED__".to_string(),
            bool_string(kit.experimental.tracing.server),
        ),
    ]);

    let mut external_in_dev = Vec::new();
    let build_ssr = is_build && !secondary_build_started;
    if is_build {
        define.insert(
            "__SVELTEKIT_APP_VERSION_POLL_INTERVAL__".to_string(),
            kit.version.poll_interval.to_string(),
        );
        define.insert(
            "__SVELTEKIT_PAYLOAD__".to_string(),
            if build_ssr {
                "{}".to_string()
            } else {
                "globalThis.__sveltekit_payload".to_string()
            },
        );
        define.insert(
            "__SVELTEKIT_HAS_SERVER_LOAD__".to_string(),
            bool_string(if secondary_build_started {
                has_server_load.unwrap_or(true)
            } else {
                true
            }),
        );
        define.insert(
            "__SVELTEKIT_HAS_UNIVERSAL_LOAD__".to_string(),
            bool_string(if secondary_build_started {
                has_universal_load.unwrap_or(true)
            } else {
                true
            }),
        );
    } else {
        define.insert(
            "__SVELTEKIT_APP_VERSION_POLL_INTERVAL__".to_string(),
            "0".to_string(),
        );
        define.insert(
            "__SVELTEKIT_PAYLOAD__".to_string(),
            "globalThis.__sveltekit_dev".to_string(),
        );
        define.insert(
            "__SVELTEKIT_HAS_SERVER_LOAD__".to_string(),
            "true".to_string(),
        );
        define.insert(
            "__SVELTEKIT_HAS_UNIVERSAL_LOAD__".to_string(),
            "true".to_string(),
        );
        external_in_dev = vec!["cookie".to_string(), "set-cookie-parser".to_string()];
    }

    let remote_functions_optimizer = if kit.experimental.remote_functions {
        let filter_pattern = format!(
            ".remote({})$",
            kit.module_extensions
                .iter()
                .map(|ext| regex::escape(ext))
                .collect::<Vec<_>>()
                .join("|")
        );
        let contents = String::new();
        Some(if is_rolldown {
            ViteOptimizeRemoteFunctionsPlan::Rolldown {
                filter_pattern,
                contents,
            }
        } else {
            ViteOptimizeRemoteFunctionsPlan::Esbuild {
                filter_pattern,
                contents,
            }
        })
    } else {
        None
    };

    Ok(ViteSetupPlan {
        root: ".".to_string(),
        resolve_aliases,
        server_allow,
        watch_ignored,
        optimize_entries,
        optimize_exclude,
        ssr_no_external,
        define,
        build_ssr,
        external_in_dev,
        remote_functions_optimizer,
    })
}

fn relative_path(cwd: &Utf8Path, path: &Utf8Path) -> Utf8PathBuf {
    path.strip_prefix(cwd)
        .map(Utf8Path::to_path_buf)
        .unwrap_or_else(|_| path.to_path_buf())
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).expect("json string")
}

fn bool_string(value: bool) -> String {
    if value { "true" } else { "false" }.to_string()
}
