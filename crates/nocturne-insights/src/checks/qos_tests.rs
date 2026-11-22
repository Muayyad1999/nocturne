//! The QoS checks, case by case.
//!
//! Where the configurations come from, because it decides what these tests
//! prove:
//!
//! * [`REAL_ASR_INTERFACES`] is **real**: the `show running-config interface`
//!   output of an ASR from the ntc-templates corpus, kept with the driver
//!   fixtures (`nocturne-drivers/tests/profiles/cisco_ios/config/SOURCE.md`
//!   has its origin). It attaches eight policies to six sub-interfaces and
//!   defines none of them — which is exactly what a partial capture looks
//!   like, and why it is the negative case.
//! * Everything else is **written by hand** in IOS syntax and says so where
//!   it is defined. No complete running-config with policy-map definitions
//!   exists in the repository or in that corpus.
//! * The metric series are written here in the format `docs/dev/spec-qos.md`
//!   fixes for monitoring (`qos.queue_drop_ps`, `qos.drop_ps`, `qos.drop_pct`,
//!   `qos.bps`; instance `<interface>|<in|out>|<policy>/<class>`). Nothing
//!   recorded them from a device.

use nocturne_drivers::hier::Syntax;

use crate::model::Severity;
use crate::report::CheckReport;
use crate::settings::{Settings, Thresholds};
use crate::testdata::*;
use crate::world::World;

/// Real (see the module comment). Used verbatim.
const REAL_ASR_INTERFACES: &str =
    include_str!("../../../nocturne-drivers/tests/profiles/cisco_ios/config/asr-subinterfaces.txt");

/// The policies the real capture attaches, in the order it first names them.
const ATTACHED: [&str; 8] = [
    "VIPSIP_POLICY_2048_V1",
    "VIPSIP_POLICY_2048_OUT",
    "DATA_POLICY_100M_V1",
    "DATA_POLICY_100M_OUT",
    "VIPSIP_POLICY_1000_V1",
    "VIPSIP_POLICY_1000_OUT",
    "VIPSIP_POLICY_800_V1",
    "VIPSIP_POLICY_800_OUT",
];

/// Written by hand: a policy-map of the given name with one policed class.
fn policy(name: &str) -> String {
    format!(
        "policy-map {name}\n class class-default\n  police cir 2048000\n   conform-action \
         transmit\n   exceed-action drop\n"
    )
}

/// Written by hand: definitions for every policy the real capture attaches,
/// except the ones named.
fn definitions(except: &[&str]) -> String {
    ATTACHED
        .iter()
        .filter(|name| !except.contains(name))
        .map(|name| policy(name))
        .collect()
}

/// The real interface sections inside what a whole IOS-XE running-config
/// has around them. The wrapper — `version`, `hostname`, the policy-maps
/// handed in, the VTY lines, `end` — is written by hand.
fn whole(policies: &str, extra: &str) -> String {
    format!(
        "version 16.9\nhostname PE-ASR\n!\n{policies}!\n{REAL_ASR_INTERFACES}!\n{extra}!\n\
         line vty 0 4\n transport input ssh\n!\nend\n"
    )
}

fn evidence<'a>(report: &'a CheckReport, label: &str) -> Vec<&'a str> {
    report
        .findings
        .iter()
        .flat_map(|finding| finding.evidence.iter())
        .filter(|item| item.label == label)
        .map(|item| item.value.as_str())
        .collect()
}

fn reasons(report: &CheckReport) -> Vec<&'static str> {
    report.skips().map(|(_, reason)| reason).collect()
}

// --------------------------------------------------------- partial capture

#[test]
fn the_real_interface_only_capture_is_not_judged_by_either_check() {
    let partial = world(vec![device("a", "PE-ASR").config(REAL_ASR_INTERFACES)]);
    for check in ["qos_policy_missing", "qos_policy_unattached"] {
        let report = run(check, &partial);
        assert!(
            report.findings.is_empty(),
            "{check} judged a capture that has attachments and no definitions: {:?}",
            subjects(&report)
        );
        assert!(
            report.evaluated().is_empty(),
            "{check} must not call it clean"
        );
        let reasons = reasons(&report);
        assert_eq!(reasons.len(), 1);
        assert!(reasons[0].contains("only part of one"), "{}", reasons[0]);
    }

    // The same sections in a whole configuration that defines none of the
    // policies are judged: it is the missing `hostname`, not the content,
    // that held the check back above.
    let whole = world(vec![device("a", "PE-ASR").config(&whole("", ""))]);
    assert_eq!(run("qos_policy_missing", &whole).findings.len(), 8);
}

// ---------------------------------------------------------- policy missing

#[test]
fn an_attachment_to_an_undefined_policy_is_one_finding_quoting_every_line_that_names_it() {
    let config = whole(&definitions(&["VIPSIP_POLICY_2048_V1"]), "");
    let report = run(
        "qos_policy_missing",
        &world(vec![device("a", "PE-ASR").config(&config)]),
    );
    assert_eq!(subjects(&report), ["a:policy-map VIPSIP_POLICY_2048_V1"]);
    // Three real sub-interfaces attach it; each is quoted exactly.
    for interface in [
        "interface GigabitEthernet2/0/4.223415",
        "interface GigabitEthernet2/0/4.223436",
        "interface GigabitEthernet2/0/4.223449",
    ] {
        assert_eq!(
            evidence(&report, interface),
            ["service-policy input VIPSIP_POLICY_2048_V1"],
            "{interface}"
        );
    }
    assert_eq!(
        evidence(&report, "Definition"),
        ["no `policy-map VIPSIP_POLICY_2048_V1` in the stored configuration"]
    );
    let suggestion = report.findings[0]
        .suggestion
        .as_ref()
        .expect("a suggestion");
    assert_eq!(
        suggestion.config_lines, None,
        "what the fix is depends on which side is wrong, so none is offered as lines"
    );

    // Negative: every attached policy is defined.
    let clean = run(
        "qos_policy_missing",
        &world(vec![
            device("a", "PE-ASR").config(&whole(&definitions(&[]), "")),
        ]),
    );
    assert!(clean.findings.is_empty(), "{:?}", subjects(&clean));
    assert_eq!(clean.evaluated().len(), 1, "decided, not skipped");
}

