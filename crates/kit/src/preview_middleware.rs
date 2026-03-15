use camino::Utf8PathBuf;

use crate::{PreviewServerPlan, ValidatedKitConfig};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewMiddlewareStep {
    RemoveBaseMiddleware {
        name: String,
    },
    ServeClientAssets {
        scope: String,
        dir: Utf8PathBuf,
        immutable_prefix: String,
        immutable_cache_control: String,
    },
    GuardBasePath {
        base: String,
    },
    ServePrerenderedDependencies {
        scope: String,
        dir: Utf8PathBuf,
        etag: String,
    },
    ServePrerenderedPages {
        scope: String,
        dir: Utf8PathBuf,
        app_dir: String,
        etag: String,
    },
    SsrRequestHandler,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewMiddlewarePlan {
    pub steps: Vec<PreviewMiddlewareStep>,
}

pub fn build_preview_middleware_plan(
    kit: &ValidatedKitConfig,
    server_plan: &PreviewServerPlan,
) -> PreviewMiddlewarePlan {
    PreviewMiddlewarePlan {
        steps: vec![
            PreviewMiddlewareStep::RemoveBaseMiddleware {
                name: server_plan
                    .base_middleware_name
                    .clone()
                    .unwrap_or_else(|| "viteBaseMiddleware".to_string()),
            },
            PreviewMiddlewareStep::ServeClientAssets {
                scope: server_plan.assets.clone(),
                dir: server_plan.output_client_dir.clone(),
                immutable_prefix: format!("/{}/immutable", kit.app_dir),
                immutable_cache_control: server_plan.immutable_cache_control.clone(),
            },
            PreviewMiddlewareStep::GuardBasePath {
                base: server_plan.base.clone(),
            },
            PreviewMiddlewareStep::ServePrerenderedDependencies {
                scope: server_plan.base.clone(),
                dir: server_plan.prerendered_dependencies_dir.clone(),
                etag: server_plan.etag.clone(),
            },
            PreviewMiddlewareStep::ServePrerenderedPages {
                scope: server_plan.base.clone(),
                dir: server_plan.prerendered_pages_dir.clone(),
                app_dir: kit.app_dir.clone(),
                etag: server_plan.etag.clone(),
            },
            PreviewMiddlewareStep::SsrRequestHandler,
        ],
    }
}
