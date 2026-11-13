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
//! let rows = ntc::parse("cisco_ios", "sh ip int br", "Vlan1  10.0.0.1  YES NVRAM  up  up\n")?;
//! assert_eq!(rows.unwrap()[0]["IP_ADDRESS"].as_str(), "10.0.0.1");
//! # Ok::<(), nocturne_textfsm::Error>(())
//! ```
//!
//! # How the behaviour is pinned to the reference
//!
//! * `tests/semantics/*.yml` state, case by case, what the Python
//!   implementation does; `tools/semantics.py check` proves those statements
//!   and `tests/semantics.rs` holds this crate to them.
//! * `tests/fixtures.rs` parses a sample of real device output from the
//!   ntc-templates test corpus; `tests/conformance.rs` parses all of it when
//!   `NTC_TEMPLATES_DIR` points at a checkout of the corpus.
//! * Three more comparisons run on data produced with the scripts in
//!   `tools/`: every regex evaluation the reference performs on the corpus
//!   (`PYREGEX_ORACLE`), random regular expressions (`PYREGEX_FUZZ`) and
//!   random templates (`TEXTFSM_CASES`).

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