#[test]
fn a_class_that_names_an_undefined_child_policy_is_reported_with_the_class_it_sits_in() {
    // Written by hand: a shaper whose class hands its traffic to a child
    // policy that the configuration does not have.
    let parent = "policy-map VIPSIP_POLICY_2048_OUT\n class class-default\n  shape average \
                  2048000\n  service-policy VIPSIP_QUEUES\n";
    let config = whole(
        &format!("{}{parent}", definitions(&["VIPSIP_POLICY_2048_OUT"])),
        "",
    );
    let report = run(
        "qos_policy_missing",
        &world(vec![device("a", "PE-ASR").config(&config)]),
    );
    assert_eq!(subjects(&report), ["a:policy-map VIPSIP_QUEUES"]);
    assert_eq!(
        evidence(
            &report,
            "policy-map VIPSIP_POLICY_2048_OUT / class class-default"
        ),
        ["service-policy VIPSIP_QUEUES"]
    );

    // With the child defined there is nothing to say.
    let defined = whole(
        &format!(
            "{}{parent}{}",
            definitions(&["VIPSIP_POLICY_2048_OUT"]),
            policy("VIPSIP_QUEUES")
        ),
        "",
    );
    assert!(
        run(
            "qos_policy_missing",
            &world(vec![device("a", "PE-ASR").config(&defined)])
        )
        .findings
        .is_empty()
    );
}

#[test]
fn a_reference_this_cannot_be_sure_of_is_never_called_missing() {
    // Every case is written by hand, and is a line that names a policy the
    // configuration does not define — for a reason that is not a fault.
    let cases: [(&str, &str); 6] = [
        (
            "a control-plane policy the platform supplies without printing it",
            "control-plane\n service-policy input copp-system-policy\n",
        ),
        (
            "a wireless profile using a built-in policy",
            "wireless profile policy default-policy-profile\n service-policy input platinum-up\n",
        ),
        (
            "AutoQoS under `auto qos global compact`, which hides the generated definitions",
            "interface GigabitEthernet1/0/5\n service-policy input \
             AutoQos-4.0-CiscoPhone-Input-Policy\n",
        ),
        (
            "a description that mentions an old policy",
            "interface GigabitEthernet1/0/6\n description was service-policy output OLD-WAN\n",
        ),
        (
            "a typed policy, which lives in a namespace of its own",
            "interface GigabitEthernet1/0/7\n service-policy type control subscriber DOT1X\n",
        ),
        (
            "a policy defined with a type and attached without one",
            "policy-map type control-plane COPP\n class class-default\ninterface \
             GigabitEthernet1/0/8\n service-policy input COPP\n",
        ),
    ];
    for (what, lines) in cases {
        let report = run(
            "qos_policy_missing",
            &world(vec![
                device("a", "PE-ASR").config(&whole(&definitions(&[]), lines)),
            ]),
        );
        assert!(
            report.findings.is_empty(),
            "{what}: {:?}",
            subjects(&report)
        );
        assert_eq!(report.evaluated().len(), 1, "{what}: still decided");
    }
}

#[test]
fn a_policy_named_in_an_nhrp_group_map_is_a_reference_like_any_other() {
    // Written by hand: per-tunnel QoS on a DMVPN hub.
    let hub = "interface Tunnel0\n ip nhrp map group SPOKES-10M service-policy output \
               SPOKE-SHAPER-10M\n";
    let missing = run(
        "qos_policy_missing",
        &world(vec![
            device("a", "HUB").config(&whole(&definitions(&[]), hub)),
        ]),
    );
    assert_eq!(subjects(&missing), ["a:policy-map SPOKE-SHAPER-10M"]);
    assert_eq!(
        evidence(&missing, "interface Tunnel0"),
        ["ip nhrp map group SPOKES-10M service-policy output SPOKE-SHAPER-10M"]
    );

    // And where it is defined, that line is what keeps it from being unattached.
    let defined = whole(
        &format!("{}{}", definitions(&[]), policy("SPOKE-SHAPER-10M")),
        hub,
    );
    let world = world(vec![device("a", "HUB").config(&defined)]);
    assert!(run("qos_policy_missing", &world).findings.is_empty());
    assert!(run("qos_policy_unattached", &world).findings.is_empty());
}

// ------------------------------------------------------- policy unattached

#[test]
fn a_policy_map_nothing_names_is_reported_and_a_child_policy_counts_as_used() {
    // Written by hand: a parent that is attached (by the real interfaces), its
    // child that only the parent's class names, and a policy nobody names.
    let parent = "policy-map VIPSIP_POLICY_2048_OUT\n class class-default\n  shape average \
                  2048000\n  service-policy VIPSIP_QUEUES\n";
    let child = "policy-map VIPSIP_QUEUES\n class VOICE\n  priority percent 30\n class \
                 class-default\n  fair-queue\n";
    let leftover = "policy-map OLD_POLICY_512_OUT\n class VOICE\n  priority 128\n class \
                    class-default\n  fair-queue\n";
    let config = whole(
        &format!(
            "{}{parent}{child}{leftover}",
            definitions(&["VIPSIP_POLICY_2048_OUT"])
        ),
        "",
    );
    let report = run(
        "qos_policy_unattached",
        &world(vec![device("a", "PE-ASR").config(&config)]),
    );
    assert_eq!(subjects(&report), ["a:policy-map OLD_POLICY_512_OUT"]);
    assert_eq!(
        evidence(&report, "Configuration"),
        ["policy-map OLD_POLICY_512_OUT"]
    );
    assert_eq!(
        evidence(&report, "Classes"),
        ["class VOICE / class class-default"]
    );
    assert_eq!(
        report.findings[0].severity, None,
        "it keeps the catalogue's severity, which is information"
    );
    assert_eq!(
        crate::catalogue::get("qos_policy_unattached").map(|check| check.severity),
        Some(Severity::Info)
    );

    // A listing that repeats the section is still one policy and one finding.
    let twice = whole(
        &format!(
            "{}{parent}{child}{leftover}{leftover}",
            definitions(&["VIPSIP_POLICY_2048_OUT"])
        ),
        "",
    );
    assert_eq!(
        run(
            "qos_policy_unattached",
            &world(vec![device("a", "PE-ASR").config(&twice)])
        )
        .findings
        .len(),
        1
    );

    // Negative: without the leftover, every policy is named somewhere.
    let tidy = whole(
        &format!(
            "{}{parent}{child}",
            definitions(&["VIPSIP_POLICY_2048_OUT"])
        ),
        "",
    );
    let clean = run(
        "qos_policy_unattached",
        &world(vec![device("a", "PE-ASR").config(&tidy)]),
    );
    assert!(clean.findings.is_empty(), "{:?}", subjects(&clean));
    assert_eq!(clean.evaluated().len(), 1);
}

