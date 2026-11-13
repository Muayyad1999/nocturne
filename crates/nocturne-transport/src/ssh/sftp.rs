//! File transfer over the SFTP subsystem.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use russh::client;
use russh_sftp::client::SftpSession;
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::protocol::{FileAttributes, StatusCode};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use super::client::{SessionInner, release};
use crate::error::TransportError;

const COPY_BUFFER: usize = 64 * 1024;
/// Symbolic links of a listing resolved at the same time.
const RESOLVE_CONCURRENCY: usize = 32;
/// The last progress report waits this long for room in the channel.
const FINAL_PROGRESS_TIMEOUT: Duration = Duration::from_millis(250);
/// Suffix of the temporary file a download is written to.
const PARTIAL_SUFFIX: &str = ".nocturne-part";

/// `(bytes transferred, total bytes)` updates of a transfer. Intermediate
/// updates are dropped rather than delaying the transfer when the receiver
/// lags behind.
type Progress = mpsc::Sender<(u64, u64)>;

/// An SFTP session on an [`SshSession`](super::SshSession).
///
/// Keeps the SSH connection alive for as long as it exists.
pub struct Sftp {
    sftp: Arc<SftpSession>,
    session: Arc<SessionInner>,
}

/// A file or directory on the server.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SftpEntry {
    pub name: String,
    pub path: String,
    /// For a symbolic link: whether it points to a directory.
    pub is_dir: bool,
    pub is_link: bool,
    pub size: u64,
    /// Seconds since the Unix epoch.
    pub modified: Option<i64>,
    /// The permission bits (`0o7777` mask), e.g. `0o644`.
    pub permissions: Option<u32>,
}

impl fmt::Debug for Sftp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sftp").finish_non_exhaustive()
    }
}

impl Sftp {
    pub(crate) async fn start(
        stream: russh::ChannelStream<client::Msg>,
        session: Arc<SessionInner>,
    ) -> Result<Sftp, TransportError> {
        let sftp = SftpSession::new(stream).await.map_err(sftp_error)?;
        Ok(Sftp {
            sftp: Arc::new(sftp),
            session,
        })
    }

    /// The entries of a directory (without `.` and `..`), sorted by name.
    pub async fn list(&self, path: &str) -> Result<Vec<SftpEntry>, TransportError> {
        let listing = self.sftp.read_dir(path).await.map_err(sftp_error)?;
        let mut entries: Vec<SftpEntry> = listing
            .map(|entry| entry_from(entry.file_name(), entry.path(), &entry.metadata()))
            .collect();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        self.resolve_links(&mut entries).await;
        Ok(entries)
    }

    /// Details of one path. A symbolic link is reported with `is_link` set
    /// and the size, type and times of what it points to (when that exists).
    pub async fn stat(&self, path: &str) -> Result<SftpEntry, TransportError> {
        let attributes = self.sftp.symlink_metadata(path).await.map_err(sftp_error)?;
        let mut entry = entry_from(file_name(path), path.to_owned(), &attributes);
        if entry.is_link
            && let Ok(target) = self.sftp.metadata(path).await
        {
            follow_link(&mut entry, &target);
        }
        Ok(entry)
    }

    /// The whole content of a remote file.
    pub async fn read(&self, path: &str) -> Result<Vec<u8>, TransportError> {
        let mut file = self.sftp.open(path).await.map_err(sftp_error)?;
        let mut content = Vec::new();
        file.read_to_end(&mut content).await.map_err(io_error)?;
        file.shutdown().await.map_err(io_error)?;
        Ok(content)
    }

    /// Creates (or truncates) a remote file with the given content.
    pub async fn write(&self, path: &str, data: &[u8]) -> Result<(), TransportError> {
        let mut file = self.sftp.create(path).await.map_err(sftp_error)?;
        file.write_all(data).await.map_err(io_error)?;
        // Waits for every write, and the close, to be acknowledged.
        file.shutdown().await.map_err(io_error)
    }

    /// Copies a remote file to `local`, returning the number of bytes.
    ///
    /// The data goes to a temporary file next to `local` that replaces it
    /// only once the transfer is complete, so a failed download never leaves
    /// a truncated file behind.
    pub async fn download(
        &self,
        remote: &str,
        local: &Path,
        progress: Option<mpsc::Sender<(u64, u64)>>,
    ) -> Result<u64, TransportError> {
        let partial = partial_path(local)?;
        let mut source = self.sftp.open(remote).await.map_err(sftp_error)?;
        let total = source.metadata().await.map_or(0, |attributes| attributes.len());

        let transfer = async {
            let mut target = tokio::fs::File::create(&partial).await?;
            let copied = copy(&mut source, &mut target, total, progress.as_ref()).await?;
            target.flush().await?;
            drop(target);
            tokio::fs::rename(&partial, local).await?;
            Ok::<u64, io::Error>(copied)
        };
        match transfer.await {
            Ok(copied) => {
                report_done(progress.as_ref(), copied).await;
                Ok(copied)
            }
            Err(error) => {
                let _ = tokio::fs::remove_file(&partial).await;
                Err(io_error(error))
            }
        }
    }

