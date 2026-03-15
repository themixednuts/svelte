use std::collections::BTreeMap;

use std::collections::BTreeMap as MapById;

use rayon::prelude::*;

use crate::{
    ClientLayoutRef, ClientLeafRef, ClientRoute, KitManifest, ManifestRoute, RouteParam,
    RouterResolution, ValidatedKitConfig,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevServerNodePlan {
    pub index: usize,
    pub component: bool,
    pub universal_id: Option<String>,
    pub server_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevClientLayoutPlan {
    pub uses_server_data: bool,
    pub node: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevClientLeafPlan {
    pub uses_server_data: bool,
    pub node: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevClientRoutePlan {
    pub id: String,
    pub pattern: String,
    pub params: Vec<RouteParam>,
    pub layouts: Vec<Option<DevClientLayoutPlan>>,
    pub errors: Vec<Option<usize>>,
    pub leaf: DevClientLeafPlan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViteDevPlan {
    pub app_dir: String,
    pub app_path: String,
    pub assets: Vec<String>,
    pub mime_types: BTreeMap<String, String>,
    pub client_start: String,
    pub client_app: String,
    pub client_nodes: Option<Vec<Option<String>>>,
    pub client_routes: Option<Vec<DevClientRoutePlan>>,
    pub server_nodes: Vec<DevServerNodePlan>,
    pub remote_hashes: Vec<String>,
    pub router_resolution: RouterResolution,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DevClientPlan {
    client_nodes: Option<Vec<Option<String>>>,
    client_routes: Option<Vec<DevClientRoutePlan>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DevAssetPlan {
    assets: Vec<String>,
    mime_types: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DevServerPlan {
    server_nodes: Vec<DevServerNodePlan>,
}

pub fn build_vite_dev_plan(
    kit: &ValidatedKitConfig,
    manifest: &KitManifest,
    out_dir: &str,
    runtime_base: &str,
    remote_hashes: &[String],
) -> ViteDevPlan {
    let (client_plan, other_plans) = rayon::join(
        || build_dev_client_plan(kit, manifest, out_dir),
        || {
            rayon::join(
                || build_dev_asset_plan(manifest),
                || build_dev_server_plan(manifest),
            )
        },
    );
    let asset_plan = other_plans.0;
    let server_plan = other_plans.1;

    ViteDevPlan {
        app_dir: kit.app_dir.clone(),
        app_path: kit.app_dir.clone(),
        assets: asset_plan.assets,
        mime_types: asset_plan.mime_types,
        client_start: format!("{runtime_base}/client/entry.js"),
        client_app: format!("{out_dir}/generated/client/app.js"),
        client_nodes: client_plan.client_nodes,
        client_routes: client_plan.client_routes,
        server_nodes: server_plan.server_nodes,
        remote_hashes: remote_hashes.to_vec(),
        router_resolution: kit.router.resolution.clone(),
    }
}

fn build_dev_client_plan(
    kit: &ValidatedKitConfig,
    manifest: &KitManifest,
    out_dir: &str,
) -> DevClientPlan {
    if matches!(kit.router.resolution, RouterResolution::Client) {
        return DevClientPlan {
            client_nodes: None,
            client_routes: None,
        };
    }

    let (client_nodes, client_routes) = rayon::join(
        || {
            manifest
                .nodes
                .par_iter()
                .enumerate()
                .map(|(index, node)| {
                    if node.component.is_some() || node.universal.is_some() {
                        Some(format!(
                            "{}{}/generated/client/nodes/{index}.js",
                            kit.paths.base, out_dir
                        ))
                    } else {
                        None
                    }
                })
                .collect()
        },
        || build_client_route_plans(manifest),
    );

    DevClientPlan {
        client_nodes: Some(client_nodes),
        client_routes: Some(client_routes),
    }
}

fn build_dev_asset_plan(manifest: &KitManifest) -> DevAssetPlan {
    let (assets, mime_types) = rayon::join(
        || {
            manifest
                .assets
                .par_iter()
                .map(|asset| asset.file.as_str().to_string())
                .collect()
        },
        || build_mime_types(manifest),
    );

    DevAssetPlan { assets, mime_types }
}

fn build_dev_server_plan(manifest: &KitManifest) -> DevServerPlan {
    let server_nodes = manifest
        .nodes
        .par_iter()
        .enumerate()
        .map(|(index, node)| DevServerNodePlan {
            index,
            component: node.component.is_some(),
            universal_id: node
                .universal
                .as_ref()
                .map(|path| path.as_str().to_string()),
            server_id: node.server.as_ref().map(|path| path.as_str().to_string()),
        })
        .collect();

    DevServerPlan { server_nodes }
}

fn build_client_route_plans(manifest: &KitManifest) -> Vec<DevClientRoutePlan> {
    let client_routes = manifest
        .build_client_routes()
        .into_iter()
        .map(|route| (route.id.clone(), route))
        .collect::<MapById<_, _>>();

    manifest
        .manifest_routes
        .par_iter()
        .filter_map(|route| {
            let client_route = client_routes.get(&route.id)?;
            Some(map_client_route(route, client_route))
        })
        .collect()
}

fn map_client_route(route: &ManifestRoute, client_route: &ClientRoute) -> DevClientRoutePlan {
    DevClientRoutePlan {
        id: route.id.clone(),
        pattern: route.pattern.as_str().to_string(),
        params: route.params.clone(),
        layouts: client_route
            .layouts
            .iter()
            .cloned()
            .map(map_client_layout)
            .collect(),
        errors: client_route.errors.clone(),
        leaf: map_client_leaf(client_route.leaf.clone()),
    }
}

fn map_client_layout(layout: Option<ClientLayoutRef>) -> Option<DevClientLayoutPlan> {
    layout.map(|layout| DevClientLayoutPlan {
        uses_server_data: layout.uses_server_data,
        node: layout.node,
    })
}

fn map_client_leaf(leaf: ClientLeafRef) -> DevClientLeafPlan {
    DevClientLeafPlan {
        uses_server_data: leaf.uses_server_data,
        node: leaf.node,
    }
}

fn build_mime_types(manifest: &KitManifest) -> BTreeMap<String, String> {
    manifest
        .assets
        .par_iter()
        .filter_map(|asset| {
            let extension = asset.file.extension()?;
            let key = format!(".{extension}");
            let value = asset.type_.clone().unwrap_or_else(|| {
                mime_guess::from_ext(extension)
                    .first_raw()
                    .unwrap_or("application/octet-stream")
                    .to_string()
            });
            Some((key, value))
        })
        .collect()
}
