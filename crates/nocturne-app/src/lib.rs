//! Nocturne's application core.
//!
//! Everything the UI can do goes through [`App::rpc`]; everything the UI is
//! told arrives as an [`Event`]. The desktop shell and the headless server are
//! thin adapters over those two.

pub mod app;
pub mod connect;
pub mod credentials;
pub mod error;
pub mod inventory;
pub mod jobs;
pub mod model;
pub mod profiles;
pub mod settings;
pub mod store;
pub mod system;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod util;
pub mod vault;

pub use app::{App, AppBuilder, AppConfig, Event, NoParams, Router};
pub use error::{AppError, AppResult};

/// Register every core module on a builder. Feature crates build on these.
pub fn init(builder: &mut AppBuilder) {
    settings::init(builder);
    credentials::init(builder);
    inventory::init(builder);
    jobs::init(builder);
    profiles::init(builder);
    connect::init(builder);
    system::init(builder);
}