    /// Copies `local` to a remote file (created or truncated), returning the
    /// number of bytes.
    pub async fn upload(
        &self,
        local: &Path,
        remote: &str,
        progress: Option<mpsc::Sender<(u64, u64)>>,
    ) -> Result<u64, TransportError> {
        let mut source = tokio::fs::File::open(local).await?;
        let total = source.metadata().await?.len();
        let mut target = self.sftp.create(remote).await.map_err(sftp_error)?;
        let copied = copy(&mut source, &mut target, total, progress.as_ref())
            .await
            .map_err(io_error)?;
        target.shutdown().await.map_err(io_error)?;
        report_done(progress.as_ref(), copied).await;
        Ok(copied)
    }

    pub async fn mkdir(&self, path: &str) -> Result<(), TransportError> {
        self.sftp.create_dir(path).await.map_err(sftp_error)
    }

    /// Removes a file (or symbolic link) or an empty directory.
    pub async fn remove(&self, path: &str) -> Result<(), TransportError> {
        let attributes = self.sftp.symlink_metadata(path).await.map_err(sftp_error)?;
        let removed = if attributes.is_dir() {
            self.sftp.remove_dir(path).await
        } else {
            self.sftp.remove_file(path).await
        };
        removed.map_err(sftp_error)
    }

    pub async fn rename(&self, from: &str, to: &str) -> Result<(), TransportError> {
        self.sftp.rename(from, to).await.map_err(sftp_error)
    }

    /// The canonical absolute form of `path` (`"."` gives the home directory).
    pub async fn realpath(&self, path: &str) -> Result<String, TransportError> {
        self.sftp.canonicalize(path).await.map_err(sftp_error)
    }

    /// Ends the SFTP session. The SSH connection is closed too if nothing
    /// else is using it.
    pub async fn close(self) {
        let _ = self.sftp.close().await;
        drop(self.sftp);
        release(self.session).await;
    }

    /// Fills in what the symbolic links of a listing point to.
    async fn resolve_links(&self, entries: &mut [SftpEntry]) {
        let links: Vec<usize> = (0..entries.len())
            .filter(|&index| entries[index].is_link)
            .collect();
        for batch in links.chunks(RESOLVE_CONCURRENCY) {
            let mut lookups = JoinSet::new();
            for &index in batch {
                let sftp = Arc::clone(&self.sftp);
                let path = entries[index].path.clone();
                lookups.spawn(async move { (index, sftp.metadata(path).await) });
            }
            while let Some(joined) = lookups.join_next().await {
                // A dangling link simply keeps the attributes of the link.
                if let Ok((index, Ok(target))) = joined {
                    follow_link(&mut entries[index], &target);
                }
            }
        }
    }
}

fn entry_from(name: String, path: String, attributes: &FileAttributes) -> SftpEntry {
    SftpEntry {
        name,
        path,
        is_dir: attributes.is_dir(),
        is_link: attributes.is_symlink(),
        size: attributes.len(),
        modified: attributes.mtime.map(i64::from),
        permissions: attributes.permissions.map(|mode| mode & 0o7777),
    }
}

fn follow_link(entry: &mut SftpEntry, target: &FileAttributes) {
    entry.is_dir = target.is_dir();
    entry.size = target.len();
    entry.modified = target.mtime.map(i64::from).or(entry.modified);
    entry.permissions = target
        .permissions
        .map(|mode| mode & 0o7777)
        .or(entry.permissions);
}

/// The last component of a remote (POSIX) path.
fn file_name(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rsplit('/').next() {
        Some(name) if !name.is_empty() => name.to_owned(),
        _ => path.to_owned(),
    }
}

fn partial_path(local: &Path) -> Result<PathBuf, TransportError> {
    let Some(name) = local.file_name() else {
        return Err(TransportError::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} does not name a file", local.display()),
        )));
    };
    let mut partial = name.to_owned();
    partial.push(PARTIAL_SUFFIX);
    Ok(local.with_file_name(partial))
}

async fn copy<R, W>(
    source: &mut R,
    target: &mut W,
    total: u64,
    progress: Option<&Progress>,
) -> io::Result<u64>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buffer = vec![0u8; COPY_BUFFER];
    let mut copied = 0u64;
    loop {
        let read = source.read(&mut buffer).await?;
        if read == 0 {
            return Ok(copied);
        }
        target.write_all(&buffer[..read]).await?;
        copied += read as u64;
        if let Some(progress) = progress {
            let _ = progress.try_send((copied, total.max(copied)));
        }
    }
}

