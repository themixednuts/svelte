use std::sync::{Arc, Mutex};

use serde_json::{Map, Value};
use url::Url;

use crate::manifest::KitManifest;
use crate::{RequestError, Result};

use super::{
    ActionJsonResult, ActionRequestResult, AppState, EndpointModule, ErrorPageRequestResult,
    ExecutedErrorPage, PageErrorBoundary, PageLoadResult, RenderedPage, ResolvedRuntimeRequest,
    RuntimeEvent, RuntimeRenderState, RuntimeRequestOptions, RuntimeRouteBehavior, ServerRequest,
    ServerResponse, ServerTransportHook, ShellPageResponse,
    respond_runtime_request_materialized_with_named_page_stage,
    respond_runtime_request_materialized_with_page_stage,
};

pub struct ServerRead {
    inner: Arc<dyn Fn(&str) -> Result<Option<Vec<u8>>> + Send + Sync>,
}

impl Clone for ServerRead {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl ServerRead {
    pub fn new<F>(read: F) -> Self
    where
        F: Fn(&str) -> Result<Option<Vec<u8>>> + Send + Sync + 'static,
    {
        Self {
            inner: Arc::new(read),
        }
    }

    pub fn call(&self, file: &str) -> Result<Option<Vec<u8>>> {
        (self.inner)(file)
    }
}

pub struct ServerHookInit {
    inner: Arc<dyn Fn() -> Result<()> + Send + Sync>,
}

impl Clone for ServerHookInit {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl ServerHookInit {
    pub fn new<F>(init: F) -> Self
    where
        F: Fn() -> Result<()> + Send + Sync + 'static,
    {
        Self {
            inner: Arc::new(init),
        }
    }

    pub fn call(&self) -> Result<()> {
        (self.inner)()
    }
}

pub struct ServerHookLoader {
    inner: Arc<dyn Fn() -> Result<ServerHooks> + Send + Sync>,
}

impl Clone for ServerHookLoader {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl ServerHookLoader {
    pub fn new<F>(load: F) -> Self
    where
        F: Fn() -> Result<ServerHooks> + Send + Sync + 'static,
    {
        Self {
            inner: Arc::new(load),
        }
    }

    pub fn call(&self) -> Result<ServerHooks> {
        (self.inner)()
    }
}

pub struct ServerHandle {
    inner: Arc<dyn for<'a> Fn(ServerHandleContext<'a>) -> Result<ServerResponse> + Send + Sync>,
}

impl Clone for ServerHandle {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl ServerHandle {
    pub fn new<F>(handle: F) -> Self
    where
        F: for<'a> Fn(ServerHandleContext<'a>) -> Result<ServerResponse> + Send + Sync + 'static,
    {
        Self {
            inner: Arc::new(handle),
        }
    }

    pub fn call(&self, context: ServerHandleContext<'_>) -> Result<ServerResponse> {
        (self.inner)(context)
    }
}

pub struct ServerReroute {
    inner: Arc<dyn Fn(&Url) -> Result<Option<String>> + Send + Sync>,
}

impl Clone for ServerReroute {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl ServerReroute {
    pub fn new<F>(reroute: F) -> Self
    where
        F: Fn(&Url) -> Result<Option<String>> + Send + Sync + 'static,
    {
        Self {
            inner: Arc::new(reroute),
        }
    }

