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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collect::Recorded;
    use crate::profile::{Origin, Registry};
    use crate::session::Mode;
    use crate::session::testing::{FakeDevice, IOS_PROFILE, credentials};

    /// The permissive profile every unidentified session starts with.
    const GENERIC_PROFILE: &str = r#"
id: generic
name: Generic
cli:
  prompt: '^(?P<host>[\w.\-]+)(?:\([\w\-]+\))?[>#]\s?$'
  errors: ['^% ']
  # Paging cannot be switched off without knowing the platform, so the
  # unidentified session pages through whatever a probe prints.
  pager: {pattern: '--More--', key: " "}
  timeout_s: 2
policy:
  read_only: ['^show\b', '^uname\b', '^/\w']
detect:
  priority: -100
"#;

    /// Something that answers `show vlan brief`: what FakeDevice is.
    const SWITCH_PROFILE: &str = r#"
id: fake_ios
extends: test_ios
name: Fake IOS
detect:
  banner: ['Authorised access only']
  sysdescr: ['Fake IOS Software']
  sysobjectid: ['1.3.6.1.4.1.9.1.']
  probes:
    - {command: show vlan brief, matches: 'Voice'}
  priority: 50
policy:
  read_only: ['^show\b']
"#;

    /// A higher-priority profile whose probe this device cannot answer.
    const ROUTER_PROFILE: &str = r#"
id: other_os
name: Other OS
cli:
  prompt: '^(?P<host>[\w.\-]+)[>#]\s?$'
  timeout_s: 2
detect:
  sysdescr: ['OtherOS']
  sysobjectid: ['1.3.6.1.4.1.9.']
  probes:
    - {command: show nonsense, matches: 'OtherOS version'}
    - {command: show clock, matches: 'OtherOS'}
  priority: 90
policy:
  read_only: ['^show\b']
