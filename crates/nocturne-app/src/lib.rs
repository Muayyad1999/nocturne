//! Nocturne's application core.
//!
//! Everything the UI can do goes through [`App::rpc`]; everything the UI is
//! told arrives as an [`Event`]. The desktop shell and the headless server are
//! thin adapters over those two.

pub mod app;
pub mod credentials;
pub mod error;
pub mod inventory;
pub mod model;
pub mod settings;
pub mod store;
pub mod util;
pub mod vault;

pub use app::{App, AppBuilder, AppConfig, Event, NoParams, Router};
pub use error::{AppError, AppResult};

/// Assemble and start the application.
pub fn start(config: AppConfig) -> AppResult<App> {
    let mut builder = AppBuilder::new(config)?;
    settings::init(&mut builder);
    credentials::init(&mut builder);
    inventory::init(&mut builder);
    Ok(builder.build())
}
