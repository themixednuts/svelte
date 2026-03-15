use crate::{Error, PrerenderError};

pub fn prerender_entry_generator_mismatch_error(
    generated_from_id: &str,
    entry: &str,
    matched_id: &str,
) -> crate::Error {
    PrerenderError::EntryGeneratorMismatch {
        generated_from_id: generated_from_id.to_string(),
        entry: entry.to_string(),
        matched_id: matched_id.to_string(),
    }
    .into()
}

pub fn prerender_unseen_routes_error(routes: &[&str]) -> Error {
    let list = routes
        .iter()
        .map(|route| format!("  - {route}"))
        .collect::<Vec<_>>()
        .join("\n");

    PrerenderError::UnseenRoutes { routes: list }.into()
}
