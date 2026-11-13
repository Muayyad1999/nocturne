//! Reading a run afterwards: "47 switches say X, these 3 differ".
//!
//! Comparing raw output across devices is useless without trimming the lines
//! that are *supposed* to differ — uptimes, timestamps, counters. The patterns
//! come from the `automation` settings so a site can add its own, and the
//! groups report exactly which lines were ignored so nobody is fooled by a
//! difference that was trimmed away.

use nocturne_app::error::{AppError, AppResult};
use regex::Regex;
use serde::Serialize;

use crate::model::{DeviceResult, DeviceState, Run};
use crate::settings::Settings;

#[derive(Debug, Serialize)]
pub struct Comparison {
    pub command: String,
    /// Devices that have output for this command.
    pub devices: usize,
    /// Devices in the run with no output for it (unreached, failed early…).
    pub missing: Vec<Missing>,
    /// Largest first. The first group is the one the others are diffed against.
    pub groups: Vec<Group>,
    /// The regular expressions whose lines were left out of the comparison.
    pub ignored_patterns: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct Missing {
    pub device_id: String,
    pub name: String,
    pub state: String,
    pub reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Group {
    pub devices: Vec<GroupMember>,
    /// The output every device in the group produced (the first one's, in full).
    pub output: String,
    /// Against the largest group; empty for the largest group itself.
    pub diff: Vec<DiffLine>,
}

#[derive(Debug, Serialize)]
pub struct GroupMember {
    pub device_id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DiffLine {
    /// `=` same, `-` only in the majority, `+` only in this group.
    pub op: char,
    pub text: String,
}

fn compile(patterns: &[String]) -> Vec<Regex> {
    patterns
        .iter()
        .filter_map(|pattern| Regex::new(pattern).ok())
        .collect()
}

/// The output with volatile lines and trailing whitespace removed.
fn normalize(output: &str, volatile: &[Regex]) -> String {
    output
        .lines()
        .map(str::trim_end)
        .filter(|line| !volatile.iter().any(|pattern| pattern.is_match(line)))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn compare(
    results: &[DeviceResult],
    command: &str,
    settings: &Settings,
    extra: &[String],
) -> AppResult<Comparison> {
    let wanted = command.trim();
    if wanted.is_empty() {
        return Err(AppError::invalid("name the command to compare"));
    }
    let mut patterns = settings.volatile_patterns.clone();
    patterns.extend(extra.iter().cloned());
    for pattern in extra {
        Regex::new(pattern)
            .map_err(|err| AppError::invalid(format!("`{pattern}` is not a regexp: {err}")))?;
    }
    let volatile = compile(&patterns);

    let mut buckets: Vec<(String, String, Vec<GroupMember>)> = Vec::new();
    let mut missing = Vec::new();
    for result in results {
        let output = result
            .outputs
            .iter()
            .find(|output| output.command.trim() == wanted && output.error.is_none());
        let Some(output) = output else {
            missing.push(Missing {
                device_id: result.device_id.clone(),
                name: result.name.clone(),
                state: result.state.as_str().to_string(),
                reason: result.error.clone().or_else(|| result.reason.clone()),
            });
            continue;
        };
        let key = normalize(&output.output, &volatile);
        let member = GroupMember {
            device_id: result.device_id.clone(),
            name: result.name.clone(),
        };
        match buckets.iter_mut().find(|(bucket, ..)| bucket == &key) {
            Some((_, _, members)) => members.push(member),
            None => buckets.push((key, output.output.clone(), vec![member])),
        }
    }

    // Largest first; ties broken by the first device's name so the answer is stable.
    buckets.sort_by(|a, b| {
        b.2.len()
            .cmp(&a.2.len())
            .then_with(|| a.2[0].name.cmp(&b.2[0].name))
    });
    let devices = buckets.iter().map(|(.., members)| members.len()).sum();
    let majority = buckets.first().map(|(key, ..)| key.clone());
    let groups = buckets
        .into_iter()
        .enumerate()
        .map(|(index, (key, output, members))| Group {
            devices: members,
            output,
            diff: if index == 0 {
                Vec::new()
            } else {
                diff_lines(majority.as_deref().unwrap_or_default(), &key)
            },
        })
        .collect();

    Ok(Comparison {
        command: wanted.to_string(),
        devices,
        missing,
        groups,
        ignored_patterns: patterns,
    })
}

/// How many lines the quadratic diff is allowed to look at. Beyond that the
/// answer is "all of it changed", which is true and cheap.
const DIFF_LIMIT: usize = 2000;

/// A line diff, longest-common-subsequence over the part that actually differs.
pub fn diff_lines(before: &str, after: &str) -> Vec<DiffLine> {
    let a: Vec<&str> = before.lines().collect();
    let b: Vec<&str> = after.lines().collect();
    let mut out = Vec::new();

    let common_head = a
        .iter()
        .zip(&b)
        .take_while(|(left, right)| left == right)
        .count();
    let tail_limit = a.len().min(b.len()) - common_head;
    let common_tail = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take_while(|(left, right)| left == right)
        .take(tail_limit)
        .count();

    for line in &a[..common_head] {
        out.push(DiffLine {
            op: '=',
            text: (*line).to_string(),
        });
    }
    let mid_a = &a[common_head..a.len() - common_tail];
    let mid_b = &b[common_head..b.len() - common_tail];
    if mid_a.len() * mid_b.len() > DIFF_LIMIT * DIFF_LIMIT {
        for line in mid_a {
            out.push(DiffLine {
                op: '-',
                text: (*line).to_string(),
            });
        }
        for line in mid_b {
            out.push(DiffLine {
                op: '+',
                text: (*line).to_string(),
            });
        }
    } else {
        out.extend(lcs(mid_a, mid_b));
    }
    for line in &a[a.len() - common_tail..] {
        out.push(DiffLine {
            op: '=',
            text: (*line).to_string(),
        });
    }
    out
}

fn lcs(a: &[&str], b: &[&str]) -> Vec<DiffLine> {
    let (rows, cols) = (a.len() + 1, b.len() + 1);
    let mut table = vec![0usize; rows * cols];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            table[i * cols + j] = if a[i] == b[j] {
                table[(i + 1) * cols + j + 1] + 1
            } else {
                table[(i + 1) * cols + j].max(table[i * cols + j + 1])
            };
        }
    }
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            out.push(DiffLine {
                op: '=',
                text: a[i].to_string(),
            });
            i += 1;
            j += 1;
        } else if table[(i + 1) * cols + j] >= table[i * cols + j + 1] {
            out.push(DiffLine {
                op: '-',
                text: a[i].to_string(),
            });
            i += 1;
        } else {
            out.push(DiffLine {
                op: '+',
                text: b[j].to_string(),
            });
            j += 1;
        }
    }
    for line in &a[i..] {
        out.push(DiffLine {
            op: '-',
            text: (*line).to_string(),
        });
    }
    for line in &b[j..] {
        out.push(DiffLine {
            op: '+',
            text: (*line).to_string(),
        });
    }
    out
}