    pub fn call(&self, url: &Url) -> Result<Option<String>> {
        (self.inner)(url)
    }
}

pub struct ServerHandleContext<'a> {
    pub request: &'a ServerRequest,
    pub resolve: &'a dyn Fn(&ServerRequest) -> Result<ServerResponse>,
}

#[derive(Clone, Default)]
pub struct ServerHooks {
    pub init: Option<ServerHookInit>,
    pub handle: Option<ServerHandle>,
    pub reroute: Option<ServerReroute>,
    pub transport: std::collections::BTreeMap<String, ServerTransportHook>,
}

impl ServerHooks {
    pub fn with_init(init: ServerHookInit) -> Self {
        Self {
            init: Some(init),
            handle: None,
            reroute: None,
            transport: std::collections::BTreeMap::new(),
        }
    }
}

#[derive(Clone, Default)]
pub struct ServerInitOptions {
    pub env: Map<String, Value>,
    pub read: Option<ServerRead>,
}

pub struct Server {
    manifest: KitManifest,
    options: RuntimeRequestOptions,
    app_state: Arc<AppState>,
    env_public_prefix: String,
    env_private_prefix: String,
    public_env: Map<String, Value>,
    private_env: Map<String, Value>,
    read: Option<ServerRead>,
    hook_loader: Option<ServerHookLoader>,
    hooks: ServerHooks,
    initialized: bool,
}

impl Server {
    pub fn new(
        manifest: KitManifest,
        options: RuntimeRequestOptions,
        env_public_prefix: impl Into<String>,
        env_private_prefix: impl Into<String>,
        hook_loader: Option<ServerHookLoader>,
    ) -> Self {
        Self {
            manifest,
            options,
            app_state: Arc::new(AppState::default()),
            env_public_prefix: env_public_prefix.into(),
            env_private_prefix: env_private_prefix.into(),
            public_env: Map::new(),
            private_env: Map::new(),
            read: None,
            hook_loader,
            hooks: ServerHooks::default(),
            initialized: false,
        }
    }

    pub fn init(&mut self, options: ServerInitOptions) -> Result<()> {
        self.private_env = filter_env(
            &options.env,
            &self.env_private_prefix,
            &self.env_public_prefix,
        );
        self.public_env = filter_env(
            &options.env,
            &self.env_public_prefix,
            &self.env_private_prefix,
        );
        self.options.public_env = self.public_env.clone();
        if let Some(read) = options.read {
            self.read = Some(read);
        }

        if !self.initialized {
            let hooks = if let Some(loader) = &self.hook_loader {
                loader.call()?
            } else {
                ServerHooks::default()
            };

            if let Some(init) = &hooks.init {
                init.call()?;
            }

            self.app_state = Arc::new(AppState {
                decoders: hooks
                    .transport
                    .iter()
                    .map(|(key, transport)| (key.clone(), transport.decode.clone()))
                    .collect(),
                encoders: hooks
                    .transport
                    .iter()
                    .filter_map(|(key, transport)| {
                        transport
                            .encode
                            .as_ref()
                            .map(|encode| (key.clone(), encode.clone()))
                    })
                    .collect(),
            });
            self.hooks = hooks;
            self.initialized = true;
        }

        Ok(())
    }

    pub fn manifest(&self) -> &KitManifest {
        &self.manifest
    }

    pub fn runtime_options(&self) -> &RuntimeRequestOptions {
        &self.options
    }

    pub fn app_state(&self) -> &AppState {
        self.app_state.as_ref()
    }

    pub fn public_env(&self) -> &Map<String, Value> {
        &self.public_env
    }

    pub fn private_env(&self) -> &Map<String, Value> {
        &self.private_env
    }

    pub fn hooks(&self) -> &ServerHooks {
        &self.hooks
    }

    pub fn read(&self, file: &str) -> Result<Option<Vec<u8>>> {
        match &self.read {
            Some(read) => read.call(file),
            None => Ok(None),
        }
    }

