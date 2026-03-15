use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Adapt(#[from] AdaptError),
    #[error(transparent)]
    Analyze(#[from] AnalyzeError),
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Cookie(#[from] CookieError),
    #[error(transparent)]
    ExportValidation(#[from] ExportValidationError),
    #[error(transparent)]
    ExportsInternal(#[from] ExportsInternalError),
    #[error(transparent)]
    ExportsNode(#[from] ExportsNodeError),
    #[error(transparent)]
    ExportsPublic(#[from] ExportsPublicError),
    #[error(transparent)]
    Feature(#[from] FeatureError),
    #[error(transparent)]
    Form(#[from] FormError),
    #[error(transparent)]
    Fork(#[from] ForkError),
    #[error(transparent)]
    GenerateManifest(#[from] GenerateManifestError),
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    #[error(transparent)]
    PeerImport(#[from] PeerImportError),
    #[error(transparent)]
    Postbuild(#[from] PostbuildError),
    #[error(transparent)]
    Prerender(#[from] PrerenderError),
    #[error(transparent)]
    Request(#[from] RequestError),
    #[error(transparent)]
    RequestStore(#[from] RequestStoreError),
    #[error(transparent)]
    Routing(#[from] RoutingError),
    #[error(transparent)]
    Syntax(#[from] SyntaxError),
    #[error(transparent)]
    RuntimeLoad(#[from] RuntimeLoadError),
    #[error(transparent)]
    RuntimeApp(#[from] RuntimeAppError),
    #[error(transparent)]
    RuntimeEndpoint(#[from] RuntimeEndpointError),
    #[error(transparent)]
    RuntimeCsp(#[from] RuntimeCspError),
    #[error(transparent)]
    RuntimeRemote(#[from] RuntimeRemoteError),
    #[error(transparent)]
    RuntimeShared(#[from] RuntimeSharedError),
    #[error(transparent)]
    Telemetry(#[from] TelemetryError),
    #[error(transparent)]
    Url(#[from] UrlError),
    #[error(transparent)]
    Utility(#[from] UtilityError),
    #[error(transparent)]
    ViteBuild(#[from] ViteBuildError),
    #[error(transparent)]
    ViteBuildUtils(#[from] ViteBuildUtilsError),
    #[error(transparent)]
    ViteGuard(#[from] ViteGuardError),
    #[error(transparent)]
    ViteUtils(#[from] ViteUtilsError),
    #[error(transparent)]
    RuntimePage(#[from] RuntimePageError),
    #[error(transparent)]
    RuntimeHttp(#[from] RuntimeHttpError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid utf-8 path under routes tree")]
    InvalidUtf8Path,
    #[error("invalid route regex for {route_id}: {source}")]
    Regex {
        route_id: String,
        #[source]
        source: regex::Error,
    },
    #[error("Multiple {kind} files found in {directory} : {existing_name} and {incoming_name}")]
    DuplicateRouteFile {
        kind: &'static str,
        directory: String,
        existing_name: String,
        incoming_name: String,
    },
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum AdaptError {
    #[error("Cannot adapt without a configured adapter")]
    MissingConfiguredAdapter,
    #[error("Instrumentation file {path} not found. This is probably a bug in your adapter.")]
    MissingInstrumentationFile { path: String },
    #[error("Entrypoint file {path} not found. This is probably a bug in your adapter.")]
    MissingEntrypointFile { path: String },
}

#[derive(Debug, Error)]
pub enum AnalyzeError {
    #[error(
        "`{name}` exported from remote chunk {hash} is invalid — all exports from this file must be remote functions"
    )]
    InvalidRemoteExport { name: String, hash: String },
    #[error("Cannot prerender a route with both +page and +server files ({route_id})")]
    PrerenderPageAndEndpointConflict { route_id: String },
    #[error(
        "Mismatched route config for {route_id} — the +page and +server files must export the same config, if any"
    )]
    MismatchedRouteConfig { route_id: String },
    #[error("missing leaf node for route {route_id}")]
    MissingLeafNode { route_id: String },
    #[error("missing layout node {layout_index} for route {route_id}")]
    MissingLayoutNode {
        layout_index: usize,
        route_id: String,
    },
    #[error("Cannot prerender a +server file with POST, PATCH, PUT, or DELETE ({route_id})")]
    MutativePrerenderEndpoint { route_id: String },
    #[error("failed to parse {path}")]
    ParseModule { path: String },
    #[error("unsupported export * in {path} during static analysis")]
    UnsupportedExportAll { path: String },
}

#[derive(Debug, Error)]
pub enum ExportValidationError {
    #[error("Invalid export '{key}' ({hint})")]
    InvalidExport { key: String, hint: String },
    #[error("Invalid export '{key}' in {path} ({hint})")]
    InvalidExportInPath {
        key: String,
        path: String,
        hint: String,
    },
}

#[derive(Debug, Error)]
pub enum ExportsInternalError {
    #[error(
        "Cannot export `default` from a remote module ({file}) — please use named exports instead"
    )]
    DefaultRemoteExport { file: String },
    #[error(
        "`{name}` exported from {file} is invalid — all exports from this file must be remote functions"
    )]
    InvalidRemoteExport { name: String, file: String },
}

#[derive(Debug, Error)]
pub enum ExportsNodeError {
    #[error("Content-length of {length} exceeds limit of {limit} bytes.")]
    BodyLimitExceeded { length: usize, limit: usize },
}

#[derive(Debug, Error)]
pub enum ExportsPublicError {
    #[error("normalize_url expects an absolute URL: {input} ({message})")]
    NormalizeUrl { input: String, message: String },
    #[error("failed to denormalize URL from {base} with {next}: {message}")]
    DenormalizeUrl {
        base: String,
        next: String,
        message: String,
    },
}

#[derive(Debug, Error)]
pub enum FeatureError {
    #[error(
        "Cannot use `read` from `$app/server` in {route_id} when using {adapter_name}. Please ensure that your adapter is up to date and supports this feature."
    )]
    UnsupportedRead {
        route_id: String,
        adapter_name: String,
    },
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error(
        "The Svelte config file must have a configuration object as its default export. See https://svelte.dev/docs/kit/configuration"
    )]
    DefaultExportObjectRequired,
    #[error("Unexpected option {keypath}.{key}{hint}")]
    UnexpectedOption {
        keypath: String,
        key: String,
        hint: String,
    },
    #[error("{keypath} should be an object")]
    ExpectedObject { keypath: String },
    #[error("{keypath} should be a string, if specified")]
    ExpectedString { keypath: String },
    #[error("{keypath} should be true or false, if specified")]
    ExpectedBool { keypath: String },
    #[error("{keypath} should be a number, if specified")]
    ExpectedNumber { keypath: String },
    #[error("{keypath} must be an array of strings, if specified")]
    ExpectedStringArray { keypath: String },
    #[error("{keypath}.{key} should be a string, if specified")]
    ExpectedStringMapValue { keypath: String, key: String },
    #[error("{relative} does not exist")]
    MissingFile { relative: String },
    #[error("{relative} is missing {tag}")]
    MissingTemplateTag { relative: String, tag: &'static str },
    #[error(
        "Environment variables in {relative} must start with {public_prefix} (saw %sveltekit.env.{name}%)"
    )]
    InvalidTemplateEnvPrefix {
        relative: String,
        public_prefix: String,
        name: String,
    },
    #[error(
        "config.kit.adapter should be an object with an \"adapt\" method. See https://svelte.dev/docs/kit/adapters"
    )]
    InvalidAdapter,
    #[error("{keypath} should be a function, if specified")]
    ExpectedFunction { keypath: String },
    #[error("{keypath} should be \"fail\", \"warn\", \"ignore\" or a custom function")]
    InvalidPrerenderPolicy { keypath: String },
    #[error("{keypath} must be a valid origin")]
    InvalidOrigin { keypath: String },
    #[error("{keypath} must be a valid origin ({normalized} rather than {origin})")]
    NormalizedOriginMismatch {
        keypath: String,
        normalized: String,
        origin: String,
    },
    #[error("Could not resolve cyclic config import involving {file}")]
    CyclicImport { file: String },
    #[error("Could not parse {file}")]
    ParseModule { file: String },
    #[error("{message}")]
    InvalidValue { message: String },
}

impl ConfigError {
    pub fn invalid_value(message: impl Into<String>) -> Self {
        Self::InvalidValue {
            message: message.into(),
        }
    }

    pub fn unexpected_option(keypath: &str, key: &str, hint: Option<&str>) -> Self {
        Self::UnexpectedOption {
            keypath: keypath.to_string(),
            key: key.to_string(),
            hint: hint.unwrap_or_default().to_string(),
        }
    }
}

#[derive(Debug, Error)]
pub enum FormError {
    #[error("{message}")]
    InvalidUtf8FileText { message: String },
    #[error("Invalid path {path}")]
    InvalidPath { path: String },
    #[error("Invalid number {text}")]
    InvalidNumber { text: String },
    #[error("Numeric form fields cannot contain files")]
    NumericFieldCannotContainFiles,
    #[error("Boolean form fields cannot contain files")]
    BooleanFieldCannotContainFiles,
    #[error("Form cannot contain duplicated keys — \"{key}\" has {count} values")]
    DuplicatedKey { key: String, count: usize },
    #[error("{message}")]
    Serialization { message: String },
    #[error("missing form data")]
    MissingFormData,
    #[error("Invalid array key {key}")]
    InvalidArrayKey { key: String },
    #[error("Invalid path segment {segment}")]
    InvalidPathSegment { segment: String },
    #[error("Invalid key \"{key}\": This key is not allowed to prevent prototype pollution.")]
    PrototypePollutionKey { key: String },
    #[error("Could not deserialize binary form: {message}")]
    BinaryDeserialize { message: String },
}

#[derive(Debug, Error)]
pub enum ForkError {
    #[error("forked task failed: {message}")]
    PanicMessage { message: String },
    #[error("forked task failed: thread panicked")]
    ThreadPanicked,
}

#[derive(Debug, Error)]
pub enum CookieError {
    #[error("You must specify a `path` when setting, deleting or serializing cookies")]
    MissingPath,
    #[error("Cannot serialize cookies until after the route is determined")]
    MissingNormalizedUrl,
    #[error("Cookie \"{name}\" is too large, and will be discarded by the browser")]
    OversizedCookie { name: String },
}

#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("No matcher found for parameter '{matcher}' in route {route_id}")]
    MissingMatcher { matcher: String, route_id: String },
    #[error(
        "No routes found. If you are using a custom src/routes directory, make sure it is specified in your Svelte config file"
    )]
    NoRoutesFound,
    #[error(
        "Matcher names can only have underscores and alphanumeric characters — \"{file_name}\" is invalid"
    )]
    InvalidMatcherName { file_name: String },
    #[error("Duplicate matchers: {incoming} and {existing}")]
    DuplicateMatchers { incoming: String, existing: String },
    #[error("Files and directories prefixed with + are reserved (saw {path})")]
    ReservedPlusPath { path: String },
    #[error(
        "Only Svelte files can reference named layouts. Remove '@' from {display_name} (at {path})"
    )]
    NamedLayoutRequiresSvelte { display_name: String, path: String },
    #[error("{path} is not under {root}")]
    PathNotUnderRoot { path: String, root: String },
    #[error("missing root route")]
    MissingRootRoute,
    #[error("{source_path} references missing segment \"{target_segment}\"")]
    MissingNamedLayoutSegment {
        source_path: String,
        target_segment: String,
    },
    #[error("Invalid route {route_id} — parameters must be separated")]
    AdjacentParams { route_id: String },
    #[error("Invalid route {route_id} — brackets are unbalanced")]
    UnbalancedBrackets { route_id: String },
    #[error(
        "Invalid route {route_id} — a rest route segment is always optional, remove the outer square brackets"
    )]
    OptionalRestSegment { route_id: String },
    #[error(
        "Invalid route {route_id} — an [[optional]] route segment cannot follow a [...rest] route segment"
    )]
    OptionalAfterRest { route_id: String },
    #[error("Character escape sequence in {route_id} must be lowercase")]
    UppercaseEscape { route_id: String },
    #[error("Invalid character escape sequence in {route_id}")]
    InvalidEscape { route_id: String },
    #[error("Hexadecimal escape sequence in {route_id} must be two characters")]
    InvalidHexEscapeLength { route_id: String },
    #[error("Unicode escape sequence in {route_id} must be between four and six characters")]
    InvalidUnicodeEscapeLength { route_id: String },
    #[error("Route {route_id} should be renamed to {suggested}")]
    HashInSegment { route_id: String, suggested: String },
    #[error("The \"{existing}\" and \"{incoming}\" routes conflict with each other")]
    RouteConflict { existing: String, incoming: String },
}

#[derive(Debug, Error)]
pub enum PeerImportError {
    #[error("failed to read {path}: {message}")]
    ReadPackageJson { path: String, message: String },
    #[error("failed to parse {path}: {message}")]
    ParsePackageJson { path: String, message: String },
    #[error("Could not find valid \"{subpackage}\" export in {package_name}/package.json")]
    MissingExport {
        package_name: String,
        subpackage: String,
    },
    #[error(
        "Could not resolve peer dependency \"{package_name}\" relative to your project — please install it and try again."
    )]
    UnresolvedDependency { package_name: String },
}

#[derive(Debug, Error)]
pub enum PostbuildError {
    #[error("Failed to resolve base URL {base}: {message}")]
    ResolveBaseUrl { base: String, message: String },
    #[error("Failed to resolve URL {path}: {message}")]
    ResolveUrl { path: String, message: String },
    #[error("Failed to decode URI: {value}\nincomplete percent-encoding")]
    IncompletePercentEncoding { value: String },
    #[error("Failed to decode URI: {value}\ninvalid percent-encoding {hex}: {message}")]
    InvalidPercentEncoding {
        value: String,
        hex: String,
        message: String,
    },
    #[error("Failed to decode URI: {value}\ninvalid utf-8 after decoding: {message}")]
    InvalidDecodedUriUtf8 { value: String, message: String },
}

#[derive(Debug, Error)]
pub enum PrerenderError {
    #[error(
        "The entries export from {generated_from_id} generated entry {entry}, which was matched by {matched_id} - see the `handleEntryGeneratorMismatch` option in https://svelte.dev/docs/kit/configuration#prerender for more info.\nTo suppress or handle this error, implement `handleEntryGeneratorMismatch` in https://svelte.dev/docs/kit/configuration#prerender"
    )]
    EntryGeneratorMismatch {
        generated_from_id: String,
        entry: String,
        matched_id: String,
    },
    #[error(
        "The following routes were marked as prerenderable, but were not prerendered because they were not found while crawling your app:\n{routes}\n\nSee the `handleUnseenRoutes` option in https://svelte.dev/docs/kit/configuration#prerender for more info."
    )]
    UnseenRoutes { routes: String },
}

#[derive(Debug, Error)]
pub enum GenerateManifestError {
    #[error("Could not find file \"{file}\" in Vite manifest")]
    MissingViteManifestFile { file: String },
    #[error("Could not find file \"{file}\" in build manifest")]
    MissingBuildManifestFile { file: String },
}

#[derive(Debug, Error)]
pub enum RequestError {
    #[error("server handle resolve may only be called once")]
    ResolveAlreadyCalled,
    #[error("Missing runtime route behavior")]
    MissingRuntimeRouteBehavior,
    #[error("Missing runtime route dispatch")]
    MissingRuntimeRouteDispatch,
    #[error("Missing runtime event")]
    MissingRuntimeEvent,
    #[error("Cannot execute data request for a non-page route")]
    InvalidDataRouteExecution,
    #[error("Cannot execute endpoint route without endpoint module")]
    MissingEndpointModule,
}

#[derive(Debug, Error)]
pub enum RequestStoreError {
    #[error(
        "Can only read the current request event inside functions invoked during `handle`, such as server `load` functions, actions, endpoints, and other server hooks."
    )]
    MissingCurrentRequestEvent,
    #[error("Could not get the request store. This is an internal error.")]
    MissingRequestStore,
}

