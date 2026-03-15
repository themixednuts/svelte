use std::collections::BTreeSet;

use camino::Utf8Path;

use crate::error::{ExportValidationError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteModuleKind {
    LayoutUniversal,
    PageUniversal,
    LayoutServer,
    PageServer,
    Endpoint,
}

fn validate_module_exports_without_path(
    exports: &BTreeSet<String>,
    kind: RouteModuleKind,
) -> Result<()> {
    let allowed = match kind {
        RouteModuleKind::LayoutUniversal => VALID_LAYOUT_EXPORTS,
        RouteModuleKind::PageUniversal => VALID_PAGE_EXPORTS,
        RouteModuleKind::LayoutServer => VALID_LAYOUT_SERVER_EXPORTS,
        RouteModuleKind::PageServer => VALID_PAGE_SERVER_EXPORTS,
        RouteModuleKind::Endpoint => VALID_SERVER_EXPORTS,
    };

    for key in exports {
        if key.starts_with('_') || allowed.contains(&key.as_str()) {
            continue;
        }

        let hint = hint_for_supported_files(key, ".js").unwrap_or_else(|| {
            format!(
                "valid exports are {}, or anything with a '_' prefix",
                allowed.join(", ")
            )
        });

        return Err(ExportValidationError::InvalidExport {
            key: key.clone(),
            hint,
        }
        .into());
    }

    Ok(())
}

pub fn validate_module_exports(
    exports: &BTreeSet<String>,
    kind: RouteModuleKind,
    path: &Utf8Path,
) -> Result<()> {
    let allowed = match kind {
        RouteModuleKind::LayoutUniversal => VALID_LAYOUT_EXPORTS,
        RouteModuleKind::PageUniversal => VALID_PAGE_EXPORTS,
        RouteModuleKind::LayoutServer => VALID_LAYOUT_SERVER_EXPORTS,
        RouteModuleKind::PageServer => VALID_PAGE_SERVER_EXPORTS,
        RouteModuleKind::Endpoint => VALID_SERVER_EXPORTS,
    };

    for key in exports {
        if key.starts_with('_') || allowed.contains(&key.as_str()) {
            continue;
        }

        let ext = path
            .extension()
            .map(|extension| format!(".{extension}"))
            .unwrap_or_else(|| ".js".to_string());
        let hint = hint_for_supported_files(key, &ext).unwrap_or_else(|| {
            format!(
                "valid exports are {}, or anything with a '_' prefix",
                allowed.join(", ")
            )
        });

        return Err(ExportValidationError::InvalidExportInPath {
            key: key.clone(),
            path: path.to_string(),
            hint,
        }
        .into());
    }

    Ok(())
}

pub fn validate_layout_exports(exports: &BTreeSet<String>) -> Result<()> {
    validate_module_exports_without_path(exports, RouteModuleKind::LayoutUniversal)
}

pub fn validate_page_exports(exports: &BTreeSet<String>) -> Result<()> {
    validate_module_exports_without_path(exports, RouteModuleKind::PageUniversal)
}

pub fn validate_layout_server_exports(exports: &BTreeSet<String>) -> Result<()> {
    validate_module_exports_without_path(exports, RouteModuleKind::LayoutServer)
}

pub fn validate_page_server_exports(exports: &BTreeSet<String>) -> Result<()> {
    validate_module_exports_without_path(exports, RouteModuleKind::PageServer)
}

pub fn validate_server_exports(exports: &BTreeSet<String>) -> Result<()> {
    validate_module_exports_without_path(exports, RouteModuleKind::Endpoint)
}

fn hint_for_supported_files(key: &str, ext: &str) -> Option<String> {
    let mut supported_files = Vec::new();

    if VALID_LAYOUT_EXPORTS.contains(&key) {
        supported_files.push(format!("+layout{ext}"));
    }
    if VALID_PAGE_EXPORTS.contains(&key) {
        supported_files.push(format!("+page{ext}"));
    }
    if VALID_LAYOUT_SERVER_EXPORTS.contains(&key) {
        supported_files.push(format!("+layout.server{ext}"));
    }
    if VALID_PAGE_SERVER_EXPORTS.contains(&key) {
        supported_files.push(format!("+page.server{ext}"));
    }
    if VALID_SERVER_EXPORTS.contains(&key) {
        supported_files.push(format!("+server{ext}"));
    }

    if supported_files.is_empty() {
        None
    } else if supported_files.len() == 1 {
        Some(format!(
            "'{key}' is a valid export in {}",
            supported_files[0]
        ))
    } else {
        Some(format!(
            "'{key}' is a valid export in {} or {}",
            supported_files[..supported_files.len() - 1].join(", "),
            supported_files.last().expect("at least one supported file")
        ))
    }
}

const VALID_LAYOUT_EXPORTS: &[&str] =
    &["load", "prerender", "csr", "ssr", "trailingSlash", "config"];
const VALID_PAGE_EXPORTS: &[&str] = &[
    "load",
    "prerender",
    "csr",
    "ssr",
    "trailingSlash",
    "config",
    "entries",
];
const VALID_LAYOUT_SERVER_EXPORTS: &[&str] =
    &["load", "prerender", "csr", "ssr", "trailingSlash", "config"];
const VALID_PAGE_SERVER_EXPORTS: &[&str] = &[
    "load",
    "prerender",
    "csr",
    "ssr",
    "trailingSlash",
    "config",
    "actions",
    "entries",
];
const VALID_SERVER_EXPORTS: &[&str] = &[
    "GET",
    "POST",
    "PATCH",
    "PUT",
    "DELETE",
    "OPTIONS",
    "HEAD",
    "fallback",
    "prerender",
    "trailingSlash",
    "config",
    "entries",
];
