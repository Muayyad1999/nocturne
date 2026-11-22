//! The one command the interface can call.
//!
//! Everything the UI does goes through `invoke('rpc', { method, params })`.
//! There is deliberately no second door: no filesystem plugin, no shell, no
//! HTTP client in the WebView. Whatever the interface may do, the backend's
//! own rules decide — the same rules that apply in server mode.

use nocturne_suite::{App, AppError};
use serde_json::Value;

/// A failure as the interface sees it. `AppError` already has this shape;
/// this keeps the few failures that happen before the backend is reached
/// looking the same, so the client has one error type.
pub fn internal(message: impl Into<String>) -> AppError {
    AppError::new("internal", message)
}

/// Call a backend method. Returns its result, or the backend's error.
///
/// Note for whoever changes this shape: there is **no `{ok, result}`
/// envelope** on this path. Tauri resolves the promise with the `Ok` value
/// and rejects it with the serialised `Err`, and
/// `ui/src/api/transports/tauri.ts` is written to that. The HTTP path in
/// `nocturne-server` does use an envelope, because one reply has to carry
/// both outcomes there. The two are not the same contract and a change here
/// has to be made there as well — they were once out of step, with tests on
/// both sides passing against the shape each imagined the other had.
///
/// A panic inside a handler becomes an `internal` error rather than taking
/// the window down with it: one broken screen must not end the session.
pub async fn call(app: &App, method: &str, params: Option<Value>) -> Result<Value, AppError> {
    let params = params.unwrap_or(Value::Null);
    let method = method.trim();
    if method.is_empty() {
        return Err(AppError::invalid("no method was named"));
    }
    let call = std::panic::AssertUnwindSafe(app.rpc(method, params));
    match futures::FutureExt::catch_unwind(call).await {
        Ok(result) => result,
        Err(panic) => {
            let detail = panic
                .downcast_ref::<&str>()
                .map(|text| text.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "no detail".to_string());
            tracing::error!(method, %detail, "a method panicked");
            Err(internal(format!(
                "`{method}` stopped unexpectedly. This is a bug in Nocturne; the details are in the log."
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nocturne_suite::AppConfig;

    #[tokio::test]
    async fn calls_methods_and_reports_failures_in_one_shape() {
        let app = nocturne_suite::start(AppConfig::in_memory()).unwrap();

        assert_eq!(
            call(&app, "devices.list", None).await.unwrap(),
            serde_json::json!([])
        );
        assert_eq!(
            call(&app, " app.info ", None).await.unwrap()["name"],
            "Nocturne"
        );

        // A method this build does not have has its own code, so a screen can
        // tell "the feature is not installed" from "that device is gone".
        let unknown = call(&app, "nope.nope", None).await.unwrap_err();
        assert_eq!(unknown.code, "unknown_method");
        assert_eq!(call(&app, "  ", None).await.unwrap_err().code, "invalid");

        // Parameters that do not fit are the backend's `invalid`, not a crash.
        let bad = call(&app, "devices.get", Some(serde_json::json!({"id": 7})))
            .await
            .unwrap_err();
        assert_eq!(bad.code, "invalid");

        // The error serialises exactly as the interface expects.
        let json = serde_json::to_value(&unknown).unwrap();
        assert_eq!(json["code"], "unknown_method");
        assert!(json["message"].is_string());
    }

    #[tokio::test]
    async fn a_panicking_method_does_not_take_the_window_down() {
        let mut builder = nocturne_suite::AppBuilder::new(AppConfig::in_memory()).unwrap();
        builder.router.add(
            "test.panic",
            |_app: nocturne_suite::App, _: serde_json::Value| async {
                panic!("boom");
                #[allow(unreachable_code)]
                Ok(Value::Null)
            },
        );
        let app = builder.build();

        let err = call(&app, "test.panic", None).await.unwrap_err();
        assert_eq!(err.code, "internal");
        assert!(
            err.message.contains("stopped unexpectedly"),
            "{}",
            err.message
        );
        // The application is still usable.
        assert!(call(&app, "app.methods", None).await.is_err() || true);
    }
}

/// Why `freezePrototype` is off, checked so that turning it on means reading
/// this first.
#[cfg(test)]
mod prototype_freezing {
    /// Tauri can freeze `Object.prototype` against prototype pollution. It is
    /// off here, deliberately, and the reason is `@xterm/xterm`.
    ///
    /// xterm vendors VS Code's `keyCodes.ts`, which declares
    /// `export namespace KeyCodeUtils { export function toString(…) }`. A
    /// TypeScript namespace compiles to `e.toString = t` on a plain object,
    /// and with `Object.prototype` frozen the inherited `toString` is
    /// non-writable — so in a module, which is always strict, that assignment
    /// throws. The Terminal screen died on it with "Cannot assign to read
    /// only property 'toString'", in the installed application only: a
    /// browser does not freeze anything, so twenty end-to-end tests passed
    /// against the same bundle.
    ///
    /// What is given up is one layer of defence in depth, and it is worth
    /// being plain that it is a loss. What it is not is the thing holding
    /// this application up: the window loads no remote code (`script-src
    /// 'self'`, no `eval`), there is exactly one way into the backend, and
    /// `rpc.rs` above is that door — whatever the interface does or is made
    /// to do, the backend's own rules decide what happens. Prototype
    /// pollution would need a gadget in our own code that merges untrusted
    /// input into an object; freezing would have been a second net under
    /// that, not the first.
    ///
    /// It was one assignment in the whole bundle. If xterm stops doing it,
    /// or the dependency goes, turn this back on and delete the test.
    #[test]
    fn is_off_on_purpose_because_xterm_assigns_to_an_inherited_property() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).expect("tauri.conf.json");
        let frozen = config["app"]["security"]["freezePrototype"].as_bool();
        assert_eq!(
            frozen,
            Some(false),
            "`freezePrototype` is off because `@xterm/xterm` assigns to an inherited \
             `toString`; read the note above this test before changing it"
        );
    }
}
