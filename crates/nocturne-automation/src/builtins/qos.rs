//! Quality-of-service templates (contract: `docs/dev/spec-qos.md`, "Changing QoS").
//!
//! Each renders configuration lines and nothing else: the lines become a
//! change, and the change pipeline decides what touching an uplink or the
//! management interface costs. Every line follows a page of the vendor's own
//! documentation, named in the template's description; the pages were read on
//! 2026-11-22. Where the documentation leaves a choice (which code points are
//! "video", what share voice gets), the default follows RFC 4594 and is a
//! variable.
//!
//! The checks at the top of each body exist because the dangerous QoS mistakes
//! are lines a device accepts: shares that leave best effort nothing, a code
//! point listed in two classes, a guarantee above its limit. They are written
//! in the body, with `fail(…)`, so that a copy of the template keeps them.
//!
//! Two choices in the IOS policy that look arbitrary and are not:
//!
//! * Voice is `priority percent N`, never a bare `priority`. Cisco's command
//!   reference (`bandwidth`, "Configuring Strict Priority with Bandwidth")
//!   allows no `bandwidth` class beside a strict-priority one; the guide's own
//!   example pairs `priority percent` with `bandwidth percent`, as here.
//! * A class is left with `exit` before the next one is opened, as in that
//!   example. IOS would take the next `class` without it, but the change
//!   pipeline predicts the configuration from these lines, and its predictor
//!   reads a `class` typed inside a `class` as nested in it.
//!
//! And one rule that holds for every line: **a line is rendered the way the
//! device will show it afterwards.** The change pipeline compares the device
//! with what it predicted from these lines, and a difference rolls the change
//! back. That is why an interface gets its long name and a DSCP its
//! canonical spelling (see `IOS_DSCP` below).

use serde_json::json;

use super::{template, var, with_default, with_help};
use crate::model::{Template, TemplateKind, Variable, VariableKind};

fn qos(template: Template) -> Template {
    Template {
        tags: vec!["qos".to_string()],
        ..template
    }
}

fn choice(variable: Variable, choices: &[&str]) -> Variable {
    Variable {
        choices: choices.iter().map(|choice| choice.to_string()).collect(),
        ..variable
    }
}

/// The start of every IOS body that takes a DSCP: the names IOS has for code
/// points (Cisco IOS Quality of Service Solutions Command Reference, `match
/// dscp`: numbers 0 to 63, `af11`…`af43`, `cs1`…`cs7`, `default`, `ef`) and
/// two macros over them.
///
/// * `code_of(value)` is the code point a value names, or -1. With it `46`
///   and `ef` are one marking, which a comparison of spellings would let
///   through as two.
/// * `spelled(numbers)` writes code points as the simulated router writes
///   them back in its configuration: by name where there is one, in ascending
///   order. A `match dscp ef 44` that the device shows as `match dscp 44 ef`
///   is, to the change pipeline, a change that did not do what was planned.
///   The simulator is the only device this was checked on; IOS is assumed to
///   print the same.
///
/// It is part of the body, not a filter written in Rust, so that a copy of
/// the template carries it along with the checks that use it.
const IOS_DSCP: &str = r#"
{% set codes = {'default': 0, 'cs1': 8, 'af11': 10, 'af12': 12, 'af13': 14, 'cs2': 16, 'af21': 18, 'af22': 20, 'af23': 22, 'cs3': 24, 'af31': 26, 'af32': 28, 'af33': 30, 'cs4': 32, 'af41': 34, 'af42': 36, 'af43': 38, 'cs5': 40, 'ef': 46, 'cs6': 48, 'cs7': 56} %}
{% macro code_of(value) %}{% set word = value | string | lower %}{{ codes[word] if word in codes else (word if word is matching('^([0-9]|[1-5][0-9]|6[0-3])$') else -1) }}{% endmacro %}
{% macro spelled(numbers) %}{% for number in numbers | sort %}{% set found = namespace(word=number) %}{% for word in codes %}{% if codes[word] == number %}{% set found.word = word %}{% endif %}{% endfor %}{{ found.word }}{% if not loop.last %} {% endif %}{% endfor %}{% endmacro %}
"#;

/// What `interface` must look like once `ifname` has spelled it out: one
/// word that starts with a letter and ends in its number. The variable's
/// kind only says "one line", and `GigabitEthernet0/1 shutdown` or `range
/// Gi0/1 - 24` is one line too.
const IOS_PORT: &str = "^[A-Za-z][A-Za-z0-9_/.:-]{0,62}[0-9]$";

