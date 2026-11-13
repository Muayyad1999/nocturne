//! The whole Nocturne backend, assembled.
//!
//! The desktop shell and the headless server both call [`start`] and then talk
//! to the returned [`App`] through `App::rpc` and `App::subscribe`; neither
//! knows which feature modules exist.

pub use nocturne_app::{App, AppBuilder, AppConfig, AppError, AppResult, Event};

/// Register the core and every feature module on a builder.
pub fn init(builder: &mut AppBuilder) {
    nocturne_app::init(builder);
}

/// Assemble and start the application.
pub fn start(config: AppConfig) -> AppResult<App> {
    let mut builder = AppBuilder::new(config)?;
    init(&mut builder);
    Ok(builder.build())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn starts_and_answers_rpc() {
        let app = start(AppConfig::in_memory()).unwrap();
        let devices = app
            .rpc("devices.list", serde_json::Value::Null)
            .await
            .unwrap();
        assert_eq!(devices, serde_json::json!([]));
        assert!(app.methods().contains(&"settings.all"));
    }
}