    pub fn respond<
        FMatch,
        FEndpoint,
        FData,
        FActionJson,
        FAction,
        FRemoteAction,
        FLoadPage,
        FError,
        FFetch,
        FRemote,
        R,
        FShell,
        FPage,
        FBoundary,
        FErrorPage,
    >(
        &self,
        request: &ServerRequest,
        page_has_actions: bool,
        dev: bool,
        render_error: R,
        matches: FMatch,
        endpoint_module: FEndpoint,
        execute_data: FData,
        execute_action_json: FActionJson,
        execute_action: FAction,
        execute_remote_action: FRemoteAction,
        load_page: FLoadPage,
        execute_error_page: FError,
        execute_remote: FRemote,
        fetch_runtime_response: FFetch,
        render_shell: FShell,
        render_page: FPage,
        render_boundary: FBoundary,
        render_error_page: FErrorPage,
    ) -> Result<ServerResponse>
    where
        FMatch: Clone + FnMut(&str, &str) -> bool,
        FEndpoint: FnOnce(Option<&ResolvedRuntimeRequest<'_>>) -> Option<EndpointModule>,
        FData: FnMut(
            &ResolvedRuntimeRequest<'_>,
            &RuntimeRouteBehavior,
            &RuntimeEvent,
        ) -> Result<Option<ServerResponse>>,
        FActionJson: FnMut(
            &ResolvedRuntimeRequest<'_>,
            &RuntimeRouteBehavior,
            &RuntimeEvent,
            &mut RuntimeRenderState,
        ) -> Result<ActionJsonResult>,
        FAction: FnMut(
            &ResolvedRuntimeRequest<'_>,
            &RuntimeRouteBehavior,
            &RuntimeEvent,
            &mut RuntimeRenderState,
        ) -> Result<ActionRequestResult>,
        FRemoteAction: FnMut(
            &ResolvedRuntimeRequest<'_>,
            &RuntimeRouteBehavior,
            &RuntimeEvent,
            &mut RuntimeRenderState,
            &str,
        ) -> Result<super::RemoteFormExecutionResult>,
        FLoadPage: FnMut(
            &ResolvedRuntimeRequest<'_>,
            &RuntimeRouteBehavior,
            &RuntimeEvent,
            &mut RuntimeRenderState,
            usize,
            &Map<String, Value>,
            &Map<String, Value>,
        ) -> Result<PageLoadResult>,
        FError: FnMut(u16, Value, bool) -> Result<ErrorPageRequestResult>,
        FFetch: FnMut(&ServerRequest) -> Result<ServerResponse>,
        FRemote: FnMut(&str, &RuntimeEvent, &mut RuntimeRenderState) -> Result<ServerResponse>,
        R: FnOnce(u16, &str) -> String + Clone,
        FShell: FnOnce(&ShellPageResponse) -> Result<ServerResponse>,
        FPage: FnOnce(&RenderedPage) -> Result<ServerResponse>,
        FBoundary: FnOnce(&PageErrorBoundary) -> Result<ServerResponse>,
        FErrorPage: FnOnce(&ExecutedErrorPage) -> Result<ServerResponse>,
    {
        let resolved_request = self.apply_reroute(request)?;

        if let Some(handle) = self.hooks.handle.clone() {
            let endpoint_module = Mutex::new(Some(endpoint_module));
            let execute_data = Mutex::new(Some(execute_data));
            let execute_action_json = Mutex::new(Some(execute_action_json));
            let execute_action = Mutex::new(Some(execute_action));
            let execute_remote_action = Mutex::new(Some(execute_remote_action));
            let load_page = Mutex::new(Some(load_page));
            let execute_error_page = Mutex::new(Some(execute_error_page));
            let execute_remote = Mutex::new(Some(execute_remote));
            let fetch_runtime_response = Mutex::new(Some(fetch_runtime_response));
            let render_shell = Mutex::new(Some(render_shell));
            let render_page = Mutex::new(Some(render_page));
            let render_boundary = Mutex::new(Some(render_boundary));
            let render_error_page = Mutex::new(Some(render_error_page));

            let resolve = |request: &ServerRequest| {
                let mut state = RuntimeRenderState {
                    app_state: Arc::clone(&self.app_state),
                    error: false,
                    depth: 0,
                    ..RuntimeRenderState::default()
                };

                respond_runtime_request_materialized_with_page_stage(
                    &self.manifest,
                    request,
                    &self.options,
                    &mut state,
                    page_has_actions,
                    dev,
                    render_error.clone(),
                    matches.clone(),
                    take_resolve_arg(&endpoint_module)?,
                    take_resolve_arg(&execute_data)?,
                    take_resolve_arg(&execute_action_json)?,
                    take_resolve_arg(&execute_action)?,
                    take_resolve_arg(&execute_remote_action)?,
                    take_resolve_arg(&load_page)?,
                    take_resolve_arg(&execute_error_page)?,
                    take_resolve_arg(&execute_remote)?,
                    take_resolve_arg(&fetch_runtime_response)?,
                    take_resolve_arg(&render_shell)?,
                    take_resolve_arg(&render_page)?,
                    take_resolve_arg(&render_boundary)?,
                    take_resolve_arg(&render_error_page)?,
                )
            };

            return handle.call(ServerHandleContext {
                request: &resolved_request,
                resolve: &resolve,
            });
        }

        let mut state = RuntimeRenderState {
            app_state: Arc::clone(&self.app_state),
            error: false,
            depth: 0,
            ..RuntimeRenderState::default()
        };

        respond_runtime_request_materialized_with_page_stage(
            &self.manifest,
            &resolved_request,
            &self.options,
            &mut state,
            page_has_actions,
            dev,
            render_error,
            matches,
            endpoint_module,
            execute_data,
            execute_action_json,
            execute_action,
            execute_remote_action,
            load_page,
            execute_error_page,
            execute_remote,
            fetch_runtime_response,
            render_shell,
            render_page,
            render_boundary,
            render_error_page,
        )
    }

    pub fn respond_with_named_page_stage<
        FMatch,
        FEndpoint,
        FData,
        FActionJson,
        FAction,
        FRemoteAction,
        FLoadPage,
        FError,
        FFetch,
        FRemote,
        R,
        FShell,
        FPage,
        FBoundary,
        FErrorPage,
    >(
        &self,
        request: &ServerRequest,
        has_default_action: bool,
        named_action_count: usize,
        render_error: R,
        matches: FMatch,
        endpoint_module: FEndpoint,
        execute_data: FData,
        execute_action_json: FActionJson,
        execute_action: FAction,
        execute_remote_action: FRemoteAction,
        load_page: FLoadPage,
        execute_error_page: FError,
        execute_remote: FRemote,
        fetch_runtime_response: FFetch,
        render_shell: FShell,
        render_page: FPage,
        render_boundary: FBoundary,
        render_error_page: FErrorPage,
    ) -> Result<ServerResponse>
    where
        FMatch: Clone + FnMut(&str, &str) -> bool,
        FEndpoint: FnOnce(Option<&ResolvedRuntimeRequest<'_>>) -> Option<EndpointModule>,
        FData: FnMut(
            &ResolvedRuntimeRequest<'_>,
            &RuntimeRouteBehavior,
            &RuntimeEvent,
        ) -> Result<Option<ServerResponse>>,
        FActionJson: FnMut(
            &ResolvedRuntimeRequest<'_>,
            &RuntimeRouteBehavior,
            &RuntimeEvent,
            &mut RuntimeRenderState,
            &str,
        ) -> Result<Option<ActionJsonResult>>,
        FAction: FnMut(
            &ResolvedRuntimeRequest<'_>,
            &RuntimeRouteBehavior,
            &RuntimeEvent,
            &mut RuntimeRenderState,
            &str,
        ) -> Result<Option<ActionRequestResult>>,
        FRemoteAction: FnMut(
            &ResolvedRuntimeRequest<'_>,
            &RuntimeRouteBehavior,
            &RuntimeEvent,
            &mut RuntimeRenderState,
            &str,
        ) -> Result<super::RemoteFormExecutionResult>,
        FLoadPage: FnMut(
            &ResolvedRuntimeRequest<'_>,
            &RuntimeRouteBehavior,
            &RuntimeEvent,
            &mut RuntimeRenderState,
            usize,
            &Map<String, Value>,
            &Map<String, Value>,
        ) -> Result<PageLoadResult>,
        FError: FnMut(u16, Value, bool) -> Result<ErrorPageRequestResult>,
        FFetch: FnMut(&ServerRequest) -> Result<ServerResponse>,
        FRemote: FnMut(&str, &RuntimeEvent, &mut RuntimeRenderState) -> Result<ServerResponse>,
        R: FnOnce(u16, &str) -> String + Clone,
        FShell: FnOnce(&ShellPageResponse) -> Result<ServerResponse>,
        FPage: FnOnce(&RenderedPage) -> Result<ServerResponse>,
        FBoundary: FnOnce(&PageErrorBoundary) -> Result<ServerResponse>,
        FErrorPage: FnOnce(&ExecutedErrorPage) -> Result<ServerResponse>,
    {
        let resolved_request = self.apply_reroute(request)?;

        if let Some(handle) = self.hooks.handle.clone() {
            let endpoint_module = Mutex::new(Some(endpoint_module));
            let execute_data = Mutex::new(Some(execute_data));
            let execute_action_json = Mutex::new(Some(execute_action_json));
            let execute_action = Mutex::new(Some(execute_action));
            let execute_remote_action = Mutex::new(Some(execute_remote_action));
            let load_page = Mutex::new(Some(load_page));
            let execute_error_page = Mutex::new(Some(execute_error_page));
            let execute_remote = Mutex::new(Some(execute_remote));
            let fetch_runtime_response = Mutex::new(Some(fetch_runtime_response));
            let render_shell = Mutex::new(Some(render_shell));
            let render_page = Mutex::new(Some(render_page));
            let render_boundary = Mutex::new(Some(render_boundary));
            let render_error_page = Mutex::new(Some(render_error_page));

            let resolve = |request: &ServerRequest| {
                let mut state = RuntimeRenderState {
                    app_state: Arc::clone(&self.app_state),
                    error: false,
                    depth: 0,
                    ..RuntimeRenderState::default()
                };

                respond_runtime_request_materialized_with_named_page_stage(
                    &self.manifest,
                    request,
                    &self.options,
                    &mut state,
                    has_default_action,
                    named_action_count,
                    render_error.clone(),
                    matches.clone(),
                    take_resolve_arg(&endpoint_module)?,
                    take_resolve_arg(&execute_data)?,
                    take_resolve_arg(&execute_action_json)?,
                    take_resolve_arg(&execute_action)?,
                    take_resolve_arg(&execute_remote_action)?,
                    take_resolve_arg(&load_page)?,
                    take_resolve_arg(&execute_error_page)?,
                    take_resolve_arg(&execute_remote)?,
                    take_resolve_arg(&fetch_runtime_response)?,
                    take_resolve_arg(&render_shell)?,
                    take_resolve_arg(&render_page)?,
                    take_resolve_arg(&render_boundary)?,
                    take_resolve_arg(&render_error_page)?,
                )
            };

            return handle.call(ServerHandleContext {
                request: &resolved_request,
                resolve: &resolve,
            });
        }

        let mut state = RuntimeRenderState {
            app_state: Arc::clone(&self.app_state),
            error: false,
            depth: 0,
            ..RuntimeRenderState::default()
        };

        respond_runtime_request_materialized_with_named_page_stage(
            &self.manifest,
            &resolved_request,
            &self.options,
            &mut state,
            has_default_action,
            named_action_count,
            render_error,
            matches,
            endpoint_module,
            execute_data,
            execute_action_json,
            execute_action,
            execute_remote_action,
            load_page,
            execute_error_page,
            execute_remote,
            fetch_runtime_response,
            render_shell,
            render_page,
            render_boundary,
            render_error_page,
        )
    }

    fn apply_reroute(&self, request: &ServerRequest) -> Result<ServerRequest> {
        let Some(reroute) = &self.hooks.reroute else {
            return Ok(request.clone());
        };

        let mut rewritten = request.clone();
        if let Some(pathname) = reroute.call(&request.url)? {
            rewritten.url.set_path(&pathname);
        }

        Ok(rewritten)
    }
}

fn take_resolve_arg<T>(value: &Mutex<Option<T>>) -> Result<T> {
    value
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take()
        .ok_or_else(|| RequestError::ResolveAlreadyCalled.into())
}

fn filter_env(env: &Map<String, Value>, allowed: &str, disallowed: &str) -> Map<String, Value> {
    env.iter()
        .filter(|(key, _)| {
            key.starts_with(allowed) && (disallowed.is_empty() || !key.starts_with(disallowed))
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}
