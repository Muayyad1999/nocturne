//! Secrets as they appear in device configurations: decoding the reversible
//! ones, telling the others apart, and generating new passwords.
//!
//! Cisco "type 7" and Junos `$9$` are obfuscation, not encryption: their
//! algorithms are public and need no key, so decoding them is a matter of
//! arithmetic. Hashes cannot be decoded; [`identify`] says which is which and
//! how much each form is worth.

mod cisco7;
mod identify;
mod junos9;
mod password;

pub use cisco7::{cisco7_decode, cisco7_encode};
pub use identify::{SecretKind, identify};
pub use junos9::{junos9_decode, junos9_encode};
pub use password::{MAX_PASSWORD_LENGTH, PASSWORD_SYMBOLS, PasswordPolicy, generate_password};

/// The longest secret the encoders accept: far beyond what any device
/// takes, it only keeps junk input cheap.
const MAX_SECRET_LEN: usize = 1024;

/// The longest text the decoders and [`identify`] look at: no configuration
/// line with a secret in it comes near this.
const MAX_INPUT_LEN: usize = 8 * 1024;

/// An error for a secret that is too long to encode.
fn too_long(what: &str, length: usize) -> crate::ToolError {
    crate::ToolError::TooLarge(format!(
        "a {what} of {} characters is beyond the limit of {}",
        crate::ip::group_thousands(&length.to_string()),
        crate::ip::group_thousands(&MAX_SECRET_LEN.to_string())
    ))
}

/// Decoded bytes as text, provided they are valid UTF-8 without control
/// characters. Anything else is not a password anyone typed, which means
/// the input was not what it looked like.
fn printable_text(bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    (!text.chars().any(char::is_control)).then(|| text.to_owned())
}
