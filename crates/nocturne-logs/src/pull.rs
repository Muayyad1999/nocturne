//! `logs.pull`: reading a device's own log buffer.
//!
//! Most of a campus will never send syslog — nobody configured it, the
//! device predates the policy, it is behind a firewall that drops UDP 514.
//! For those devices the log screen would be empty, which reads as "nothing
//! happened here" when it means "nobody is listening". So the buffer the
//! device keeps for itself is read over the ordinary read-only CLI path and
//! stored like any other line, marked `via: pull`.
//!
//! Two things are deliberately different from a received line:
//!
//! * **The timestamp is the device's own.** A buffer is history; stamping it
//!   all at the moment of the pull would put a week of events in one second
//!   of the histogram. When the device wrote a date and a time it is used
//!   (interpreted in this machine's time zone, which is a guess this crate
//!   makes explicit with `ts_source: device`); when it wrote an uptime, or
//!   nothing, the time of the pull is used and `ts_source` says `received`.
//! * **Rules do not run over it.** A pulled line is old news, and a signal
//!   means "act on this now" — backing a switch up because its buffer still
//!   remembers a change from last Tuesday is not what the configuration
//!   feature is being told.

use std::collections::HashSet;

use chrono::{Datelike, Local, NaiveDate, NaiveDateTime, TimeZone};
use nocturne_app::util::now_ms;
use nocturne_app::{App, AppError, AppResult, connect, inventory};
use nocturne_drivers::hier::Syntax;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::db::{self, Prepared};
use crate::state::LogsState;

/// How many of a device's stored lines are checked for duplicates. A buffer
/// is a few hundred lines; this is several buffers' worth.
const DEDUP_WINDOW: usize = 5_000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pulled {
    pub device_id: String,
    pub command: String,
    /// Lines the buffer held that look like log lines.
    pub read: usize,
    pub stored: usize,
    /// Lines already stored from an earlier pull.
    pub duplicates: usize,
    /// Lines whose time came from the device rather than from the pull.
    pub dated_by_device: usize,
    pub oldest_at: Option<i64>,
    pub newest_at: Option<i64>,
    pub note: String,
}

/// The command that prints the log buffer of a platform, and whether its
/// output needs the "everything after the Log Buffer header" treatment.
fn command_for(platform: &nocturne_drivers::profile::Platform) -> AppResult<&'static str> {
    match platform.profile.config.syntax {
        Syntax::Ios => Ok("show logging"),
        Syntax::Routeros => Ok("/log print without-paging"),
        _ => Err(AppError::new(
            "unsupported",
            format!(
                "Nocturne does not know how to read the log buffer of a {} device ({}); \
                 point it at this Nocturne instance with `logs.device_setup` instead",
                platform.profile.name, platform.profile.id
            ),
        )),
    }
}

/// The lines of a buffer dump that are log lines.
///
/// `show logging` prints a page of counters first, then `Log Buffer (…):`
/// and the lines; RouterOS prints a column header and an index per line.
pub fn buffer_lines(output: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut in_buffer = false;
    for raw in output.lines() {
        let line = raw.trim_end();
        if line.trim().is_empty() {
            continue;
        }
        if line.contains("Log Buffer (") {
            in_buffer = true;
            continue;
        }
        // RouterOS prints "  # TIME  MESSAGE" and then numbered rows.
        if let Some(rest) = strip_routeros_index(line) {
            lines.push(rest.to_string());
            continue;
        }
        if line.trim_start().starts_with('#') {
            continue;
        }
        if in_buffer || has_mnemonic(line) {
            lines.push(line.trim_start().to_string());
        }
    }
    lines
}

/// `" 0 nov/13 09:13:01 system,info …"` → the part after the index.
fn strip_routeros_index(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 || digits > 6 {
        return None;
    }
    let rest = trimmed[digits..].trim_start();
    // Only when what follows looks like RouterOS's own time column, so an
    // IOS counter ("45: %SYS-5-…") is left for the syslog parser.
    let looks_like_time = rest.len() > 8
        && rest
            .split_whitespace()
            .next()
            .is_some_and(|first| first.contains('/') || first.matches(':').count() == 2);
    (looks_like_time && !trimmed[digits..].starts_with(':')).then_some(rest)
}