// ------------------------------------------------------------------ search

#[derive(Debug, Serialize)]
pub struct Match {
    pub device_id: String,
    pub name: String,
    pub command: String,
    /// 1-based, within that command's output.
    pub line_no: usize,
    pub line: String,
}

pub fn search(results: &[DeviceResult], query: &str, regex: bool) -> AppResult<Vec<Match>> {
    if query.trim().is_empty() {
        return Err(AppError::invalid("give something to search for"));
    }
    let pattern = if regex {
        Some(
            Regex::new(query)
                .map_err(|err| AppError::invalid(format!("`{query}` is not a regexp: {err}")))?,
        )
    } else {
        None
    };
    let needle = query.to_lowercase();
    let mut matches = Vec::new();
    for result in results {
        for output in &result.outputs {
            for (index, line) in output.output.lines().enumerate() {
                let hit = match &pattern {
                    Some(pattern) => pattern.is_match(line),
                    None => line.to_lowercase().contains(&needle),
                };
                if hit {
                    matches.push(Match {
                        device_id: result.device_id.clone(),
                        name: result.name.clone(),
                        command: output.command.clone(),
                        line_no: index + 1,
                        line: line.trim_end().to_string(),
                    });
                }
            }
        }
    }
    Ok(matches)
}

// ------------------------------------------------------------------ export

