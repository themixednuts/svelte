use crate::{Error, ViteGuardError};

pub fn browser_import_guard_error(normalized: &str, chain: &[&str]) -> Error {
    let pyramid = chain
        .iter()
        .rev()
        .enumerate()
        .map(|(index, id)| format!("{}{}", " ".repeat(index + 1), id))
        .collect::<Vec<_>>()
        .join(" imports\n");

    ViteGuardError::BrowserImport {
        normalized: normalized.to_string(),
        pyramid,
    }
    .into()
}

pub fn service_worker_import_guard_error(normalized: &str) -> Error {
    ViteGuardError::ServiceWorkerImport {
        normalized: normalized.to_string(),
    }
    .into()
}