#[derive(Debug, Error)]
pub enum SyntaxError {
    #[error("{message}")]
    ParseModule { message: String },
}

#[derive(Debug, Error)]
pub enum RoutingError {
    #[error("Invalid route segment {segment}")]
    InvalidRouteSegment { segment: String },
    #[error("Invalid route parameter in {segment}")]
    InvalidRouteParameterInSegment { segment: String },
    #[error("Invalid route parameter {raw} in {segment}")]
    InvalidRawRouteParameter { raw: String, segment: String },
    #[error(
        "Parameter '{name}' in route {route_id} cannot start or end with a slash -- this would cause an invalid route like foo//bar"
    )]
    ParameterStartsOrEndsWithSlash { name: String, route_id: String },
    #[error("Missing parameter '{name}' in route {route_id}")]
    MissingRouteParameter { name: String, route_id: String },
}

#[derive(Debug, Error)]
pub enum RuntimeHttpError {
    #[error("failed to build HTTP request: {message}")]
    BuildHttpRequest { message: String },
    #[error("failed to build HTTP response: {message}")]
    BuildHttpResponse { message: String },
    #[error("invalid absolute request URI `{uri}`: {message}")]
    InvalidAbsoluteRequestUri { uri: String, message: String },
    #[error("server request builder requires a URL")]
    MissingRequestUrl,
    #[error("invalid header name `{name}`: {message}")]
    InvalidHeaderName { name: String, message: String },
    #[error("invalid value for header `{name}`: {message}")]
    InvalidHeaderValue { name: String, message: String },
    #[error(
        "Use `event.cookies.set(name, value, options)` instead of `event.setHeaders` to set cookies"
    )]
    SetCookieViaHeaders,
    #[error("\"{name}\" header is already set")]
    DuplicateHeader { name: String },
}

