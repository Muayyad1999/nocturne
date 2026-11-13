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

/// Decoded bytes as text, provided they are valid UTF-8 without control
/// characters. Anything else is not a password anyone typed, which means
/// the input was not what it looked like.
fn printable_text(bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    (!text.chars().any(char::is_control)).then(|| text.to_owned())
}
