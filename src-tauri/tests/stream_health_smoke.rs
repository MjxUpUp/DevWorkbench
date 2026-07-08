//! Smoke test for `kernel_impl::stream_health` — uses integration-test
//! binary to sidestep the dev-workbench Windows 0xc0000139 binary startup
//! issue (memory `applib-test-binary-entrypoint-block.md`).

use app_lib::kernel_impl::stream_health::{
    classify_stream_failure, degrade_policy, DegradeAction, RootCause, StreamSignals,
};

#[test]
fn classify_prefers_specific_signals() {
    let s = StreamSignals {
        status_code: Some(400),
        idle_secs: None,
        bytes_received: 0,
        saw_message_stop: false,
        context_overflow_hint: true,
    };
    assert_eq!(
        classify_stream_failure(&s),
        RootCause::ContextExhausted
    );
}

#[test]
fn classify_idle_partial_is_model_timeout() {
    let s = StreamSignals {
        status_code: Some(200),
        idle_secs: Some(75),
        bytes_received: 5000,
        saw_message_stop: false,
        context_overflow_hint: false,
    };
    assert_eq!(classify_stream_failure(&s), RootCause::ModelTimeout);
}

#[test]
fn classify_buffer_overflow() {
    let s = StreamSignals {
        status_code: Some(200),
        idle_secs: Some(2),
        bytes_received: 12000,
        saw_message_stop: false,
        context_overflow_hint: false,
    };
    assert_eq!(classify_stream_failure(&s), RootCause::ProxyBufferFull);
}

#[test]
fn classify_premature_eof_fallback() {
    let s = StreamSignals {
        status_code: Some(200),
        idle_secs: Some(3),
        bytes_received: 500,
        saw_message_stop: false,
        context_overflow_hint: false,
    };
    assert_eq!(classify_stream_failure(&s), RootCause::PrematureEof);
}

#[test]
fn classify_zero_bytes_is_network_reset() {
    let s = StreamSignals {
        status_code: Some(503),
        idle_secs: Some(30),
        bytes_received: 0,
        saw_message_stop: false,
        context_overflow_hint: false,
    };
    assert_eq!(classify_stream_failure(&s), RootCause::NetworkReset);
}

#[test]
fn retry_policy_table() {
    assert!(RootCause::ModelTimeout.is_retryable());
    assert!(RootCause::NetworkReset.is_retryable());
    assert!(RootCause::ProxyBufferFull.is_retryable());
    assert!(RootCause::PrematureEof.is_retryable());
    assert!(!RootCause::ContextExhausted.is_retryable());
}

#[test]
fn degrade_no_output_fails() {
    assert_eq!(
        degrade_policy(RootCause::ModelTimeout, false),
        DegradeAction::FailFinal
    );
    match degrade_policy(RootCause::ContextExhausted, false) {
        DegradeAction::FailWithHint(h) => assert!(h.contains("shorten")),
        other => panic!("expected hint, got {:?}", other),
    }
}

#[test]
fn degrade_with_output_returns_partial() {
    assert_eq!(
        degrade_policy(RootCause::ModelTimeout, true),
        DegradeAction::ReturnPartial
    );
    assert_eq!(
        degrade_policy(RootCause::ContextExhausted, true),
        DegradeAction::ReturnPartial
    );
}

#[test]
fn buffer_full_suggests_max_tokens_shrink() {
    assert_eq!(
        RootCause::ProxyBufferFull.suggested_max_tokens_reduction(),
        Some(2048)
    );
    assert_eq!(
        RootCause::ModelTimeout.suggested_max_tokens_reduction(),
        None
    );
}