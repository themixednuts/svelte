use camino::Utf8Path;

use crate::{ValidatedKitConfig, posixify, resolve_entry};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DevMiddlewareStep {
    ServeAssets {
        scope: String,
        allow_origin: String,
    },
    RemoveViteStaticMiddlewares {
        names: Vec<String>,
    },
    ServeServiceWorker {
        route: String,
        entry: Option<String>,
    },
    SsrRequestHandler,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViteDevServerPlan {
    pub debounce_ms: u64,
    pub manifest_watch_roots: Vec<String>,
    pub manifest_update_regex: String,
    pub static_middleware_names: Vec<String>,
    pub service_worker_route: String,
    pub service_worker_entry: Option<String>,
    pub assets_mount_path: String,
    pub restart_on_config_change: bool,
    pub full_reload_on_non_index_app_template_change: bool,
    pub server_sync_roots: Vec<String>,
    pub steps: Vec<DevMiddlewareStep>,
}

pub fn build_vite_dev_server_plan(
    kit: &ValidatedKitConfig,
    server_https: bool,
    server_fs_strict: bool,
) -> ViteDevServerPlan {
    let manifest_watch_roots = vec![
        posixify(kit.files.routes.as_str()),
        posixify(kit.files.params.as_str()),
        posixify(kit.files.hooks.client.as_str()),
    ];
    let static_middleware_names = vec![
        "viteServeStaticMiddleware".to_string(),
        "viteServePublicMiddleware".to_string(),
    ];
    let service_worker_entry = resolve_entry(&kit.files.service_worker)
        .ok()
        .flatten()
        .map(|path| relative_display(kit, &path));
    let assets_mount_path = if kit.paths.assets.is_empty() {
        kit.paths.base.clone()
    } else {
        "/_svelte_kit_assets".to_string()
    };
    let service_worker_route = format!("{}/service-worker.js", kit.paths.base);

    let mut server_sync_roots = vec![
        posixify(kit.files.app_template.as_str()),
        posixify(kit.files.error_template.as_str()),
        posixify(kit.files.hooks.server.as_str()),
    ];
    server_sync_roots.push(posixify(kit.files.service_worker.as_str()));

    let steps = vec![
        DevMiddlewareStep::ServeAssets {
            scope: assets_mount_path.clone(),
            allow_origin: "*".to_string(),
        },
        DevMiddlewareStep::RemoveViteStaticMiddlewares {
            names: static_middleware_names.clone(),
        },
        DevMiddlewareStep::ServeServiceWorker {
            route: service_worker_route.clone(),
            entry: service_worker_entry.clone(),
        },
        DevMiddlewareStep::SsrRequestHandler,
    ];

    let _ = (server_https, server_fs_strict);

    ViteDevServerPlan {
        debounce_ms: 100,
        manifest_watch_roots,
        manifest_update_regex: r"\+(page|layout|server).*$".to_string(),
        static_middleware_names,
        service_worker_route,
        service_worker_entry,
        assets_mount_path,
        restart_on_config_change: true,
        full_reload_on_non_index_app_template_change: kit.files.app_template.as_str()
            != "index.html",
        server_sync_roots,
        steps,
    }
}

fn relative_display(kit: &ValidatedKitConfig, path: &Utf8Path) -> String {
    let root = kit
        .files
        .src
        .parent()
        .expect("src should have a parent directory");
    let relative = path.strip_prefix(root).unwrap_or(path);
    posixify(relative.as_str())
}
