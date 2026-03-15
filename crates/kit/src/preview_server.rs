use camino::{Utf8Path, Utf8PathBuf};

use crate::{SVELTE_KIT_ASSETS, ValidatedKitConfig, preview_protocol};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewServerPlan {
    pub protocol: String,
    pub base: String,
    pub assets: String,
    pub output_server_dir: Utf8PathBuf,
    pub output_client_dir: Utf8PathBuf,
    pub prerendered_dependencies_dir: Utf8PathBuf,
    pub prerendered_pages_dir: Utf8PathBuf,
    pub env_dir: Utf8PathBuf,
    pub mode: String,
    pub etag: String,
    pub immutable_cache_control: String,
    pub base_middleware_name: Option<String>,
}

pub fn build_preview_server_plan(
    cwd: &Utf8Path,
    kit: &ValidatedKitConfig,
    https_enabled: bool,
    mode: &str,
    etag: &str,
) -> PreviewServerPlan {
    let out_dir = relative_path(cwd, &kit.out_dir);
    PreviewServerPlan {
        protocol: preview_protocol(https_enabled).to_string(),
        base: kit.paths.base.clone(),
        assets: if kit.paths.assets.is_empty() {
            kit.paths.base.clone()
        } else {
            SVELTE_KIT_ASSETS.to_string()
        },
        output_server_dir: out_dir.join("output/server"),
        output_client_dir: out_dir.join("output/client"),
        prerendered_dependencies_dir: out_dir.join("output/prerendered/dependencies"),
        prerendered_pages_dir: out_dir.join("output/prerendered/pages"),
        env_dir: relative_path(cwd, Utf8Path::new(&kit.env.dir)),
        mode: mode.to_string(),
        etag: etag.to_string(),
        immutable_cache_control: "public,max-age=31536000,immutable".to_string(),
        base_middleware_name: Some("viteBaseMiddleware".to_string()),
    }
}

fn relative_path(cwd: &Utf8Path, path: &Utf8Path) -> Utf8PathBuf {
    path.strip_prefix(cwd)
        .map(Utf8Path::to_path_buf)
        .unwrap_or_else(|_| path.to_path_buf())
}
