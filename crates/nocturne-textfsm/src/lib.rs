//! TextFSM template engine with the bundled ntc-templates parser library.
//!
//! [TextFSM](https://github.com/google/textfsm) templates describe a small
//! state machine that turns the text a network device prints into rows of
//! named values. This crate implements the template language as the Python
//! reference implementation behaves, and embeds the community
//! [ntc-templates](https://github.com/networktocode/ntc-templates) library
//! (see [`ntc`]).
//!
//! ```
//! use nocturne_textfsm::{ntc, Template};
//!
//! // A template of your own ...
//! let template = Template::parse(
//!     "Value NAME (\\S+)\nValue STATE (up|down)\n\nStart\n  ^${NAME} is ${STATE} -> Record\n",
//! )?;
//! let rows = template.run("eth0 is up\neth1 is down\n")?;
//! assert_eq!(rows[1]["STATE"].as_str(), "down");
//!
//! // ... or one of the bundled ones, found the way ntc-templates does.
//! assert_eq!(ntc::find("cisco_ios", "sh ip int br"), Some("cisco_ios_show_ip_interface_brief"));
//! # Ok::<(), nocturne_textfsm::Error>(())
//! ```

mod error;
mod fsm;
pub mod ntc;
mod pyregex;
mod template;
mod text;
mod value;

pub use error::{Error, RunError, TemplateError};
pub use template::Template;
pub use value::{Record, Value};