#[derive(Debug, Error)]
pub enum TelemetryError {
    #[error(
        "Tracing is enabled (see `config.kit.experimental.instrumentation.server` in your svelte.config.js), but `@opentelemetry/api` is not available. This error will likely resolve itself when you set up your tracing instrumentation in `instrumentation.server.js`. For more information, see https://svelte.dev/docs/kit/observability#opentelemetry-api"
    )]
    MissingOpenTelemetryApi,
}

#[derive(Debug, Error)]
pub enum RuntimeLoadError {
    #[error("Invalid response header value for \"{name}\": {message}")]
    InvalidResponseHeaderValue { name: String, message: String },
    #[error(
        "Failed to get response header \"{name}\" — it must be included by the `filterSerializedResponseHeaders` option: https://svelte.dev/docs/kit/hooks#Server-hooks-handle{route_suffix}"
    )]
    FilteredResponseHeader { name: String, route_suffix: String },
    #[error("{message}")]
    ResponseBodyUnavailable { message: String },
    #[error("failed to parse JSON response body: {message}")]
    JsonResponseParse { message: String },
    #[error("invalid dependency `{dependency}`: {message}")]
    InvalidDependency { dependency: String, message: String },
    #[error("invalid fetch url: {message}")]
    InvalidFetchUrl { message: String },
}