pub fn export(run: &Run, results: &[DeviceResult], format: &str) -> AppResult<String> {
    match format {
        "json" => serde_json::to_string_pretty(&serde_json::json!({
            "run": run, "results": results
        }))
        .map_err(|err| AppError::internal(err.to_string())),
        "csv" => {
            let mut writer = csv::Writer::from_writer(Vec::new());
            let io = |err: csv::Error| AppError::internal(format!("could not write CSV: {err}"));
            writer
                .write_record(["device", "state", "command", "error", "output"])
                .map_err(io)?;
            for result in results {
                if result.outputs.is_empty() {
                    writer
                        .write_record([
                            result.name.as_str(),
                            result.state.as_str(),
                            "",
                            result.error.as_deref().unwrap_or_default(),
                            "",
                        ])
                        .map_err(io)?;
                }
                for output in &result.outputs {
                    writer
                        .write_record([
                            result.name.as_str(),
                            result.state.as_str(),
                            output.command.as_str(),
                            output.error.as_deref().unwrap_or_default(),
                            output.output.as_str(),
                        ])
                        .map_err(io)?;
                }
            }
            String::from_utf8(
                writer
                    .into_inner()
                    .map_err(|err| AppError::internal(err.to_string()))?,
            )
            .map_err(|err| AppError::internal(err.to_string()))
        }
        "text" => {
            let mut out = format!(
                "# {} — {} device(s): {} ok, {} failed, {} unreached, {} skipped\n",
                run.name, run.devices, run.ok, run.failed, run.unreached, run.skipped
            );
            if let Some(caveat) = &run.caveat {
                out.push_str(&format!("# {caveat}\n"));
            }
            for result in results {
                out.push_str(&format!(
                    "\n===== {} [{}]{}\n",
                    result.name,
                    result.state.as_str(),
                    result
                        .error
                        .as_ref()
                        .map(|error| format!(" {error}"))
                        .unwrap_or_default()
                ));
                if result.state == DeviceState::Skipped
                    && let Some(reason) = &result.reason
                {
                    out.push_str(&format!("{reason}\n"));
                }
                for output in &result.outputs {
                    out.push_str(&format!("\n--- {}\n{}\n", output.command, output.output));
                }
                for step in &result.steps {
                    out.push_str(&format!(
                        "\n--- step `{}` [{}] {}\n",
                        step.name, step.state, step.detail
                    ));
                }
            }
            Ok(out)
        }
        other => Err(AppError::invalid(format!(
            "unknown export format `{other}` (csv, json or text)"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_diff_marks_only_the_lines_that_changed() {
        let diff = diff_lines(
            "hostname SW1\nntp server 192.0.2.1\nend",
            "hostname SW1\nntp server 192.0.2.9\nend",
        );
        assert_eq!(
            diff,
            vec![
                DiffLine {
                    op: '=',
                    text: "hostname SW1".into()
                },
                DiffLine {
                    op: '-',
                    text: "ntp server 192.0.2.1".into()
                },
                DiffLine {
                    op: '+',
                    text: "ntp server 192.0.2.9".into()
                },
                DiffLine {
                    op: '=',
                    text: "end".into()
                },
            ]
        );
    }

    #[test]
    fn identical_input_diffs_to_nothing_but_context() {
        let diff = diff_lines("a\nb", "a\nb");
        assert!(diff.iter().all(|line| line.op == '='));
        assert_eq!(diff.len(), 2);
    }
}