#[test]
fn a_policy_that_is_named_anywhere_or_that_nobody_wrote_is_not_called_unattached() {
    // Each case, written by hand, adds a policy-map that no interface
    // attaches, and the reason that is not a finding.
    let cases: [(&str, String); 5] = [
        (
            "attached to the control plane",
            format!(
                "{}control-plane\n service-policy input COPP\n",
                policy("COPP")
            ),
        ),
        (
            "named by an interface template",
            format!(
                "{}template PHONE\n service-policy input PHONE-IN\n",
                policy("PHONE-IN")
            ),
        ),
        (
            "named by a frame-relay map-class",
            format!(
                "{}map-class frame-relay FR-256\n service-policy output FR-OUT\n",
                policy("FR-OUT")
            ),
        ),
        (
            "an AutoQoS set, most of which is never attached",
            format!(
                "{}{}",
                policy("AutoQos-4.0-Output-Policy"),
                policy("AutoQos-4.0-Trust-Cos-Input-Policy")
            ),
        ),
        (
            "the control-plane policer a Catalyst 9000 writes itself",
            policy("system-cpp-policy"),
        ),
    ];
    for (what, lines) in cases {
        let report = run(
            "qos_policy_unattached",
            &world(vec![
                device("a", "PE-ASR").config(&whole(&definitions(&[]), &lines)),
            ]),
        );
        assert!(
            report.findings.is_empty(),
            "{what}: {:?}",
            subjects(&report)
        );
        assert_eq!(report.evaluated().len(), 1, "{what}: still decided");
    }
}

#[test]
fn a_device_that_gives_policies_to_sessions_is_not_judged_for_unattached_ones() {
    // Written by hand: three kinds of BNG. The subscriber policies are
    // applied by name from RADIUS, and no line of the configuration says so.
    let cases: [(&str, &str); 4] = [
        (
            "802.1X ports, whose sessions RADIUS authorises",
            "aaa new-model\naaa authorization network default group radius\n",
        ),
        (
            "PPP sessions cloned from a virtual template",
            "interface Virtual-Template1\n ip unnumbered Loopback0\n ppp authentication chap\n",
        ),
        (
            "IP sessions on an access interface",
            "interface GigabitEthernet0/0/1.100\n encapsulation dot1Q 100\n ip subscriber \
             routed\n  initiator unclassified ip-address\n",
        ),
        (
            "IOS-XR sessions built from a dynamic template",
            "dynamic-template\n type ppp PPP-TPL\n  ppp authentication chap\n",
        ),
    ];
    for (what, lines) in cases {
        let bng = format!("{}{}{lines}", policy("SUB-10M-IN"), policy("SUB-10M-OUT"));
        let world = world(vec![
            device("a", "BNG").config(&whole(&definitions(&[]), &bng)),
        ]);
        let report = run("qos_policy_unattached", &world);
        assert!(
            report.findings.is_empty(),
            "{what}: {:?}",
            subjects(&report)
        );
        assert!(report.evaluated().is_empty(), "{what}");
        assert!(reasons(&report)[0].contains("RADIUS"), "{what}");
        // What the configuration does say can still be checked.
        assert_eq!(
            run("qos_policy_missing", &world).evaluated().len(),
            1,
            "{what}"
        );
    }
}

#[test]
fn a_configuration_these_checks_cannot_read_is_not_evaluated_and_says_which_kind() {
    // Written by hand, in the grammars named. Every one but the first two
    // parses as `Syntax::Ios`, which is why the grammar alone decides nothing.
    let nxos = "hostname N9K\npolicy-map type qos CLASSIFY\n class type qos VOICE\n  set \
                qos-group 5\ninterface Ethernet1/1\n service-policy type qos input CLASSIFY\n";
    // An NX-OS listing with no QoS in it at all: the layout of its
    // `show running-config`, which has no closing `end`.
    let nxos_plain = "version 9.3(8) Bios:version 05.45\nhostname N9K\nvdc N9K id 1\n  \
                      limit-resource vlan minimum 16 maximum 4094\nfeature lldp\ninterface \
                      Ethernet1/1\n  switchport mode trunk\nline console\nline vty\nboot nxos \
                      bootflash:/nxos.9.3.8.bin\n";
    let os10 = "hostname OS10\ninterface ethernet1/1/1\n service-policy input type qos CLASSIFY\n";
    // IOS XR, in the layout of Cisco's configuration guides: maps closed with
    // `end-…-map`, `!` inside the sections. Read as IOS it would have an
    // unattached policy (SPARE) and a missing one (NOT-DEFINED).
    let xr = "hostname PE-XR\nclass-map match-any VOICE\n match dscp ef\n end-class-map\n!\n\
              policy-map CORE-OUT\n class VOICE\n  priority level 1\n !\n class class-default\n \
              !\n end-policy-map\n!\npolicy-map SPARE\n class class-default\n !\n \
              end-policy-map\n!\ninterface TenGigE0/0/0/1\n service-policy output CORE-OUT\n \
              service-policy input NOT-DEFINED\n!\nend\n";
    // IOS XR with no QoS in it: a route policy is enough to say what it is.
    let xr_plain = "hostname P-XR\ninterface TenGigE0/0/0/1\n ipv4 address 10.0.0.1 \
                    255.255.255.252\n!\nroute-policy PASS\n  pass\nend-policy\n!\nrouter bgp \
                    64512\n neighbor 10.0.0.2\n  remote-as 64513\n  address-family ipv4 \
                    unicast\n   route-policy PASS in\n  !\n !\n!\nend\n";
    let procurve = "hostname \"SW-5406\"\npolicy qos \"voice\"\n 10 class ipv4 \"rtp\" action \
                    priority 7\ninterface A1\n service-policy \"voice\" in\n";
    // The same line in a listing that is otherwise a whole IOS one.
    let unread = "hostname SW1\ninterface GigabitEthernet1/0/1\n service-policy \"voice\" \
                  in\nend\n";
    // A dialect that attaches a policy the IOS way and defines it another
    // (Dell's older switches, from memory). Read as IOS, PM-IN is missing.
    let other_maps = "hostname S4810\npolicy-map-input PM-IN\n service-queue 0 class-map VOICE \
                      qos-policy QP-VOICE\ninterface TenGigabitEthernet 0/1\n service-policy \
                      input PM-IN\nend\n";
    let routeros = "/queue simple add name=alice target=10.0.0.5/32 max-limit=10M/10M\n";
    // Forty levels of nesting: not a configuration, whatever it is.
    let deep = format!(
        "hostname DEEP\n{}end\n",
        (0..40)
            .map(|level| format!("{}level {level}\n", " ".repeat(level)))
            .collect::<String>()
    );
    let cases: [(&str, crate::world::DeviceData, &str); 11] = [
        (
            "nothing stored",
            device("a", "SW1"),
            "no stored running configuration",
        ),
        (
            "RouterOS",
            device("a", "RB1").syntax(Syntax::Routeros).config(routeros),
            "IOS-style",
        ),
        (
            "typed QoS, NX-OS style",
            device("a", "N9K").config(nxos),
            "typed QoS policies",
        ),
        (
            "NX-OS with no QoS in it",
            device("a", "N9K").config(nxos_plain),
            "does not close with `end`",
        ),
        (
            "the type after the direction, OS10 style",
            device("a", "OS10").config(os10),
            "typed QoS policies",
        ),
        ("IOS XR", device("a", "PE-XR").config(xr), "the IOS XR way"),
        (
            "IOS XR with no QoS in it",
            device("a", "P-XR").config(xr_plain),
            "the IOS XR way",
        ),
        (
            "ProCurve, which prints no `end` either",
            device("a", "SW-5406").config(procurve),
            "does not close with `end`",
        ),
        (
            "a service-policy line in a form not read",
            device("a", "SW1").config(unread),
            "in a form these checks do not read",
        ),
        (
            "policy maps defined under another keyword",
            device("a", "S4810").config(other_maps),
            "defines policy maps in a form",
        ),
        (
            "something nested far deeper than a configuration",
            device("a", "DEEP").config(&deep),
            "nests deeper",
        ),
    ];
    for (what, data, expected) in cases {
        let world = world(vec![data]);
        for check in ["qos_policy_missing", "qos_policy_unattached"] {
            let report = run(check, &world);
            assert!(report.findings.is_empty(), "{what}/{check}");
            assert!(report.evaluated().is_empty(), "{what}/{check}: not clean");
            let reasons = reasons(&report);
            assert_eq!(reasons.len(), 1, "{what}/{check}");
            assert!(
                reasons[0].contains(expected),
                "{what}/{check}: {}",
                reasons[0]
            );
        }
    }
}

