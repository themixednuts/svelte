use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::{PeerImportError, Result};

pub fn resolve_peer_dependency(cwd: &Path, dependency: &str) -> Result<PathBuf> {
    let (name, subpackage) = split_dependency(dependency);

    let mut dir = cwd.to_path_buf();
    loop {
        let package_dir = dir.join("node_modules").join(&name);
        let package_json = package_dir.join("package.json");
        if package_json.exists() {
            let package_json_path = package_json.display().to_string();
            let package: Value =
                serde_json::from_str(&std::fs::read_to_string(&package_json).map_err(|error| {
                    PeerImportError::ReadPackageJson {
                        path: package_json_path.clone(),
                        message: error.to_string(),
                    }
                })?)
                .map_err(|error| PeerImportError::ParsePackageJson {
                    path: package_json_path.clone(),
                    message: error.to_string(),
                })?;

            let exports = package
                .get("exports")
                .ok_or_else(|| PeerImportError::MissingExport {
                    package_name: name.clone(),
                    subpackage: subpackage.clone(),
                })?;
            let exported = resolve_export_target(exports, &subpackage).ok_or_else(|| {
                PeerImportError::MissingExport {
                    package_name: name.clone(),
                    subpackage: subpackage.clone(),
                }
            })?;
            return Ok(package_dir.join(exported));
        }

        if !dir.pop() {
            break;
        }
    }

    Err(PeerImportError::UnresolvedDependency { package_name: name }.into())
}

fn split_dependency(dependency: &str) -> (String, String) {
    let mut parts = dependency.split('/').collect::<Vec<_>>();
    let mut name = parts.remove(0).to_string();
    if name.starts_with('@') && !parts.is_empty() {
        name.push('/');
        name.push_str(parts.remove(0));
    }

    let subpackage = if parts.is_empty() {
        ".".to_string()
    } else {
        format!("./{}", parts.join("/"))
    };
    (name, subpackage)
}

fn resolve_export_target(exports: &Value, subpackage: &str) -> Option<String> {
    let mut current = exports.get(subpackage)?.clone();
    loop {
        match current {
            Value::String(value) => return Some(value),
            Value::Object(object) => {
                current = object
                    .get("import")
                    .or_else(|| object.get("default"))
                    .cloned()?;
            }
            _ => return None,
        }
    }
}
