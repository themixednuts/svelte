use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use camino::Utf8Path;
use oxc_allocator::Allocator;
use oxc_ast::ast::{
    BindingPattern, Declaration, Expression, ModuleExportName, ObjectPropertyKind, PropertyKey,
    Statement, VariableDeclarator,
};
use oxc_parser::Parser;
use oxc_span::SourceType;
use serde_json::Value;

use crate::{
    builder::{
        BuilderPrerenderOption, BuilderRouteApi, BuilderRoutePage, BuilderServerMetadata,
        BuilderServerMetadataRoute,
    },
    config::ValidatedConfig,
    error::{AnalyzeError, Result},
    exports::{RouteModuleKind, validate_module_exports},
    exports_internal::{RemoteExport, RemoteFunctionKind},
    features::{AdapterFeatures, check_feature, list_route_features},
    generate_manifest::BuildData,
    manifest::{KitManifest, ManifestRoute},
    routing::resolve_route,
};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AnalyzedNodeMetadata {
    pub has_server_load: bool,
    pub has_universal_load: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AnalyzedMetadata {
    pub nodes: Vec<AnalyzedNodeMetadata>,
    pub routes: BuilderServerMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalyzedRemoteExport {
    pub kind: RemoteFunctionKind,
    pub dynamic: bool,
}

pub fn analyze_server_metadata(
    cwd: &Utf8Path,
    config: &ValidatedConfig,
    manifest: &KitManifest,
) -> Result<AnalyzedMetadata> {
    analyze_server_metadata_with_features(cwd, config, manifest, None, None, None)
}

pub fn analyze_server_metadata_with_features(
    cwd: &Utf8Path,
    _config: &ValidatedConfig,
    manifest: &KitManifest,
    build_data: Option<&BuildData<'_>>,
    tracked_features: Option<&BTreeMap<String, Vec<String>>>,
    adapter: Option<&AdapterFeatures>,
) -> Result<AnalyzedMetadata> {
    let nodes = manifest
        .nodes
        .iter()
        .map(|node| {
            let server_options = node
                .server
                .as_ref()
                .and_then(|path| read_route_options(&cwd.join(path)));
            let universal_options = node
                .universal
                .as_ref()
                .and_then(|path| read_route_options(&cwd.join(path)));

            AnalyzedNodeMetadata {
                has_server_load: server_options
                    .as_ref()
                    .map(|options| {
                        options.contains_key("load") || options.contains_key("trailingSlash")
                    })
                    .unwrap_or(false),
                has_universal_load: universal_options
                    .as_ref()
                    .map(|options| options.contains_key("load"))
                    .unwrap_or(false),
            }
        })
        .collect::<Vec<_>>();

    let mut routes = BTreeMap::new();
    for route in &manifest.manifest_routes {
        routes.insert(
            route.id.clone(),
            analyze_route(cwd, manifest, route, build_data, tracked_features, adapter)?,
        );
    }

    Ok(AnalyzedMetadata {
        nodes,
        routes: BuilderServerMetadata { routes },
    })
}

pub fn analyze_remote_metadata(
    remotes: &BTreeMap<String, BTreeMap<String, RemoteExport>>,
) -> Result<BTreeMap<String, BTreeMap<String, AnalyzedRemoteExport>>> {
    let mut analyzed = BTreeMap::new();

    for (hash, exports) in remotes {
        let mut analyzed_exports = BTreeMap::new();
        for (name, export) in exports {
            let info = export
                .info()
                .ok_or_else(|| AnalyzeError::InvalidRemoteExport {
                    name: name.clone(),
                    hash: hash.clone(),
                })?;

            analyzed_exports.insert(
                name.clone(),
                AnalyzedRemoteExport {
                    kind: info.kind,
                    dynamic: info.kind != RemoteFunctionKind::Prerender || info.dynamic,
                },
            );
        }
        analyzed.insert(hash.clone(), analyzed_exports);
    }

    Ok(analyzed)
}

fn analyze_route(
    cwd: &Utf8Path,
    manifest: &KitManifest,
    route: &ManifestRoute,
    build_data: Option<&BuildData<'_>>,
    tracked_features: Option<&BTreeMap<String, Vec<String>>>,
    adapter: Option<&AdapterFeatures>,
) -> Result<BuilderServerMetadataRoute> {
    let page = analyze_page_route(cwd, manifest, route)?;
    let endpoint = analyze_endpoint_route(cwd, route)?;

    if page.as_ref().and_then(|page| page.prerender.clone()) == Some(BuilderPrerenderOption::True)
        && endpoint
            .as_ref()
            .and_then(|endpoint| endpoint.prerender.clone())
            == Some(BuilderPrerenderOption::True)
    {
        return Err(AnalyzeError::PrerenderPageAndEndpointConflict {
            route_id: route.id.clone(),
        }
        .into());
    }

    let page_config = page.as_ref().map(|page| page.config.clone());
    let endpoint_config = endpoint.as_ref().map(|endpoint| endpoint.config.clone());
    if let (Some(Value::Object(page_config)), Some(Value::Object(endpoint_config))) =
        (&page_config, &endpoint_config)
    {
        let keys = page_config
            .keys()
            .chain(endpoint_config.keys())
            .cloned()
            .collect::<BTreeSet<_>>();
        if keys
            .iter()
            .any(|key| page_config.get(key) != endpoint_config.get(key))
        {
            return Err(AnalyzeError::MismatchedRouteConfig {
                route_id: route.id.clone(),
            }
            .into());
        }
    }

    let page_methods = page
        .as_ref()
        .map(|page| page.methods.clone())
        .unwrap_or_default();
    let api_methods = endpoint
        .as_ref()
        .map(|endpoint| endpoint.methods.clone())
        .unwrap_or_default();
    let methods = page_methods
        .iter()
        .chain(api_methods.iter())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();

    let prerender = page
        .as_ref()
        .and_then(|page| page.prerender.clone())
        .or_else(|| {
            endpoint
                .as_ref()
                .and_then(|endpoint| endpoint.prerender.clone())
        });
    let entries = page
        .as_ref()
        .and_then(|page| page.entries.clone())
        .or_else(|| {
            endpoint
                .as_ref()
                .and_then(|endpoint| endpoint.entries.clone())
        });

    let config = page_config.or(endpoint_config).unwrap_or(Value::Null);

    if prerender != Some(BuilderPrerenderOption::True) {
        if let (Some(build_data), Some(tracked_features)) = (build_data, tracked_features) {
            for feature in list_route_features(route, build_data, tracked_features) {
                check_feature(&route.id, &config, &feature, adapter)?;
            }
        }
    }

    Ok(BuilderServerMetadataRoute {
        config,
        api: BuilderRouteApi {
            methods: api_methods,
        },
        page: BuilderRoutePage {
            methods: page_methods,
        },
        methods,
        prerender,
        entries,
    })
}

#[derive(Debug, Clone)]
struct PageAnalysis {
    config: Value,
    entries: Option<Vec<String>>,
    methods: Vec<String>,
    prerender: Option<BuilderPrerenderOption>,
}

#[derive(Debug, Clone)]
struct EndpointAnalysis {
    config: Value,
    entries: Option<Vec<String>>,
    methods: Vec<String>,
    prerender: Option<BuilderPrerenderOption>,
}

fn analyze_page_route(
    cwd: &Utf8Path,
    manifest: &KitManifest,
    route: &ManifestRoute,
) -> Result<Option<PageAnalysis>> {
    let Some(page) = route.page.as_ref() else {
        return Ok(None);
    };

    let leaf = manifest
        .nodes
        .get(page.leaf)
        .ok_or_else(|| AnalyzeError::MissingLeafNode {
            route_id: route.id.clone(),
        })?;
    let leaf_universal_options = leaf
        .universal
        .as_ref()
        .and_then(|path| read_route_options(&cwd.join(path)));
    let leaf_server_options = leaf
        .server
        .as_ref()
        .and_then(|path| read_route_options(&cwd.join(path)));

    if let Some(universal) = &leaf.universal {
        validate_page_or_layout_exports(&cwd.join(universal), RouteModuleKind::PageUniversal)?;
    }
    if let Some(server) = &leaf.server {
        validate_page_or_layout_exports(&cwd.join(server), RouteModuleKind::PageServer)?;
    }
    for layout_index in page.layouts.iter().flatten() {
        let layout =
            manifest
                .nodes
                .get(*layout_index)
                .ok_or_else(|| AnalyzeError::MissingLayoutNode {
                    layout_index: *layout_index,
                    route_id: route.id.clone(),
                })?;
        if let Some(universal) = &layout.universal {
            validate_page_or_layout_exports(
                &cwd.join(universal),
                RouteModuleKind::LayoutUniversal,
            )?;
        }
        if let Some(server) = &layout.server {
            validate_page_or_layout_exports(&cwd.join(server), RouteModuleKind::LayoutServer)?;
        }
    }

    let methods = if leaf
        .server
        .as_ref()
        .map(|path| exported_names(&cwd.join(path)))
        .transpose()?
        .map(|exports| exports.contains("actions"))
        .unwrap_or(false)
    {
        vec!["GET".to_string(), "POST".to_string()]
    } else {
        vec!["GET".to_string()]
    };

    let mut config = serde_json::Map::new();
    let mut has_config = false;
    let mut prerender = None;

    for node in page
        .layouts
        .iter()
        .flatten()
        .filter_map(|layout_index| manifest.nodes.get(*layout_index))
        .chain(std::iter::once(leaf))
    {
        let server_options = node
            .server
            .as_ref()
            .and_then(|path| read_route_options(&cwd.join(path)));
        let universal_options = node
            .universal
            .as_ref()
            .and_then(|path| read_route_options(&cwd.join(path)));

        if let Some(Value::Object(entries)) = universal_options
            .as_ref()
            .and_then(|options| options.get("config"))
        {
            config.extend(entries.clone());
            has_config = true;
        }
        if let Some(Value::Object(entries)) = server_options
            .as_ref()
            .and_then(|options| options.get("config"))
        {
            config.extend(entries.clone());
            has_config = true;
        }

        let node_prerender = universal_options
            .as_ref()
            .and_then(|options| options.get("prerender"))
            .and_then(prerender_option)
            .or_else(|| {
                server_options
                    .as_ref()
                    .and_then(|options| options.get("prerender"))
                    .and_then(prerender_option)
            });
        if node_prerender.is_some() {
            prerender = node_prerender;
        }
    }

    Ok(Some(PageAnalysis {
        config: if has_config {
            Value::Object(config)
        } else {
            Value::Null
        },
        entries: leaf_universal_options
            .as_ref()
            .and_then(|options| options.get("entries"))
            .or_else(|| {
                leaf_server_options
                    .as_ref()
                    .and_then(|options| options.get("entries"))
            })
            .and_then(|entries| resolve_entries(&route.id, entries)),
        methods,
        prerender,
    }))
}

fn analyze_endpoint_route(
    cwd: &Utf8Path,
    route: &ManifestRoute,
) -> Result<Option<EndpointAnalysis>> {
    let Some(endpoint) = route.endpoint.as_ref() else {
        return Ok(None);
    };

    let path = cwd.join(&endpoint.file);
    let exports = exported_names(&path)?;
    let endpoint_options = read_route_options(&path);
    validate_module_exports(&exports, RouteModuleKind::Endpoint, &path)?;

    let mut methods = Vec::new();
    for method in ENDPOINT_METHODS {
        if exports.contains(*method) {
            methods.push((*method).to_string());
        }
    }
    if exports.contains("fallback") {
        methods.push("*".to_string());
    }

    let prerender = endpoint_options
        .as_ref()
        .and_then(|options| options.get("prerender"))
        .and_then(prerender_option);
    if prerender == Some(BuilderPrerenderOption::True)
        && methods
            .iter()
            .any(|method| matches!(method.as_str(), "POST" | "PATCH" | "PUT" | "DELETE"))
    {
        return Err(AnalyzeError::MutativePrerenderEndpoint {
            route_id: route.id.clone(),
        }
        .into());
    }

    Ok(Some(EndpointAnalysis {
        config: endpoint_options
            .as_ref()
            .and_then(|options| options.get("config"))
            .cloned()
            .unwrap_or(Value::Null),
        entries: endpoint_options
            .as_ref()
            .and_then(|options| options.get("entries"))
            .and_then(|entries| resolve_entries(&route.id, entries)),
        methods,
        prerender,
    }))
}

fn resolve_entries(route_id: &str, entries: &Value) -> Option<Vec<String>> {
    let items = entries.as_array()?;
    let mut resolved = Vec::new();

    for item in items {
        let object = item.as_object()?;
        let params = object
            .iter()
            .map(|(key, value)| Some((key.clone(), value.as_str()?.to_string())))
            .collect::<Option<BTreeMap<_, _>>>()?;
        resolved.push(resolve_route(route_id, &params).ok()?);
    }

    Some(resolved)
}

fn prerender_option(value: &Value) -> Option<BuilderPrerenderOption> {
    match value {
        Value::Bool(true) => Some(BuilderPrerenderOption::True),
        Value::Bool(false) => Some(BuilderPrerenderOption::False),
        Value::String(value) if value == "auto" => Some(BuilderPrerenderOption::Auto),
        _ => None,
    }
}

fn exported_names(path: &Utf8Path) -> Result<BTreeSet<String>> {
    let source = fs::read_to_string(path)?;
    let allocator = Allocator::default();
    let parsed = Parser::new(
        &allocator,
        &source,
        SourceType::from_path(path.as_std_path())
            .unwrap_or_else(|_| SourceType::ts())
            .with_module(true),
    )
    .parse();
    if !parsed.errors.is_empty() {
        return Err(AnalyzeError::ParseModule {
            path: path.to_string(),
        }
        .into());
    }

    let mut exports = BTreeSet::new();
    for statement in &parsed.program.body {
        match statement {
            Statement::ExportDefaultDeclaration(_) => {
                exports.insert("default".to_string());
            }
            Statement::ExportAllDeclaration(_) => {
                return Err(AnalyzeError::UnsupportedExportAll {
                    path: path.to_string(),
                }
                .into());
            }
            Statement::ExportNamedDeclaration(declaration) => {
                if declaration.source.is_some() {
                    for specifier in &declaration.specifiers {
                        if let Some(name) = module_export_name(&specifier.exported) {
                            exports.insert(name);
                        }
                    }
                    continue;
                }

                if let Some(inner) = declaration.declaration.as_ref() {
                    collect_declaration_exports(inner, &mut exports);
                }

                for specifier in &declaration.specifiers {
                    if let Some(name) = module_export_name(&specifier.exported) {
                        exports.insert(name);
                    }
                }
            }
            _ => {}
        }
    }

    Ok(exports)
}

fn collect_declaration_exports(declaration: &Declaration<'_>, exports: &mut BTreeSet<String>) {
    match declaration {
        Declaration::VariableDeclaration(declaration) => {
            for declarator in &declaration.declarations {
                collect_variable_export(declarator, exports);
            }
        }
        Declaration::FunctionDeclaration(declaration) => {
            if let Some(id) = declaration.id.as_ref() {
                exports.insert(id.name.as_str().to_string());
            }
        }
        Declaration::ClassDeclaration(declaration) => {
            if let Some(id) = declaration.id.as_ref() {
                exports.insert(id.name.as_str().to_string());
            }
        }
        _ => {}
    }
}

fn collect_variable_export(declarator: &VariableDeclarator<'_>, exports: &mut BTreeSet<String>) {
    if let Some(name) = binding_name(&declarator.id) {
        exports.insert(name);
    }
}

fn binding_name(pattern: &BindingPattern<'_>) -> Option<String> {
    match pattern {
        BindingPattern::BindingIdentifier(identifier) => Some(identifier.name.as_str().to_string()),
        BindingPattern::AssignmentPattern(pattern) => binding_name(&pattern.left),
        _ => None,
    }
}

fn module_export_name(name: &ModuleExportName<'_>) -> Option<String> {
    match name {
        ModuleExportName::IdentifierName(identifier) => Some(identifier.name.as_str().to_string()),
        ModuleExportName::IdentifierReference(identifier) => {
            Some(identifier.name.as_str().to_string())
        }
        ModuleExportName::StringLiteral(literal) => Some(literal.value.as_str().to_string()),
    }
}

fn validate_page_or_layout_exports(path: &Utf8Path, kind: RouteModuleKind) -> Result<()> {
    let exports = exported_names(path)?;
    validate_module_exports(&exports, kind, path)
}

const ENDPOINT_METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS", "HEAD"];

type RouteOptions = BTreeMap<String, Value>;

#[derive(Debug, Clone, PartialEq)]
enum AnalyzedValue {
    Dynamic,
    Static(Value),
}

fn read_route_options(path: &Utf8Path) -> Option<RouteOptions> {
    let source = fs::read_to_string(path).ok()?;
    statically_analyze_route_options(&source)
}

fn statically_analyze_route_options(source: &str) -> Option<RouteOptions> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::ts().with_module(true)).parse();
    if !parsed.errors.is_empty() {
        return None;
    }

    let mut declarations = BTreeMap::<String, AnalyzedValue>::new();
    let mut options = RouteOptions::new();

    for statement in &parsed.program.body {
        match statement {
            Statement::ExportDefaultDeclaration(_) => return None,
            Statement::ExportAllDeclaration(declaration) => {
                let Some(exported) = declaration.exported.as_ref().and_then(module_export_name)
                else {
                    return None;
                };
                if VALID_ROUTE_OPTIONS.contains(&exported.as_str()) {
                    return None;
                }
            }
            Statement::ExportNamedDeclaration(declaration) => {
                if declaration.source.is_some() {
                    for specifier in &declaration.specifiers {
                        let Some(exported) = module_export_name(&specifier.exported) else {
                            return None;
                        };
                        if VALID_ROUTE_OPTIONS.contains(&exported.as_str()) {
                            return None;
                        }
                    }
                    continue;
                }

                if let Some(inner) = declaration.declaration.as_ref() {
                    analyze_route_declaration(inner, true, &mut declarations, &mut options)?;
                    continue;
                }

                for specifier in &declaration.specifiers {
                    let Some(exported) = module_export_name(&specifier.exported) else {
                        return None;
                    };
                    if !VALID_ROUTE_OPTIONS.contains(&exported.as_str()) {
                        continue;
                    }

                    let Some(local) = module_export_name(&specifier.local) else {
                        return None;
                    };
                    let Some(value) = declarations.get(&local) else {
                        return None;
                    };

                    match value {
                        AnalyzedValue::Static(value) => {
                            options.insert(exported, value.clone());
                        }
                        AnalyzedValue::Dynamic => return None,
                    }
                }
            }
            Statement::VariableDeclaration(declaration) => {
                analyze_route_variable_declaration(
                    declaration,
                    false,
                    &mut declarations,
                    &mut options,
                )?;
            }
            Statement::FunctionDeclaration(declaration) => {
                analyze_route_named_declaration(
                    declaration
                        .id
                        .as_ref()
                        .map(|id| id.name.as_str().to_string()),
                    false,
                    &mut declarations,
                    &mut options,
                    true,
                )?;
            }
            Statement::ClassDeclaration(declaration) => {
                analyze_route_named_declaration(
                    declaration
                        .id
                        .as_ref()
                        .map(|id| id.name.as_str().to_string()),
                    false,
                    &mut declarations,
                    &mut options,
                    false,
                )?;
            }
            _ => {}
        }
    }

    Some(options)
}

