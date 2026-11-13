//! Platform-specific socket features. Everything here has a caller-side fallback on the
//! platforms where the module is absent.

#[cfg(target_os = "linux")]
pub(crate) mod linux;
