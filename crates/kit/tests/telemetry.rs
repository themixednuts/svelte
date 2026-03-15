use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use svelte_kit::{
    Error, HttpError, RecordSpanError, RecordSpanParams, Redirect, TelemetryApi,
    TelemetryAttributes, TelemetryError, TelemetryException, TelemetrySpan, TelemetryStatus,
    TelemetryTracer, TelemetryValue, load_otel, record_span,
};

#[test]
fn otel_is_none_when_tracing_is_disabled() {
    let otel = load_otel(false, Err(())).expect("disabled tracing should not error");
    assert!(otel.is_none());
}

#[test]
fn otel_is_defined_when_tracing_is_enabled() {
    let tracer = Arc::new(MockTracer::default());
    let otel = load_otel(
        true,
        Ok(TelemetryApi {
            tracer,
            span_status_error_code: 2,
        }),
    )
    .expect("enabled tracing should succeed");
    assert!(otel.is_some());
}

#[test]
fn otel_errors_when_enabled_but_missing() {
    let error = match load_otel(true, Err(())) {
        Ok(_) => panic!("missing otel should error"),
        Err(error) => error,
    };
    assert_eq!(
        error.to_string(),
        "Tracing is enabled (see `config.kit.experimental.instrumentation.server` in your svelte.config.js), but `@opentelemetry/api` is not available. This error will likely resolve itself when you set up your tracing instrumentation in `instrumentation.server.js`. For more information, see https://svelte.dev/docs/kit/observability#opentelemetry-api"
    );
    assert!(matches!(
        error,
        Error::Telemetry(TelemetryError::MissingOpenTelemetryApi)
    ));
}

#[tokio::test]
async fn record_span_uses_noop_span_when_disabled() {
    let result = record_span(
        None,
        RecordSpanParams {
            name: "test",
            attributes: BTreeMap::new(),
            fn_call: |span: Arc<dyn TelemetrySpan>| async move {
                span.end();
                Ok::<_, RecordSpanError>("result")
            },
        },
    )
    .await
    .expect("noop span should succeed");

    assert_eq!(result, "result");
}

#[tokio::test]
async fn record_span_sets_attributes_for_known_and_unknown_errors() {
    let tracer = Arc::new(MockTracer::default());
    let otel = TelemetryApi {
        tracer: tracer.clone(),
        span_status_error_code: 2,
    };

    let error = record_span(
        Some(&otel),
        RecordSpanParams {
            name: "test",
            attributes: BTreeMap::from([(
                "test-attribute".to_string(),
                TelemetryValue::Bool(true),
            )]),
            fn_call: |_span| async move {
                Err::<(), _>(RecordSpanError::Http(HttpError::new(
                    500,
                    "Found but badly",
                )))
            },
        },
    )
    .await
    .expect_err("http error should bubble");

    assert_eq!(
        error,
        RecordSpanError::Http(HttpError::new(500, "Found but badly"))
    );
    let state = tracer.state.lock().expect("tracer state");
    assert_eq!(state.started_name.as_deref(), Some("test"));
    assert_eq!(
        state.started_attributes,
        BTreeMap::from([("test-attribute".to_string(), TelemetryValue::Bool(true))])
    );
    let span = state.last_span.clone().expect("span state");
    drop(state);

    let span = span.lock().expect("span");
    assert_eq!(
        span.attributes,
        vec![BTreeMap::from([
            (
                "test.result.type".to_string(),
                TelemetryValue::from("known_error")
            ),
            (
                "test.result.status".to_string(),
                TelemetryValue::from(500_u16)
            ),
            (
                "test.result.message".to_string(),
                TelemetryValue::from("Found but badly"),
            ),
        ])]
    );
    assert_eq!(
        span.exceptions,
        vec![TelemetryException {
            name: "HttpError".to_string(),
            message: "Found but badly".to_string(),
            stack: None,
        }]
    );
    assert_eq!(
        span.statuses,
        vec![TelemetryStatus {
            code: 2,
            message: Some("Found but badly".to_string()),
        }]
    );
    assert_eq!(span.end_count, 1);
}

#[tokio::test]
async fn record_span_handles_redirect_and_generic_errors() {
    let tracer = Arc::new(MockTracer::default());
    let otel = TelemetryApi {
        tracer: tracer.clone(),
        span_status_error_code: 2,
    };

    let redirect = record_span(
        Some(&otel),
        RecordSpanParams {
            name: "test",
            attributes: BTreeMap::new(),
            fn_call: |_span| async move {
                Err::<(), _>(RecordSpanError::Redirect(Redirect::new(
                    302,
                    "/redirect-location",
                )))
            },
        },
    )
    .await
    .expect_err("redirect should bubble");
    assert_eq!(
        redirect,
        RecordSpanError::Redirect(Redirect::new(302, "/redirect-location"))
    );

    let generic = record_span(
        Some(&otel),
        RecordSpanParams {
            name: "test",
            attributes: BTreeMap::new(),
            fn_call: |_span| async move {
                Err::<(), _>(RecordSpanError::Error {
                    name: "Error".to_string(),
                    message: "Something went wrong".to_string(),
                    stack: Some("stack".to_string()),
                })
            },
        },
    )
    .await
    .expect_err("generic error should bubble");
    assert_eq!(
        generic,
        RecordSpanError::Error {
            name: "Error".to_string(),
            message: "Something went wrong".to_string(),
            stack: Some("stack".to_string()),
        }
    );

    let other = record_span(
        Some(&otel),
        RecordSpanParams {
            name: "test",
            attributes: BTreeMap::new(),
            fn_call: |_span| async move {
                Err::<(), _>(RecordSpanError::Other("string error".to_string()))
            },
        },
    )
    .await
    .expect_err("other error should bubble");
    assert_eq!(other, RecordSpanError::Other("string error".to_string()));
}

#[derive(Default)]
struct MockTracer {
    state: Mutex<MockTracerState>,
}

#[derive(Default)]
struct MockTracerState {
    started_name: Option<String>,
    started_attributes: TelemetryAttributes,
    last_span: Option<Arc<Mutex<MockSpanState>>>,
}

impl TelemetryTracer for MockTracer {
    fn start_active_span(
        &self,
        name: &str,
        attributes: &TelemetryAttributes,
    ) -> Arc<dyn TelemetrySpan> {
        let span = Arc::new(Mutex::new(MockSpanState::default()));
        let mut state = self.state.lock().expect("tracer state");
        state.started_name = Some(name.to_string());
        state.started_attributes = attributes.clone();
        state.last_span = Some(span.clone());
        Arc::new(MockSpan(span))
    }
}

#[derive(Default)]
struct MockSpanState {
    attributes: Vec<TelemetryAttributes>,
    statuses: Vec<TelemetryStatus>,
    exceptions: Vec<TelemetryException>,
    end_count: usize,
}

struct MockSpan(Arc<Mutex<MockSpanState>>);

impl TelemetrySpan for MockSpan {
    fn set_attributes(&self, attributes: TelemetryAttributes) {
        self.0.lock().expect("span").attributes.push(attributes);
    }

    fn set_status(&self, status: TelemetryStatus) {
        self.0.lock().expect("span").statuses.push(status);
    }

    fn record_exception(&self, exception: TelemetryException) {
        self.0.lock().expect("span").exceptions.push(exception);
    }

    fn end(&self) {
        self.0.lock().expect("span").end_count += 1;
    }
}
