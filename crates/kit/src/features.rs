use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::{
    error::{FeatureError, Result},
    generate_manifest::BuildData,
    manifest::ManifestRoute,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterFeatures {
    pub name: String,
    pub supports_read: bool,
}

pub fn check_feature(
    route_id: &str,
    _config: &Value,
    feature: &str,
    adapter: Option<&AdapterFeatures>,
) -> Result<()> {
    let Some(adapter) = adapter else {
        return Ok(());
    };

    match feature {
        "$app/server:read" if !adapter.supports_read => Err(FeatureError::UnsupportedRead {
            route_id: route_id.to_string(),
            adapter_name: adapter.name.clone(),
        }
        .into()),
        _ => Ok(()),
    }
}

pub fn list_route_features(
    route: &ManifestRoute,
    build_data: &BuildData<'_>,
    tracked_features: &BTreeMap<String, Vec<String>>,
) -> Vec<String> {
    let mut features = BTreeSet::new();
    let mut visited = BTreeSet::new();

    let visit = |id: &str, visited: &mut BTreeSet<String>, features: &mut BTreeSet<String>| {
        visit_chunk(
            id,
            build_data.server_manifest,
            tracked_features,
            visited,
            features,
        );
    };

    let Some(route_data) = build_data
        .manifest_data
        .manifest_routes
        .iter()
        .find(|candidate| candidate.id == route.id)
    else {
        return Vec::new();
    };

    if let Some(page) = &route_data.page {
        for node_index in page
            .layouts
            .iter()
            .flatten()
            .copied()
            .chain(std::iter::once(page.leaf))
        {
            let Some(node) = build_data.manifest_data.nodes.get(node_index) else {
                continue;
            };

            if let Some(server) = &node.server {
                visit(server.as_str(), &mut visited, &mut features);
            }
        }
    }

    if let Some(endpoint) = &route_data.endpoint {
        visit(endpoint.file.as_str(), &mut visited, &mut features);
    }

    if let Some(server_hook) = &build_data.manifest_data.hooks.server {
        visit(server_hook.as_str(), &mut visited, &mut features);
    }

    features.into_iter().collect()
}

fn visit_chunk(
    id: &str,
    server_manifest: &BTreeMap<String, crate::generate_manifest::BuildManifestChunk>,
    tracked_features: &BTreeMap<String, Vec<String>>,
    visited: &mut BTreeSet<String>,
    features: &mut BTreeSet<String>,
) {
    if !visited.insert(id.to_string()) {
        return;
    }

    let Some(chunk) = server_manifest.get(id) else {
        return;
    };

    if let Some(chunk_features) = tracked_features.get(&chunk.file) {
        features.extend(chunk_features.iter().cloned());
    }

    for import in &chunk.imports {
        visit_chunk(import, server_manifest, tracked_features, visited, features);
    }
}
