use std::fs;

use camino::{Utf8Path, Utf8PathBuf};

use crate::constants::SVELTE_KIT_ASSETS;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewPaths {
    pub base: String,
    pub assets: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrerenderedMatch {
    pub file: Utf8PathBuf,
    pub content_type_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrerenderedResolution {
    File(PrerenderedMatch),
    Redirect(String),
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewPlan {
    pub protocol: String,
    pub base: String,
    pub assets: String,
    pub server_dir: Utf8PathBuf,
    pub client_dir: Utf8PathBuf,
    pub prerendered_dependencies_dir: Utf8PathBuf,
    pub prerendered_pages_dir: Utf8PathBuf,
    pub expected_server_files: Vec<Utf8PathBuf>,
}

pub fn preview_protocol(https_enabled: bool) -> &'static str {
    if https_enabled { "https" } else { "http" }
}

pub fn preview_paths(base: &str, assets: &str) -> PreviewPaths {
    PreviewPaths {
        base: base.to_string(),
        assets: if assets.is_empty() {
            base.to_string()
        } else {
            SVELTE_KIT_ASSETS.to_string()
        },
    }
}

pub fn preview_root_redirect(base: &str, pathname: &str, search: &str) -> Option<String> {
    if base.len() > 1 && pathname == base {
        let mut location = format!("{base}/");
        if !search.is_empty() {
            location.push_str(search);
        }
        Some(location)
    } else {
        None
    }
}

pub fn resolve_prerendered_request(
    out_dir: &Utf8Path,
    app_dir: &str,
    pathname: &str,
    search: &str,
) -> PrerenderedResolution {
    let category = if pathname.starts_with(&format!("/{app_dir}/remote/")) {
        "data"
    } else {
        "pages"
    };

    let filename = out_dir
        .join("output")
        .join("prerendered")
        .join(category)
        .join(pathname.trim_start_matches('/'));

    if is_file(&filename) {
        return PrerenderedResolution::File(PrerenderedMatch {
            file: filename,
            content_type_path: pathname.to_string(),
        });
    }

    let has_trailing_slash = pathname.ends_with('/');
    let html_filename = if has_trailing_slash {
        filename.join("index.html")
    } else {
        Utf8PathBuf::from(format!("{filename}.html"))
    };

    if is_file(&html_filename) {
        return PrerenderedResolution::File(PrerenderedMatch {
            file: html_filename,
            content_type_path: pathname.to_string(),
        });
    }

    let redirect = if has_trailing_slash {
        let target = Utf8PathBuf::from(format!("{}.html", filename.as_str().trim_end_matches('/')));
        if is_file(&target) {
            Some(pathname.trim_end_matches('/').to_string())
        } else {
            None
        }
    } else {
        let target = filename.join("index.html");
        if is_file(&target) {
            Some(format!("{pathname}/"))
        } else {
            None
        }
    };

    if let Some(mut redirect) = redirect {
        if !search.is_empty() {
            redirect.push_str(search);
        }
        return PrerenderedResolution::Redirect(redirect);
    }

    PrerenderedResolution::Missing
}

fn is_file(path: &Utf8Path) -> bool {
    fs::metadata(path)
        .map(|metadata| metadata.is_file())
        .unwrap_or(false)
}

pub fn build_preview_plan(
    out_dir: &Utf8Path,
    base: &str,
    assets: &str,
    https_enabled: bool,
) -> PreviewPlan {
    let mapped = preview_paths(base, assets);
    let server_dir = out_dir.join("output").join("server");
    PreviewPlan {
        protocol: preview_protocol(https_enabled).to_string(),
        base: mapped.base,
        assets: mapped.assets,
        client_dir: out_dir.join("output").join("client"),
        prerendered_dependencies_dir: out_dir
            .join("output")
            .join("prerendered")
            .join("dependencies"),
        prerendered_pages_dir: out_dir.join("output").join("prerendered").join("pages"),
        expected_server_files: vec![server_dir.join("internal.js"), server_dir.join("index.js")],
        server_dir,
    }
}
