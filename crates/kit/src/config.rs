use std::fs;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use camino::{Utf8Path, Utf8PathBuf};
use oxc_allocator::Allocator;
use oxc_ast::ast::{
    BindingPattern, Declaration, ExportDefaultDeclarationKind, Expression,
    ImportDeclarationSpecifier, ModuleExportName, ObjectPropertyKind, PropertyKey, Span, Statement,
    VariableDeclarator,
};
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType};
use regex::Regex;
use serde_json::{Map, Value};
use url::Url;

use crate::error::{ConfigError, Result};
use crate::manifest::{KitManifest, ManifestConfig};

const FUNCTION_SOURCE_MARKER_KEY: &str = "__svelte_kit_function_source";
const FUNCTION_SOURCE_KIND_KEY: &str = "__svelte_kit_function_kind";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedConfig {
    pub compiler_options: ValidatedCompilerOptions,
    pub extensions: Vec<String>,
    pub kit: ValidatedKitConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedCompilerOptions {
    pub experimental: ValidatedCompilerExperimentalOptions,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedCompilerExperimentalOptions {
    pub async_: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedKitConfig {
    pub adapter: Option<ValidatedAdapterConfig>,
    pub alias: std::collections::BTreeMap<String, String>,
    pub app_dir: String,
    pub csp: ValidatedCspConfig,
    pub csrf: ValidatedCsrfConfig,
    pub embedded: bool,
    pub env: ValidatedEnvConfig,
    pub experimental: ValidatedExperimentalConfig,
    pub files: ValidatedFilesConfig,
    pub inline_style_threshold: u64,
    pub module_extensions: Vec<String>,
    pub out_dir: Utf8PathBuf,
    pub output: ValidatedOutputConfig,
    pub paths: ValidatedPathsConfig,
    pub prerender: ValidatedPrerenderConfig,
    pub router: ValidatedRouterConfig,
    pub service_worker: ValidatedServiceWorkerConfig,
    pub typescript: ValidatedTypeScriptConfig,
    pub version: ValidatedVersionConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedAdapterConfig {
    pub raw: Map<String, Value>,
    pub source: Option<JsSource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct JsSource {
    source: Arc<str>,
    kind: JsSourceKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JsSourceKind {
    Expression,
    Function,
    Method,
    CallExpression,
    IdentifierReference,
}

impl JsSource {
    pub fn new(source: impl Into<Arc<str>>, kind: JsSourceKind) -> Self {
        Self {
            source: source.into(),
            kind,
        }
    }

    pub fn as_str(&self) -> &str {
        &self.source
    }

    pub fn kind(&self) -> JsSourceKind {
        self.kind
    }
}

impl JsSourceKind {
    fn as_marker(self) -> &'static str {
        match self {
            Self::Expression => "expression",
            Self::Function => "function",
            Self::Method => "method",
            Self::CallExpression => "call-expression",
            Self::IdentifierReference => "identifier-reference",
        }
    }

    fn from_marker(value: &str) -> Self {
        match value {
            "function" => Self::Function,
            "method" => Self::Method,
            "call-expression" => Self::CallExpression,
            "identifier-reference" => Self::IdentifierReference,
            _ => Self::Expression,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedEnvConfig {
    pub dir: String,
    pub public_prefix: String,
    pub private_prefix: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedCsrfConfig {
    pub check_origin: bool,
    pub trusted_origins: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedCspConfig {
    pub mode: CspMode,
    pub directives: ValidatedCspDirectives,
    pub report_only: ValidatedCspDirectives,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedCspDirectives {
    pub string_lists: std::collections::BTreeMap<String, Vec<String>>,
    pub upgrade_insecure_requests: bool,
    pub block_all_mixed_content: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CspMode {
    Auto,
    Hash,
    Nonce,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedExperimentalConfig {
    pub instrumentation: ValidatedInstrumentationConfig,
    pub remote_functions: bool,
    pub tracing: ValidatedTracingConfig,
    pub fork_preloads: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedInstrumentationConfig {
    pub server: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedTracingConfig {
    pub server: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedFilesConfig {
    pub src: Utf8PathBuf,
    pub assets: Utf8PathBuf,
    pub hooks: ValidatedHooksConfig,
    pub lib: Utf8PathBuf,
    pub params: Utf8PathBuf,
    pub routes: Utf8PathBuf,
    pub service_worker: Utf8PathBuf,
    pub app_template: Utf8PathBuf,
    pub error_template: Utf8PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedHooksConfig {
    pub client: Utf8PathBuf,
    pub server: Utf8PathBuf,
    pub universal: Utf8PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedOutputConfig {
    pub preload_strategy: PreloadStrategy,
    pub bundle_strategy: BundleStrategy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreloadStrategy {
    ModulePreload,
    PreloadJs,
    PreloadMjs,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundleStrategy {
    Split,
    Single,
    Inline,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedPathsConfig {
    pub base: String,
    pub assets: String,
    pub relative: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedPrerenderConfig {
    pub concurrency: u64,
    pub crawl: bool,
    pub entries: Vec<String>,
    pub handle_http_error: PrerenderPolicy,
    pub handle_missing_id: PrerenderPolicy,
    pub handle_entry_generator_mismatch: PrerenderPolicy,
    pub handle_unseen_routes: PrerenderPolicy,
    pub origin: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrerenderPolicy {
    Fail,
    Warn,
    Ignore,
    Source(JsSource),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedRouterConfig {
    pub type_: RouterType,
    pub resolution: RouterResolution,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedServiceWorkerConfig {
    pub files: ServiceWorkerFilesFilter,
    pub register: bool,
    pub options: Option<Map<String, Value>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceWorkerFilesFilter {
    IgnoreDsStore,
    Source(JsSource),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedTypeScriptConfig {
    pub config: TypeScriptConfigHook,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeScriptConfigHook {
    Identity,
    Source(JsSource),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouterType {
    Pathname,
    Hash,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouterResolution {
    Client,
    Server,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedVersionConfig {
    pub name: String,
    pub poll_interval: u64,
}

#[derive(Debug, Clone)]
pub struct LoadedKitProject {
    pub cwd: Utf8PathBuf,
    pub config: ValidatedConfig,
    pub manifest: KitManifest,
    pub template: String,
    pub error_page: String,
}

pub fn load_config(cwd: &Utf8Path) -> Result<ValidatedConfig> {
    let Some(path) = find_config_path(cwd) else {
        return validate_config(&Value::Object(Map::new()), cwd);
    };

    let source = fs::read_to_string(&path)?;
    let config = parse_config_source(&path, &source)?;
    validate_config(&config, cwd)
}

pub fn load_project(cwd: &Utf8Path) -> Result<LoadedKitProject> {
    let config = load_config(cwd)?;
    let manifest_config = ManifestConfig::from_validated_config(&config, cwd.to_path_buf());
    let manifest = KitManifest::discover(&manifest_config)?;
    let template = load_template(cwd, &config)?;
    let error_page = load_error_page(&config)?;

    Ok(LoadedKitProject {
        cwd: cwd.to_path_buf(),
        config,
        manifest,
        template,
        error_page,
    })
}

pub fn validate_config(config: &Value, cwd: &Utf8Path) -> Result<ValidatedConfig> {
    let Some(config_object) = config.as_object() else {
        return Err(ConfigError::DefaultExportObjectRequired.into());
    };

    let compiler_options = parse_compiler_options(config_object.get("compilerOptions"))?;
    let extensions = parse_extensions(config_object.get("extensions"), "config.extensions")?;
    let kit = parse_kit_config(config_object.get("kit"), cwd)?;

    if kit.router.resolution == RouterResolution::Server && kit.router.type_ == RouterType::Hash {
        return Err(ConfigError::invalid_value(
            "The `router.resolution` option cannot be 'server' if `router.type` is 'hash'",
        )
        .into());
    }

    if kit.router.resolution == RouterResolution::Server
        && kit.output.bundle_strategy != BundleStrategy::Split
    {
        return Err(ConfigError::invalid_value(
            "The `router.resolution` option cannot be 'server' if `output.bundleStrategy` is 'inline' or 'single'",
        )
        .into());
    }

    Ok(ValidatedConfig {
        compiler_options,
        extensions,
        kit,
    })
}

pub fn load_template(cwd: &Utf8Path, config: &ValidatedConfig) -> Result<String> {
    let relative = config
        .kit
        .files
        .app_template
        .strip_prefix(cwd)
        .map(Utf8Path::to_path_buf)
        .unwrap_or_else(|_| config.kit.files.app_template.clone());
    let relative = Utf8PathBuf::from(relative.as_str().replace('\\', "/"));

    if !config.kit.files.app_template.is_file() {
        return Err(ConfigError::MissingFile {
            relative: relative.to_string(),
        }
        .into());
    }

    let contents = fs::read_to_string(&config.kit.files.app_template)?;
    for tag in ["%sveltekit.head%", "%sveltekit.body%"] {
        if !contents.contains(tag) {
            return Err(ConfigError::MissingTemplateTag {
                relative: relative.to_string(),
                tag,
            }
            .into());
        }
    }

    for captures in Regex::new(r"%sveltekit\.env\.([^%]+)%")
        .expect("valid env placeholder regex")
        .captures_iter(&contents)
    {
        let name = captures.get(1).expect("env placeholder capture").as_str();
        if !name.starts_with(&config.kit.env.public_prefix) {
            return Err(ConfigError::InvalidTemplateEnvPrefix {
                relative: relative.to_string(),
                public_prefix: config.kit.env.public_prefix.clone(),
                name: name.to_string(),
            }
            .into());
        }
    }

    Ok(contents)
}

pub fn load_error_page(config: &ValidatedConfig) -> Result<String> {
    if config.kit.files.error_template.is_file() {
        return fs::read_to_string(&config.kit.files.error_template).map_err(Into::into);
    }

    Ok(include_str!("default-error.html").to_string())
}

impl ManifestConfig {
    pub fn from_validated_config(config: &ValidatedConfig, cwd: Utf8PathBuf) -> Self {
        Self {
            routes_dir: config.kit.files.routes.clone(),
            cwd: cwd.clone(),
            fallback_dir: cwd,
            params_dir: config.kit.files.params.clone(),
            assets_dir: config.kit.files.assets.clone(),
            hooks_client: config.kit.files.hooks.client.clone(),
            hooks_server: config.kit.files.hooks.server.clone(),
            hooks_universal: config.kit.files.hooks.universal.clone(),
            component_extensions: config.extensions.clone(),
            module_extensions: config.kit.module_extensions.clone(),
        }
    }
}

impl ValidatedServiceWorkerConfig {
    pub fn includes(&self, filename: &str) -> bool {
        match &self.files {
            ServiceWorkerFilesFilter::IgnoreDsStore => !filename.contains(".DS_Store"),
            ServiceWorkerFilesFilter::Source(_) => true,
        }
    }

    pub fn custom_filter_source(&self) -> Option<&str> {
        match &self.files {
            ServiceWorkerFilesFilter::Source(source) => Some(source.as_str()),
            ServiceWorkerFilesFilter::IgnoreDsStore => None,
        }
    }
}

impl PrerenderPolicy {
    pub fn custom_source(&self) -> Option<&str> {
        match self {
            Self::Source(source) => Some(source.as_str()),
            _ => None,
        }
    }
}

impl ValidatedAdapterConfig {
    pub fn adapt_source(&self) -> Option<&str> {
        self.source.as_ref().map(JsSource::as_str)
    }
}

impl ValidatedTypeScriptConfig {
    pub fn custom_config_source(&self) -> Option<&str> {
        match &self.config {
            TypeScriptConfigHook::Source(source) => Some(source.as_str()),
            TypeScriptConfigHook::Identity => None,
        }
    }
}

fn parse_kit_config(input: Option<&Value>, cwd: &Utf8Path) -> Result<ValidatedKitConfig> {
    let object = as_optional_object(input, "config.kit")?;
    reject_unknown_keys(
        object,
        "config.kit",
        &[
            "adapter",
            "alias",
            "appDir",
            "csp",
            "csrf",
            "embedded",
            "env",
            "experimental",
            "files",
            "inlineStyleThreshold",
            "moduleExtensions",
            "outDir",
            "output",
            "paths",
            "prerender",
            "router",
            "serviceWorker",
            "typescript",
            "version",
        ],
    )?;
    let app_dir = parse_app_dir(object.get("appDir"))?;
    let files = parse_files_config(object.get("files"), cwd)?;

    Ok(ValidatedKitConfig {
        adapter: parse_adapter_config(object.get("adapter"))?,
        alias: parse_string_map(object.get("alias"), "config.kit.alias")?,
        app_dir,
        csp: parse_csp_config(object.get("csp"))?,
        csrf: parse_csrf_config(object.get("csrf"))?,
        embedded: parse_bool(object.get("embedded"), "config.kit.embedded", false)?,
        env: parse_env_config(object.get("env"), cwd)?,
        experimental: parse_experimental_config(object.get("experimental"))?,
        files,
        inline_style_threshold: parse_u64(
            object.get("inlineStyleThreshold"),
            "config.kit.inlineStyleThreshold",
            0,
        )?,
        module_extensions: parse_string_array(
            object.get("moduleExtensions"),
            "config.kit.moduleExtensions",
            &[".js", ".ts"],
        )?,
        out_dir: resolve_path(
            cwd,
            parse_string(object.get("outDir"), "config.kit.outDir")?
                .as_deref()
                .unwrap_or(".svelte-kit"),
        ),
        output: parse_output_config(object.get("output"))?,
        paths: parse_paths_config(object.get("paths"))?,
        prerender: parse_prerender_config(object.get("prerender"))?,
        router: parse_router_config(object.get("router"))?,
        service_worker: parse_service_worker_config(object.get("serviceWorker"))?,
        typescript: parse_typescript_config(object.get("typescript"))?,
        version: parse_version_config(object.get("version"))?,
    })
}

fn parse_compiler_options(input: Option<&Value>) -> Result<ValidatedCompilerOptions> {
    let object = as_optional_object(input, "config.compilerOptions")?;
    let experimental = object
        .get("experimental")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    Ok(ValidatedCompilerOptions {
        experimental: ValidatedCompilerExperimentalOptions {
            async_: parse_bool(
                experimental.get("async"),
                "config.compilerOptions.experimental.async",
                false,
            )?,
        },
    })
}

fn parse_adapter_config(input: Option<&Value>) -> Result<Option<ValidatedAdapterConfig>> {
    match input {
        None | Some(Value::Null) => Ok(None),
        Some(value) if js_source_from_value(value).is_some() => Ok(Some(ValidatedAdapterConfig {
            raw: Map::new(),
            source: Some(js_source_from_value(value).expect("source marker checked")),
        })),
        Some(Value::Object(object)) => Ok(Some(ValidatedAdapterConfig {
            raw: object.clone(),
            source: object.get("adapt").and_then(js_source_from_value),
        })),
        Some(_) => Err(ConfigError::InvalidAdapter.into()),
    }
}

fn parse_env_config(input: Option<&Value>, cwd: &Utf8Path) -> Result<ValidatedEnvConfig> {
    let object = as_optional_object(input, "config.kit.env")?;
    reject_unknown_keys(
        object,
        "config.kit.env",
        &["dir", "publicPrefix", "privatePrefix"],
    )?;

    Ok(ValidatedEnvConfig {
        dir: parse_string(object.get("dir"), "config.kit.env.dir")?
            .unwrap_or_else(|| cwd.as_str().to_string()),
        public_prefix: parse_string(object.get("publicPrefix"), "config.kit.env.publicPrefix")?
            .unwrap_or_else(|| "PUBLIC_".to_string()),
        private_prefix: parse_string(object.get("privatePrefix"), "config.kit.env.privatePrefix")?
            .unwrap_or_default(),
    })
}

fn parse_csp_config(input: Option<&Value>) -> Result<ValidatedCspConfig> {
    let object = as_optional_object(input, "config.kit.csp")?;
    reject_unknown_keys(
        object,
        "config.kit.csp",
        &["mode", "directives", "reportOnly"],
    )?;

    Ok(ValidatedCspConfig {
        mode: match parse_enum(
            object.get("mode"),
            "config.kit.csp.mode",
            &["auto", "hash", "nonce"],
            "auto",
        )?
        .as_str()
        {
            "hash" => CspMode::Hash,
            "nonce" => CspMode::Nonce,
            _ => CspMode::Auto,
        },
        directives: parse_csp_directives(object.get("directives"), "config.kit.csp.directives")?,
        report_only: parse_csp_directives(object.get("reportOnly"), "config.kit.csp.reportOnly")?,
    })
}

fn parse_csrf_config(input: Option<&Value>) -> Result<ValidatedCsrfConfig> {
    let object = as_optional_object(input, "config.kit.csrf")?;
    reject_unknown_keys(
        object,
        "config.kit.csrf",
        &["checkOrigin", "trustedOrigins"],
    )?;

    Ok(ValidatedCsrfConfig {
        check_origin: parse_bool(
            object.get("checkOrigin"),
            "config.kit.csrf.checkOrigin",
            true,
        )?,
        trusted_origins: parse_string_array(
            object.get("trustedOrigins"),
            "config.kit.csrf.trustedOrigins",
            &[],
        )?,
    })
}

fn parse_csp_directives(input: Option<&Value>, keypath: &str) -> Result<ValidatedCspDirectives> {
    const STRING_LIST_KEYS: &[&str] = &[
        "child-src",
        "default-src",
        "frame-src",
        "worker-src",
        "connect-src",
        "font-src",
        "img-src",
        "manifest-src",
        "media-src",
        "object-src",
        "prefetch-src",
        "script-src",
        "script-src-elem",
        "script-src-attr",
        "style-src",
        "style-src-elem",
        "style-src-attr",
        "base-uri",
        "sandbox",
        "form-action",
        "frame-ancestors",
        "navigate-to",
        "report-uri",
        "report-to",
        "require-trusted-types-for",
        "trusted-types",
        "require-sri-for",
        "plugin-types",
        "referrer",
    ];

    let object = as_optional_object(input, keypath)?;
    let mut string_lists = std::collections::BTreeMap::new();

    for key in object.keys() {
        if STRING_LIST_KEYS.contains(&key.as_str())
            || matches!(
                key.as_str(),
                "upgrade-insecure-requests" | "block-all-mixed-content"
            )
        {
            continue;
        }

        return Err(ConfigError::unexpected_option(keypath, key, None).into());
    }

    for key in STRING_LIST_KEYS {
        let values = parse_string_array(object.get(*key), &format!("{keypath}.{key}"), &[])?;
        if !values.is_empty() {
            string_lists.insert((*key).to_string(), values);
        }
    }

    Ok(ValidatedCspDirectives {
        string_lists,
        upgrade_insecure_requests: parse_bool(
            object.get("upgrade-insecure-requests"),
            &format!("{keypath}.upgrade-insecure-requests"),
            false,
        )?,
        block_all_mixed_content: parse_bool(
            object.get("block-all-mixed-content"),
            &format!("{keypath}.block-all-mixed-content"),
            false,
        )?,
    })
}

fn parse_experimental_config(input: Option<&Value>) -> Result<ValidatedExperimentalConfig> {
    let object = as_optional_object(input, "config.kit.experimental")?;
    reject_unknown_keys(
        object,
        "config.kit.experimental",
        &[
            "tracing",
            "instrumentation",
            "remoteFunctions",
            "forkPreloads",
        ],
    )?;

    Ok(ValidatedExperimentalConfig {
        instrumentation: parse_instrumentation_config(object.get("instrumentation"))?,
        remote_functions: parse_bool(
            object.get("remoteFunctions"),
            "config.kit.experimental.remoteFunctions",
            false,
        )?,
        tracing: parse_tracing_config(object.get("tracing"))?,
        fork_preloads: parse_bool(
            object.get("forkPreloads"),
            "config.kit.experimental.forkPreloads",
            false,
        )?,
    })
}

fn parse_instrumentation_config(input: Option<&Value>) -> Result<ValidatedInstrumentationConfig> {
    let object = as_optional_object(input, "config.kit.experimental.instrumentation")?;
    reject_unknown_keys(
        object,
        "config.kit.experimental.instrumentation",
        &["server"],
    )?;

    Ok(ValidatedInstrumentationConfig {
        server: parse_bool(
            object.get("server"),
            "config.kit.experimental.instrumentation.server",
            false,
        )?,
    })
}

fn parse_tracing_config(input: Option<&Value>) -> Result<ValidatedTracingConfig> {
    if matches!(input, None | Some(Value::Null)) {
        return Ok(ValidatedTracingConfig { server: false });
    }

    let object = as_optional_object(input, "config.kit.experimental.tracing")?;
    reject_unknown_keys(object, "config.kit.experimental.tracing", &["server"])?;

    Ok(ValidatedTracingConfig {
        server: parse_bool(
            object.get("server"),
            "config.kit.experimental.tracing.server",
            false,
        )?,
    })
}

fn parse_files_config(input: Option<&Value>, cwd: &Utf8Path) -> Result<ValidatedFilesConfig> {
    let object = as_optional_object(input, "config.kit.files")?;

    for key in object.keys() {
        match key.as_str() {
            "src" | "assets" | "hooks" | "lib" | "params" | "routes" | "serviceWorker"
            | "appTemplate" | "errorTemplate" => {}
            _ => {
                return Err(ConfigError::unexpected_option("config.kit.files", key, None).into());
            }
        }
    }

    let src = resolve_path(
        cwd,
        parse_string(object.get("src"), "config.kit.files.src")?
            .as_deref()
            .unwrap_or("src"),
    );
    let assets = resolve_path(
        cwd,
        parse_string(object.get("assets"), "config.kit.files.assets")?
            .as_deref()
            .unwrap_or("static"),
    );
    let hooks = parse_hooks_config(object.get("hooks"), cwd, &src)?;
    let lib = parse_file_path(
        object.get("lib"),
        "config.kit.files.lib",
        cwd,
        &src.join("lib"),
    )?;
    let params = parse_file_path(
        object.get("params"),
        "config.kit.files.params",
        cwd,
        &src.join("params"),
    )?;
    let routes = parse_file_path(
        object.get("routes"),
        "config.kit.files.routes",
        cwd,
        &src.join("routes"),
    )?;
    let service_worker = parse_file_path(
        object.get("serviceWorker"),
        "config.kit.files.serviceWorker",
        cwd,
        &src.join("service-worker"),
    )?;
    let app_template = parse_file_path(
        object.get("appTemplate"),
        "config.kit.files.appTemplate",
        cwd,
        &src.join("app.html"),
    )?;
    let error_template = parse_file_path(
        object.get("errorTemplate"),
        "config.kit.files.errorTemplate",
        cwd,
        &src.join("error.html"),
    )?;

    Ok(ValidatedFilesConfig {
        src,
        assets,
        hooks,
        lib,
        params,
        routes,
        service_worker,
        app_template,
        error_template,
    })
}

fn parse_hooks_config(
    input: Option<&Value>,
    cwd: &Utf8Path,
    src: &Utf8Path,
) -> Result<ValidatedHooksConfig> {
    let object = as_optional_object(input, "config.kit.files.hooks")?;

    for key in object.keys() {
        match key.as_str() {
            "client" | "server" | "universal" => {}
            _ => {
                return Err(
                    ConfigError::unexpected_option("config.kit.files.hooks", key, None).into(),
                );
            }
        }
    }

    Ok(ValidatedHooksConfig {
        client: parse_file_path(
            object.get("client"),
            "config.kit.files.hooks.client",
            cwd,
            &src.join("hooks.client"),
        )?,
        server: parse_file_path(
            object.get("server"),
            "config.kit.files.hooks.server",
            cwd,
            &src.join("hooks.server"),
        )?,
        universal: parse_file_path(
            object.get("universal"),
            "config.kit.files.hooks.universal",
            cwd,
            &src.join("hooks"),
        )?,
    })
}

fn parse_output_config(input: Option<&Value>) -> Result<ValidatedOutputConfig> {
    let object = as_optional_object(input, "config.kit.output")?;
    reject_unknown_keys(
        object,
        "config.kit.output",
        &["preloadStrategy", "bundleStrategy"],
    )?;

    Ok(ValidatedOutputConfig {
        preload_strategy: match parse_enum(
            object.get("preloadStrategy"),
            "config.kit.output.preloadStrategy",
            &["modulepreload", "preload-js", "preload-mjs"],
            "modulepreload",
        )?
        .as_str()
        {
            "preload-js" => PreloadStrategy::PreloadJs,
            "preload-mjs" => PreloadStrategy::PreloadMjs,
            _ => PreloadStrategy::ModulePreload,
        },
        bundle_strategy: match parse_enum(
            object.get("bundleStrategy"),
            "config.kit.output.bundleStrategy",
            &["split", "single", "inline"],
            "split",
        )?
        .as_str()
        {
            "single" => BundleStrategy::Single,
            "inline" => BundleStrategy::Inline,
            _ => BundleStrategy::Split,
        },
    })
}

fn parse_paths_config(input: Option<&Value>) -> Result<ValidatedPathsConfig> {
    let object = as_optional_object(input, "config.kit.paths")?;
    reject_unknown_keys(object, "config.kit.paths", &["base", "assets", "relative"])?;
    let base = parse_string(object.get("base"), "config.kit.paths.base")?.unwrap_or_default();
    let assets = parse_string(object.get("assets"), "config.kit.paths.assets")?.unwrap_or_default();

    if !base.is_empty() && (base.ends_with('/') || !base.starts_with('/')) {
        return Err(ConfigError::invalid_value(
            "config.kit.paths.base option must either be the empty string or a root-relative path that starts but doesn't end with '/'. See https://svelte.dev/docs/kit/configuration#paths",
        )
        .into());
    }

    if !assets.is_empty() {
        if !Regex::new(r"^[a-z]+://")
            .expect("valid assets regex")
            .is_match(&assets)
        {
            return Err(ConfigError::invalid_value(
                "config.kit.paths.assets option must be an absolute path, if specified. See https://svelte.dev/docs/kit/configuration#paths",
            )
            .into());
        }

        if assets.ends_with('/') {
            return Err(ConfigError::invalid_value(
                "config.kit.paths.assets option must not end with '/'. See https://svelte.dev/docs/kit/configuration#paths",
            )
            .into());
        }
    }

    Ok(ValidatedPathsConfig {
        base,
        assets,
        relative: parse_bool(object.get("relative"), "config.kit.paths.relative", true)?,
    })
}

fn parse_prerender_config(input: Option<&Value>) -> Result<ValidatedPrerenderConfig> {
    let object = as_optional_object(input, "config.kit.prerender")?;
    reject_unknown_keys(
        object,
        "config.kit.prerender",
        &[
            "concurrency",
            "crawl",
            "entries",
            "handleHttpError",
            "handleMissingId",
            "handleEntryGeneratorMismatch",
            "handleUnseenRoutes",
            "origin",
        ],
    )?;
    let entries = parse_string_array(
        object.get("entries"),
        "config.kit.prerender.entries",
        &["*"],
    )?;

    for entry in &entries {
        if entry != "*" && !entry.starts_with('/') {
            return Err(ConfigError::invalid_value(format!(
                "Each member of config.kit.prerender.entries must be either '*' or an absolute path beginning with '/' — saw '{entry}'"
            ))
            .into());
        }
    }

    Ok(ValidatedPrerenderConfig {
        concurrency: parse_u64(
            object.get("concurrency"),
            "config.kit.prerender.concurrency",
            1,
        )?,
        crawl: parse_bool(object.get("crawl"), "config.kit.prerender.crawl", true)?,
        entries,
        handle_http_error: parse_prerender_policy(
            object.get("handleHttpError"),
            "config.kit.prerender.handleHttpError",
        )?,
        handle_missing_id: parse_prerender_policy(
            object.get("handleMissingId"),
            "config.kit.prerender.handleMissingId",
        )?,
        handle_entry_generator_mismatch: parse_prerender_policy(
            object.get("handleEntryGeneratorMismatch"),
            "config.kit.prerender.handleEntryGeneratorMismatch",
        )?,
        handle_unseen_routes: parse_prerender_policy(
            object.get("handleUnseenRoutes"),
            "config.kit.prerender.handleUnseenRoutes",
        )?,
        origin: parse_origin(object.get("origin"), "config.kit.prerender.origin")?,
    })
}

fn parse_router_config(input: Option<&Value>) -> Result<ValidatedRouterConfig> {
    let object = as_optional_object(input, "config.kit.router")?;
    reject_unknown_keys(object, "config.kit.router", &["type", "resolution"])?;

    Ok(ValidatedRouterConfig {
        type_: match parse_enum(
            object.get("type"),
            "config.kit.router.type",
            &["pathname", "hash"],
            "pathname",
        )?
        .as_str()
        {
            "hash" => RouterType::Hash,
            _ => RouterType::Pathname,
        },
        resolution: match parse_enum(
            object.get("resolution"),
            "config.kit.router.resolution",
            &["client", "server"],
            "client",
        )?
        .as_str()
        {
            "server" => RouterResolution::Server,
            _ => RouterResolution::Client,
        },
    })
}

fn parse_service_worker_config(input: Option<&Value>) -> Result<ValidatedServiceWorkerConfig> {
    let object = as_optional_object(input, "config.kit.serviceWorker")?;
    reject_unknown_keys(
        object,
        "config.kit.serviceWorker",
        &["register", "options", "files"],
    )?;
    let options = match object.get("options") {
        Some(Value::Object(options)) => Some(options.clone()),
        Some(_) => {
            return Err(ConfigError::ExpectedObject {
                keypath: "config.kit.serviceWorker.options".to_string(),
            }
            .into());
        }
        None => None,
    };
    let files = match object.get("files") {
        None => ServiceWorkerFilesFilter::IgnoreDsStore,
        Some(value) => {
            let Some(source) = js_source_from_value(value) else {
                return Err(ConfigError::ExpectedFunction {
                    keypath: "config.kit.serviceWorker.files".to_string(),
                }
                .into());
            };
            ServiceWorkerFilesFilter::Source(source)
        }
    };

    Ok(ValidatedServiceWorkerConfig {
        files,
        register: parse_bool(
            object.get("register"),
            "config.kit.serviceWorker.register",
            true,
        )?,
        options,
    })
}

fn parse_typescript_config(input: Option<&Value>) -> Result<ValidatedTypeScriptConfig> {
    let object = as_optional_object(input, "config.kit.typescript")?;
    reject_unknown_keys(object, "config.kit.typescript", &["config"])?;

    Ok(ValidatedTypeScriptConfig {
        config: match object.get("config") {
            None => TypeScriptConfigHook::Identity,
            Some(value) => {
                let Some(source) = js_source_from_value(value) else {
                    return Err(ConfigError::ExpectedFunction {
                        keypath: "config.kit.typescript.config".to_string(),
                    }
                    .into());
                };
                TypeScriptConfigHook::Source(source)
            }
        },
    })
}

fn parse_version_config(input: Option<&Value>) -> Result<ValidatedVersionConfig> {
    let object = as_optional_object(input, "config.kit.version")?;
    reject_unknown_keys(object, "config.kit.version", &["name", "pollInterval"])?;

    Ok(ValidatedVersionConfig {
        name: parse_string(object.get("name"), "config.kit.version.name")?
            .unwrap_or_else(default_version_name),
        poll_interval: parse_u64(
            object.get("pollInterval"),
            "config.kit.version.pollInterval",
            0,
        )?,
    })
}

fn parse_extensions(input: Option<&Value>, keypath: &str) -> Result<Vec<String>> {
    let extensions = parse_string_array(input, keypath, &[".svelte"])?;

    for extension in &extensions {
        if !extension.starts_with('.') {
            return Err(ConfigError::invalid_value(format!(
                "Each member of {keypath} must start with '.' — saw '{extension}'"
            ))
            .into());
        }

        if !Regex::new(r"^(\.[a-z0-9]+)+$")
            .expect("valid extensions regex")
            .is_match(extension)
        {
            return Err(ConfigError::invalid_value(format!(
                "File extensions must be alphanumeric — saw '{extension}'"
            ))
            .into());
        }
    }

    Ok(extensions)
}

fn parse_app_dir(input: Option<&Value>) -> Result<String> {
    let app_dir = parse_string(input, "config.kit.appDir")?.unwrap_or_else(|| "_app".to_string());

    if app_dir.is_empty() {
        return Err(ConfigError::invalid_value("config.kit.appDir cannot be empty").into());
    }

    if app_dir.starts_with('/') || app_dir.ends_with('/') {
        return Err(ConfigError::invalid_value(
            "config.kit.appDir cannot start or end with '/'. See https://svelte.dev/docs/kit/configuration",
        )
        .into());
    }

    Ok(app_dir)
}

fn parse_file_path(
    input: Option<&Value>,
    keypath: &str,
    cwd: &Utf8Path,
    fallback: &Utf8Path,
) -> Result<Utf8PathBuf> {
    Ok(match parse_string(input, keypath)? {
        Some(path) => resolve_path(cwd, &path),
        None => fallback.to_path_buf(),
    })
}

fn parse_string(input: Option<&Value>, keypath: &str) -> Result<Option<String>> {
    match input {
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(ConfigError::ExpectedString {
            keypath: keypath.to_string(),
        }
        .into()),
        None => Ok(None),
    }
}

fn parse_string_array(
    input: Option<&Value>,
    keypath: &str,
    fallback: &[&str],
) -> Result<Vec<String>> {
    match input {
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| match value {
                Value::String(value) => Ok(value.clone()),
                _ => Err(ConfigError::ExpectedStringArray {
                    keypath: keypath.to_string(),
                }
                .into()),
            })
            .collect(),
        Some(_) => Err(ConfigError::ExpectedStringArray {
            keypath: keypath.to_string(),
        }
        .into()),
        None => Ok(fallback.iter().map(|value| (*value).to_string()).collect()),
    }
}

fn parse_string_map(
    input: Option<&Value>,
    keypath: &str,
) -> Result<std::collections::BTreeMap<String, String>> {
    let object = as_optional_object(input, keypath)?;
    let mut map = std::collections::BTreeMap::new();

    for (key, value) in object {
        let Value::String(value) = value else {
            return Err(ConfigError::ExpectedStringMapValue {
                keypath: keypath.to_string(),
                key: key.clone(),
            }
            .into());
        };
        map.insert(key.clone(), value.clone());
    }

    Ok(map)
}

fn reject_unknown_keys(object: &Map<String, Value>, keypath: &str, allowed: &[&str]) -> Result<()> {
    for key in object.keys() {
        if allowed.contains(&key.as_str()) {
            continue;
        }

        return Err(ConfigError::unexpected_option(
            keypath,
            key,
            if keypath == "config.kit" && key == "extensions" {
                Some(" (did you mean config.extensions?)")
            } else {
                None
            },
        )
        .into());
    }

    Ok(())
}

fn js_source_from_value(value: &Value) -> Option<JsSource> {
    let Value::Object(object) = value else {
        return None;
    };
    let Value::String(source) = object.get(FUNCTION_SOURCE_MARKER_KEY)? else {
        return None;
    };
    let kind = object
        .get(FUNCTION_SOURCE_KIND_KEY)
        .and_then(Value::as_str)
        .map(JsSourceKind::from_marker)
        .unwrap_or(JsSourceKind::Expression);
    Some(JsSource::new(source.to_string(), kind))
}

fn parse_prerender_policy(input: Option<&Value>, keypath: &str) -> Result<PrerenderPolicy> {
    match input {
        None => Ok(PrerenderPolicy::Fail),
        Some(Value::String(value)) => Ok(match value.as_str() {
            "fail" => PrerenderPolicy::Fail,
            "warn" => PrerenderPolicy::Warn,
            "ignore" => PrerenderPolicy::Ignore,
            _ => {
                return Err(ConfigError::InvalidPrerenderPolicy {
                    keypath: keypath.to_string(),
                }
                .into());
            }
        }),
        Some(value) => js_source_from_value(value)
            .map(PrerenderPolicy::Source)
            .ok_or_else(|| {
                ConfigError::InvalidPrerenderPolicy {
                    keypath: keypath.to_string(),
                }
                .into()
            }),
    }
}

fn parse_bool(input: Option<&Value>, keypath: &str, fallback: bool) -> Result<bool> {
    match input {
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err(ConfigError::ExpectedBool {
            keypath: keypath.to_string(),
        }
        .into()),
        None => Ok(fallback),
    }
}

fn parse_u64(input: Option<&Value>, keypath: &str, fallback: u64) -> Result<u64> {
    match input {
        Some(Value::Number(value)) => value.as_u64().ok_or_else(|| {
            ConfigError::ExpectedNumber {
                keypath: keypath.to_string(),
            }
            .into()
        }),
        Some(_) => Err(ConfigError::ExpectedNumber {
            keypath: keypath.to_string(),
        }
        .into()),
        None => Ok(fallback),
    }
}

fn parse_enum(
    input: Option<&Value>,
    keypath: &str,
    choices: &[&str],
    fallback: &str,
) -> Result<String> {
    let value = parse_string(input, keypath)?.unwrap_or_else(|| fallback.to_string());
    if choices.contains(&value.as_str()) {
        return Ok(value);
    }

    let message = if choices.len() == 2 {
        format!(
            "{keypath} should be either \"{}\" or \"{}\"",
            choices[0], choices[1]
        )
    } else {
        format!(
            "{keypath} should be one of {} or \"{}\"",
            choices[..choices.len() - 1]
                .iter()
                .map(|choice| format!("\"{choice}\""))
                .collect::<Vec<_>>()
                .join(", "),
            choices[choices.len() - 1]
        )
    };

    Err(ConfigError::invalid_value(message).into())
}

fn as_optional_object<'a>(
    input: Option<&'a Value>,
    keypath: &str,
) -> Result<&'a Map<String, Value>> {
    match input {
        Some(Value::Object(object)) => Ok(object),
        Some(_) => Err(ConfigError::ExpectedObject {
            keypath: keypath.to_string(),
        }
        .into()),
        None => {
            static EMPTY: std::sync::OnceLock<Map<String, Value>> = std::sync::OnceLock::new();
            Ok(EMPTY.get_or_init(Map::new))
        }
    }
}

fn resolve_path(cwd: &Utf8Path, path: &str) -> Utf8PathBuf {
    let path = Utf8Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

fn parse_origin(input: Option<&Value>, keypath: &str) -> Result<String> {
    let origin =
        parse_string(input, keypath)?.unwrap_or_else(|| "http://sveltekit-prerender".to_string());

    let parsed = Url::parse(&origin).map_err(|_| ConfigError::InvalidOrigin {
        keypath: keypath.to_string(),
    })?;
    let normalized = parsed.origin().ascii_serialization();
    if origin != normalized {
        return Err(ConfigError::NormalizedOriginMismatch {
            keypath: keypath.to_string(),
            normalized,
            origin,
        }
        .into());
    }

    Ok(normalized)
}

fn default_version_name() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_millis()
        .to_string()
}

fn find_config_path(cwd: &Utf8Path) -> Option<Utf8PathBuf> {
    ["svelte.config.js", "svelte.config.ts"]
        .into_iter()
        .map(|name| cwd.join(name))
        .find(|path| path.is_file())
}

#[derive(Debug, Clone, Default)]
struct ParsedModule {
    declarations: std::collections::BTreeMap<String, Value>,
    named_exports: std::collections::BTreeMap<String, Value>,
    default_export: Option<Value>,
}

fn parse_config_source(path: &Utf8Path, source: &str) -> Result<Value> {
    let mut cache = std::collections::BTreeMap::new();
    let mut visiting = std::collections::BTreeSet::new();
    let module = parse_config_module(path, Some(source), &mut cache, &mut visiting)?;

    module
        .default_export
        .ok_or_else(|| ConfigError::DefaultExportObjectRequired.into())
}

fn parse_config_module(
    path: &Utf8Path,
    source_override: Option<&str>,
    cache: &mut std::collections::BTreeMap<Utf8PathBuf, ParsedModule>,
    visiting: &mut std::collections::BTreeSet<Utf8PathBuf>,
) -> Result<ParsedModule> {
    if let Some(module) = cache.get(path) {
        return Ok(module.clone());
    }

    let owned_path = path.to_path_buf();
    if !visiting.insert(owned_path.clone()) {
        return Err(ConfigError::CyclicImport {
            file: path.file_name().unwrap_or(path.as_str()).to_string(),
        }
        .into());
    }

    let source = match source_override {
        Some(source) => source.to_string(),
        None => fs::read_to_string(path)?,
    };
    let allocator = Allocator::default();
    let source_type = if path.extension() == Some("ts") || path.extension() == Some("mts") {
        SourceType::ts().with_module(true)
    } else {
        SourceType::mjs()
    };
    let parsed = Parser::new(&allocator, &source, source_type).parse();
    if !parsed.errors.is_empty() {
        return Err(ConfigError::ParseModule {
            file: path.file_name().unwrap_or("svelte.config.js").to_string(),
        }
        .into());
    }

    let mut module = ParsedModule::default();

    for statement in &parsed.program.body {
        match statement {
            Statement::ImportDeclaration(declaration) => {
                populate_imports(path, declaration, cache, visiting, &mut module.declarations)?;
            }
            Statement::VariableDeclaration(declaration) => {
                for declarator in &declaration.declarations {
                    collect_variable_declaration(&source, declarator, &mut module.declarations);
                }
            }
            Statement::FunctionDeclaration(declaration) => {
                if let Some(identifier) = declaration.id.as_ref() {
                    module.declarations.insert(
                        identifier.name.as_str().to_string(),
                        function_source_marker(&source, declaration.span, JsSourceKind::Function),
                    );
                }
            }
            Statement::ExportNamedDeclaration(declaration) => {
                if let Some(inner) = declaration.declaration.as_ref() {
                    collect_declared_exports(&source, inner, &mut module.declarations);
                }
            }
            _ => {}
        }
    }

    for statement in &parsed.program.body {
        match statement {
            Statement::ExportNamedDeclaration(declaration) => {
                populate_named_exports(
                    path,
                    declaration,
                    cache,
                    visiting,
                    &module.declarations,
                    &mut module.named_exports,
                )?;
            }
            Statement::ExportAllDeclaration(declaration) => {
                populate_export_all(
                    path,
                    declaration,
                    cache,
                    visiting,
                    &mut module.named_exports,
                )?;
            }
            Statement::ExportDefaultDeclaration(declaration) => {
                module.default_export =
                    export_default_value(&source, &declaration.declaration, &module.declarations);
            }
            _ => {}
        }
    }

    if module.default_export.is_none() {
        module.default_export = module.named_exports.get("default").cloned();
    }

    visiting.remove(&owned_path);
    cache.insert(owned_path, module.clone());
    Ok(module)
}

fn export_default_value(
    source: &str,
    declaration: &ExportDefaultDeclarationKind<'_>,
    declarations: &std::collections::BTreeMap<String, Value>,
) -> Option<Value> {
    if let Some(expression) = declaration.as_expression() {
        expression_to_json(source, expression, declarations)
    } else {
        match declaration {
            ExportDefaultDeclarationKind::FunctionDeclaration(_)
            | ExportDefaultDeclarationKind::ClassDeclaration(_) => None,
            _ => None,
        }
    }
}

fn expression_to_json(
    source: &str,
    expression: &Expression<'_>,
    declarations: &std::collections::BTreeMap<String, Value>,
) -> Option<Value> {
    match expression {
        Expression::ParenthesizedExpression(expression) => {
            expression_to_json(source, &expression.expression, declarations)
        }
        Expression::BooleanLiteral(value) => Some(Value::Bool(value.value)),
        Expression::Identifier(identifier) => declarations
            .get(identifier.name.as_str())
            .cloned()
            .or_else(|| {
                Some(function_source_marker(
                    source,
                    identifier.span,
                    JsSourceKind::IdentifierReference,
                ))
            }),
        Expression::NullLiteral(_) => Some(Value::Null),
        Expression::NumericLiteral(value) => {
            serde_json::Number::from_f64(value.value).map(Value::Number)
        }
        Expression::StringLiteral(value) => Some(Value::String(value.value.to_string())),
        Expression::TSAsExpression(expression) => {
            expression_to_json(source, &expression.expression, declarations)
        }
        Expression::TSSatisfiesExpression(expression) => {
            expression_to_json(source, &expression.expression, declarations)
        }
        Expression::TSNonNullExpression(expression) => {
            expression_to_json(source, &expression.expression, declarations)
        }
        Expression::TSInstantiationExpression(expression) => {
            expression_to_json(source, &expression.expression, declarations)
        }
        Expression::ArrowFunctionExpression(value) => Some(function_source_marker(
            source,
            value.span,
            JsSourceKind::Function,
        )),
        Expression::FunctionExpression(value) => Some(function_source_marker(
            source,
            value.span,
            JsSourceKind::Function,
        )),
        Expression::TemplateLiteral(value)
            if value.expressions.is_empty() && value.quasis.len() == 1 =>
        {
            Some(Value::String(
                value.quasis[0].value.cooked.as_ref()?.to_string(),
            ))
        }
        Expression::ArrayExpression(array) => {
            let mut values = Vec::with_capacity(array.elements.len());
            for element in &array.elements {
                let expression = element.as_expression()?;
                values.push(expression_to_json(source, expression, declarations)?);
            }
            Some(Value::Array(values))
        }
        Expression::StaticMemberExpression(expression) => resolve_static_member_expression(
            source,
            &expression.object,
            expression.property.name.as_str(),
            expression.span,
            declarations,
        ),
        Expression::ComputedMemberExpression(expression) => {
            let Value::String(property) =
                expression_to_json(source, &expression.expression, declarations)?
            else {
                return None;
            };
            resolve_static_member_expression(
                source,
                &expression.object,
                &property,
                expression.span,
                declarations,
            )
        }
        Expression::ObjectExpression(object) => {
            let mut entries = Map::new();
            for property in &object.properties {
                match property {
                    ObjectPropertyKind::ObjectProperty(property) => {
                        if property.computed {
                            return None;
                        }
                        let key = property_key_name(&property.key)?;
                        let value = if property.method {
                            function_source_marker(source, property.span, JsSourceKind::Method)
                        } else {
                            expression_to_json(source, &property.value, declarations)?
                        };
                        entries.insert(key, value);
                    }
                    ObjectPropertyKind::SpreadProperty(property) => {
                        let Value::Object(spread_entries) =
                            expression_to_json(source, &property.argument, declarations)?
                        else {
                            return None;
                        };
                        entries.extend(spread_entries);
                    }
                }
            }
            Some(Value::Object(entries))
        }
        Expression::CallExpression(expression) => Some(function_source_marker(
            source,
            expression.span,
            JsSourceKind::CallExpression,
        )),
        _ => None,
    }
}

fn resolve_static_member_expression(
    source: &str,
    object: &Expression<'_>,
    property: &str,
    span: Span,
    declarations: &std::collections::BTreeMap<String, Value>,
) -> Option<Value> {
    let object = expression_to_json(source, object, declarations)?;
    match object {
        Value::Object(entries) => entries.get(property).cloned().or_else(|| {
            Some(function_source_marker(
                source,
                span,
                JsSourceKind::Expression,
            ))
        }),
        Value::Array(values) => property
            .parse::<usize>()
            .ok()
            .and_then(|index| values.get(index).cloned())
            .or_else(|| {
                Some(function_source_marker(
                    source,
                    span,
                    JsSourceKind::Expression,
                ))
            }),
        _ => Some(function_source_marker(
            source,
            span,
            JsSourceKind::Expression,
        )),
    }
}

fn collect_variable_declaration(
    source: &str,
    declarator: &VariableDeclarator<'_>,
    declarations: &mut std::collections::BTreeMap<String, Value>,
) {
    let Some(init) = declarator.init.as_ref() else {
        return;
    };

    let value = expression_to_json(source, init, declarations)
        .unwrap_or_else(|| function_source_marker(source, init.span(), JsSourceKind::Expression));
    collect_binding_values(source, &declarator.id, value, declarations);
}

fn collect_declared_exports(
    source: &str,
    declaration: &Declaration<'_>,
    declarations: &mut std::collections::BTreeMap<String, Value>,
) {
    match declaration {
        Declaration::VariableDeclaration(declaration) => {
            for declarator in &declaration.declarations {
                collect_variable_declaration(source, declarator, declarations);
            }
        }
        Declaration::FunctionDeclaration(declaration) => {
            if let Some(identifier) = declaration.id.as_ref() {
                declarations.insert(
                    identifier.name.as_str().to_string(),
                    function_source_marker(source, declaration.span, JsSourceKind::Function),
                );
            }
        }
        _ => {}
    }
}

fn populate_named_exports(
    path: &Utf8Path,
    declaration: &oxc_ast::ast::ExportNamedDeclaration<'_>,
    cache: &mut std::collections::BTreeMap<Utf8PathBuf, ParsedModule>,
    visiting: &mut std::collections::BTreeSet<Utf8PathBuf>,
    declarations: &std::collections::BTreeMap<String, Value>,
    named_exports: &mut std::collections::BTreeMap<String, Value>,
) -> Result<()> {
    if let Some(inner) = declaration.declaration.as_ref() {
        match inner {
            Declaration::VariableDeclaration(declaration) => {
                for declarator in &declaration.declarations {
                    let mut names = Vec::new();
                    binding_names(&declarator.id, &mut names);
                    for name in names {
                        if let Some(value) = declarations.get(&name).cloned() {
                            named_exports.insert(name, value);
                        }
                    }
                }
            }
            Declaration::FunctionDeclaration(declaration) => {
                if let Some(identifier) = declaration.id.as_ref() {
                    let name = identifier.name.as_str().to_string();
                    if let Some(value) = declarations.get(&name).cloned() {
                        named_exports.insert(name, value);
                    }
                }
            }
            _ => {}
        }
        return Ok(());
    }

    if let Some(export_source) = declaration.source.as_ref() {
        let Some(target) = resolve_relative_config_module(path, export_source.value.as_str())
        else {
            return Ok(());
        };
        let imported_module = parse_config_module(&target, None, cache, visiting)?;
        for specifier in &declaration.specifiers {
            let Some(exported) = module_export_name(&specifier.exported) else {
                continue;
            };
            let Some(local) = module_export_name(&specifier.local) else {
                continue;
            };
            let value = if local == "default" {
                imported_module.default_export.clone()
            } else {
                imported_module.named_exports.get(&local).cloned()
            };
            if let Some(value) = value {
                named_exports.insert(exported, value);
            }
        }
        return Ok(());
    }

    for specifier in &declaration.specifiers {
        let Some(exported) = module_export_name(&specifier.exported) else {
            continue;
        };
        let Some(local) = module_export_name(&specifier.local) else {
            continue;
        };
        if let Some(value) = declarations.get(&local).cloned() {
            named_exports.insert(exported, value);
        }
    }

    Ok(())
}

fn populate_imports(
    path: &Utf8Path,
    declaration: &oxc_ast::ast::ImportDeclaration<'_>,
    cache: &mut std::collections::BTreeMap<Utf8PathBuf, ParsedModule>,
    visiting: &mut std::collections::BTreeSet<Utf8PathBuf>,
    declarations: &mut std::collections::BTreeMap<String, Value>,
) -> Result<()> {
    let Some(target) = resolve_relative_config_module(path, declaration.source.value.as_str())
    else {
        return Ok(());
    };
    let imported_module = parse_config_module(&target, None, cache, visiting)?;

    for specifier in declaration.specifiers.as_ref().into_iter().flatten() {
        match specifier {
            ImportDeclarationSpecifier::ImportDefaultSpecifier(specifier) => {
                if let Some(value) = imported_module.default_export.clone() {
                    declarations.insert(specifier.local.name.as_str().to_string(), value);
                }
            }
            ImportDeclarationSpecifier::ImportSpecifier(specifier) => {
                let Some(imported) = module_export_name(&specifier.imported) else {
                    continue;
                };
                let value = if imported == "default" {
                    imported_module.default_export.clone()
                } else {
                    imported_module.named_exports.get(&imported).cloned()
                };
                if let Some(value) = value {
                    declarations.insert(specifier.local.name.as_str().to_string(), value);
                }
            }
            ImportDeclarationSpecifier::ImportNamespaceSpecifier(specifier) => {
                let mut namespace = Map::new();
                for (key, value) in &imported_module.named_exports {
                    namespace.insert(key.clone(), value.clone());
                }
                if let Some(default_export) = imported_module.default_export.clone() {
                    namespace.insert("default".to_string(), default_export);
                }
                declarations.insert(
                    specifier.local.name.as_str().to_string(),
                    Value::Object(namespace),
                );
            }
        }
    }

    Ok(())
}

fn populate_export_all(
    path: &Utf8Path,
    declaration: &oxc_ast::ast::ExportAllDeclaration<'_>,
    cache: &mut std::collections::BTreeMap<Utf8PathBuf, ParsedModule>,
    visiting: &mut std::collections::BTreeSet<Utf8PathBuf>,
    named_exports: &mut std::collections::BTreeMap<String, Value>,
) -> Result<()> {
    let Some(target) = resolve_relative_config_module(path, declaration.source.value.as_str())
    else {
        return Ok(());
    };
    let imported_module = parse_config_module(&target, None, cache, visiting)?;

    if let Some(exported) = declaration.exported.as_ref().and_then(module_export_name) {
        let mut namespace = Map::new();
        for (key, value) in &imported_module.named_exports {
            namespace.insert(key.clone(), value.clone());
        }
        if let Some(default_export) = imported_module.default_export.clone() {
            namespace.insert("default".to_string(), default_export);
        }
        named_exports.insert(exported, Value::Object(namespace));
        return Ok(());
    }

    for (key, value) in &imported_module.named_exports {
        named_exports.insert(key.clone(), value.clone());
    }

    Ok(())
}

fn resolve_relative_config_module(path: &Utf8Path, specifier: &str) -> Option<Utf8PathBuf> {
    if !specifier.starts_with("./") && !specifier.starts_with("../") {
        return None;
    }

    let base = path.parent()?;
    let candidate = base.join(specifier);
    let mut candidates = Vec::new();

    if candidate.is_file() {
        candidates.push(candidate.clone());
    }

    if candidate.extension().is_none() {
        for extension in ["ts", "js", "mts", "mjs"] {
            candidates.push(Utf8PathBuf::from(format!("{candidate}.{extension}")));
        }
        for extension in ["ts", "js", "mts", "mjs"] {
            candidates.push(candidate.join(format!("index.{extension}")));
        }
    }

    candidates.into_iter().find(|candidate| candidate.is_file())
}

fn binding_names(pattern: &BindingPattern<'_>, names: &mut Vec<String>) {
    match pattern {
        BindingPattern::BindingIdentifier(identifier) => {
            names.push(identifier.name.as_str().to_string());
        }
        BindingPattern::AssignmentPattern(pattern) => binding_names(&pattern.left, names),
        BindingPattern::ObjectPattern(pattern) => {
            for property in &pattern.properties {
                binding_names(&property.value, names);
            }
            if let Some(rest) = pattern.rest.as_ref() {
                binding_names(&rest.argument, names);
            }
        }
        BindingPattern::ArrayPattern(pattern) => {
            for element in &pattern.elements {
                let Some(element) = element.as_ref() else {
                    continue;
                };
                binding_names(element, names);
            }
            if let Some(rest) = pattern.rest.as_ref() {
                binding_names(&rest.argument, names);
            }
        }
    }
}

fn collect_binding_values(
    source: &str,
    pattern: &BindingPattern<'_>,
    value: Value,
    declarations: &mut std::collections::BTreeMap<String, Value>,
) {
    match pattern {
        BindingPattern::BindingIdentifier(identifier) => {
            declarations.insert(identifier.name.as_str().to_string(), value);
        }
        BindingPattern::AssignmentPattern(pattern) => {
            let value = if value.is_null() {
                expression_to_json(source, &pattern.right, declarations).unwrap_or_else(|| {
                    function_source_marker(source, pattern.right.span(), JsSourceKind::Expression)
                })
            } else {
                value
            };
            collect_binding_values(source, &pattern.left, value, declarations);
        }
        BindingPattern::ObjectPattern(pattern) => {
            let Value::Object(entries) = value else {
                return;
            };
            let mut rest_entries = entries.clone();
            for property in &pattern.properties {
                if property.computed {
                    return;
                }
                let Some(key) = property_key_name(&property.key) else {
                    return;
                };
                let property_value = entries.get(&key).cloned().unwrap_or(Value::Null);
                rest_entries.remove(&key);
                collect_binding_values(source, &property.value, property_value, declarations);
            }
            if let Some(rest) = pattern.rest.as_ref() {
                collect_binding_values(
                    source,
                    &rest.argument,
                    Value::Object(rest_entries),
                    declarations,
                );
            }
        }
        BindingPattern::ArrayPattern(pattern) => {
            let Value::Array(values) = value else {
                return;
            };
            for (index, element) in pattern.elements.iter().enumerate() {
                let Some(element) = element.as_ref() else {
                    continue;
                };
                let element_value = values.get(index).cloned().unwrap_or(Value::Null);
                collect_binding_values(source, element, element_value, declarations);
            }
            if let Some(rest) = pattern.rest.as_ref() {
                let rest_values = values.into_iter().skip(pattern.elements.len()).collect();
                collect_binding_values(
                    source,
                    &rest.argument,
                    Value::Array(rest_values),
                    declarations,
                );
            }
        }
    }
}

fn function_source_marker(source: &str, span: Span, kind: JsSourceKind) -> Value {
    let start = span.start as usize;
    let end = span.end as usize;
    Value::Object(Map::from_iter([
        (
            FUNCTION_SOURCE_MARKER_KEY.to_string(),
            Value::String(source[start..end].to_string()),
        ),
        (
            FUNCTION_SOURCE_KIND_KEY.to_string(),
            Value::String(kind.as_marker().to_string()),
        ),
    ]))
}

fn module_export_name(name: &ModuleExportName<'_>) -> Option<String> {
    match name {
        ModuleExportName::IdentifierName(name) => Some(name.name.as_str().to_string()),
        ModuleExportName::IdentifierReference(name) => Some(name.name.as_str().to_string()),
        ModuleExportName::StringLiteral(name) => Some(name.value.as_str().to_string()),
    }
}

fn property_key_name(key: &PropertyKey<'_>) -> Option<String> {
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