#[test]
fn a_listing_cut_off_before_its_end_is_not_judged() {
    // The real interface sections in a hand-written whole configuration that
    // has one leftover policy — and then the same text stopped early. IOS
    // prints its policy-maps before its interfaces, so a cut after them
    // leaves every definition and none of the attachments.
    let complete = whole(
        &format!("{}{}", definitions(&[]), policy("OLD_POLICY_512_OUT")),
        "",
    );
    let judged = run(
        "qos_policy_unattached",
        &world(vec![device("a", "PE-ASR").config(&complete)]),
    );
    assert_eq!(subjects(&judged), ["a:policy-map OLD_POLICY_512_OUT"]);

    let at = |needle: &str| complete.find(needle).expect(needle);
    let cuts: [(&str, usize); 4] = [
        (
            "in the middle of a policy-map",
            at("policy-map OLD_POLICY_512_OUT") + "policy-map OLD_POLICY_512_OUT\n class".len(),
        ),
        (
            "after the policy-maps and before the first interface",
            at("interface GigabitEthernet2/0/4.223415"),
        ),
        (
            "between two interfaces",
            at("interface GigabitEthernet2/0/4.223478"),
        ),
        ("one line before the end", at("\nend\n") + 1),
    ];
    for (what, length) in cuts {
        let world = world(vec![device("a", "PE-ASR").config(&complete[..length])]);
        for check in ["qos_policy_missing", "qos_policy_unattached"] {
            let report = run(check, &world);
            assert!(
                report.findings.is_empty(),
                "{check} judged a listing cut {what}: {:?}",
                subjects(&report)
            );
            assert!(report.evaluated().is_empty(), "{check}, cut {what}");
            let reasons = reasons(&report);
            assert_eq!(reasons.len(), 1, "{check}, cut {what}");
            assert!(
                reasons[0].contains("does not close with `end`"),
                "{check}, cut {what}: {}",
                reasons[0]
            );
        }
    }
}

// ------------------------------------------------- where a policy is named

#[test]
fn a_policy_named_in_one_unusual_place_is_attached_and_missing_only_where_the_grammar_is_known() {
    // Written by hand. Each case names SHAPE-10M in one place and nowhere
    // else; the flag says whether that is a place where a policy the
    // configuration does not define is called missing.
    let cases: [(&str, &str, bool); 9] = [
        (
            "a sub-interface",
            "interface GigabitEthernet0/0/1.300\n encapsulation dot1Q 300\n service-policy \
             output SHAPE-10M\n",
            true,
        ),
        (
            "a port-channel",
            "interface Port-channel12\n service-policy output SHAPE-10M\n",
            true,
        ),
        (
            "a port-channel sub-interface",
            "interface Port-channel12.40\n encapsulation dot1Q 40\n service-policy output \
             SHAPE-10M\n",
            true,
        ),
        (
            "an interface range, as it is typed rather than as a device stores it",
            "interface range GigabitEthernet1/0/1 - 24\n service-policy input SHAPE-10M\n",
            true,
        ),
        (
            "a service instance under a port",
            "interface GigabitEthernet0/0/2\n service instance 10 ethernet\n  encapsulation \
             dot1q 10\n  service-policy output SHAPE-10M\n",
            true,
        ),
        (
            "an ATM PVC",
            "interface ATM0/1/0.1 point-to-point\n pvc 8/35\n  service-policy output SHAPE-10M\n",
            true,
        ),
        (
            "an interface template",
            "template UPLINK\n service-policy output SHAPE-10M\n",
            false,
        ),
        (
            "the control plane",
            "control-plane\n service-policy input SHAPE-10M\n",
            false,
        ),
        (
            "a VLAN's configuration",
            "vlan configuration 300\n service-policy input SHAPE-10M\n",
            false,
        ),
    ];
    for (what, lines, judged) in cases {
        let defined = world(vec![device("a", "PE-ASR").config(&whole(
            &format!("{}{}", definitions(&[]), policy("SHAPE-10M")),
            lines,
        ))]);
        for check in ["qos_policy_missing", "qos_policy_unattached"] {
            let report = run(check, &defined);
            assert!(
                report.findings.is_empty(),
                "{check}, {what}: {:?}",
                subjects(&report)
            );
            assert_eq!(report.evaluated().len(), 1, "{check}, {what}");
        }

        let undefined = world(vec![
            device("a", "PE-ASR").config(&whole(&definitions(&[]), lines)),
        ]);
        let report = run("qos_policy_missing", &undefined);
        let expected: &[&str] = if judged {
            &["a:policy-map SHAPE-10M"]
        } else {
            &[]
        };
        assert_eq!(subjects(&report), expected, "{what}");
        assert_eq!(report.evaluated().len(), 1, "{what}");
    }
}

