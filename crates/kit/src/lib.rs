mod adapt;
mod adapt_entry;
mod analyze;
mod array;
mod builder;
mod config;
mod constants;
mod cookie;
mod css;
mod entities;
mod env;
mod error;
mod escape;
mod exports;
mod exports_internal;
mod exports_node;
mod exports_public;
mod features;
mod filesystem;
mod fork;
mod form_utils;
mod functions;
mod generate_manifest;
mod hash;
mod http;
mod manifest;
mod misc;
mod node_polyfills;
mod page_options;
mod pathname;
mod peer_import;
mod postbuild;
mod postbuild_analyze;
mod postbuild_prerender;
mod prerender_errors;
mod prerender_html;
mod prerender_output;
mod prerender_paths;
mod prerender_policy;
mod preview;
mod preview_middleware;
mod preview_server;
mod promise;
mod queue;
mod request_store;
mod routing;
mod runtime;
mod runtime_app;
mod runtime_utils;
mod sequence;
mod streaming;
mod sync;
mod syntax_errors;
mod telemetry;
mod url;
mod version;
mod vite_build_server;
mod vite_build_utils;
mod vite_dev;
mod vite_dev_server;
mod vite_entry;
mod vite_guard;
mod vite_module_ids;
mod vite_overrides;
mod vite_service_worker;
mod vite_setup;
mod vite_static_analysis;
mod vite_static_analysis_index;
mod vite_utils;

