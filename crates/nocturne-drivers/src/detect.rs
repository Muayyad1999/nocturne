//! Working out which profile an unknown device needs.
//!
//! A user adds `10.0.0.1` and nothing else. There are three chances to
//! identify it, cheapest first:
//!
//! 1. **SNMP** ([`from_snmp`]): `sysDescr` and `sysObjectID`, which the app
//!    has usually read already.
//! 2. **Banner** ([`from_banner`]): the SSH identification string, the
//!    pre-authentication banner and what the device prints at login.
//! 3. **Probe** ([`probe_session`]): log in with the permissive `generic`
//!    profile, run the probe commands the profiles declare and look at what
//!    comes back, then continue the same session with the detected profile.
//!
//! All three only match text against the `detect:` section of the loaded
//! profiles, so supporting detection for a new platform is a matter of
//! filling that section in. When several profiles match, the one with the
//! highest `detect.priority` wins.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::collect::Runner;
use crate::profile::{Platform, Registry};
use crate::session::{Channel, CliSession, Credentials, DriverError, SessionOptions};

/// The profile every session can start with, and the answer when nothing else fits.
pub const GENERIC: &str = "generic";

/// Which profile a device needs, and why we think so.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Detection {
    pub profile_id: String,
    pub method: DetectMethod,
    /// The text that matched, trimmed to about 200 characters.
    pub evidence: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectMethod {
    Snmp,
    Banner,
    Probe,
}

const EVIDENCE_CHARS: usize = 200;

/// From SNMP `sysDescr` / `sysObjectID` (the app asks SNMP; this only matches).
///
/// A profile matches when one of its `detect.sysdescr` patterns matches or
/// one of its `detect.sysobjectid` prefixes fits. Among the matches the
/// highest priority wins; at equal priority a `sysDescr` match beats an
/// object-id prefix (a vendor's prefix covers all its operating systems),
/// and a longer prefix beats a shorter one.
pub fn from_snmp(registry: &Registry, sysdescr: &str, sysobjectid: &str) -> Option<Detection> {
    let object_id = sysobjectid.trim().trim_start_matches('.');
    registry
        .platforms()
        .filter(|platform| !platform.is_abstract())
        .filter_map(|platform| {
            let described = platform
                .sysdescr
                .iter()
                .find_map(|pattern| pattern.find(sysdescr))
                .map(|found| line_at(sysdescr, found.start()));
            let prefix = platform
                .profile
                .detect
                .sysobjectid
                .iter()
                .map(|prefix| prefix.trim().trim_start_matches('.'))
                .filter(|prefix| !prefix.is_empty() && object_id.starts_with(prefix))
                .max_by_key(|prefix| prefix.len());
            if described.is_none() && prefix.is_none() {
                return None;
            }
            let rank = (
                platform.profile.detect.priority,
                described.is_some(),
                prefix.map_or(0, str::len),
            );
            let evidence = match (described, prefix) {
                (Some(line), _) => format!("sysDescr: {line}"),
                (None, Some(prefix)) => {
                    format!("sysObjectID {object_id} is under {prefix}")
                }
                (None, None) => String::new(),
            };
            Some((rank, platform, evidence))
        })
        // Among equally good matches the profile loaded last wins, so that a
        // user's profile takes precedence over a built-in one.
        .max_by_key(|(rank, _, _)| *rank)
        .map(|(_, platform, evidence)| Detection {
            profile_id: platform.id().to_string(),
            method: DetectMethod::Snmp,
            evidence: clip(&evidence),
        })
}

/// From the SSH identification string, pre-auth banner and login screen.
pub fn from_banner(registry: &Registry, text: &str) -> Option<Detection> {
    registry
        .platforms()
        .filter(|platform| !platform.is_abstract())
        .filter_map(|platform| {
            let found = platform
                .banner
                .iter()
                .find_map(|pattern| pattern.find(text))?;
            Some((platform, line_at(text, found.start())))
        })
        .max_by_key(|(platform, _)| platform.profile.detect.priority)
        .map(|(platform, line)| Detection {
            profile_id: platform.id().to_string(),
            method: DetectMethod::Banner,
            evidence: clip(line),
        })
}