#[test]
fn a_policy_three_levels_deep_is_followed_all_the_way_down() {
    // Written by hand, in the layout IOS-XE gives a shaped parent: the child
    // policy is printed one level further in, under `shape average`.
    let grandchild = "policy-map VOICE-POLICER\n class class-default\n  police cir 256000\n";
    let child = "policy-map WAN-QUEUES\n class VOICE\n  priority percent 20\n  service-policy \
                 VOICE-POLICER\n class class-default\n  fair-queue\n";
    let parent = "policy-map WAN-SHAPER\n class class-default\n  shape average 20000000\n   \
                  service-policy WAN-QUEUES\n";
    let uplink = "interface GigabitEthernet0/0/0\n service-policy output WAN-SHAPER\n";
    let both = |policies: &[&str], interfaces: &str| {
        let world = world(vec![device("a", "WAN1").config(&whole(
            &format!("{}{}", definitions(&[]), policies.concat()),
            interfaces,
        ))]);
        (
            run("qos_policy_missing", &world),
            run("qos_policy_unattached", &world),
        )
    };

    let (missing, unattached) = both(&[grandchild, child, parent], uplink);
    assert!(missing.findings.is_empty(), "{:?}", subjects(&missing));
    assert!(
        unattached.findings.is_empty(),
        "{:?}",
        subjects(&unattached)
    );
    assert_eq!(
        (missing.evaluated().len(), unattached.evaluated().len()),
        (1, 1)
    );

    // The bottom one gone: named by a class two levels down.
    let (missing, unattached) = both(&[child, parent], uplink);
    assert_eq!(subjects(&missing), ["a:policy-map VOICE-POLICER"]);
    assert_eq!(
        evidence(&missing, "policy-map WAN-QUEUES / class VOICE"),
        ["service-policy VOICE-POLICER"]
    );
    assert!(unattached.findings.is_empty());

    // The middle one gone: named from under `shape average`. And the bottom
    // one is then named by nothing that exists.
    let (missing, unattached) = both(&[grandchild, parent], uplink);
    assert_eq!(subjects(&missing), ["a:policy-map WAN-QUEUES"]);
    assert_eq!(
        evidence(
            &missing,
            "policy-map WAN-SHAPER / class class-default / shape average 20000000"
        ),
        ["service-policy WAN-QUEUES"]
    );
    assert_eq!(subjects(&unattached), ["a:policy-map VOICE-POLICER"]);

    // Nothing attached: one finding, for the top. Its children are named.
    let (missing, unattached) = both(&[grandchild, child, parent], "");
    assert!(missing.findings.is_empty());
    assert_eq!(subjects(&unattached), ["a:policy-map WAN-SHAPER"]);
}

#[test]
fn names_that_differ_only_in_case_are_two_policies_and_the_finding_says_so() {
    // Written by hand. IOS compares policy names exactly, so `Wan-Edge`
    // beside `WAN-EDGE` is a second policy — typically a typing mistake.
    let typo = "policy-map Wan-Edge\n class class-default\n  fair-queue\n";
    let uplink = "interface GigabitEthernet0/0/0\n service-policy output WAN-EDGE\n";
    let both = |policies: String| {
        let world =
            world(vec![device("a", "WAN1").config(&whole(
                &format!("{}{policies}", definitions(&[])),
                uplink,
            ))]);
        (
            run("qos_policy_missing", &world),
            run("qos_policy_unattached", &world),
        )
    };

    // Both exist and one is attached: the other is left over.
    let (missing, unattached) = both(format!("{}{typo}", policy("WAN-EDGE")));
    assert!(missing.findings.is_empty(), "{:?}", subjects(&missing));
    assert_eq!(subjects(&unattached), ["a:policy-map Wan-Edge"]);
    assert_eq!(
        evidence(&unattached, "Named under a similar name"),
        [
            "a `service-policy` line names `WAN-EDGE` — policy names are case-sensitive, so that \
          is a different policy"
        ]
    );

    // Only the mistyped one exists: the attachment names nothing.
    let (missing, unattached) = both(typo.to_string());
    assert_eq!(subjects(&missing), ["a:policy-map WAN-EDGE"]);
    assert_eq!(
        evidence(&missing, "Defined under a similar name"),
        [
            "`policy-map Wan-Edge` — policy names are case-sensitive, so that is a different \
          policy"
        ]
    );
    assert_eq!(subjects(&unattached), ["a:policy-map Wan-Edge"]);

    // A policy that is simply not there has no such line.
    let (missing, _) = both(String::new());
    assert_eq!(subjects(&missing), ["a:policy-map WAN-EDGE"]);
    assert!(evidence(&missing, "Defined under a similar name").is_empty());
}

#[test]
fn a_name_that_would_change_a_command_is_reported_without_being_put_into_one() {
    // Written by hand, and hostile: device text that would turn a suggested
    // `show` into a different command if it were pasted into one.
    let lines = "policy-map SPARE|append\n class class-default\ninterface \
                 GigabitEthernet0/0/9\n service-policy output WAN|redirect\n";
    let world = world(vec![
        device("a", "WAN1").config(&whole(&definitions(&[]), lines)),
    ]);
    let commands = |check: &'static str| {
        let report = run(check, &world);
        assert_eq!(report.findings.len(), 1, "{check}: {:?}", subjects(&report));
        (
            report.findings[0].subject.clone(),
            report.findings[0]
                .suggestion
                .as_ref()
                .and_then(|suggestion| suggestion.commands.clone())
                .unwrap_or_default(),
        )
    };
    assert_eq!(
        commands("qos_policy_missing"),
        (
            "policy-map WAN|redirect".to_string(),
            vec!["show running-config | include policy-map|service-policy".to_string()]
        )
    );
    assert_eq!(
        commands("qos_policy_unattached"),
        (
            "policy-map SPARE|append".to_string(),
            vec!["show running-config | include service-policy".to_string()]
        )
    );

    // The same for a class series whose interface is not a name.
    let mut odd = router();
    sampled(
        &mut odd,
        "qos.queue_drop_ps",
        "Gi0/0?|out|WAN-EDGE/VOICE",
        &steady(42.0),
    );
    let report = run("qos_class_dropping", &odd);
    assert_eq!(subjects(&report), ["a:Gi0/0? out WAN-EDGE/VOICE"]);
    assert_eq!(
        report.findings[0]
            .suggestion
            .as_ref()
            .and_then(|suggestion| suggestion.commands.clone()),
        None
    );
}