fn analyze_route_declaration(
    declaration: &Declaration<'_>,
    exported: bool,
    declarations: &mut BTreeMap<String, AnalyzedValue>,
    options: &mut RouteOptions,
) -> Option<()> {
    match declaration {
        Declaration::VariableDeclaration(declaration) => {
            analyze_route_variable_declaration(declaration, exported, declarations, options)
        }
        Declaration::FunctionDeclaration(declaration) => analyze_route_named_declaration(
            declaration
                .id
                .as_ref()
                .map(|id| id.name.as_str().to_string()),
            exported,
            declarations,
            options,
            true,
        ),
        Declaration::ClassDeclaration(declaration) => analyze_route_named_declaration(
            declaration
                .id
                .as_ref()
                .map(|id| id.name.as_str().to_string()),
            exported,
            declarations,
            options,
            false,
        ),
        _ => Some(()),
    }
}

fn analyze_route_variable_declaration(
    declaration: &oxc_ast::ast::VariableDeclaration<'_>,
    exported: bool,
    declarations: &mut BTreeMap<String, AnalyzedValue>,
    options: &mut RouteOptions,
) -> Option<()> {
    for declarator in &declaration.declarations {
        analyze_route_variable_declarator(declarator, exported, declarations, options)?;
    }
    Some(())
}