fn has_mnemonic(line: &str) -> bool {
    let bytes = line.as_bytes();
    bytes.iter().enumerate().any(|(index, byte)| {
        *byte == b'%'
            && line[index + 1..]
                .split(':')
                .next()
                .is_some_and(|tag| tag.len() < 64 && tag.matches('-').count() >= 2)
    })
}

/// Months as every platform in reach spells them.
const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];

/// A device's own timestamp as milliseconds since the epoch.
///
/// Year-less forms (which is most of them) are read as the most recent
/// matching date: a line stamped `Dec 31` seen on `Jan 2` belongs to last
/// year. The time zone is this machine's, because the line does not say and
/// a NOC's devices and its workstation are normally in the same one.
/// `bare_time` says whether a lone `09:13:01` is a time of day. It is on
/// RouterOS, whose `/log print` prints only the time for anything logged
/// today; it is not on IOS, where `service timestamps log uptime` prints
/// `00:02:11` meaning two minutes after the device booted. The two cannot be
/// told apart from the text, so the platform decides, and a timestamp that
/// cannot be read leaves the entry stamped with the time of the pull.
pub fn device_time(timestamp: &str, now: i64, bare_time: bool) -> Option<i64> {
    let text = timestamp.trim().trim_start_matches(['*', '.']).trim();
    if text.is_empty() {
        return None;
    }
    if text.as_bytes().first().is_some_and(u8::is_ascii_digit) && text.len() >= 10 {
        if let Ok(fixed) = chrono::DateTime::parse_from_rfc3339(text) {
            return Some(fixed.timestamp_millis());
        }
        for format in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S%.f"] {
            if let Ok(naive) = NaiveDateTime::parse_from_str(text, format) {
                return local(naive);
            }
        }
    }
    let mut month = None;
    let mut day = None;
    let mut year = None;
    let mut time = None;
    for token in text.split([' ', '\t', '/']).filter(|part| !part.is_empty()) {
        let lowered = token.to_ascii_lowercase();
        if month.is_none()
            && lowered.len() >= 3
            && let Some(index) = MONTHS.iter().position(|name| lowered.starts_with(name))
        {
            month = Some(index as u32 + 1);
            continue;
        }
        if token.contains(':') && token.matches(':').count() == 2 {
            time = Some(token);
            continue;
        }
        if let Ok(number) = token.parse::<u32>() {
            match number {
                1_900..=2_200 => year = Some(number as i32),
                1..=31 if day.is_none() => day = Some(number),
                _ => {}
            }
        }
    }
    let time = time?;
    let (hour, minute, second, nanos) = {
        let mut parts = time.split(':');
        let hour: u32 = parts.next()?.parse().ok()?;
        let minute: u32 = parts.next()?.parse().ok()?;
        let rest = parts.next()?;
        let (second, fraction) = match rest.split_once('.') {
            Some((second, fraction)) => (second, fraction),
            None => (rest, ""),
        };
        let second: u32 = second.parse().ok()?;
        let nanos: u32 = fraction
            .chars()
            .filter(char::is_ascii_digit)
            .chain(std::iter::repeat('0'))
            .take(9)
            .collect::<String>()
            .parse()
            .unwrap_or(0);
        (hour, minute, second, nanos)
    };
    let reference = Local.timestamp_millis_opt(now).single()?.naive_local();
    let (month, day) = match (month, day) {
        (Some(month), Some(day)) => (month, day),
        (None, None) if bare_time => (reference.month(), reference.day()),
        _ => return None,
    };
    let build = |year: i32| -> Option<i64> {
        let date = NaiveDate::from_ymd_opt(year, month, day)?;
        local(date.and_hms_nano_opt(hour, minute, second, nanos)?)
    };
    match year {
        Some(year) => build(year),
        None => {
            let candidate = build(reference.year())?;
            // A date in the future means the year rolled over since.
            if candidate > now + 86_400_000 {
                build(reference.year() - 1)
            } else {
                Some(candidate)
            }
        }
    }
}