/// The largest rate any of these templates takes, in kbit/s: 128 Gbit/s. It
/// is not what a device can shape (the device has the last word on that); it
/// is what keeps a slip of the keyboard, or a number made to overflow, out
/// of a configuration line.
const MOST_KBPS: i64 = 128_000_000;

/// A body with the two constants above written into it.
fn body(text: &str) -> String {
    text.replace("@IOS_PORT@", IOS_PORT)
        .replace("@MOST_KBPS@", &MOST_KBPS.to_string())
}

/// The same, behind the DSCP names and macros.
fn body_with_dscp(text: &str) -> String {
    body(&[IOS_DSCP, text].concat())
}

pub(super) fn all() -> Vec<Template> {
    use VariableKind::*;
    vec![
        // ------------------------------------------------------------- IOS
        qos(template(
            "ios_qos_wan_edge",
            "QoS policy for a WAN uplink (IOS)",
            "Voice, video, critical data and best effort on an uplink, in IOS modular QoS: \
             three class maps that match by DSCP, a policy map that gives voice a priority \
             queue and video and critical data a guaranteed share, and `service-policy \
             output` on the interface. Whatever matches no class is best effort \
             (`class-default`) and lives on what the others leave. The shares are percentages \
             of the interface's bandwidth; give the circuit rate when the circuit is slower \
             than the port, and a parent policy shapes everything to it with this policy \
             inside the shaper. A port holds one output policy: detach the old one in the \
             same change. Class maps that already exist under these names are added to, not \
             replaced. The three shares may add up to 99 percent; IOS before 12.4(20)T \
             reserves 75 percent of an interface at most unless `max-reserved-bandwidth` \
             raises it, and refuses to attach a policy that asks for more. A marking is \
             written the way the device shows it: by name where it has one, a class's \
             markings in ascending order. Syntax: Cisco, QoS Modular QoS Command-Line \
             Interface Configuration \
             Guide, \"Applying QoS Features Using the MQC\" \
             (https://www.cisco.com/c/en/us/td/docs/ios-xml/ios/qos_mqc/configuration/xe-16/qos-mqc-xe-16-book/qos-apply.html); \
             QoS: Congestion Management Configuration Guide, \"Low Latency Queueing with \
             Priority Percentage Support\" \
             (https://www.cisco.com/c/en/us/td/docs/ios-xml/ios/qos_conmgt/configuration/xe-16/qos-conmgt-xe-16-book/qos-conmgt-llq-pps.html); \
             Cisco IOS Quality of Service Solutions Command Reference, `bandwidth \
             (policy-map class)`, `match dscp`, `max-reserved-bandwidth` and `shape \
             (policy-map class)` \
             (https://www.cisco.com/c/en/us/td/docs/ios-xml/ios/qos/command/qos-cr-book/qos-a1.html, \
             https://www.cisco.com/c/en/us/td/docs/ios-xml/ios/qos/command/qos-cr-book/qos-m1.html, \
             https://www.cisco.com/c/en/us/td/docs/ios-xml/ios/qos/command/qos-cr-book/qos-s1.html).",
            TemplateKind::Config,
            &["cisco_ios"],
            vec![
                with_help(
                    var("interface", "WAN interface", Interface, true),
                    "The physical uplink. The policy is attached outbound.",
                ),
                with_help(
                    with_default(
                        var("policy_name", "Policy name", String, false),
                        "WAN-EDGE".into(),
                    ),
                    "One word, 31 characters at most. The class maps are named after it: \
                     WAN-EDGE-VOICE, WAN-EDGE-VIDEO, WAN-EDGE-CRITICAL.",
                ),
                with_help(
                    with_default(var("voice_dscp", "Voice DSCP", List, false), json!(["ef"])),
                    "Numbers (0 to 63) or names (ef, af11 to af43, cs1 to cs7, default), eight \
                     at most. EF is RFC 4594's telephony class.",
                ),
                with_help(
                    with_default(
                        var("voice_percent", "Voice share (%)", Int, false),
                        10.into(),
                    ),
                    "The priority queue: served first, and held to this share when the link \
                     is congested.",
                ),
                with_help(
                    with_default(
                        var("video_dscp", "Video DSCP", List, false),
                        json!(["af41", "af42", "af43"]),
                    ),
                    "AF41 to AF43 is RFC 4594's multimedia conferencing class.",
                ),
                with_help(
                    with_default(
                        var("video_percent", "Video share (%)", Int, false),
                        20.into(),
                    ),
                    "Guaranteed when the link is congested.",
                ),
                with_help(
                    with_default(
                        var("critical_dscp", "Critical data DSCP", List, false),
                        json!(["af21", "af22", "af23", "cs3"]),
                    ),
                    "AF21 to AF23 is RFC 4594's low-latency data class. CS3 is what Cisco \
                     auto-QoS marks call signalling with; RFC 4594 itself uses CS5.",
                ),
                with_help(
                    with_default(
                        var("critical_percent", "Critical data share (%)", Int, false),
                        25.into(),
                    ),
                    "Guaranteed when the link is congested. The three shares may add up to 99 \
                     at most; what is left is best effort's. IOS before 12.4(20)T takes 75 \
                     at most.",
                ),
                with_help(
                    with_default(
                        var("shape_kbps", "Circuit rate (kbit/s)", Int, false),
                        0.into(),
                    ),
                    "0 for no shaper. Otherwise everything leaving the interface is shaped to \
                     this rate and the shares are shares of it: 50000 for a 50 Mbit/s circuit. \
                     Each share has to come to 8 kbit/s at least (voice: 1). \
                     Cisco's command reference gives 1 kbit/s to 1 Gbit/s for a router on IOS \
                     15.2(1)T; hardware with faster ports takes more, and the device refuses \
                     a rate it cannot shape.",
                ),
            ],
            &body_with_dscp(
                r#"
{% macro check_share(value, label) %}
{% if value < 1 or value > 100 %}{{ fail(label ~ " must be between 1 and 100 percent, not " ~ value) }}{% endif %}
{% endmacro %}
{% set port = interface | ifname %}
{% if port is not matching('@IOS_PORT@') %}{{ fail("WAN interface must be one interface, by its name: GigabitEthernet0/1") }}{% endif %}
{% if policy_name is not matching('^[A-Za-z0-9_-]{1,31}$') %}{{ fail("Policy name must be one word of letters, digits, `-` and `_`, 31 characters at most: the class maps are named after it and an IOS name ends at 40") }}{% endif %}
{% set seen = namespace(codes=[], classes=[]) %}
{% for label, values in [("Voice DSCP", voice_dscp), ("Video DSCP", video_dscp), ("Critical data DSCP", critical_dscp)] %}
{% if not values %}{{ fail(label ~ ": give at least one DSCP") }}{% endif %}
{% if values | length > 8 %}{{ fail(label ~ ": " ~ (values | length) ~ " values were given, and one `match dscp` line takes eight at most") }}{% endif %}
{% set own = namespace(codes=[]) %}
{% for value in values %}
{% set code = code_of(value) | int %}
{% if code < 0 %}{{ fail(label ~ ": `" ~ value ~ "` is not a DSCP. Give a number from 0 to 63 or one of ef, af11 to af43, cs1 to cs7, default") }}{% endif %}
{% if code in seen.codes %}{{ fail("DSCP `" ~ spelled([code]) ~ "` is listed more than once (a name and its number are one marking). A packet is put in the first class that lists its marking and never reaches the second") }}{% endif %}
{% set seen.codes = seen.codes + [code] %}
{% set own.codes = own.codes + [code] %}
{% endfor %}
{% set seen.classes = seen.classes + [own.codes] %}
{% endfor %}
{{ check_share(voice_percent, "Voice share") }}
{{ check_share(video_percent, "Video share") }}
{{ check_share(critical_percent, "Critical data share") }}
{% set reserved = voice_percent + video_percent + critical_percent %}
{% if reserved > 99 %}{{ fail("The three shares add up to " ~ reserved ~ " percent. IOS lets `priority percent` and `bandwidth percent` reserve 99 percent of an interface at most, and what is left over is all that best effort gets") }}{% endif %}
{% if shape_kbps and (shape_kbps < 8 or shape_kbps > @MOST_KBPS@) %}{{ fail("Circuit rate must be 0 (no shaper) or between 8 and @MOST_KBPS@ kbit/s, not " ~ shape_kbps) }}{% endif %}
{% if shape_kbps %}
{% for label, share, least in [("Voice share", voice_percent, 1), ("Video share", video_percent, 8), ("Critical data share", critical_percent, 8)] %}
{% if shape_kbps * share // 100 < least %}{{ fail(label ~ ": " ~ share ~ " percent of a " ~ shape_kbps ~ " kbit/s circuit is less than " ~ least ~ " kbit/s. A router refuses to attach a policy whose class comes to less than its release allows: 1 kbit/s for the priority class, and 8 kbit/s for the others on IOS 15.1(1)T") }}{% endif %}
{% endfor %}
{% endif %}
class-map match-any {{ policy_name }}-VOICE
 match dscp {{ spelled(seen.classes[0]) }}
class-map match-any {{ policy_name }}-VIDEO
 match dscp {{ spelled(seen.classes[1]) }}
class-map match-any {{ policy_name }}-CRITICAL
 match dscp {{ spelled(seen.classes[2]) }}
policy-map {{ policy_name }}
 class {{ policy_name }}-VOICE
  priority percent {{ voice_percent }}
  exit
 class {{ policy_name }}-VIDEO
  bandwidth percent {{ video_percent }}
  exit
 class {{ policy_name }}-CRITICAL
  bandwidth percent {{ critical_percent }}
{% if shape_kbps %}
policy-map {{ policy_name }}-SHAPE
 class class-default
  shape average {{ shape_kbps * 1000 }}
  service-policy {{ policy_name }}
interface {{ port }}
 service-policy output {{ policy_name }}-SHAPE
{% else %}
interface {{ port }}
 service-policy output {{ policy_name }}
{% endif %}
"#,
            ),
        )),
        qos(template(
            "ios_qos_shaped_uplink",
            "Shape an uplink to its circuit rate (IOS)",
            "Hold everything that leaves an interface to the rate of the circuit behind it, \
             so that the queue builds here, where it can be seen and managed, and not in \
             the provider's policer: a policy map that shapes `class-default`, attached \
             with `service-policy output`. Nothing is prioritised; for classes inside the \
             shaper use the WAN uplink policy instead. A port holds one output policy: \
             detach the old one in the same change. Syntax: Cisco IOS Quality of Service \
             Solutions Command Reference, `shape (policy-map class)` \
             (https://www.cisco.com/c/en/us/td/docs/ios-xml/ios/qos/command/qos-cr-book/qos-s1.html); \
             QoS Modular QoS Command-Line Interface Configuration Guide, \"Applying QoS \
             Features Using the MQC\", the default traffic class and attaching a policy \
             (https://www.cisco.com/c/en/us/td/docs/ios-xml/ios/qos_mqc/configuration/xe-16/qos-mqc-xe-16-book/qos-apply.html).",
            TemplateKind::Config,
            &["cisco_ios"],
            vec![
                with_help(
                    var("interface", "Uplink interface", Interface, true),
                    "The physical uplink. The policy is attached outbound.",
                ),
                with_help(
                    var("rate_kbps", "Circuit rate (kbit/s)", Int, true),
                    "What the circuit carries, not what the port negotiates: 50000 for \
                     50 Mbit/s. Cisco's command reference gives 1 kbit/s to 1 Gbit/s for a \
                     router on IOS 15.2(1)T; hardware with faster ports takes more, and the \
                     device refuses a rate it cannot shape.",
                ),
                with_help(
                    with_default(var("policy_name", "Policy name", String, false), "".into()),
                    "One word. Left empty, the policy is named after its rate (SHAPE-50000K), \
                     so two uplinks with different circuits do not share one.",
                ),
            ],
            &body(
                r#"
{% set port = interface | ifname %}
{% if port is not matching('@IOS_PORT@') %}{{ fail("Uplink interface must be one interface, by its name: GigabitEthernet0/1") }}{% endif %}
{% if rate_kbps < 8 or rate_kbps > @MOST_KBPS@ %}{{ fail("Circuit rate must be between 8 and @MOST_KBPS@ kbit/s, not " ~ rate_kbps) }}{% endif %}
{% set name = policy_name or ('SHAPE-' ~ rate_kbps ~ 'K') %}
{% if name is not matching('^[A-Za-z0-9_-]{1,40}$') %}{{ fail("Policy name must be one word of letters, digits, `-` and `_`, 40 characters at most") }}{% endif %}
policy-map {{ name }}
 class class-default
  shape average {{ rate_kbps * 1000 }}
interface {{ port }}
 service-policy output {{ name }}
"#,
            ),
        )),
        qos(template(
            "ios_qos_access_port_dscp",
            "Trust or set DSCP on an access port (Catalyst 2960, 2960-S, 3750)",
            "For the Catalyst switches whose QoS is the `mls qos` command set: Catalyst \
             2960, 2960-S and 3750 on Cisco IOS 12.2(55)SE, the releases whose guides this \
             follows. Mode `trust` keeps the DSCP the attached device sent (`mls qos trust \
             dscp` on the port). Mode `set` overwrites it for everything arriving on the \
             port: a policy map whose `class-default` sets the DSCP, attached with \
             `service-policy input`; that mode is not for the 2960-S, which has no \
             `class-default` in a policy map. On the 2960 and 2960-S both modes need the \
             LAN Base image. Neither does anything until QoS is enabled globally (`mls \
             qos`), and enabling it is not a port setting: from then on every port that is \
             not trusted rewrites DSCP to 0, uplinks included, so the template only adds \
             that line when asked. Not for Catalyst 3850 and 9300 (IOS XE): there the \
             default trust mode of a port is DSCP already, and it is changed with a policy \
             map. Other families have their own syntax, so read their guide first. Syntax: \
             Catalyst 3750 Software Configuration Guide, Release 12.2(55)SE, \"Configuring \
             QoS\" \
             (https://www.cisco.com/c/en/us/td/docs/switches/lan/catalyst3750/software/release/12-2_55_se/configuration/guide/scg3750/swqos.html); \
             Catalyst 2960 and 2960-S Software Configuration Guide, 12.2(55)SE, \
             \"Configuring QoS\" \
             (https://www.cisco.com/c/en/us/td/docs/switches/lan/catalyst2960/software/release/12-2_55_se/configuration/guide/scg_2960/swqos.html). \
             For what it is not for: Quality of Service Configuration Guide, Cisco IOS XE \
             Cupertino 17.9.x (Catalyst 9300 Switches), and Cisco IOS XE Gibraltar 16.12.x \
             (Catalyst 3850 Switches), \"Trust Behavior for Wired Ports\" \
             (https://www.cisco.com/c/en/us/td/docs/switches/lan/catalyst9300/software/release/17-9/configuration_guide/qos/b_179_qos_9300_cg/configuring_qos.html, \
             https://www.cisco.com/c/en/us/td/docs/switches/lan/catalyst3850/software/release/16-12/configuration_guide/qos/b_1612_qos_3850_cg/configuring_qos.html).",
            TemplateKind::Config,
            &["cisco_ios"],
            vec![
                var("interface", "Access port", Interface, true),
                with_help(
                    choice(
                        with_default(var("mode", "Mode", Choice, false), "trust".into()),
                        &["trust", "set"],
                    ),
                    "trust: keep the DSCP the device sends. set: overwrite it.",
                ),
                with_help(
                    with_default(var("dscp", "DSCP to set", String, false), "".into()),
                    "Mode `set` only: a number from 0 to 63 or a name (ef, af11 to af43, \
                     cs1 to cs7, default).",
                ),
                with_help(
                    with_default(var("policy_name", "Policy name", String, false), "".into()),
                    "Mode `set` only. Left empty, the policy is named after the marking \
                     (MARK-DSCP-AF21, and MARK-DSCP-EF for 46 as for ef) and can be shared \
                     by every port that gets it.",
                ),
                with_help(
                    with_default(
                        var("enable_qos", "Also enable QoS globally", Bool, false),
                        false.into(),
                    ),
                    "Adds `mls qos`. Every port that is not trusted then resets DSCP to 0: \
                     trust the uplinks first.",
                ),
            ],
            &body_with_dscp(
                r#"
{% set mode = mode | lower %}
{% set port = interface | ifname %}
{% if port is not matching('@IOS_PORT@') %}{{ fail("Access port must be one interface, by its name: GigabitEthernet1/0/5") }}{% endif %}
{% set given = dscp | string | lower %}
{% set code = code_of(given) | int %}
{% set mark = spelled([code]) if code >= 0 else '' %}
{% set name = policy_name or ('MARK-DSCP-' ~ (mark | upper)) %}
{% if mode == 'set' %}
{% if code < 0 %}{{ fail("DSCP to set: give a number from 0 to 63 or one of ef, af11 to af43, cs1 to cs7, default") }}{% endif %}
{% if name is not matching('^[A-Za-z0-9_-]{1,40}$') %}{{ fail("Policy name must be one word of letters, digits, `-` and `_`, 40 characters at most") }}{% endif %}
{% elif given or policy_name %}{{ fail("`DSCP to set` and `Policy name` belong to mode `set`. In mode `trust` the port keeps the marking it receives, so choose `set` or leave them empty") }}
{% endif %}
{% if enable_qos %}
mls qos
{% endif %}
{% if mode == 'set' %}
policy-map {{ name }}
 class class-default
  set dscp {{ mark }}
interface {{ port }}
 service-policy input {{ name }}
{% else %}
interface {{ port }}
 mls qos trust dscp
{% endif %}
"#,
            ),
        )),
        // -------------------------------------------------------- RouterOS
        qos(template(
            "routeros_qos_simple_queue",
            "Simple queue for one subscriber (RouterOS)",
            "Limit one subscriber's upload and download with a simple queue, and \
             optionally guarantee part of it. Upload and download are the subscriber's: \
             upload is what the target sends. IPv4 only: the pages this follows show \
             no other target. Connections that are fast-tracked bypass \
             simple queues, so the subscriber's traffic must not match a FastTrack rule. \
             Syntax: MikroTik RouterOS documentation, \"Queues\", Simple Queue \
             (https://help.mikrotik.com/docs/spaces/ROS/pages/328088/Queues); the `k` \
             suffix as in \"Queue Burst\" \
             (https://help.mikrotik.com/docs/spaces/ROS/pages/137986091/Queue+Burst).",
            TemplateKind::Config,
            &["mikrotik_routeros"],
            vec![
                with_help(
                    var("queue_name", "Queue name", String, true),
                    "One word of 64 characters at most, unique on the router: the \
                     subscriber's id.",
                ),
                with_help(
                    var("target", "Subscriber address", Cidr, true),
                    "IPv4, with its prefix length: 192.168.88.251/32 for one host.",
                ),
                with_help(
                    var("upload_kbps", "Upload limit (kbit/s)", Int, true),
                    "The most the subscriber may send.",
                ),
                with_help(
                    var("download_kbps", "Download limit (kbit/s)", Int, true),
                    "The most the subscriber may receive.",
                ),
                with_help(
                    with_default(
                        var(
                            "guaranteed_upload_kbps",
                            "Guaranteed upload (kbit/s)",
                            Int,
                            false,
                        ),
                        0.into(),
                    ),
                    "0 for none. Give both guaranteed rates or neither.",
                ),
                with_help(
                    with_default(
                        var(
                            "guaranteed_download_kbps",
                            "Guaranteed download (kbit/s)",
                            Int,
                            false,
                        ),
                        0.into(),
                    ),
                    "0 for none.",
                ),
            ],
            &body(
                r#"
{% if ':' in target %}{{ fail("Subscriber address must be an IPv4 address with its prefix length: the RouterOS pages this template follows show a simple queue with an IPv4 target only") }}{% endif %}
{% if queue_name is not matching('^[A-Za-z0-9_.@-]{1,64}$') %}{{ fail("Queue name must be one word of letters, digits and . _ - @, 64 characters at most") }}{% endif %}
{% if upload_kbps < 1 or download_kbps < 1 or upload_kbps > @MOST_KBPS@ or download_kbps > @MOST_KBPS@ %}{{ fail("Upload limit and Download limit must be at least 1 kbit/s and @MOST_KBPS@ kbit/s at most") }}{% endif %}
{% set up = guaranteed_upload_kbps %}
{% set down = guaranteed_download_kbps %}
{% if up < 0 or down < 0 %}{{ fail("A guaranteed rate cannot be negative") }}{% endif %}
{% if (up and not down) or (down and not up) %}{{ fail("Give both guaranteed rates or neither: RouterOS writes them as one `limit-at` pair, upload/download") }}{% endif %}
{% if up > upload_kbps %}{{ fail("Guaranteed upload (" ~ up ~ " kbit/s) is above the upload limit (" ~ upload_kbps ~ " kbit/s)") }}{% endif %}
{% if down > download_kbps %}{{ fail("Guaranteed download (" ~ down ~ " kbit/s) is above the download limit (" ~ download_kbps ~ " kbit/s)") }}{% endif %}
/queue simple add name={{ queue_name }} target={{ target }} max-limit={{ upload_kbps }}k/{{ download_kbps }}k{% if up %} limit-at={{ up }}k/{{ down }}k{% endif %}
"#,
            ),
        )),
        qos(template(
            "routeros_qos_queue_tree",
            "Queue tree for an uplink with marked classes (RouterOS)",
            "Voice, video, critical data and best effort on an uplink: mangle rules mark \
             what leaves the interface by its DSCP, and a queue tree on the interface \
             holds the total to the uplink rate, guarantees every class its share \
             (`limit-at`) and lets each borrow up to the whole rate, voice first \
             (`priority=1`) and best effort last (`priority=8`). The first rule marks \
             every packet leaving the interface as best effort and the DSCP rules after \
             it re-mark the classes, so a packet mark set earlier on that traffic is \
             replaced. The rules are added at the end of the postrouting chain. One \
             direction only: this shapes what the router sends out of the interface. \
             IPv4 only (`/ip firewall mangle`). Fast-tracked connections bypass the \
             firewall, so their packets are never marked and match no class: keep this \
             traffic out of FastTrack. Syntax: MikroTik RouterOS documentation, \
             \"Connection rate\", the queue tree of its traffic prioritisation example \
             (https://help.mikrotik.com/docs/spaces/ROS/pages/131366985/Connection+rate); \
             \"HTB (Hierarchical Token Bucket)\" for `limit-at`, `max-limit` and \
             `priority` \
             (https://help.mikrotik.com/docs/spaces/ROS/pages/137986076/HTB+Hierarchical+Token+Bucket); \
             \"Mangle\" for `mark-packet` \
             (https://help.mikrotik.com/docs/spaces/ROS/pages/48660587/Mangle); \"Common \
             Firewall Matchers and Actions\" for `dscp` and `out-interface` \
             (https://help.mikrotik.com/docs/spaces/ROS/pages/250708064/Common+Firewall+Matchers+and+Actions); \
             \"Packet Flow in RouterOS\" for the order of mangle and the interface queue \
             (https://help.mikrotik.com/docs/spaces/ROS/pages/328227/Packet+Flow+in+RouterOS).",
            TemplateKind::Config,
            &["mikrotik_routeros"],
            vec![
                with_help(
                    var("interface", "Uplink interface", Interface, true),
                    "The interface towards the provider, by its RouterOS name: ether1.",
                ),
                with_help(
                    var("rate_kbps", "Uplink rate (kbit/s)", Int, true),
                    "What the uplink carries outbound: 20000 for 20 Mbit/s.",
                ),
                with_help(
                    with_default(
                        var("name_prefix", "Name prefix", String, false),
                        "wan".into(),
                    ),
                    "One word, and not the interface's own name. Queues and packet marks \
                     are named after it: wan, wan-voice, wan-video, wan-critical, \
                     wan-best-effort.",
                ),
                with_help(
                    with_default(var("voice_dscp", "Voice DSCP", List, false), json!([46])),
                    "Numbers, 0 to 63: RouterOS matches DSCP by number. 46 is EF.",
                ),
                with_help(
                    with_default(
                        var("voice_percent", "Voice share (%)", Int, false),
                        10.into(),
                    ),
                    "Guaranteed share of the uplink rate.",
                ),
                with_help(
                    with_default(
                        var("video_dscp", "Video DSCP", List, false),
                        json!([34, 36, 38]),
                    ),
                    "34, 36 and 38 are AF41, AF42 and AF43.",
                ),
                with_help(
                    with_default(
                        var("video_percent", "Video share (%)", Int, false),
                        20.into(),
                    ),
                    "Guaranteed share of the uplink rate.",
                ),
                with_help(
                    with_default(
                        var("critical_dscp", "Critical data DSCP", List, false),
                        json!([18, 20, 22, 24]),
                    ),
                    "18, 20 and 22 are AF21, AF22 and AF23; 24 is CS3.",
                ),
                with_help(
                    with_default(
                        var("critical_percent", "Critical data share (%)", Int, false),
                        25.into(),
                    ),
                    "Guaranteed share of the uplink rate. The three shares may add up to 99 \
                     at most; best effort is guaranteed what is left.",
                ),
            ],
            &body(
                r#"
{% macro check_dscp(values, label) %}
{% if not values %}{{ fail(label ~ ": give at least one DSCP") }}{% endif %}
{% for value in values %}
{% if value | string is not matching('^([0-9]|[1-5][0-9]|6[0-3])$') %}{{ fail(label ~ ": `" ~ value ~ "` is not a DSCP number. RouterOS matches DSCP by number, 0 to 63: EF is 46, AF41 is 34") }}{% endif %}
{% endfor %}
{% endmacro %}
{% macro check_share(value, label) %}
{% if value < 1 or value > 100 %}{{ fail(label ~ " must be between 1 and 100 percent, not " ~ value) }}{% endif %}
{% endmacro %}
{% if interface is not matching('^[A-Za-z0-9_.@:/+-]{1,64}$') %}{{ fail("Uplink interface must be the interface's RouterOS name, one word of 64 characters at most: ether1") }}{% endif %}
{% if rate_kbps > @MOST_KBPS@ %}{{ fail("Uplink rate must be @MOST_KBPS@ kbit/s at most, not " ~ rate_kbps) }}{% endif %}
{% if name_prefix is not matching('^[A-Za-z0-9_.-]{1,32}$') %}{{ fail("Name prefix must be one word of letters, digits and . _ -, 32 characters at most") }}{% endif %}
{% if name_prefix | lower == interface | lower or name_prefix | lower == 'global' %}{{ fail("Name prefix must not be `global` or the interface's own name: the first queue is named after the prefix, and the classes are attached to it by that name") }}{% endif %}
{{ check_dscp(voice_dscp, "Voice DSCP") }}
{{ check_dscp(video_dscp, "Video DSCP") }}
{{ check_dscp(critical_dscp, "Critical data DSCP") }}
{% set listed = (voice_dscp + video_dscp + critical_dscp) | map('string') | list %}
{% for value in listed %}
{% if listed | select('equalto', value) | list | length > 1 %}{{ fail("DSCP " ~ value ~ " is listed more than once. The first rule that matches a packet marks it, and the second never sees it") }}{% endif %}
{% endfor %}
{{ check_share(voice_percent, "Voice share") }}
{{ check_share(video_percent, "Video share") }}
{{ check_share(critical_percent, "Critical data share") }}
{% set reserved = voice_percent + video_percent + critical_percent %}
{% if reserved > 99 %}{{ fail("The three shares add up to " ~ reserved ~ " percent. The guarantees of the classes may not exceed the uplink rate, and best effort needs a share of its own: 99 percent at most") }}{% endif %}
{% set voice = rate_kbps * voice_percent // 100 %}
{% set video = rate_kbps * video_percent // 100 %}
{% set critical = rate_kbps * critical_percent // 100 %}
{% set best = rate_kbps - voice - video - critical %}
{% if voice < 1 or video < 1 or critical < 1 or best < 1 %}{{ fail("An uplink rate of " ~ rate_kbps ~ " kbit/s is too small to divide: a class would be guaranteed less than 1 kbit/s") }}{% endif %}
/ip firewall mangle add chain=postrouting out-interface={{ interface }} action=mark-packet new-packet-mark={{ name_prefix }}-best-effort passthrough=yes
{% for value in voice_dscp %}
/ip firewall mangle add chain=postrouting out-interface={{ interface }} dscp={{ value }} action=mark-packet new-packet-mark={{ name_prefix }}-voice passthrough=no
{% endfor %}
{% for value in video_dscp %}
/ip firewall mangle add chain=postrouting out-interface={{ interface }} dscp={{ value }} action=mark-packet new-packet-mark={{ name_prefix }}-video passthrough=no
{% endfor %}
{% for value in critical_dscp %}
/ip firewall mangle add chain=postrouting out-interface={{ interface }} dscp={{ value }} action=mark-packet new-packet-mark={{ name_prefix }}-critical passthrough=no
{% endfor %}
/queue tree add name={{ name_prefix }} parent={{ interface }} max-limit={{ rate_kbps }}k
/queue tree add name={{ name_prefix }}-voice parent={{ name_prefix }} packet-mark={{ name_prefix }}-voice limit-at={{ voice }}k max-limit={{ rate_kbps }}k priority=1
/queue tree add name={{ name_prefix }}-video parent={{ name_prefix }} packet-mark={{ name_prefix }}-video limit-at={{ video }}k max-limit={{ rate_kbps }}k priority=3
/queue tree add name={{ name_prefix }}-critical parent={{ name_prefix }} packet-mark={{ name_prefix }}-critical limit-at={{ critical }}k max-limit={{ rate_kbps }}k priority=5
/queue tree add name={{ name_prefix }}-best-effort parent={{ name_prefix }} packet-mark={{ name_prefix }}-best-effort limit-at={{ best }}k max-limit={{ rate_kbps }}k priority=8
"#,
            ),
        )),
    ]
}