#[derive(Debug, Error)]
pub enum RuntimePageError {
    #[error("Cannot prerender pages with actions")]
    PrerenderActions,
    #[error("{route_id} is not prerenderable")]
    NotPrerenderable { route_id: String },
    #[error("Cannot execute page load for a non-page route")]
    NonPageLoadRoute,
    #[error("Cannot execute page request for a non-page route")]
    NonPageRequestRoute,
    #[error("Cannot execute error page without fallback nodes")]
    MissingErrorPageFallbackNodes,
    #[error("Redirect {status} to {location} while loading error page")]
    ErrorPageLoadRedirect { status: u16, location: String },
    #[error("Error page load failed with status {status}: {error}")]
    ErrorPageLoadFailed { status: u16, error: String },
    #[error(
        "Data returned from action inside {route_id} is not serializable. Form actions need to return plain objects or fail(). E.g. return {{ success: true }} or return fail(400, {{ message: \"invalid\" }});"
    )]
    InvalidActionPayload { route_id: String },
}

#[derive(Debug, Error)]
pub enum RuntimeAppError {
    #[error("No transport decoder registered for `{kind}`")]
    MissingTransportDecoder { kind: String },
    #[error("No transport encoder registered for `{kind}`")]
    MissingTransportEncoder { kind: String },
    #[error("Transport encoder `{kind}` did not encode the provided value")]
    UnencodedTransportValue { kind: String },
    #[error("failed to serialize transport payload: {message}")]
    SerializeTransportPayload { message: String },
    #[error("invalid transport payload: {message}")]
    InvalidTransportPayload { message: String },
    #[error("invalid remote payload: {message}")]
    InvalidRemotePayload { message: String },
    #[error("invalid remote payload utf-8: {message}")]
    InvalidRemotePayloadUtf8 { message: String },
}