"#;

    fn registry() -> Registry {
        let mut registry = Registry::empty();
        for profile in [GENERIC_PROFILE, IOS_PROFILE, SWITCH_PROFILE, ROUTER_PROFILE] {
            registry.add(profile, Origin::Builtin).unwrap();
        }
        registry
    }

    #[test]
    fn snmp_prefers_a_description_over_an_object_id_and_the_longer_prefix() {
        let registry = registry();
        // Both profiles claim the Cisco enterprise tree; `other_os` has the
        // higher priority, `fake_ios` the longer prefix — priority first.
        let found = from_snmp(&registry, "", "1.3.6.1.4.1.9.1.516").unwrap();
        assert_eq!(found.profile_id, "other_os");
        assert_eq!(found.method, DetectMethod::Snmp);
        assert!(found.evidence.contains("1.3.6.1.4.1.9."), "{found:?}");

        // A description that names the platform wins over another profile's
        // object-id prefix, because it is the more specific evidence.
        let found = from_snmp(&registry, "Fake IOS Software, Version 1.0", "").unwrap();
        assert_eq!(found.profile_id, "fake_ios");
        assert_eq!(found.evidence, "sysDescr: Fake IOS Software, Version 1.0");

        assert!(from_snmp(&registry, "Something else", "1.2.3.4").is_none());
        assert!(
            from_snmp(&registry, "", "").is_none(),
            "an empty object id matches no prefix"
        );
    }

    #[test]
    fn banner_matching_reports_the_line_that_matched() {
        let registry = registry();
        let found = from_banner(&registry, "\r\nAuthorised access only!\r\n\r\n").unwrap();
        assert_eq!(found.profile_id, "fake_ios");
        assert_eq!(found.method, DetectMethod::Banner);
        assert_eq!(found.evidence, "Authorised access only!");
        assert!(from_banner(&registry, "SSH-2.0-OpenSSH_9.6").is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn probes_run_in_priority_order_once_each_and_stop_at_the_first_match() {
        let registry = registry();
        let mut device = Recorded::new()
            .with("show nonsense", "% Invalid input detected at '^' marker.")
            .with("show clock", "*12:00:00.000 UTC Mon Mar 1 1993")
            .with(
                "show vlan brief",
                "VLAN Name    Status\n30   Voice   active",
            );
        let found = from_probes(&registry, &mut device).await.unwrap().unwrap();
        assert_eq!(found.profile_id, "fake_ios");
        assert_eq!(found.method, DetectMethod::Probe);
        assert!(
            found.evidence.starts_with("`show vlan brief`:"),
            "{found:?}"
        );
        assert_eq!(
            device.commands_run(),
            ["show nonsense", "show clock", "show vlan brief"],
            "the higher-priority profile's probes are tried first, each command once"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_command_shared_by_two_profiles_is_run_once() {
        let mut registry = registry();
        registry
            .add(
                "id: third_os\nname: Third\ndetect:\n  probes:\n    - {command: show clock, matches: 'ThirdOS'}\n  priority: 95\npolicy:\n  read_only: ['^show\\b']\n",
                Origin::Builtin,
            )
            .unwrap();
        let mut device = Recorded::new()
            .with("show nonsense", "% Invalid input")
            .with("show clock", "*12:00:00.000 UTC Mon Mar 1 1993");
        assert!(
            from_probes(&registry, &mut device).await.unwrap().is_none(),
            "nothing this device said matches any profile's probe"
        );
        assert_eq!(
            device.commands_run(),
            ["show clock", "show nonsense", "show vlan brief"],
            "`show clock` is asked for once although two profiles probe with it"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn what_a_device_says_when_it_rejects_a_probe_can_identify_it_too() {
        let mut registry = Registry::empty();
        registry.add(GENERIC_PROFILE, Origin::Builtin).unwrap();
        registry
            .add(
                "id: picky\nname: Picky\ndetect:\n  probes:\n    - {command: show version, matches: 'Unknown command: show'}\n  priority: 10\npolicy:\n  read_only: ['^show\\b']\n",
                Origin::Builtin,
            )
            .unwrap();
        let mut device = Recorded::new();
        let found = from_probes(&registry, &mut device).await.unwrap();
        assert!(
            found.is_none(),
            "this device has no recorded answer at all, so nothing matches"
        );
        let mut device = Recorded::new().with("show version", "Unknown command: show");
        let found = from_probes(&registry, &mut device).await.unwrap().unwrap();
        assert_eq!(found.profile_id, "picky");
    }

    #[tokio::test(start_paused = true)]
    async fn a_probed_session_continues_with_the_profile_that_matched() {
        let registry = registry();
        let (mut session, found) = probe_session(
            Box::new(FakeDevice::ssh("SW1")),
            &registry,
            &credentials(),
            &SessionOptions::default(),
            Some("SSH-2.0-Cisco-1.25"),
        )
        .await
        .unwrap();

        assert_eq!(found.profile_id, "fake_ios");
        assert_eq!(found.method, DetectMethod::Probe);
        assert_eq!(session.platform().id(), "fake_ios");
        assert_eq!(
            session.mode(),
            Mode::Privileged,
            "re-binding runs the detected profile's enable"
        );
        assert_eq!(session.hostname(), "SW1");
        // The probes that failed on the way did not leave the session confused.
        assert_eq!(
            session.exec("show clock").await.unwrap().output,
            "*12:00:00.000 UTC Mon Mar 1 1993"
        );
        // Paging was turned off by the detected profile's init commands, so a
        // long listing comes back in one piece.
        assert_eq!(
            session
                .exec("show vlan brief")
                .await
                .unwrap()
                .output
                .lines()
                .count(),
            6
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_device_nothing_matches_stays_on_the_generic_profile() {
        let mut registry = Registry::empty();
        registry.add(GENERIC_PROFILE, Origin::Builtin).unwrap();
        registry
            .add(
                "id: nowhere\nname: Nowhere\ndetect:\n  probes:\n    - {command: show inventory, matches: 'NOWHERE'}\n  priority: 5\npolicy:\n  read_only: ['^show\\b']\n",
                Origin::Builtin,
            )
            .unwrap();
        let (mut session, found) = probe_session(
            Box::new(FakeDevice::ssh("SW1")),
            &registry,
            &credentials(),
            &SessionOptions::default(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(found.profile_id, GENERIC);
        assert!(
            found.evidence.contains("nothing matched")
                && found.evidence.contains("`show inventory`"),
            "the evidence says what was tried: {found:?}"
        );
        assert_eq!(session.platform().id(), GENERIC);
        // Unidentified, but still usable.
        assert_eq!(
            session.exec("show clock").await.unwrap().output,
            "*12:00:00.000 UTC Mon Mar 1 1993"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_banner_decides_when_no_probe_matches() {
        let mut registry = Registry::empty();
        registry.add(GENERIC_PROFILE, Origin::Builtin).unwrap();
        registry.add(IOS_PROFILE, Origin::Builtin).unwrap();
        registry
            .add(
                "id: banner_only\nextends: test_ios\nname: Banner only\ndetect:\n  banner: ['Authorised access only']\n  priority: 40\npolicy:\n  read_only: ['^show\\b']\n",
                Origin::Builtin,
            )
            .unwrap();
        let (session, found) = probe_session(
            Box::new(FakeDevice::ssh("SW1")),
            &registry,
            &credentials(),
            &SessionOptions::default(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(found.profile_id, "banner_only");
        assert_eq!(found.method, DetectMethod::Banner);
        assert_eq!(
            session.platform().id(),
            "banner_only",
            "the login screen is evidence when no probe command answers"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn without_a_generic_profile_there_is_nothing_to_log_in_with() {
        let registry = Registry::empty();
        let result = probe_session(
            Box::new(FakeDevice::ssh("SW1")),
            &registry,
            &credentials(),
            &SessionOptions::default(),
            None,
        )
        .await
        .map(|_| ());
        let Err(err) = result else {
            panic!("a session was opened without any profile to drive it");
        };
        assert!(err.to_string().contains("generic"), "{err}");
    }

    #[test]
    fn the_built_in_pack_identifies_the_platforms_it_claims() {
        let (registry, errors) = Registry::builtin();
        assert!(errors.is_empty(), "{errors:#?}");
        assert_eq!(
            from_banner(&registry, "SSH-2.0-ROSSSH")
                .map(|found| found.profile_id)
                .as_deref(),
            Some("mikrotik_routeros")
        );
        assert_eq!(
            from_snmp(
                &registry,
                "Cisco IOS Software, C3750E Software, Version 15.2(4)E10",
                "1.3.6.1.4.1.9.1.516"
            )
            .map(|found| found.profile_id)
            .as_deref(),
            Some("cisco_ios")
        );
        assert_eq!(
            from_snmp(
                &registry,
                "Linux fw 5.15.0-91-generic #101-Ubuntu SMP x86_64",
                "1.3.6.1.4.1.8072.3.2.10"
            )
            .map(|found| found.profile_id)
            .as_deref(),
            Some("linux")
        );
        assert_eq!(
            from_snmp(
                &registry,
                "RouterOS CCR2004-1G-12S+2XS",
                "1.3.6.1.4.1.14988.1"
            )
            .map(|found| found.profile_id)
            .as_deref(),
            Some("mikrotik_routeros")
        );
        assert!(
            from_banner(&registry, "SSH-2.0-OpenSSH_9.6").is_none(),
            "a bare OpenSSH banner identifies nothing"
        );
        assert_eq!(
            probe_commands(&registry),
            ["show version", "/system resource print", "uname -sr"],
            "the probes are tried most specific first, and `generic` has none"
        );
    }
}
