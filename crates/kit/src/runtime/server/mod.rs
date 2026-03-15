use http::HeaderMap;
use serde_json::Value;

mod app;
mod behavior;
mod data;
mod endpoint;
mod event;
mod index;
mod page;
mod page_crypto;
mod page_csp;
mod page_load;
mod page_serialize;
mod remote;
mod request;
mod types;
mod utils;
mod validate_headers;
pub use app::*;
pub use behavior::*;
pub use data::*;
pub use endpoint::*;
pub use event::*;
pub use index::*;
pub use page::*;
pub use page_crypto::*;
pub use page_csp::*;
pub use page_load::*;
pub use page_serialize::*;
pub use remote::*;
pub use request::*;
pub use types::*;
pub use utils::*;
pub use validate_headers::*;

const ENDPOINT_METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS", "HEAD"];
const PAGE_METHODS: &[&str] = &["GET", "POST", "HEAD"];
const ALLOWED_PAGE_METHODS: &[&str] = &["GET", "HEAD", "OPTIONS"];
const INVALIDATED_PARAM: &str = "x-sveltekit-invalidated";
const TRAILING_SLASH_PARAM: &str = "x-sveltekit-trailing-slash";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageRenderPlan {
    pub ssr: bool,
    pub csr: bool,
    pub prerender: bool,
    pub should_prerender_data: bool,
    pub data_pathname: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorPageRenderPlan {
    pub status: u16,
    pub error: Value,
    pub ssr: bool,
    pub csr: bool,
    pub branch_node_indexes: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeResponseEffects {
    pub headers: HeaderMap,
    pub set_cookie_headers: Vec<String>,
}

impl Default for RuntimeResponseEffects {
    fn default() -> Self {
        Self {
            headers: HeaderMap::new(),
            set_cookie_headers: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellPageResponse {
    pub status: u16,
    pub ssr: bool,
    pub csr: bool,
    pub action: Option<PageActionExecution>,
    pub effects: RuntimeResponseEffects,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageLoadedNode {
    pub node_index: usize,
    pub server_data: Option<Value>,
    pub data: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageLoadResult {
    Loaded {
        server_data: Option<Value>,
        data: Option<Value>,
    },
    Redirect {
        status: u16,
        location: String,
    },
    Error {
        status: u16,
        error: Value,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageErrorBoundary {
    pub status: u16,
    pub error: Value,
    pub branch: Vec<PageLoadedNode>,
    pub error_node_index: usize,
    pub ssr: bool,
    pub csr: bool,
    pub action: Option<PageActionExecution>,
    pub effects: RuntimeResponseEffects,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedPage {
    pub plan: PageRenderPlan,
    pub branch: Vec<PageLoadedNode>,
    pub action: Option<PageActionExecution>,
    pub effects: RuntimeResponseEffects,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutedErrorPage {
    pub plan: ErrorPageRenderPlan,
    pub branch: Vec<PageLoadedNode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorPageRequestResult {
    Rendered(ExecutedErrorPage),
    Redirect(ServerResponse),
    Static(ServerResponse),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageRuntimeDecision {
    Render(PageRenderPlan),
    Shell(ShellPageResponse),
    Early(ServerResponse),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageExecutionResult {
    Rendered { branch: Vec<PageLoadedNode> },
    Redirect(ServerResponse),
    ErrorBoundary(PageErrorBoundary),
    Fatal { status: u16, error: Value },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageRequestResult {
    Rendered(RenderedPage),
    Shell(ShellPageResponse),
    Early(ServerResponse),
    Redirect(ServerResponse),
    ErrorBoundary(PageErrorBoundary),
    Fatal {
        status: u16,
        error: Value,
        effects: RuntimeResponseEffects,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageActionExecution {
    pub headers: HeaderMap,
    pub result: ActionRequestResult,
}

impl PageActionExecution {
    pub fn status(&self) -> u16 {
        match &self.result {
            ActionRequestResult::Success { status, .. }
            | ActionRequestResult::Failure { status, .. }
            | ActionRequestResult::Redirect { status, .. } => *status,
            ActionRequestResult::Error { error } => error
                .get("status")
                .and_then(Value::as_u64)
                .map(|status| status as u16)
                .unwrap_or(500),
        }
    }

    pub fn redirect_response(&self) -> Option<ServerResponse> {
        let ActionRequestResult::Redirect { status, location } = &self.result else {
            return None;
        };

        let mut response = ServerResponse::new(*status);
        response.set_header("location", location);
        Some(response)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeExecutionResult {
    Response(ServerResponse),
    Page(PageRequestResult),
    NotFound,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeRespondResult {
    Response(ServerResponse),
    Page(PageRequestResult),
    ErrorPage(ErrorPageRequestResult),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionJsonResult {
    Success { status: u16, data: Option<Value> },
    Failure { status: u16, data: Value },
    Redirect { status: u16, location: String },
    Error { status: u16, error: Value },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionRequestResult {
    Success { status: u16, data: Option<Value> },
    Failure { status: u16, data: Value },
    Redirect { status: u16, location: String },
    Error { error: Value },
}
