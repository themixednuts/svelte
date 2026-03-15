use std::env;

use crate::config::ValidatedKitConfig;
use crate::{
    Error, ServerResponse, ViteUtilsError, env_dynamic_private_module_id,
    env_dynamic_public_module_id, env_static_private_module_id, env_static_public_module_id,
    escape_html_with_mode, negotiate, posixify, service_worker_module_id,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViteAlias {
    pub find: ViteAliasFind,
    pub replacement: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViteAliasFind {
    Literal(String),
    Pattern(String),
}

pub fn get_config_aliases(config: &ValidatedKitConfig) -> Result<Vec<ViteAlias>, Error> {
    let mut aliases = vec![ViteAlias {
        find: ViteAliasFind::Literal("$lib".to_string()),
        replacement: normalize_relative_path(&posixify(&config.files.lib)),
    }];

    let cwd = env::current_dir()
        .map_err(Error::from)?
        .to_string_lossy()
        .replace('\\', "/");

    for (key, value) in &config.alias {
        let mut normalized = posixify(value);
        if normalized.ends_with("/*") {
            normalized.truncate(normalized.len() - 2);
        }

        if let Some(prefix) = key.strip_suffix("/*") {
            aliases.push(ViteAlias {
                find: ViteAliasFind::Pattern(format!("/^{}\\/(.+)$/", escape_for_regexp(prefix))),
                replacement: format!("{cwd}/{normalized}/$1").replace("//", "/"),
            });
            continue;
        }

        if config.alias.contains_key(&format!("{key}/*")) {
            aliases.push(ViteAlias {
                find: ViteAliasFind::Pattern(format!("/^{}$/", escape_for_regexp(key))),
                replacement: format!("{cwd}/{normalized}"),
            });
            continue;
        }

        aliases.push(ViteAlias {
            find: ViteAliasFind::Literal(key.clone()),
            replacement: normalized,
        });
    }

    Ok(aliases)
}

pub fn error_for_missing_config(feature_name: &str, path: &str, value: &str) -> crate::Error {
    let parts = path.split('.').collect::<Vec<_>>();
    let mut lines = Vec::new();

    for (index, part) in parts.iter().enumerate() {
        let indent = "  ".repeat(index);
        if index == parts.len() - 1 {
            lines.push(format!("{indent}{part}: {value}"));
        } else {
            lines.push(format!("{indent}{part}: {{"));
        }
    }

    for index in (0..parts.len().saturating_sub(1)).rev() {
        let indent = "  ".repeat(index);
        lines.push(format!("{indent}}}"));
    }

    ViteUtilsError::MissingConfig {
        feature_name: feature_name.to_string(),
        config_snippet: lines.join("\n"),
    }
    .into()
}

pub fn normalize_vite_id(id: &str, lib: &str, cwd: &str) -> String {
    let mut id = id.split('?').next().unwrap_or(id).to_string();

    if id.starts_with(lib) {
        id = id.replacen(lib, "$lib", 1);
    }

    if id.starts_with(cwd) {
        id = id
            .strip_prefix(cwd)
            .unwrap_or(&id)
            .trim_start_matches('/')
            .to_string();
    }

    if id.ends_with("/runtime/app/server/index.js") {
        return "$app/server".to_string();
    }

    if id == env_static_private_module_id() {
        return "$env/static/private".to_string();
    }

    if id == env_static_public_module_id() {
        return "$env/static/public".to_string();
    }

    if id == env_dynamic_private_module_id() {
        return "$env/dynamic/private".to_string();
    }

    if id == env_dynamic_public_module_id() {
        return "$env/dynamic/public".to_string();
    }

    if id == service_worker_module_id() {
        return "$service-worker".to_string();
    }

    posixify(&id)
}

pub fn strip_virtual_prefix(id: &str) -> String {
    id.strip_prefix('\0')
        .and_then(|rest| rest.strip_prefix("virtual:"))
        .unwrap_or(id)
        .to_string()
}

pub fn vite_not_found_response(url: &str, accept: &str, base: &str) -> ServerResponse {
    let negotiated = negotiate(accept, &["text/plain", "text/html"]);

    if url == "/" && negotiated == Some("text/html") {
        let mut response = ServerResponse::new(307);
        response.set_header("location", base);
        return response;
    }

    let prefixed = format!("{base}{url}");
    let mut response = ServerResponse::new(404);

    if negotiated == Some("text/html") {
        response.set_header("content-type", "text/html");
        response.body = Some(format!(
            "The server is configured with a public base URL of {} - did you mean to visit <a href=\"{}\">{}</a> instead?",
            escape_html_with_mode(base, false),
            escape_html_with_mode(&prefixed, true),
            escape_html_with_mode(&prefixed, false),
        ));
        return response;
    }

    response.body = Some(format!(
        "The server is configured with a public base URL of {} - did you mean to visit {} instead?",
        escape_html_with_mode(base, false),
        escape_html_with_mode(&prefixed, false),
    ));
    response
}

fn escape_for_regexp(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '.' | '*' | '+' | '?' | '^' | '$' | '{' | '}' | '(' | ')' | '|' | '[' | ']' | '\\' => {
                escaped.push('\\');
                escaped.push(ch);
            }
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn normalize_relative_path(value: &str) -> String {
    value.strip_prefix("./").unwrap_or(value).to_string()
}