#[derive(Debug, Error)]
pub enum RuntimeEndpointError {
    #[error("Cannot prerender endpoints that have mutative methods")]
    PrerenderMutativeEndpoint,
    #[error("{route_id} is not prerenderable")]
    NotPrerenderable { route_id: String },
    #[error("invalid endpoint handler method `{method}`: {message}")]
    InvalidHandlerMethod { method: String, message: String },
}

#[derive(Debug, Error)]
pub enum RuntimeCspError {
    #[error(
        "`content-security-policy-report-only` must be specified with either the `report-to` or `report-uri` directives, or both"
    )]
    MissingReportOnlySink,
}

#[derive(Debug, Error)]
pub enum RuntimeRemoteError {
    #[error("Invalid remote argument: {message}")]
    InvalidArgument { message: String },
}

#[derive(Debug, Error)]
pub enum RuntimeSharedError {
    #[error(
        "a load function {location_description} returned {kind}, but must return a plain object at the top level (i.e. `return {{...}}`)"
    )]
    InvalidLoadResponse {
        location_description: String,
        kind: String,
    },
}

#[derive(Debug, Error)]
pub enum UrlError {
    #[error(
        "Failed to decode pathname: {pathname}\ninvalid utf-8 after percent-decoding: {message}"
    )]
    InvalidPathnameUtf8 { pathname: String, message: String },
    #[error("Failed to decode URI: {uri}\ninvalid utf-8 after percent-decoding: {message}")]
    InvalidUriUtf8 { uri: String, message: String },
    #[error("Failed to decode URI: {uri}\nincomplete percent-encoding")]
    IncompletePercentEncoding { uri: String },
    #[error("Failed to decode URI: {uri}\ninvalid percent-encoding {hex}: {message}")]
    InvalidPercentEncoding {
        uri: String,
        hex: String,
        message: String,
    },
}

