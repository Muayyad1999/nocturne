//! Text tools: the regular-expression tester, a line diff, digests and Base64.
//!
//! The regex tester is `nocturne_tools::text::regex_test` and the diff is
//! `nocturne_drivers::hier::text_diff` — the same diff the configuration
//! screens show, so two texts compared here look exactly like two
//! configurations compared there.

use nocturne_app::{App, AppError, AppResult, Router};
use nocturne_drivers::hier;
use nocturne_tools::text;
use serde::{Deserialize, Serialize};

use crate::errors::tool;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct RegexParams {
    pub pattern: String,
    pub text: String,
    /// `i` case-insensitive, `m` multi-line (`^` and `$` per line). Either
    /// spelling works: `flags: "im"` or the two booleans.
    pub flags: Option<String>,
    pub case_insensitive: Option<bool>,
    pub multiline: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct DiffParams {
    pub a: String,
    pub b: String,
    /// Unchanged lines kept around each change; left out, everything is kept.
    pub context: Option<usize>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct BytesParams {
    /// The text to work on, as text.
    pub text: Option<String>,
    /// The bytes to work on, as Base64 — for anything that is not text.
    pub base64: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Base64Params {
    /// `encode` or `decode`.
    pub mode: String,
    pub text: String,
}

#[derive(Debug, Serialize)]
pub struct Digests {
    #[serde(flatten)]
    pub hashes: text::Hashes,
    pub bytes: usize,
    pub base64: String,
}

#[derive(Debug, Serialize)]
pub struct Base64Result {
    pub base64: String,
    pub bytes: usize,
    /// The decoded bytes as text, when they are valid UTF-8.
    pub text: Option<String>,
    /// Said only when the bytes are not text, so an empty box is not a mystery.
    pub note: Option<String>,
}

/// The largest text `tools.diff` compares, per side.
const MAX_DIFF_BYTES: usize = 4 * 1024 * 1024;

/// The bytes a `{text | base64}` parameter pair means.
fn bytes_of(params: &BytesParams) -> AppResult<Vec<u8>> {
    match (&params.text, &params.base64) {
        (Some(_), Some(_)) => Err(AppError::invalid(
            "give either `text` or `base64`, not both",
        )),
        (Some(text), None) => Ok(text.as_bytes().to_vec()),
        (None, Some(encoded)) => text::base64_decode(encoded).map_err(tool),
        (None, None) => Err(AppError::invalid("give `text` or `base64` to hash")),
    }
}

pub fn register(router: &mut Router) {
    // A bad pattern comes back in `error`, not as a failed call: this method
    // is called on every keystroke while somebody types a regex, and a
    // half-typed `(` is not a failure of anything.
    router.add("tools.regex", |_: App, params: RegexParams| async move {
        let flags = params.flags.unwrap_or_default().to_ascii_lowercase();
        let case_insensitive = params
            .case_insensitive
            .unwrap_or_else(|| flags.contains('i'));
        let multiline = params.multiline.unwrap_or_else(|| flags.contains('m'));
        Ok(text::regex_test(
            &params.pattern,
            &params.text,
            multiline,
            case_insensitive,
        ))
    });
    router.add("tools.diff", |_: App, params: DiffParams| async move {
        // A line diff is quadratic in the worst case and this method is on the
        // same thread as everything else; two configurations are kilobytes, so
        // the bound is generous and the refusal is clear.
        if params.a.len() > MAX_DIFF_BYTES || params.b.len() > MAX_DIFF_BYTES {
            return Err(AppError::invalid(format!(
                "each side of a diff may be at most {} MiB; for whole configurations use the configuration screens, which diff them by version",
                MAX_DIFF_BYTES / (1024 * 1024)
            )));
        }
        Ok(hier::text_diff(&params.a, &params.b, params.context))
    });
    router.add("tools.hash", |_: App, params: BytesParams| async move {
        let bytes = bytes_of(&params)?;
        Ok(Digests {
            hashes: text::hashes(&bytes),
            bytes: bytes.len(),
            base64: text::base64_encode(&bytes),
        })
    });
    router.add("tools.base64", |_: App, params: Base64Params| async move {
        match params.mode.trim().to_ascii_lowercase().as_str() {
            "encode" | "" => Ok(Base64Result {
                base64: text::base64_encode(params.text.as_bytes()),
                bytes: params.text.len(),
                text: Some(params.text),
                note: None,
            }),
            "decode" => {
                let bytes = text::base64_decode(&params.text).map_err(tool)?;
                let decoded = String::from_utf8(bytes.clone()).ok();
                Ok(Base64Result {
                    base64: params.text.trim().to_string(),
                    bytes: bytes.len(),
                    note: decoded.is_none().then(|| {
                        format!(
                            "{} bytes that are not valid UTF-8 text (a key, a certificate or a binary blob)",
                            bytes.len()
                        )
                    }),
                    text: decoded,
                })
            }
            other => Err(AppError::invalid(format!(
                "`{other}` is not a mode; use `encode` or `decode`"
            ))),
        }
    });
}
