//! Syslog severity levels.

use serde::{Deserialize, Serialize};

/// One syslog severity.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SyslogLevel {
    /// The numeric severity, 0 (most severe) to 7.
    pub level: u8,
    /// Its name in RFC 5424.
    pub name: String,
    /// The keyword Cisco IOS uses for it (`logging trap warnings`).
    pub cisco_keyword: String,
    /// What a message of this severity means.
    pub meaning: String,
}

/// `(RFC 5424 name, Cisco keyword, meaning)` for severities 0 to 7.
const LEVELS: [(&str, &str, &str); 8] = [
    ("Emergency", "emergencies", "System is unusable"),
    ("Alert", "alerts", "Immediate action needed"),
    ("Critical", "critical", "Critical conditions"),
    ("Error", "errors", "Error conditions"),
    ("Warning", "warnings", "Warning conditions"),
    (
        "Notice",
        "notifications",
        "Normal but significant conditions",
    ),
    ("Informational", "informational", "Informational messages"),
    ("Debug", "debugging", "Debugging messages"),
];

/// The eight syslog severities, most severe first. Configuring a level on a
/// device logs that level and every more severe (lower-numbered) one.
pub fn syslog_levels() -> Vec<SyslogLevel> {
    LEVELS
        .iter()
        .zip(0u8..)
        .map(|((name, cisco_keyword, meaning), level)| SyslogLevel {
            level,
            name: (*name).to_owned(),
            cisco_keyword: (*cisco_keyword).to_owned(),
            meaning: (*meaning).to_owned(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_zero_to_seven() {
        let levels = syslog_levels();
        let rows: Vec<(u8, &str, &str)> = levels
            .iter()
            .map(|level| {
                (
                    level.level,
                    level.name.as_str(),
                    level.cisco_keyword.as_str(),
                )
            })
            .collect();
        assert_eq!(
            rows,
            [
                (0, "Emergency", "emergencies"),
                (1, "Alert", "alerts"),
                (2, "Critical", "critical"),
                (3, "Error", "errors"),
                (4, "Warning", "warnings"),
                (5, "Notice", "notifications"),
                (6, "Informational", "informational"),
                (7, "Debug", "debugging"),
            ]
        );
        assert!(levels.iter().all(|level| !level.meaning.is_empty()));
    }
}
