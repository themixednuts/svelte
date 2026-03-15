use std::cell::RefCell;

use crate::{RequestStoreError, Result, RuntimeEvent};

#[derive(Debug, Clone)]
pub struct RequestStore {
    pub event: RuntimeEvent,
}

impl RequestStore {
    pub fn new(event: RuntimeEvent) -> Self {
        Self { event }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TracingState<T> {
    pub enabled: bool,
    pub root: T,
    pub current: T,
}

thread_local! {
    static REQUEST_STORE: RefCell<Option<RequestStore>> = const { RefCell::new(None) };
}

pub fn merge_tracing<T: Clone>(tracing: &TracingState<T>, current: T) -> TracingState<T> {
    TracingState {
        enabled: tracing.enabled,
        root: tracing.root.clone(),
        current,
    }
}

pub fn get_request_event() -> Result<RuntimeEvent> {
    try_get_request_store()
        .map(|store| store.event)
        .ok_or_else(|| RequestStoreError::MissingCurrentRequestEvent.into())
}

pub fn get_request_store() -> Result<RequestStore> {
    try_get_request_store().ok_or_else(|| RequestStoreError::MissingRequestStore.into())
}

pub fn try_get_request_store() -> Option<RequestStore> {
    REQUEST_STORE.with(|store| store.borrow().clone())
}

pub fn with_request_store<T>(store: Option<RequestStore>, callback: impl FnOnce() -> T) -> T {
    REQUEST_STORE.with(|current| {
        let previous = current.replace(store);
        let result = callback();
        current.replace(previous);
        result
    })
}