// ---------------------------------------------------------- class dropping

const MINUTE: i64 = 60_000;
const VOICE: &str = "GigabitEthernet0/0|out|WAN-EDGE/VOICE";

/// Twelve samples of one value across the last fifty minutes — the cadence
/// monitoring reads QoS at.
fn steady(value: f64) -> Vec<(i64, f64)> {
    (0..12)
        .map(|index| (NOW - (50 - index * 4) * MINUTE, value))
        .collect()
}

fn sampled(world: &mut World, metric: &str, instance: &str, values: &[(i64, f64)]) {
    let series = world
        .store()
        .series_id("a", metric, instance)
        .expect("series");
    let samples: Vec<(i64, i64, f64)> = values
        .iter()
        .map(|(at, value)| (series, *at, *value))
        .collect();
    world.store().samples_insert(&samples).expect("samples");
    world.reindex_series();
}

fn router() -> World {
    world(vec![device("a", "WAN1")])
}

#[test]
fn a_class_dropping_to_congestion_is_reported_with_the_figures_it_was_decided_from() {
    let mut congested = router();
    sampled(&mut congested, "qos.queue_drop_ps", VOICE, &steady(42.0));
    sampled(&mut congested, "qos.drop_ps", VOICE, &steady(42.0));
    sampled(&mut congested, "qos.drop_pct", VOICE, &steady(3.5));
    sampled(&mut congested, "qos.bps", VOICE, &steady(1_200_000.0));
    // A healthy class beside it.
    let data = "GigabitEthernet0/0|out|WAN-EDGE/class-default";
    sampled(&mut congested, "qos.queue_drop_ps", data, &steady(0.0));

    let report = run("qos_class_dropping", &congested);
    assert_eq!(
        subjects(&report),
        ["a:GigabitEthernet0/0 out WAN-EDGE/VOICE"]
    );
    let finding = &report.findings[0];
    assert_eq!(finding.severity, Some(Severity::Medium));
    assert_eq!(
        finding.title,
        "Class VOICE of policy WAN-EDGE (output on GigabitEthernet0/0) has been dropping 42 \
         packets a second to congestion for 44 minutes"
    );
    assert_eq!(
        evidence(&report, "Congestion drops"),
        ["average 42.0/s, peak 42.0/s (threshold 10.0/s)"]
    );
    assert_eq!(
        evidence(&report, "Window"),
        ["12 samples over 44 minutes, 100 % of them above the threshold"]
    );
    assert_eq!(
        evidence(&report, "Share of offered packets dropped"),
        ["average 3.5 %, peak 3.5 % (every drop, whatever the reason)"]
    );
    assert_eq!(
        evidence(&report, "Traffic in the class"),
        ["average 1.2 Mb/s, peak 1.2 Mb/s"]
    );
    let worst = evidence(&report, "Peak hour");
    assert_eq!(worst.len(), 1);
    assert!(
        worst[0].ends_with("UTC, averaging 42.0 drops/s"),
        "{}",
        worst[0]
    );
    assert_eq!(
        finding.suggestion.as_ref().and_then(|s| s.commands.clone()),
        Some(vec![
            "show policy-map interface GigabitEthernet0/0 output".to_string()
        ])
    );
}

#[test]
fn what_was_not_measured_is_said_to_be_unrecorded_rather_than_left_out_or_made_up() {
    let mut congested = router();
    sampled(&mut congested, "qos.queue_drop_ps", VOICE, &steady(42.0));
    let report = run("qos_class_dropping", &congested);
    assert_eq!(report.findings.len(), 1);
    assert_eq!(
        evidence(&report, "Share of offered packets dropped"),
        ["not recorded for this class"]
    );
    assert_eq!(
        evidence(&report, "Traffic in the class"),
        ["not recorded for this class"]
    );
}

#[test]
fn a_policer_that_drops_is_the_policy_working_and_is_not_reported() {
    // The class loses eighty packets a second and none of them to its queue.
    let mut policed = router();
    sampled(&mut policed, "qos.queue_drop_ps", VOICE, &steady(0.0));
    sampled(&mut policed, "qos.drop_ps", VOICE, &steady(80.0));
    let report = run("qos_class_dropping", &policed);
    assert!(report.findings.is_empty(), "{:?}", subjects(&report));
    assert_eq!(report.evaluated().len(), 1, "decided, not skipped");

    // A class with no queue at all has no congestion series. On a device
    // that reports congestion for its other classes, its drops are a
    // policer's too.
    let mut no_queue = router();
    sampled(&mut no_queue, "qos.queue_drop_ps", VOICE, &steady(0.0));
    let scavenger = "GigabitEthernet0/0|in|WAN-IN/SCAVENGER";
    sampled(&mut no_queue, "qos.drop_ps", scavenger, &steady(80.0));
    let report = run("qos_class_dropping", &no_queue);
    assert!(report.findings.is_empty(), "{:?}", subjects(&report));
    assert_eq!(report.evaluated().len(), 1);

    // The same when the queueing policy is gone and only the policed class
    // is still measured: the device is known to count congestion apart, so
    // classes without that counter have no queue. A verdict, not a gap.
    let mut only_policed = router();
    let earlier: Vec<(i64, f64)> = steady(0.0)
        .into_iter()
        .map(|(at, value)| (at - 6 * 60 * MINUTE, value))
        .collect();
    sampled(&mut only_policed, "qos.queue_drop_ps", VOICE, &earlier);
    sampled(&mut only_policed, "qos.drop_ps", scavenger, &steady(80.0));
    let report = run("qos_class_dropping", &only_policed);
    assert!(report.findings.is_empty(), "{:?}", subjects(&report));
    assert_eq!(report.evaluated().len(), 1, "evaluated and clean");
}