pub use adapt::{
    compress_directory, create_instrumentation_facade, has_server_instrumentation_file,
    instrument_entrypoint,
};
pub use adapt_entry::{
    AdaptInvocationResult, AdaptProjectResult, AdaptStatus, adapt_project, invoke_adapter,
};
pub use analyze::{
    AnalyzedMetadata, AnalyzedNodeMetadata, AnalyzedRemoteExport, analyze_remote_metadata,
    analyze_server_metadata, analyze_server_metadata_with_features,
};
pub use array::compact;
pub use builder::{
    BuilderAdapterEntry, BuilderEntryContext, BuilderFacade, BuilderPrerenderOption,
    BuilderPrerendered, BuilderRouteApi, BuilderRouteDefinition, BuilderRouteFilter,
    BuilderRoutePage, BuilderServerMetadata, BuilderServerMetadataRoute, PrerenderedAsset,
    PrerenderedPage, PrerenderedRedirect, RouteSegment, build_route_definitions,
};
pub use config::{
    BundleStrategy, CspMode, JsSource, JsSourceKind, LoadedKitProject, PreloadStrategy,
    PrerenderPolicy, RouterResolution, RouterType, ServiceWorkerFilesFilter, TypeScriptConfigHook,
    ValidatedAdapterConfig, ValidatedCompilerExperimentalOptions, ValidatedCompilerOptions,
    ValidatedConfig, ValidatedCspConfig, ValidatedCspDirectives, ValidatedCsrfConfig,
    ValidatedEnvConfig, ValidatedExperimentalConfig, ValidatedFilesConfig, ValidatedHooksConfig,
    ValidatedInstrumentationConfig, ValidatedKitConfig, ValidatedOutputConfig,
    ValidatedPathsConfig, ValidatedPrerenderConfig, ValidatedRouterConfig,
    ValidatedServiceWorkerConfig, ValidatedTracingConfig, ValidatedTypeScriptConfig,
    ValidatedVersionConfig, load_config, load_error_page, load_project, load_template,
    validate_config,
};
pub use constants::{
    ENDPOINT_METHODS_PUBLIC, GENERATED_COMMENT, MUTATIVE_METHODS, PAGE_METHODS_PUBLIC,
    SVELTE_KIT_ASSETS, endpoint_methods,
};
pub use cookie::{
    Cookie, CookieEntry, CookieJar, CookieOptions, CookieParseOptions, ResolvedCookieOptions,
    SameSite, domain_matches, path_matches,
};
pub use css::{CssUrlRewriteOptions, fix_css_urls, tippex_comments_and_strings};
pub use entities::decode_entities;
pub use env::{EnvKind, create_dynamic_module, create_static_module, is_valid_identifier};
pub use error::{
    AdaptError, AnalyzeError, ConfigError, CookieError, Error, ExportValidationError,
    ExportsInternalError, ExportsNodeError, ExportsPublicError, FeatureError, ForkError, FormError,
    GenerateManifestError, ManifestError, PeerImportError, PostbuildError, PrerenderError,
    RequestError, RequestStoreError, Result, RoutingError, RuntimeAppError, RuntimeCspError,
    RuntimeEndpointError, RuntimeHttpError, RuntimeLoadError, RuntimePageError, RuntimeRemoteError,
    RuntimeSharedError, SyntaxError, TelemetryError, UrlError, UtilityError, ViteBuildError,
    ViteBuildUtilsError, ViteGuardError, ViteUtilsError,
};
pub use escape::{escape_for_interpolation, escape_html_utf16, escape_html_with_mode};
pub use exports::{
    RouteModuleKind, validate_layout_exports, validate_layout_server_exports,
    validate_module_exports, validate_page_exports, validate_page_server_exports,
    validate_server_exports,
};
pub use exports_internal::{
    ActionFailure, HttpErrorClass, RedirectClass, RemoteExport, RemoteFunctionInfo,
    RemoteFunctionKind, SvelteKitErrorClass, ValidationErrorClass, init_remote_functions,
};
pub use exports_node::{NodeRequest, create_readable_stream, get_node_request, set_node_response};
pub use exports_public::{NormalizedUrl, normalize_url};
pub use features::{AdapterFeatures, check_feature, list_route_features};
pub use filesystem::{CopyOptions, copy, mkdirp, posixify, resolve_entry};
pub use fork::forked;
pub use form_utils::{
    BinaryFormRequest, DeserializedBinaryForm, FormData, FormFile, FormInputValue, FormObject,
    FormValue, SerializedBinaryForm, convert_formdata, deep_set, deserialize_binary_form,
    serialize_binary_form, set_nested_value, split_path,
};
pub use functions::{OnceFn, once};
pub use generate_manifest::{
    AssetDependencies, BuildData, BuildManifestChunk, GeneratedServerManifest, RemoteChunk,
    StylesheetMapEntry, assets_base, filter_fonts, find_deps, find_server_assets,
    generate_manifest, resolve_manifest_symlink,
};
pub use hash::{HashValue, hash_values};
pub use http::{BINARY_FORM_CONTENT_TYPE, is_form_content_type, negotiate};
pub use manifest::{
    Asset, ClientLayoutRef, ClientLeafRef, ClientRoute, DiscoveredRoute, Hooks, KitManifest,
    ManifestConfig, ManifestEndpoint, ManifestNode, ManifestNodeKind, ManifestRoute,
    ManifestRoutePage, NodeFiles, PageFiles, discover_assets, discover_hooks, discover_matchers,
    discover_routes,
};
pub use misc::json_stringify;
pub use node_polyfills::{NodePolyfill, available_node_polyfills, install_node_polyfills};
pub use page_options::{PageOptions, read_page_options, statically_analyze_page_options};
pub use pathname::{
    add_data_suffix, add_resolution_suffix, has_data_suffix, has_resolution_suffix,
    strip_data_suffix, strip_resolution_suffix,
};
pub use peer_import::resolve_peer_dependency;
pub use postbuild::{CrawlResult, crawl};
pub use postbuild_analyze::{PostbuildAnalyzeResult, analyze_postbuild};
pub use postbuild_prerender::{
    FallbackGenerationPlan, PrerenderExecutionPlan, build_fallback_generation_plan,
    build_prerender_execution_plan,
};
pub use prerender_errors::{
    prerender_entry_generator_mismatch_error, prerender_unseen_routes_error,
};
pub use prerender_html::{
    relative_service_worker_path, render_http_equiv_meta_tag, render_service_worker_registration,
};
pub use prerender_output::{
    render_prerender_redirect_html, serialize_missing_ids_jsonl, service_worker_prerender_paths,
};
pub use prerender_paths::{prepend_base_path, prerender_output_filename};
pub use prerender_policy::{
    fallback_page_filename, public_asset_output_path, should_prerender_linked_server_route,
};
pub use preview::{
    PrerenderedMatch, PrerenderedResolution, PreviewPaths, PreviewPlan, build_preview_plan,
    preview_paths, preview_protocol, preview_root_redirect, resolve_prerendered_request,
};
pub use preview_middleware::{
    PreviewMiddlewarePlan, PreviewMiddlewareStep, build_preview_middleware_plan,
};
pub use preview_server::{PreviewServerPlan, build_preview_server_plan};
pub use promise::{PromiseState, PromiseWithResolvers, with_resolvers};
pub use queue::{AsyncQueue, QueueClosedError, TaskHandle, queue};
pub use request_store::{
    RequestStore, TracingState, get_request_event, get_request_store, merge_tracing,
    try_get_request_store, with_request_store,
};
pub use routing::{
    FoundRoute, ParsedRouteId, RouteParam, exec_route_match, find_route, get_route_segments,
    parse_route_id, remove_optional_params, resolve_route, sort_routes,
};
pub use runtime::server::{
    ActionJsonResult, ActionRequestResult, AppState, Csp, CspProvider, DataRequestEnvelope,
    DataRequestNode, EndpointError, EndpointModule, EndpointResult, ErrorPageRenderPlan,
    ErrorPageRequestResult, ExecutedErrorPage, FetchedResponse, PageActionExecution,
    PageErrorBoundary, PageExecutionResult, PageLoadResult, PageLoadedNode, PageRenderPlan,
    PageRequestResult, PageRuntimeDecision, ParsedRemoteId, PreparedDataRequest,
    PreparedRemoteInvocation, PreparedRequestUrl, PreparedRuntimeExecution,
    PreprocessedRuntimeRequest, PrerenderDependency, PrerenderState, RemoteCallExecution,
    RemoteCallKind, RemoteCallRequest, RemoteFormExecutionResult, RemoteFormPostResult,
    RemoteFunctionResponse, RenderedPage, RequestEventState, RequestKind, ResolvedRuntimeRequest,
    RouteResolutionAssets, RuntimeCspConfig, RuntimeCspDirectives, RuntimeCspMode,
    RuntimeCspOptions, RuntimeDevalueError, RuntimeEvent, RuntimeExecutionResult,
    RuntimeFatalError, RuntimePageNodes, RuntimeRenderState, RuntimeRequestOptions,
    RuntimeRespondResult, RuntimeRouteBehavior, RuntimeRouteDispatch, Server, ServerDataNode,
    ServerDataUses, ServerHandle, ServerHandleContext, ServerHookInit, ServerHookLoader,
    ServerHooks, ServerInitOptions, ServerLoadContext, ServerRead, ServerRequest,
    ServerRequestBuilder, ServerRequestEvent, ServerReroute, ServerResponse, ServerResponseBuilder,
    ServerTransportDecoder, ServerTransportEncoder, ServerTransportHook, ShellPageResponse,
    SpecialRuntimeRequestOptions, UniversalFetch, UniversalFetchBody, UniversalFetchContext,
    UniversalFetchCookieHeader, UniversalFetchCookieSetter, UniversalFetchCredentials,
    UniversalFetchHandle, UniversalFetchHandleContext, UniversalFetchMode, UniversalFetchOptions,
    UniversalFetchRawResponse, UniversalFetchResponse, UniversalFetchResponseHeaders,
    UniversalLoadContext, action_json_response, allowed_methods, apply_page_prerender_policy,
    build_runtime_event, check_csrf, check_incorrect_fail_use, check_remote_request_origin,
    clarify_devalue_error, create_server_routing_response, create_universal_fetch,
    data_json_response, data_request_not_found_response, decode_app_value, decode_transport_value,
    dispatch_special_runtime_request, encode_app_value, encode_transport_value,
    execute_data_request, execute_error_page_load, execute_error_page_request,
    execute_named_page_action_json_request, execute_named_page_request_from_request,
    execute_named_runtime_page_request, execute_named_runtime_page_stage,
    execute_page_action_json_request, execute_page_load, execute_page_request,
    execute_page_request_from_request, execute_page_request_with_action,
    execute_prepared_runtime_request, execute_prepared_runtime_request_with_named_page_stage,
    execute_prepared_runtime_request_with_page_stage, execute_remote_call,
    execute_runtime_page_request, execute_runtime_page_stage, finalize_route_response,
    format_server_error, get_global_name, get_node_type, get_remote_action, get_remote_id,
    handle_fatal_error, handle_remote_call, handle_remote_form_action_request,
    handle_remote_form_post, handle_remote_form_post_result, has_prerendered_path,
    invalidated_data_node_flags, is_action_json_request, is_action_request, is_endpoint_request,
    is_pojo, load_data, load_page_nodes, load_server_data, materialize_page_request_result,
    maybe_not_modified_response, method_not_allowed_message, method_not_allowed_response,
    no_actions_action_json_response, no_actions_action_request_result, page_method_response,
    page_request_requires_shell_only, parse_remote_arg, parse_remote_id, parse_transport_payload,
    plain_text_response, prepare_data_request, prepare_error_page_render, prepare_page_render_plan,
    prepare_request_url, prepare_runtime_execution, preprocess_runtime_request,
    public_env_response, redirect_data_response, redirect_response, remote_json_response,
    remote_json_response_with_status, render_data_request, render_endpoint,
    render_shell_page_response, request_normalization_redirect, resolve_named_page_action_request,
    resolve_page_action_request, resolve_page_runtime_decision, resolve_remote_request_url,
    resolve_route_request_response, resolve_runtime_request, resolve_runtime_route_behavior,
    resolve_runtime_route_dispatch, respond_runtime_request,
    respond_runtime_request_materialized_with_named_page_stage,
    respond_runtime_request_materialized_with_page_stage,
    respond_runtime_request_with_named_page_stage, respond_runtime_request_with_page_stage,
    response_with_vary_accept, route_data_node_indexes, runtime_normalization_response,
    serialize_data, serialize_uses, set_response_cookies, set_response_header, sha256,
    static_error_page, stringify_remote_arg, stringify_transport_payload, validate_headers,
};
pub use runtime::shared::{create_remote_key, validate_depends, validate_load_response};
pub use runtime_app::resolve_app_module;
pub use runtime_utils::{base64_decode, base64_encode, get_relative_path};
pub use sequence::{
    FilterSerializedResponseHeaders, Handle, HandleContext, PageChunk, Preload, PreloadInput,
    Resolve, ResolveOptions, TransformPageChunk, filter_serialized_response_headers, handle,
    preload, resolve_fn, sequence, transform_page_chunk,
};
pub use streaming::{AsyncIterator, create_async_iterator};
pub use sync::{
    GeneratedAmbient, GeneratedClientManifest, GeneratedNodeModule, GeneratedNonAmbient,
    GeneratedRoot, GeneratedServerInternal, GeneratedTsConfig, SyncWriteResult,
    create_sync_project, generate_ambient, generate_client_manifest, generate_non_ambient,
    generate_root, generate_server_internal, generate_tsconfig, init_sync_project,
    update_sync_project_for_file, write_all_sync_types, write_all_types, write_server_project,
    write_sync_project,
};
pub use syntax_errors::parse_module_syntax;
pub use telemetry::{
    HttpError, RecordSpanError, RecordSpanParams, Redirect, TelemetryApi, TelemetryAttributes,
    TelemetryException, TelemetrySpan, TelemetryStatus, TelemetryTracer, TelemetryValue, load_otel,
    noop_span, record_span,
};
pub use url::{
    decode_params, decode_pathname, decode_uri, is_root_relative, normalize_path, resolve,
    strip_hash, try_decode_pathname,
};
pub use version::VERSION;
pub use vite_build_server::{
    BuildServerNodesPlan, ClientBuildAsset, InlineStylesExport, PlannedBuildOutputFile,
    PreparedInlineStylesheetModule, ServerNodeBuildArtifacts, ServerNodeBuildInput,
    ServerNodeModulePlan, build_server_node_artifacts, build_server_nodes_plan,
    render_inline_stylesheet_module, render_server_node_module,
};
pub use vite_build_utils::create_function_as_string;
pub use vite_dev::{
    DevClientLayoutPlan, DevClientLeafPlan, DevClientRoutePlan, DevServerNodePlan, ViteDevPlan,
    build_vite_dev_plan,
};
pub use vite_dev_server::{DevMiddlewareStep, ViteDevServerPlan, build_vite_dev_server_plan};
pub use vite_entry::{ViteBuildOrchestrationPlan, build_vite_orchestration_plan};
pub use vite_guard::{browser_import_guard_error, service_worker_import_guard_error};
pub use vite_module_ids::{
    ViteModuleIds, app_server_module_id, env_dynamic_private_module_id,
    env_dynamic_public_module_id, env_static_private_module_id, env_static_public_module_id,
    service_worker_module_id, sveltekit_environment_module_id, sveltekit_server_module_id,
};
pub use vite_overrides::{find_overridden_vite_config, warn_overridden_vite_config};
pub use vite_service_worker::{
    ServiceWorkerBuildEntry, ServiceWorkerBuildInvocationPlan, collect_service_worker_build_files,
    create_service_worker_module, render_service_worker_module,
    resolve_service_worker_virtual_module, service_worker_build_invocation_plan,
    service_worker_build_plan, service_worker_entry_output_filename,
    service_worker_runtime_asset_url, should_rename_service_worker_output,
};
pub use vite_setup::{ViteOptimizeRemoteFunctionsPlan, ViteSetupPlan, build_vite_setup_plan};
pub use vite_static_analysis::{has_children, should_ignore};
pub use vite_static_analysis_index::statically_analyze_vite_page_options;
pub use vite_utils::{
    ViteAlias, ViteAliasFind, error_for_missing_config, get_config_aliases, normalize_vite_id,
    strip_virtual_prefix, vite_not_found_response,
};
