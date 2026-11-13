//! Writing a session to its log file.
//!
//! A log is plain text: what a person would have read on the terminal, one
//! line per line, each prefixed with the local time (with UTC offset) at
//! which it was completed. Escape sequences, redraws and pager prompts are
//! resolved by the same screen model the CLI driver uses. Input is never
//! logged by itself — what the device echoes is in its output, and what it
//! does not echo (passwords) stays out.

use std::io;
use std::path::Path;

use chrono::{DateTime, Local, SecondsFormat};
use nocturne_app::util::now_ms;
use nocturne_app::{App, AppError, AppResult};
use nocturne_drivers::term::Screen;
use nocturne_transport::TransportKind;
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncWriteExt, BufWriter};

use crate::logs::{self, ActiveLog, LogRecord};
use crate::model::kind_name;
use crate::text::incomplete_utf8_suffix;

/// Output without a line break after which the line is written as it stands,
/// so that a stream that never ends a line cannot grow the screen model.
const MAX_UNBROKEN: usize = 64 * 1024;
/// Longest title kept in a file name.
const MAX_TITLE: usize = 80;

/// What a log is a log of.
pub(crate) struct Subject<'a> {
    pub title: &'a str,
    pub device_id: Option<&'a str>,
    pub kind: TransportKind,
    pub peer: &'a str,
}

pub(crate) struct SessionLog {
    /// `YYYY-MM/<title>-<started>.log`, relative to the sessions directory.
    name: String,
    file: BufWriter<File>,
    screen: Screen,
    /// The start of a UTF-8 character whose rest is in the next chunk.
    carry: Vec<u8>,
    /// Bytes seen since a line was last completed.
    unbroken: usize,
    _active: ActiveLog,
}

impl SessionLog {
    /// Start a log file for a session and write its opening line.
    pub async fn create(app: &App, subject: Subject<'_>) -> AppResult<SessionLog> {
        let Some(root) = logs::sessions_dir(app) else {
            return Err(AppError::unavailable(
                "this Nocturne instance keeps no files (it runs without a data directory), so sessions cannot be logged",
            ));
        };
        let started = Local::now();
        let month = started.format("%Y-%m").to_string();
        let directory = root.join(&month);
        tokio::fs::create_dir_all(&directory)
            .await
            .map_err(|err| cannot_write(&directory, &err))?;

        let stem = format!(
            "{}-{}",
            sanitise(subject.title),
            started.format("%Y%m%d-%H%M%S")
        );
        let (file_name, file) = create_unique(&directory, &stem).await?;
        let name = format!("{month}/{file_name}");
        let active = logs::record(
            app,
            &LogRecord {
                name: name.clone(),
                device_id: subject.device_id.map(str::to_string),
                title: subject.title.to_string(),
                kind: subject.kind,
                started_at: now_ms(),
            },
        )?;

        let mut log = SessionLog {
            name,
            file: BufWriter::new(file),
            screen: Screen::new(),
            carry: Vec::new(),
            unbroken: 0,
            _active: active,
        };
        let opening = format!(
            "--- log started: {} ({}, {}) ---",
            subject.title,
            kind_name(subject.kind),
            subject.peer
        );
        log.write(&[opening], started)
            .await
            .map_err(|err| cannot_write(&root.join(&log.name), &err))?;
        Ok(log)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Log output from the device. Whatever it completes is on disk when this returns.
    pub async fn feed(&mut self, data: &[u8]) -> io::Result<()> {
        let lines = self.absorb(data);
        if lines.is_empty() {
            return Ok(());
        }
        self.write(&lines, Local::now()).await
    }

    /// The lines `data` completes.
    fn absorb(&mut self, data: &[u8]) -> Vec<String> {
        let mut bytes = std::mem::take(&mut self.carry);
        bytes.extend_from_slice(data);
        let complete = bytes.len() - incomplete_utf8_suffix(&bytes);
        self.screen.feed(&bytes[..complete]);
        self.carry = bytes.split_off(complete);

        let lines = self.screen.take_lines();
        if !lines.is_empty() {
            self.unbroken = 0;
            return lines;
        }
        self.unbroken += data.len();
        if self.unbroken <= MAX_UNBROKEN {
            return lines;
        }
        // Also clears an escape sequence that was opened and never closed,
        // which would otherwise swallow everything after it.
        self.unbroken = 0;
        std::mem::take(&mut self.screen).finish()
    }

    async fn write(&mut self, lines: &[String], at: DateTime<Local>) -> io::Result<()> {
        let stamp = at.to_rfc3339_opts(SecondsFormat::Millis, false);
        let mut block = String::new();
        for line in lines {
            block.push_str(&stamp);
            if !line.is_empty() {
                block.push(' ');
                block.push_str(line);
            }
            block.push('\n');
        }
        self.file.write_all(block.as_bytes()).await?;
        self.file.flush().await
    }

    /// Write what is left on the current line and the closing line.
    pub async fn finish(mut self, app: &App, reason: &str) -> io::Result<()> {
        let mut lines = std::mem::take(&mut self.screen).finish();
        lines.push(format!("--- log ended: {reason} ---"));
        let written = self.write(&lines, Local::now()).await;
        logs::mark_ended(app, &self.name);
        written
    }
}

fn cannot_write(path: &Path, err: &io::Error) -> AppError {
    AppError::new(
        "io",
        format!(
            "the session log {} cannot be written: {err}",
            path.display()
        ),
    )
}

/// Create `<stem>.log`, or `<stem>-2.log` … when sessions to one target
/// started within the same second.
async fn create_unique(directory: &Path, stem: &str) -> AppResult<(String, File)> {
    for attempt in 1..=1000 {
        let file_name = if attempt == 1 {
            format!("{stem}.log")
        } else {
            format!("{stem}-{attempt}.log")
        };
        let path = directory.join(&file_name);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await
        {
            Ok(file) => return Ok((file_name, file)),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(cannot_write(&path, &err)),
        }
    }
    Err(AppError::new(
        "io",
        format!(
            "no free file name for the session log {stem}.log in {}",
            directory.display()
        ),
    ))
}

/// A session title as part of a file name on any operating system.
pub(crate) fn sanitise(title: &str) -> String {
    let mut name = String::new();
    for ch in title.chars() {
        let keep = ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_' | '@' | '+' | '=');
        if keep {
            name.push(ch);
        } else if !name.ends_with('_') {
            name.push('_');
        }
    }
    let name: String = name
        .trim_matches(|ch| matches!(ch, '_' | '.' | '-'))
        .chars()
        .take(MAX_TITLE)
        .collect();
    if name.is_empty() {
        "session".to_string()
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_become_safe_file_names() {
        assert_eq!(sanitise("SW1"), "SW1");
        assert_eq!(sanitise("admin@10.0.0.1"), "admin@10.0.0.1");
        assert_eq!(sanitise("Local shell"), "Local_shell");
        assert_eq!(sanitise("/dev/ttyUSB0"), "dev_ttyUSB0");
        assert_eq!(sanitise("COM3"), "COM3");
        assert_eq!(sanitise("admin@[fe80::1]:2222"), "admin@_fe80_1_2222");
        assert_eq!(sanitise("../../etc/passwd"), "etc_passwd");
        assert_eq!(sanitise("a\\b:c*d?e\"f<g>h|i"), "a_b_c_d_e_f_g_h_i");
        assert_eq!(sanitise("çœur ☃"), "ur");
        assert_eq!(sanitise("..."), "session");
        assert_eq!(sanitise(""), "session");
        assert_eq!(sanitise(&"x".repeat(300)).len(), MAX_TITLE);
    }
}