#[test]
fn a_device_with_one_drop_counter_gets_a_lower_finding_that_says_it_cannot_tell() {
    // RouterOS reports `dropped` per queue and nothing finer.
    let alice = "ether1|out|subscribers/alice";
    let mut world = world(vec![device("a", "RB1").syntax(Syntax::Routeros)]);
    sampled(&mut world, "qos.drop_ps", alice, &steady(25.0));
    let report = run("qos_class_dropping", &world);
    assert_eq!(subjects(&report), ["a:ether1 out subscribers/alice"]);
    let finding = &report.findings[0];
    assert_eq!(finding.severity, Some(Severity::Low));
    assert!(
        finding.title.ends_with(
            "has been dropping 25 packets a second for 44 minutes (congestion or a rate limit: \
             the counters do not say which)"
        ),
        "{}",
        finding.title
    );
    assert!(
        finding.detail.contains("one drop counter"),
        "{}",
        finding.detail
    );
    assert_eq!(
        evidence(&report, "Drops (congestion and policing together)"),
        ["average 25.0/s, peak 25.0/s (threshold 10.0/s)"]
    );
    assert!(evidence(&report, "Congestion drops").is_empty());
    assert_eq!(
        finding.suggestion.as_ref().and_then(|s| s.commands.clone()),
        None,
        "an IOS command is not offered for a device that is not IOS"
    );
}

#[test]
fn one_burst_or_a_handful_of_samples_is_not_sustained_dropping() {
    // Look-alike: one poll in twelve over the threshold.
    let mut burst = steady(0.0);
    burst[6] = (burst[6].0, 500.0);
    let mut spiky = router();
    sampled(&mut spiky, "qos.queue_drop_ps", VOICE, &burst);
    let report = run("qos_class_dropping", &spiky);
    assert!(report.findings.is_empty());
    assert_eq!(report.evaluated().len(), 1);

    // Not enough data: three samples, however bad.
    let mut thin = router();
    sampled(
        &mut thin,
        "qos.queue_drop_ps",
        VOICE,
        &[
            (NOW - 40 * MINUTE, 90.0),
            (NOW - 20 * MINUTE, 90.0),
            (NOW - MINUTE, 90.0),
        ],
    );
    assert!(run("qos_class_dropping", &thin).findings.is_empty());

    // Not enough data: drops for five minutes only.
    let mut short = router();
    sampled(
        &mut short,
        "qos.queue_drop_ps",
        VOICE,
        &(0..6)
            .map(|index| (NOW - (5 - index) * MINUTE, 90.0))
            .collect::<Vec<_>>(),
    );
    assert!(run("qos_class_dropping", &short).findings.is_empty());
}

#[test]
fn a_device_without_qos_samples_is_not_evaluated_and_the_reason_is_given() {
    // Nothing measured at all — other metrics do not count.
    let mut unmeasured = router();
    sampled(
        &mut unmeasured,
        "if.util_pct",
        "GigabitEthernet0/0",
        &steady(95.0),
    );
    let report = run("qos_class_dropping", &unmeasured);
    assert!(report.findings.is_empty());
    assert!(report.evaluated().is_empty(), "never 'not dropping'");
    let reasons_given = reasons(&report);
    assert_eq!(reasons_given.len(), 1);
    assert!(
        reasons_given[0].starts_with("no QoS samples for this device"),
        "{}",
        reasons_given[0]
    );

    // Measured once, long ago: the series exists and says nothing about now.
    let mut stale = router();
    let old: Vec<(i64, f64)> = steady(90.0)
        .into_iter()
        .map(|(at, value)| (at - 6 * 60 * MINUTE, value))
        .collect();
    sampled(&mut stale, "qos.queue_drop_ps", VOICE, &old);
    let report = run("qos_class_dropping", &stale);
    assert!(report.findings.is_empty());
    assert!(report.evaluated().is_empty());
    assert_eq!(reasons(&report), ["no QoS samples inside the window"]);
}

#[test]
fn the_drop_threshold_comes_from_the_settings() {
    let mut light = router();
    sampled(&mut light, "qos.queue_drop_ps", VOICE, &steady(6.0));
    assert!(run("qos_class_dropping", &light).findings.is_empty());
    let strict = Settings {
        thresholds: Thresholds {
            qos_drop_ps: 5.0,
            ..Thresholds::default()
        },
        ..Settings::default()
    };
    let report = run_with("qos_class_dropping", &light, &strict);
    assert_eq!(report.findings.len(), 1);
    assert_eq!(
        evidence(&report, "Congestion drops"),
        ["average 6.0/s, peak 6.0/s (threshold 5.0/s)"]
    );
}

#[test]
fn a_policy_named_after_an_interface_is_split_from_its_class_at_the_last_slash() {
    let instance = "Serial0/0/0|out|AutoQoS-Policy-Se0/0/0/class-default";
    let mut world = router();
    sampled(&mut world, "qos.queue_drop_ps", instance, &steady(20.0));
    let report = run("qos_class_dropping", &world);
    assert_eq!(
        subjects(&report),
        ["a:Serial0/0/0 out AutoQoS-Policy-Se0/0/0/class-default"]
    );
    assert!(
        report.findings[0].title.starts_with(
            "Class class-default of policy AutoQoS-Policy-Se0/0/0 (output on Serial0/0/0)"
        ),
        "{}",
        report.findings[0].title
    );

    // A series whose name is not in the documented form is still reported,
    // under the name it has, without a command that would need its parts.
    let mut odd = router();
    sampled(&mut odd, "qos.queue_drop_ps", "wan-uplink", &steady(20.0));
    let report = run("qos_class_dropping", &odd);
    assert_eq!(subjects(&report), ["a:wan-uplink"]);
    assert!(report.findings[0].title.starts_with("QoS class wan-uplink"));
    assert_eq!(
        report.findings[0]
            .suggestion
            .as_ref()
            .and_then(|s| s.commands.clone()),
        None
    );
}

/// Samples every four minutes from `from` to `to` minutes ago, inclusive.
fn every_four_minutes(from: i64, to: i64, value: f64) -> Vec<(i64, f64)> {
    (0..)
        .map(|step| from - step * 4)
        .take_while(|ago| *ago >= to)
        .map(|ago| (NOW - ago * MINUTE, value))
        .collect()
}