fn local(naive: NaiveDateTime) -> Option<i64> {
    Local
        .from_local_datetime(&naive)
        .earliest()
        .map(|moment| moment.timestamp_millis())
}

/// Read a device's log buffer and store what is not already there.
pub async fn pull(app: &App, device_reference: &str) -> AppResult<Pulled> {
    let device = inventory::find(app, device_reference)?;
    let mut cli = connect::cli(app, &device.id, connect::CliOptions::default()).await?;
    let command = match command_for(&cli.platform) {
        Ok(command) => command,
        Err(err) => {
            cli.close().await;
            return Err(err);
        }
    };
    let output = cli.read(command).await;
    cli.close().await;
    let output = output?;
    if let Some(error) = &output.error {
        return Err(AppError::new(
            "device_error",
            format!("`{command}` was refused by {}: {error}", device.name),
        ));
    }
    store(app, &device.id, command, &output.output)
}

/// Store the lines of a buffer dump. Separate from the session so the
/// parsing and de-duplication can be tested against recorded output.
pub fn store(app: &App, device_id: &str, command: &str, output: &str) -> AppResult<Pulled> {
    let state = app.state::<LogsState>();
    let now = now_ms();
    let index = crate::attribute::refresh(app, now, 30_000)?;
    let known: HashSet<i64> = db::fingerprints_of(app.store(), device_id, DEDUP_WINDOW)?
        .into_iter()
        .collect();
    let device = inventory::get(app, device_id)?;
    let source = device
        .address
        .trim()
        .parse::<std::net::IpAddr>()
        .map(|ip| std::net::SocketAddr::new(ip, 0))
        .unwrap_or_else(|_| "0.0.0.0:0".parse().expect("a valid socket address"));

    // RouterOS writes a bare time for today's lines; IOS writes an uptime
    // that looks the same and means something else (see `device_time`).
    let bare_time_is_today = command.starts_with("/log");
    let lines = buffer_lines(output);
    let mut batch: Vec<Prepared> = Vec::new();
    let mut duplicates = 0;
    let mut dated_by_device = 0;
    let mut seen = HashSet::new();
    for line in &lines {
        let parsed = nocturne_netprobe::syslog::parse(line.as_bytes(), source);
        let fp = db::fingerprint(&parsed.raw);
        if known.contains(&fp) || !seen.insert(fp) {
            duplicates += 1;
            continue;
        }
        let mut prepared =
            crate::ingest::prepare(&state, &index, &parsed, now, "pull", Some(device_id));
        let dated = parsed
            .timestamp
            .as_deref()
            .and_then(|timestamp| device_time(timestamp, now, bare_time_is_today));
        if let Some(at) = dated {
            prepared.entry.ts = at;
            dated_by_device += 1;
        }
        if let Some(body) = prepared
            .entry
            .body
            .as_mut()
            .and_then(|body| body.as_object_mut())
        {
            body.insert(
                "ts_source".into(),
                json!(if dated.is_some() {
                    "device"
                } else {
                    "received"
                }),
            );
        }
        batch.push(prepared);
    }
    // Oldest first, so the ids of a buffer keep the order it was written in.
    batch.sort_by_key(|item| item.entry.ts);
    let stored = db::insert_batch(app.store(), &mut batch)?.stored;
    state.counters.stored(stored as u64);
    crate::tail::offer(app, &batch);
    let note = if lines.is_empty() {
        format!(
            "`{command}` returned no log lines: the device's buffer is empty or logging to it is off"
        )
    } else {
        String::new()
    };
    app.audit(
        "logs.pull",
        Some(device_id),
        format!("Read {stored} log lines from `{}`", device.name),
        json!({"command": command, "read": lines.len(), "duplicates": duplicates}),
    );
    Ok(Pulled {
        device_id: device_id.to_string(),
        command: command.to_string(),
        read: lines.len(),
        stored,
        duplicates,
        dated_by_device,
        oldest_at: batch.first().map(|item| item.entry.ts),
        newest_at: batch.last().map(|item| item.entry.ts),
        note,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape `show logging` prints on IOS and IOS-XE.
    const SHOW_LOGGING: &str = "\
Syslog logging: enabled (0 messages dropped, 3 messages rate-limited, 0 flushes, 0 overruns, xml disabled, filtering disabled)
    Console logging: level debugging, 31 messages logged, xml disabled
    Buffer logging: level debugging, 31 messages logged, xml disabled
    Trap logging: level informational, 33 message lines logged
        Logging to 10.10.99.100 (udp port 514, audit disabled, link up), 33 message lines logged

Log Buffer (8192 bytes):
*Mar  1 00:12:02.123: %SYS-5-CONFIG_I: Configured from console by admin on vty0 (10.0.0.5)
*Mar  1 00:12:10.001: %LINK-3-UPDOWN: Interface GigabitEthernet1/0/3, changed state to down
*Mar  1 00:12:11.002: %LINEPROTO-5-UPDOWN: Line protocol on Interface GigabitEthernet1/0/3, changed state to down
";

    const LOG_PRINT: &str = "\
 #   TIME                 MESSAGE
 0   nov/13/2026 09:13:01 system,info,account user admin logged in from 10.0.0.5 via ssh
 1   nov/13/2026 09:14:55 interface,info ether3 link down
";

    #[test]
    fn the_counters_at_the_top_of_show_logging_are_not_log_lines() {
        let lines = buffer_lines(SHOW_LOGGING);
        assert_eq!(lines.len(), 3, "{lines:#?}");
        assert!(lines[0].starts_with("*Mar  1 00:12:02.123: %SYS-5-CONFIG_I"));
        assert!(
            !lines.iter().any(|line| line.contains("Buffer logging")),
            "the header counted as a log line"
        );
    }

    #[test]
    fn routeros_rows_lose_their_index_column() {
        let lines = buffer_lines(LOG_PRINT);
        assert_eq!(
            lines,
            [
                "nov/13/2026 09:13:01 system,info,account user admin logged in from 10.0.0.5 via ssh",
                "nov/13/2026 09:14:55 interface,info ether3 link down",
            ]
        );
    }

    #[test]
    fn a_cisco_sequence_number_is_left_for_the_syslog_parser() {
        let lines = buffer_lines("45: *Mar  1 00:02:11.003: %SYS-5-RESTART: System restarted");
        assert_eq!(
            lines,
            ["45: *Mar  1 00:02:11.003: %SYS-5-RESTART: System restarted"]
        );
    }

    fn at(text: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(text)
            .expect("a time")
            .timestamp_millis()
    }

    #[test]
    fn a_device_timestamp_without_a_year_is_read_as_the_most_recent_one() {
        let now = at("2026-11-13T09:00:00+00:00");
        let march = device_time("*Mar  1 00:12:02.123", now, false).expect("parsed");
        let december = device_time("Dec 31 23:59:00", now, false).expect("parsed");
        assert!(march < now, "March this year is behind us");
        assert!(
            december < now,
            "a December line seen in November is last December, not next"
        );
        assert!(
            now - december < 400 * 86_400_000,
            "and not from the last century"
        );
    }

    #[test]
    fn the_forms_devices_write_are_all_understood() {
        let now = at("2026-11-13T09:00:00+00:00");
        for form in [
            "2026-11-13T09:13:01.123+00:00",
            "2026-11-13 09:13:01",
            "nov/13/2026 09:13:01",
            "Nov 13 09:13:01",
            "Nov 13 2026 09:13:01",
            "2026 Nov 13 09:13:01",
            "09:13:01",
        ] {
            let parsed = device_time(form, now, true)
                .unwrap_or_else(|| panic!("`{form}` is a time a device writes"));
            assert!(
                (parsed - at("2026-11-13T09:13:01+00:00")).abs() < 86_400_000,
                "`{form}` landed at {parsed}"
            );
        }
        assert_eq!(
            device_time("1w2d", now, true),
            None,
            "an uptime is not a date"
        );
        assert_eq!(device_time("", now, true), None);
        assert_eq!(
            device_time("00:02:11", now, false),
            None,
            "on IOS a bare time is an uptime, and a line stamped with one \
             keeps the time of the pull rather than a made-up date"
        );
    }
}