fn analyze_route_variable_declarator(
    declarator: &VariableDeclarator<'_>,
    exported: bool,
    declarations: &mut BTreeMap<String, AnalyzedValue>,
    options: &mut RouteOptions,
) -> Option<()> {
    let Some(name) = binding_name(&declarator.id) else {
        if exported {
            return None;
        }
        return Some(());
    };

    let value = analyze_route_initializer(&name, declarator.init.as_ref());
    declarations.insert(name.clone(), value.clone());

    if exported && VALID_ROUTE_OPTIONS.contains(&name.as_str()) {
        match value {
            AnalyzedValue::Static(value) => {
                options.insert(name, value);
            }
            AnalyzedValue::Dynamic => return None,
        }
    }

    Some(())
}

fn analyze_route_named_declaration(
    name: Option<String>,
    exported: bool,
    declarations: &mut BTreeMap<String, AnalyzedValue>,
    options: &mut RouteOptions,
    is_function: bool,
) -> Option<()> {
    let Some(name) = name else {
        return Some(());
    };

    let value = if name == "load" && is_function {
        AnalyzedValue::Static(Value::Null)
    } else {
        AnalyzedValue::Dynamic
    };
    declarations.insert(name.clone(), value.clone());

    if exported && VALID_ROUTE_OPTIONS.contains(&name.as_str()) {
        match value {
            AnalyzedValue::Static(value) => {
                options.insert(name, value);
            }
            AnalyzedValue::Dynamic => return None,
        }
    }

    Some(())
}