#[derive(Debug, Error)]
pub enum UtilityError {
    #[error("failed to serialize JSON: {message}")]
    JsonSerialize { message: String },
    #[error("invalid base64 payload: {message}")]
    InvalidBase64Payload { message: String },
}

#[derive(Debug, Error)]
pub enum ViteBuildError {
    #[error("failed to stringify CSS source: {message}")]
    CssStringify { message: String },
    #[error("missing stylesheet filename for {file}")]
    MissingStylesheetFilename { file: String },
    #[error("missing client stylesheet source for {file}")]
    MissingClientStylesheetSource { file: String },
}

#[derive(Debug, Error)]
pub enum ViteBuildUtilsError {
    #[error("failed to stringify function body: {message}")]
    FunctionBodyStringify { message: String },
    #[error("failed to trim JSON string delimiters")]
    MissingJsonStringDelimiters,
}

#[derive(Debug, Error)]
pub enum ViteGuardError {
    #[error(
        "Cannot import {normalized} into code that runs in the browser, as this could leak sensitive information.\n\n{pyramid}\n\nIf you're only using the import as a type, change it to `import type`."
    )]
    BrowserImport { normalized: String, pyramid: String },
    #[error(
        "Cannot import {normalized} into service-worker code. Only the modules $service-worker and $env/static/public are available in service workers."
    )]
    ServiceWorkerImport { normalized: String },
}

#[derive(Debug, Error)]
pub enum ViteUtilsError {
    #[error(
        "To enable {feature_name}, add the following to your `svelte.config.js`:\n\n{config_snippet}"
    )]
    MissingConfig {
        feature_name: String,
        config_snippet: String,
    },
}
