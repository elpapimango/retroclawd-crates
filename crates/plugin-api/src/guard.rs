//! Panic containment for the plugin side of the C boundary.
//!
//! A Rust panic must never unwind out of an `extern "C"` function (that
//! aborts the whole process, taking the host and every other machine with
//! it). Plugins wrap the body of every exported function in [`guarded`] /
//! [`guarded_call`]: a panic is caught, the first one is remembered (its text
//! is what the host shows), and the call returns an error value. After a
//! crash the plugin is treated as dead -- see [`has_crashed`] -- and its
//! fast-path functions report a halted machine rather than looping forever.
//!
//! The state is per library: every plugin links its own copy of this crate.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Mutex;

use crate::abi::{RcCallResult, StatusCode};

static CRASH: Mutex<Option<String>> = Mutex::new(None);

/// The text of the first panic caught in this library, if any.
#[must_use]
pub fn crash_message() -> Option<String> {
    CRASH
        .lock()
        .map_or_else(|e| e.into_inner().clone(), |g| g.clone())
}

/// Whether a panic was ever caught in this library.
#[must_use]
pub fn has_crashed() -> bool {
    crash_message().is_some()
}

fn payload_text(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_string()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "unknown panic".to_string()
    }
}

fn record(site: &str, payload: &(dyn std::any::Any + Send)) -> String {
    let text = format!("panic in {site}: {}", payload_text(payload));
    let mut slot = CRASH.lock().unwrap_or_else(|e| e.into_inner());
    if slot.is_none() {
        *slot = Some(text.clone());
    }
    text
}

/// Runs `f`; a panic is recorded and `on_panic` returned instead.
pub fn guarded<T>(site: &str, on_panic: T, f: impl FnOnce() -> T) -> T {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(value) => value,
        Err(payload) => {
            record(site, payload.as_ref());
            on_panic
        }
    }
}

/// Runs a call that returns an [`RcCallResult`]; a panic (or an earlier
/// crash of this plugin) becomes an `InternalError` carrying the text.
pub fn guarded_call(site: &str, f: impl FnOnce() -> RcCallResult) -> RcCallResult {
    if let Some(message) = crash_message() {
        return RcCallResult::error_with_message(
            StatusCode::InternalError,
            &format!("the plugin crashed earlier ({message})"),
        );
    }
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(result) => result,
        Err(payload) => {
            let text = record(site, payload.as_ref());
            RcCallResult::error_with_message(StatusCode::InternalError, &text)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // One test only: the crash record is process-global by design.
    #[test]
    fn a_panic_is_contained_remembered_and_turns_later_calls_into_errors() {
        assert!(!has_crashed());
        assert_eq!(guarded("fine", 1, || 7), 7);
        assert!(!has_crashed());

        let value = guarded("step", 4u32, || -> u32 { panic!("index out of range") });
        assert_eq!(value, 4);
        assert_eq!(
            crash_message().as_deref(),
            Some("panic in step: index out of range")
        );

        // A second panic does not replace the first message.
        let _ = guarded("pc", 0u16, || -> u16 { panic!("later") });
        assert_eq!(
            crash_message().as_deref(),
            Some("panic in step: index out of range")
        );

        // Calls after a crash are refused with the original text.
        let result = guarded_call("instance_call", RcCallResult::empty_ok);
        assert_eq!(result.status.code, StatusCode::InternalError as u32);
        // SAFETY: produced by `error_with_message` in this library.
        let text = String::from_utf8(unsafe { result.payload.into_vec() }).unwrap();
        assert!(text.contains("crashed earlier"), "{text}");
        assert!(text.contains("index out of range"), "{text}");
    }
}