/// From probe commands, over anything that can run a command: recorded
/// output pasted by a user, or a session that is already open.
///
/// The registry's probe commands are tried in priority order, each distinct
/// command once, stopping at the first profile whose probe matches. What a
/// device says when it rejects a command is matched too — an error message
/// can identify a platform as well as any output.
pub async fn from_probes(
    registry: &Registry,
    runner: &mut dyn Runner,
) -> Result<Option<Detection>, DriverError> {
    Ok(first_match(registry, None, &mut ViaRunner(runner))
        .await?
        .map(|(_, detection)| detection))
}

/// Log in with the permissive `generic` profile, run the registry's probe
/// commands (each distinct command once, in priority order, stopping at the
/// first profile whose probe matches), then re-bind the session to the
/// detected platform: its prompt patterns, enable, and init commands.
///
/// `banner_hint` is whatever the transport saw before the CLI started (the
/// SSH identification string, a pre-authentication banner). Together with
/// the login screen it decides which profile's probes are tried first, and
/// it is the fallback when no probe matches. A probe is the stronger
/// evidence: several platforms share one SSH banner.
///
/// When nothing matches, the session stays bound to `generic` and the
/// [`Detection`] names `generic`, with evidence that says so — the user can
/// still use the terminal and run read-only commands. The same happens when
/// a profile matched but does not recognise the device's prompt.
///
/// `options.enable` and `options.init` apply to the detected platform. A
/// transcript can be attached afterwards with [`CliSession::set_tap`].
pub async fn probe_session(
    channel: Box<dyn Channel>,
    registry: &Registry,
    credentials: &Credentials,
    options: &SessionOptions,
    banner_hint: Option<&str>,
) -> Result<(CliSession, Detection), DriverError> {
    let generic = registry.get(GENERIC).ok_or_else(|| {
        DriverError::Unsupported(format!(
            "no `{GENERIC}` profile is loaded, so there is nothing to log in to an unidentified device with"
        ))
    })?;
    let mut session = CliSession::start(channel, generic, credentials, options, None).await?;

    let seen_before_prompt = [banner_hint.unwrap_or_default(), session.login_screen()].join("\n");
    let hinted = from_banner(registry, &seen_before_prompt);
    let preferred = hinted.as_ref().map(|hint| hint.profile_id.as_str());

    let probed = first_match(registry, preferred, &mut ViaSession(&mut session)).await?;
    let candidate = probed.or_else(|| {
        let hint = hinted.clone()?;
        Some((registry.get(&hint.profile_id)?, hint))
    });
    let Some((platform, detection)) = candidate else {
        let commands = probe_commands(registry);
        return Ok((
            session,
            Detection {
                profile_id: GENERIC.to_string(),
                method: DetectMethod::Probe,
                evidence: clip(&if commands.is_empty() {
                    "no loaded profile declares a probe command, so nothing could be tried"
                        .to_string()
                } else {
                    format!(
                        "nothing matched: the answers to {} identify no known platform",
                        commands
                            .iter()
                            .map(|command| format!("`{command}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }),
            },
        ));
    };
    if platform.id() == GENERIC {
        return Ok((session, detection));
    }

    match session.rebind(platform.clone(), credentials, options).await {
        Ok(()) => Ok((session, detection)),
        // The profile fits what the device says but not its prompt: keep the
        // working generic session rather than fail the connection.
        Err(DriverError::Timeout { waiting_for, .. }) if session.platform().id() == GENERIC => {
            Ok((
                session,
                Detection {
                    profile_id: GENERIC.to_string(),
                    method: detection.method,
                    evidence: clip(&format!(
                        "looked like `{}` ({}), but gave up {waiting_for}",
                        platform.id(),
                        detection.evidence
                    )),
                },
            ))
        }
        Err(err) => Err(err),
    }
}

/// Something probe commands can be put to.
#[async_trait]
trait Ask: Send {
    /// What the device printed, or `None` when it said nothing usable.
    async fn ask(&mut self, command: &str) -> Result<Option<String>, DriverError>;
}

struct ViaRunner<'a>(&'a mut dyn Runner);

#[async_trait]
impl Ask for ViaRunner<'_> {
    async fn ask(&mut self, command: &str) -> Result<Option<String>, DriverError> {
        match self.0.run(command).await {
            Ok(output) => Ok(Some(output)),
            Err(DriverError::Rejected { error, .. }) => Ok(Some(error)),
            Err(err) => Err(err),
        }
    }
}

struct ViaSession<'a>(&'a mut CliSession);

#[async_trait]
impl Ask for ViaSession<'_> {
    async fn ask(&mut self, command: &str) -> Result<Option<String>, DriverError> {
        let answer = match self.0.exec(command).await {
            // Rejected or not, what came back is what there is to match.
            Ok(output) => Some(output.output),
            // The device is still thinking about a command it does not know
            // (or waiting for input): get back to the prompt and move on.
            Err(DriverError::Timeout { .. }) => None,
            Err(err) => return Err(err),
        };
        // Whatever the command left behind must not be read as the answer to the next one.
        self.0.sync().await?;
        Ok(answer)
    }
}

/// Every distinct probe command, in the order they would be tried.
fn probe_commands(registry: &Registry) -> Vec<String> {
    let mut commands: Vec<String> = Vec::new();
    for (platform, probe) in registry.probes() {
        let command = probe.command.trim();
        if platform.id() != GENERIC && !commands.iter().any(|known| known == command) {
            commands.push(command.to_string());
        }
    }
    commands
}

async fn first_match(
    registry: &Registry,
    preferred: Option<&str>,
    asker: &mut dyn Ask,
) -> Result<Option<(Arc<Platform>, Detection)>, DriverError> {
    let mut probes = registry.probes();
    // Stable: within the preferred profile and within the rest, priority order stays.
    probes.sort_by_key(|(platform, _)| Some(platform.id()) != preferred);

    let mut answers: HashMap<String, Option<String>> = HashMap::new();
    for (platform, probe) in probes {
        if platform.id() == GENERIC {
            continue;
        }
        let Some(pattern) = platform
            .profile
            .detect
            .probes
            .iter()
            .position(|known| *known == probe)
            .and_then(|index| platform.probes.get(index))
        else {
            continue;
        };
        let command = probe.command.trim();
        let answer = match answers.get(command) {
            Some(answer) => answer,
            None => {
                let answer = asker.ask(command).await?;
                answers.entry(command.to_string()).or_insert(answer)
            }
        };
        if let Some(found) = answer
            .as_deref()
            .and_then(|text| pattern.find(text).map(|found| line_at(text, found.start())))
        {
            let detection = Detection {
                profile_id: platform.id().to_string(),
                method: DetectMethod::Probe,
                evidence: clip(&format!("`{command}`: {found}")),
            };
            return Ok(Some((platform, detection)));
        }
    }
    Ok(None)
}

/// The line of `text` that contains byte offset `at`, without surrounding white space.
fn line_at(text: &str, at: usize) -> &str {
    let start = text[..at].rfind('\n').map_or(0, |newline| newline + 1);
    let end = text[at..]
        .find('\n')
        .map_or(text.len(), |newline| at + newline);
    text[start..end].trim()
}

fn clip(text: &str) -> String {
    let text = text.trim();
    if text.chars().count() <= EVIDENCE_CHARS {
        text.to_string()
    } else {
        let mut clipped: String = text.chars().take(EVIDENCE_CHARS - 1).collect();
        clipped.push('…');
        clipped
    }
}