fn analyze_route_initializer(name: &str, init: Option<&Expression<'_>>) -> AnalyzedValue {
    let Some(init) = init else {
        return AnalyzedValue::Dynamic;
    };

    if name == "load"
        && matches!(
            init,
            Expression::ArrowFunctionExpression(_) | Expression::FunctionExpression(_)
        )
    {
        return AnalyzedValue::Static(Value::Null);
    }

    route_literal_value(init)
        .map(AnalyzedValue::Static)
        .unwrap_or(AnalyzedValue::Dynamic)
}

fn route_literal_value(expression: &Expression<'_>) -> Option<Value> {
    match expression {
        Expression::ParenthesizedExpression(expression) => {
            route_literal_value(&expression.expression)
        }
        Expression::ArrayExpression(array) => Some(Value::Array(
            array
                .elements
                .iter()
                .map(|element| route_literal_value(element.as_expression()?))
                .collect::<Option<Vec<_>>>()?,
        )),
        Expression::BooleanLiteral(value) => Some(Value::Bool(value.value)),
        Expression::NullLiteral(_) => Some(Value::Null),
        Expression::NumericLiteral(value) => {
            serde_json::Number::from_f64(value.value).map(Value::Number)
        }
        Expression::ObjectExpression(object) => Some(Value::Object({
            let mut entries = serde_json::Map::new();
            for property in &object.properties {
                match property {
                    ObjectPropertyKind::ObjectProperty(property) => {
                        if property.computed || property.method {
                            return None;
                        }

                        let key = route_property_key_name(&property.key)?;
                        entries.insert(key, route_literal_value(&property.value)?);
                    }
                    ObjectPropertyKind::SpreadProperty(property) => {
                        let Value::Object(spread_entries) =
                            route_literal_value(&property.argument)?
                        else {
                            return None;
                        };
                        entries.extend(spread_entries);
                    }
                }
            }
            entries
        })),
        Expression::StringLiteral(value) => Some(Value::String(value.value.to_string())),
        Expression::TemplateLiteral(value)
            if value.expressions.is_empty() && value.quasis.len() == 1 =>
        {
            Some(Value::String(
                value.quasis[0].value.cooked.as_ref()?.to_string(),
            ))
        }
        Expression::TSAsExpression(expression) => route_literal_value(&expression.expression),
        Expression::TSSatisfiesExpression(expression) => {
            route_literal_value(&expression.expression)
        }
        Expression::TSNonNullExpression(expression) => route_literal_value(&expression.expression),
        Expression::TSInstantiationExpression(expression) => {
            route_literal_value(&expression.expression)
        }
        _ => None,
    }
}

fn route_property_key_name(key: &PropertyKey<'_>) -> Option<String> {
    match key {
        PropertyKey::StaticIdentifier(identifier) => Some(identifier.name.to_string()),
        PropertyKey::Identifier(identifier) => Some(identifier.name.to_string()),
        PropertyKey::StringLiteral(literal) => Some(literal.value.to_string()),
        PropertyKey::TemplateLiteral(literal)
            if literal.expressions.is_empty() && literal.quasis.len() == 1 =>
        {
            Some(literal.quasis[0].value.cooked.as_ref()?.to_string())
        }
        _ => None,
    }
}

const VALID_ROUTE_OPTIONS: &[&str] = &[
    "ssr",
    "prerender",
    "csr",
    "trailingSlash",
    "config",
    "entries",
    "load",
];
