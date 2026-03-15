use serde_json::{Map, Value};

use crate::manifest::{KitManifest, ManifestNode, ManifestRoute, ManifestRoutePage};

use super::RuntimeRouteBehavior;

#[derive(Debug, Clone)]
pub struct RuntimePageNodes<'a> {
    pub(crate) nodes: Vec<Option<&'a ManifestNode>>,
}

impl<'a> RuntimePageNodes<'a> {
    pub fn from_route(page: &ManifestRoutePage, manifest: &'a KitManifest) -> Self {
        Self {
            nodes: load_page_nodes(page, manifest),
        }
    }

    pub fn layouts(&self) -> &[Option<&'a ManifestNode>] {
        let len = self.nodes.len().saturating_sub(1);
        &self.nodes[..len]
    }

    pub fn page(&self) -> Option<&'a ManifestNode> {
        self.nodes.last().copied().flatten()
    }

    pub fn csr(&self) -> bool {
        self.bool_option("csr").unwrap_or(true)
    }

    pub fn ssr(&self) -> bool {
        self.bool_option("ssr").unwrap_or(true)
    }

    pub fn prerender(&self) -> bool {
        self.bool_option("prerender").unwrap_or(false)
    }

    pub fn trailing_slash(&self) -> String {
        self.string_option("trailingSlash")
            .unwrap_or_else(|| "never".to_string())
    }

    pub fn get_config(&self) -> Option<Map<String, Value>> {
        self.page()
            .and_then(|node| node.page_options.as_ref())
            .and_then(|options| options.get("config"))
            .and_then(Value::as_object)
            .cloned()
    }

    pub fn should_prerender_data(&self) -> bool {
        self.nodes.iter().flatten().any(|node| {
            node.server_page_options.as_ref().is_some_and(|options| {
                options.contains_key("load") || options.contains_key("trailingSlash")
            })
        })
    }

    fn bool_option(&self, key: &str) -> Option<bool> {
        self.option_value(key).and_then(Value::as_bool)
    }

    fn string_option(&self, key: &str) -> Option<String> {
        self.option_value(key)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    fn option_value(&self, key: &str) -> Option<&Value> {
        self.nodes.iter().flatten().fold(None, |current, node| {
            node.page_options
                .as_ref()
                .and_then(|options| options.get(key))
                .or(current)
        })
    }
}

pub fn load_page_nodes<'a>(
    page: &ManifestRoutePage,
    manifest: &'a KitManifest,
) -> Vec<Option<&'a ManifestNode>> {
    let mut nodes = page
        .layouts
        .iter()
        .map(|layout| layout.and_then(|index| manifest.nodes.get(index)))
        .collect::<Vec<_>>();
    nodes.push(manifest.nodes.get(page.leaf));
    nodes
}

pub fn resolve_runtime_route_behavior(
    manifest: &KitManifest,
    route: &ManifestRoute,
    request_pathname: &str,
    base: &str,
) -> RuntimeRouteBehavior {
    if !base.is_empty() {
        let base_path = base.trim_end_matches('/');
        if request_pathname == base_path || request_pathname == format!("{base_path}/") {
            return RuntimeRouteBehavior {
                trailing_slash: "always".to_string(),
                prerender: false,
                config: Map::new(),
            };
        }
    }

    if let Some(page) = route.page.as_ref() {
        let nodes = RuntimePageNodes::from_route(page, manifest);
        return RuntimeRouteBehavior {
            trailing_slash: nodes.trailing_slash(),
            prerender: nodes.prerender(),
            config: nodes.get_config().unwrap_or_default(),
        };
    }

    if let Some(endpoint) = route.endpoint.as_ref() {
        let page_options = endpoint.page_options.as_ref();
        return RuntimeRouteBehavior {
            trailing_slash: page_options
                .and_then(|options| options.get("trailingSlash"))
                .and_then(Value::as_str)
                .unwrap_or("never")
                .to_string(),
            prerender: page_options
                .and_then(|options| options.get("prerender"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            config: page_options
                .and_then(|options| options.get("config"))
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default(),
        };
    }

    RuntimeRouteBehavior {
        trailing_slash: "never".to_string(),
        prerender: false,
        config: Map::new(),
    }
}