async fn report_done(progress: Option<&Progress>, copied: u64) {
    if let Some(progress) = progress {
        let _ = tokio::time::timeout(FINAL_PROGRESS_TIMEOUT, progress.send((copied, copied))).await;
    }
}

fn sftp_error(error: SftpError) -> TransportError {
    match error {
        SftpError::Status(status) => {
            let kind = match status.status_code {
                StatusCode::NoSuchFile => io::ErrorKind::NotFound,
                StatusCode::PermissionDenied => io::ErrorKind::PermissionDenied,
                StatusCode::OpUnsupported => io::ErrorKind::Unsupported,
                StatusCode::NoConnection | StatusCode::ConnectionLost => {
                    return TransportError::Closed;
                }
                _ => io::ErrorKind::Other,
            };
            let message = if status.error_message.trim().is_empty() {
                status.status_code.to_string()
            } else {
                status.error_message
            };
            TransportError::Io(io::Error::new(kind, message))
        }
        SftpError::Timeout => TransportError::Timeout,
        SftpError::IO(message) => TransportError::Io(io::Error::other(message)),
        SftpError::UnexpectedPacket => {
            TransportError::Protocol("unexpected SFTP packet".to_owned())
        }
        SftpError::Limited(message) | SftpError::UnexpectedBehavior(message) => {
            TransportError::Protocol(message)
        }
    }
}

/// Errors of SFTP file handles arrive wrapped in `io::Error`.
fn io_error(error: io::Error) -> TransportError {
    if error.kind() == io::ErrorKind::TimedOut {
        TransportError::Timeout
    } else {
        TransportError::Io(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_file_names() {
        assert_eq!(file_name("/etc/hosts"), "hosts");
        assert_eq!(file_name("/var/log/"), "log");
        assert_eq!(file_name("relative.txt"), "relative.txt");
        assert_eq!(file_name("/"), "/");
        assert_eq!(file_name("."), ".");
    }

    #[test]
    fn partial_file_sits_next_to_the_target() {
        let partial = partial_path(Path::new("/tmp/dir/config.txt")).unwrap();
        assert_eq!(partial, Path::new("/tmp/dir/config.txt.nocturne-part"));
        assert!(partial_path(Path::new("/")).is_err());
    }

    #[test]
    fn entries_expose_type_size_and_permission_bits() {
        let mut attributes = FileAttributes {
            size: Some(42),
            permissions: Some(0o100_644),
            mtime: Some(1_700_000_000),
            ..FileAttributes::default()
        };
        let file = entry_from("a".into(), "/x/a".into(), &attributes);
        assert!(!file.is_dir && !file.is_link);
        assert_eq!(file.size, 42);
        assert_eq!(file.permissions, Some(0o644));
        assert_eq!(file.modified, Some(1_700_000_000));

        attributes.permissions = Some(0o040_755);
        assert!(entry_from("d".into(), "/x/d".into(), &attributes).is_dir);

        attributes.permissions = Some(0o120_777);
        let mut link = entry_from("l".into(), "/x/l".into(), &attributes);
        assert!(link.is_link && !link.is_dir);
        let target = FileAttributes {
            size: Some(4096),
            permissions: Some(0o040_700),
            ..FileAttributes::default()
        };
        follow_link(&mut link, &target);
        assert!(link.is_link && link.is_dir);
        assert_eq!(link.size, 4096);
        assert_eq!(link.permissions, Some(0o700));
        assert_eq!(link.modified, Some(1_700_000_000));
    }

    #[test]
    fn status_codes_map_to_io_kinds() {
        let status = |code| {
            SftpError::Status(russh_sftp::protocol::Status {
                id: 1,
                status_code: code,
                error_message: String::new(),
                language_tag: String::new(),
            })
        };
        let TransportError::Io(error) = sftp_error(status(StatusCode::NoSuchFile)) else {
            panic!("expected an I/O error");
        };
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert_eq!(error.to_string(), "No such file");
        let TransportError::Io(error) = sftp_error(status(StatusCode::PermissionDenied)) else {
            panic!("expected an I/O error");
        };
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(sftp_error(status(StatusCode::ConnectionLost)).code(), "closed");
        assert_eq!(sftp_error(SftpError::Timeout).code(), "timeout");
    }

    #[tokio::test]
    async fn copy_reports_progress() {
        let data = vec![7u8; COPY_BUFFER * 2 + 100];
        let (tx, mut rx) = mpsc::channel(16);
        let mut target = Vec::new();
        let copied = copy(&mut &data[..], &mut target, data.len() as u64, Some(&tx))
            .await
            .unwrap();
        assert_eq!(copied as usize, data.len());
        assert_eq!(target, data);
        let mut last = (0, 0);
        while let Ok(update) = rx.try_recv() {
            assert!(update.0 > last.0);
            assert_eq!(update.1, data.len() as u64);
            last = update;
        }
        assert_eq!(last.0, data.len() as u64);
    }
}
