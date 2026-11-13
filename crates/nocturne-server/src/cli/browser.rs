//! `--open`: show the web UI in the user's browser.
//!
//! The link contains the access token, and command lines are visible to
//! every user of a machine. So the browser is not handed the link itself but
//! a small private page in the data directory that forwards to it.

use std::fs;
use std::path::{Path, PathBuf};

const FILE_NAME: &str = "server-open.html";

/// The forwarding page. It is removed when this is dropped.
pub(super) struct Launcher {
    page: PathBuf,
}

impl Launcher {
    /// Open `url` in the default browser. Failing to is not fatal: the link
    /// is on standard output either way.
    pub(super) fn open(data_dir: &Path, url: &str) -> Option<Launcher> {
        let page = data_dir.join(FILE_NAME);
        let _ = fs::remove_file(&page);
        if let Err(err) = write_private(&page, &forwarding_page(url)) {
            tracing::warn!(%err, "could not prepare the page that opens the browser");
            return None;
        }
        let launcher = Launcher { page };
        match open::that_detached(&launcher.page) {
            Ok(()) => Some(launcher),
            Err(err) => {
                tracing::warn!(%err, "could not open a browser; open the printed link by hand");
                None
            }
        }
    }
}

impl Drop for Launcher {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.page);
    }
}

fn write_private(path: &Path, content: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(content.as_bytes())
}

fn forwarding_page(url: &str) -> String {
    let url = escape_attribute(url);
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta http-equiv=\"refresh\" content=\"0; url={url}\">\n<title>Opening Nocturne</title>\n\
         </head>\n<body>\n<p>Opening Nocturne. If nothing happens, <a href=\"{url}\">continue</a>.</p>\n\
         </body>\n</html>\n"
    )
}

fn escape_attribute(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_forwards_to_the_link() {
        let page = forwarding_page("https://127.0.0.1:7878/#token=abc");
        assert!(page.contains(
            r#"<meta http-equiv="refresh" content="0; url=https://127.0.0.1:7878/#token=abc">"#
        ));
        assert!(page.contains(r#"<a href="https://127.0.0.1:7878/#token=abc">"#));
        assert_eq!(escape_attribute(r#"a&b"c<d>"#), "a&amp;b&quot;c&lt;d&gt;");
    }

    #[cfg(unix)]
    #[test]
    fn the_page_is_private_and_never_overwrites() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        write_private(&path, "x").unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(write_private(&path, "y").is_err());
    }
}