#[test]
fn samples_that_stop_are_not_evaluated_rather_than_called_clean_or_still_dropping() {
    // The QoS read worked until twenty-two minutes ago and has failed since:
    // a failed read writes no sample, so the series simply stops.
    for (what, value) in [("a clean class", 0.0), ("a dropping class", 90.0)] {
        let mut stopped = router();
        sampled(
            &mut stopped,
            "qos.queue_drop_ps",
            VOICE,
            &every_four_minutes(58, 22, value),
        );
        let report = run("qos_class_dropping", &stopped);
        assert!(
            report.findings.is_empty(),
            "{what}: {:?}",
            subjects(&report)
        );
        assert!(
            report.evaluated().is_empty(),
            "{what}: neither clean nor dropping is known"
        );
        let reasons = reasons(&report);
        assert_eq!(reasons.len(), 1, "{what}");
        assert!(
            reasons[0].starts_with("the newest QoS sample is older than the sustained period"),
            "{what}: {}",
            reasons[0]
        );
    }

    // One class's series stops while the device is still read: the class is
    // gone or its counters could not be compared. Not known to be dropping
    // now, and the device is still decided for.
    let mut one_gone = router();
    sampled(
        &mut one_gone,
        "qos.queue_drop_ps",
        VOICE,
        &every_four_minutes(58, 22, 90.0),
    );
    sampled(
        &mut one_gone,
        "qos.queue_drop_ps",
        "GigabitEthernet0/0|out|WAN-EDGE/class-default",
        &steady(0.0),
    );
    let report = run("qos_class_dropping", &one_gone);
    assert!(report.findings.is_empty(), "{:?}", subjects(&report));
    assert_eq!(report.evaluated().len(), 1);

    // The queueing policy went twenty-two minutes ago and the policed class
    // is still measured: the device is being read, and has no queue left.
    let mut requeued = router();
    sampled(
        &mut requeued,
        "qos.queue_drop_ps",
        VOICE,
        &every_four_minutes(58, 22, 90.0),
    );
    sampled(
        &mut requeued,
        "qos.drop_ps",
        "GigabitEthernet0/0|in|WAN-IN/SCAVENGER",
        &steady(80.0),
    );
    let report = run("qos_class_dropping", &requeued);
    assert!(report.findings.is_empty(), "{:?}", subjects(&report));
    assert_eq!(report.evaluated().len(), 1, "measured, and clean");
}

#[test]
fn only_the_stretch_since_the_last_hole_in_a_series_is_judged() {
    // Three reads, half an hour of failed ones, then the read comes back.
    let before = every_four_minutes(58, 50, 90.0);

    // Three samples since: too little to call sustained, whatever came before.
    let mut resumed = router();
    sampled(&mut resumed, "qos.queue_drop_ps", VOICE, &before);
    sampled(
        &mut resumed,
        "qos.queue_drop_ps",
        VOICE,
        &every_four_minutes(8, 0, 90.0),
    );
    let report = run("qos_class_dropping", &resumed);
    assert!(
        report.findings.is_empty(),
        "six samples either side of a hole are not fifty-eight minutes: {:?}",
        report
            .findings
            .iter()
            .map(|finding| &finding.title)
            .collect::<Vec<_>>()
    );
    assert_eq!(report.evaluated().len(), 1, "measured now, so decided");

    // Twenty minutes since: reported, for the twenty minutes that were
    // measured and with the figures of those twenty minutes only.
    let mut longer = router();
    sampled(&mut longer, "qos.queue_drop_ps", VOICE, &before);
    sampled(
        &mut longer,
        "qos.queue_drop_ps",
        VOICE,
        &every_four_minutes(20, 0, 30.0),
    );
    sampled(
        &mut longer,
        "qos.drop_pct",
        VOICE,
        &every_four_minutes(58, 50, 50.0),
    );
    sampled(
        &mut longer,
        "qos.drop_pct",
        VOICE,
        &every_four_minutes(20, 0, 2.0),
    );
    let report = run("qos_class_dropping", &longer);
    assert_eq!(report.findings.len(), 1);
    assert!(
        report.findings[0]
            .title
            .ends_with("has been dropping 30 packets a second to congestion for 20 minutes"),
        "{}",
        report.findings[0].title
    );
    assert_eq!(
        evidence(&report, "Congestion drops"),
        ["average 30.0/s, peak 30.0/s (threshold 10.0/s)"]
    );
    assert_eq!(
        evidence(&report, "Window"),
        ["6 samples over 20 minutes, 100 % of them above the threshold"]
    );
    assert_eq!(
        evidence(&report, "Share of offered packets dropped"),
        ["average 2.0 %, peak 2.0 % (every drop, whatever the reason)"]
    );
}

#[test]
fn drops_during_one_hour_that_is_over_are_not_dropping_now() {
    let quiet = |world: &mut World, from: i64, to: i64| {
        sampled(
            world,
            "qos.queue_drop_ps",
            VOICE,
            &every_four_minutes(from, to, 0.0),
        );
    };
    let busy = |world: &mut World, from: i64, to: i64| {
        sampled(
            world,
            "qos.queue_drop_ps",
            VOICE,
            &every_four_minutes(from, to, 90.0),
        );
    };

    // The busy hour ended two hours ago.
    let mut over = router();
    busy(&mut over, 240, 184);
    quiet(&mut over, 180, 0);
    let report = run("qos_class_dropping", &over);
    assert!(report.findings.is_empty(), "{:?}", subjects(&report));
    assert_eq!(report.evaluated().len(), 1, "measured, and clean now");

    // It ended half an hour ago: half the window, not most of it.
    let mut ending = router();
    busy(&mut ending, 92, 32);
    quiet(&mut ending, 28, 0);
    let report = run("qos_class_dropping", &ending);
    assert!(report.findings.is_empty(), "{:?}", subjects(&report));
    assert_eq!(report.evaluated().len(), 1);

    // It is the hour that is running now, after two quiet ones.
    let mut running = router();
    quiet(&mut running, 240, 64);
    busy(&mut running, 60, 0);
    let report = run("qos_class_dropping", &running);
    assert_eq!(report.findings.len(), 1);
    assert_eq!(
        evidence(&report, "Window"),
        ["16 samples over 60 minutes, 100 % of them above the threshold"]
    );
    assert_eq!(evidence(&report, "Peak hour").len(), 1);
}

#[test]
fn a_drop_threshold_of_zero_is_a_setting_to_put_right_and_not_a_fleet_of_findings() {
    // Every class that is measured at all is "at or above" zero.
    let mut idle = router();
    sampled(&mut idle, "qos.queue_drop_ps", VOICE, &steady(0.0));
    for threshold in [0.0, -5.0] {
        let settings = Settings {
            thresholds: Thresholds {
                qos_drop_ps: threshold,
                ..Thresholds::default()
            },
            ..Settings::default()
        };
        let report = run_with("qos_class_dropping", &idle, &settings);
        assert!(
            report.findings.is_empty(),
            "{threshold}: {:?}",
            subjects(&report)
        );
        assert!(report.evaluated().is_empty(), "{threshold}");
        assert!(
            reasons(&report)[0].contains("not above zero"),
            "{threshold}: {}",
            reasons(&report)[0]
        );
    }
}
