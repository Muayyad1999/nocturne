//! Quality of service: class maps, policy maps, service policies, the drop
//! model and `show policy-map interface`.

mod common;

use common::*;
use nocturne_netsim::{FlowSpec, Lab, LabError, LabSpec, NodeSpec};

/// PC1 - SW1 - R1 - R2 - PC2, fully configured and settled. R1 g0/0 faces
/// the users and R1 g0/1 is the "WAN" towards R2.
fn network() -> (Lab, [u32; 5]) {
    let mut lab = Lab::new("qos");
    let h1 = pc(&mut lab, "PC1", "10.1.0.10", Some("10.1.0.1"));
    let sw = node(&mut lab, "switch", "SW1");
    let r1 = node(&mut lab, "router", "R1");
    let r2 = node(&mut lab, "router", "R2");
    let h2 = pc(&mut lab, "PC2", "10.2.0.10", Some("10.2.0.1"));
    link(&mut lab, h1, "Ethernet0", sw, "fa0/1");
    link(&mut lab, sw, "gi0/1", r1, "g0/0");
    link(&mut lab, r1, "g0/1", r2, "g0/1");
    link(&mut lab, r2, "g0/0", h2, "Ethernet0");
    cfg(
        &mut lab,
        r1,
        "interface g0/0\n ip address 10.1.0.1 255.255.255.0\n no shutdown\ninterface g0/1\n ip address 10.0.0.1 255.255.255.252\n no shutdown\nip route 10.2.0.0 255.255.255.0 10.0.0.2",
    );
    cfg(
        &mut lab,
        r2,
        "interface g0/0\n ip address 10.2.0.1 255.255.255.0\n no shutdown\ninterface g0/1\n ip address 10.0.0.2 255.255.255.252\n no shutdown\nip route 10.1.0.0 255.255.255.0 10.0.0.1",
    );
    lab.settle(60 * SECOND);
    (lab, [h1, sw, r1, r2, h2])
}

/// A constant flow from PC1 to PC2.
fn flow(lab: &mut Lab, name: &str, bps: u64, dscp: Option<u8>) -> u32 {
    lab.add_flow(FlowSpec {
        name: name.into(),
        src: "PC1".into(),
        dst_ip: "10.2.0.10".into(),
        bps,
        dscp,
        ..FlowSpec::default()
    })
    .unwrap()
}

/// A shaped uplink with a queueing policy behind the shaper.
const SHAPED_UPLINK: &str = "\
class-map match-any VOICE
 match dscp ef
class-map match-any VIDEO
 match dscp af41
policy-map WAN-QUEUE
 class VOICE
  priority 1000
 class VIDEO
  bandwidth percent 40
 class class-default
policy-map WAN-EDGE
 class class-default
  shape average 5000000
  service-policy WAN-QUEUE
interface GigabitEthernet0/1
 service-policy output WAN-EDGE";

/// A policer for everything that comes from the user network.
const POLICED_USERS: &str = "\
ip access-list extended USERS
 permit ip 10.1.0.0 0.0.0.255 any
class-map match-all USERS
 match access-group name USERS
policy-map LIMIT-USERS
 class USERS
  police 2000000
interface GigabitEthernet0/0
 service-policy input LIMIT-USERS";

/// The lines of the `nth` class called `class` in `show policy-map
/// interface` output: from its `Class-map:` line to the next class or
/// nested policy.
fn class_block(output: &str, class: &str, nth: usize) -> String {
    let header = format!("Class-map: {class} (");
    let mut seen = 0;
    let mut lines = output.lines().skip_while(|line| {
        let hit = line.trim_start().starts_with(&header);
        if hit {
            seen += 1;
        }
        !(hit && seen > nth)
    });
    let mut block = vec![
        lines
            .next()
            .unwrap_or_else(|| panic!("no class {class} #{nth} in:\n{output}")),
    ];
    block.extend(lines.take_while(|line| {
        let line = line.trim_start();
        !line.starts_with("Class-map:") && !line.starts_with("Service-policy")
    }));
    block.join("\n")
}

/// Every number on the line of `block` that starts with `starts`.
fn numbers(block: &str, starts: &str) -> Vec<u64> {
    let line = block
        .lines()
        .map(str::trim_start)
        .find(|line| line.starts_with(starts))
        .unwrap_or_else(|| panic!("no line starting {starts:?} in:\n{block}"));
    line.split(|c: char| !c.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .map(|part| part.parse().unwrap())
        .collect()
}

/// Packets and bytes classified into a class: the line under its name.
fn classified(block: &str) -> [u64; 2] {
    let line = block.lines().nth(1).unwrap_or_default();
    assert!(line.ends_with(" bytes"), "{block}");
    let found = numbers(line, "");
    [found[0], found[1]]
}

/// Octets a flow of `bps` carries in `seconds`.
fn octets(bps: u64, seconds: u64) -> u64 {
    bps * seconds / 8
}

fn interface(lab: &Lab, node: u32, name: &str) -> nocturne_netsim::IfCounters {
    lab.metrics(node)
        .unwrap()
        .interfaces
        .into_iter()
        .find(|i| i.name == name)
        .unwrap()
}

// ------------------------------------------------------------ command line

#[test]
fn class_maps_and_policy_maps_have_their_own_modes_and_prompts() {
    let (mut lab, [_, _, r1, _, _]) = network();
    let c = lab.console_open(r1).unwrap();
    typed(&mut lab, c, "enable");
    typed(&mut lab, c, "configure terminal");
    assert!(typed(&mut lab, c, "class-map match-any VOICE").ends_with("R1(config-cmap)#"));
    assert!(typed(&mut lab, c, "match dscp ef").ends_with("R1(config-cmap)#"));
    // A global command typed in a sub-mode leaves the sub-mode.
    assert!(typed(&mut lab, c, "policy-map WAN").ends_with("R1(config-pmap)#"));
    assert!(typed(&mut lab, c, "class VOICE").ends_with("R1(config-pmap-c)#"));
    assert!(typed(&mut lab, c, "priority 128").ends_with("R1(config-pmap-c)#"));
    // Naming the next class from inside one moves on to it.
    assert!(typed(&mut lab, c, "class class-default").ends_with("R1(config-pmap-c)#"));
    assert!(typed(&mut lab, c, "bandwidth percent 20").ends_with("R1(config-pmap-c)#"));
    // Leaving a class goes back to its policy map, and from there out.
    assert!(typed(&mut lab, c, "exit").ends_with("R1(config-pmap)#"));
    assert!(typed(&mut lab, c, "exit").ends_with("R1(config)#"));
    // Abbreviations work as everywhere else.
    assert!(typed(&mut lab, c, "pol WAN").ends_with("R1(config-pmap)#"));
    assert!(typed(&mut lab, c, "cl VOICE").ends_with("R1(config-pmap-c)#"));
    let help = typed(&mut lab, c, "?");
    for word in [
        "bandwidth",
        "police",
        "priority",
        "service-policy",
        "set",
        "shape",
    ] {
        assert!(help.contains(&format!("\r\n  {word} ")), "{word}: {help}");
    }
    assert!(typed(&mut lab, c, "int g0/1").ends_with("R1(config-if)#"));
    assert!(typed(&mut lab, c, "serv out WAN").ends_with("R1(config-if)#"));
    assert!(typed(&mut lab, c, "end").contains("R1#"));
    let run = lab.running_config(r1).unwrap();
    assert!(
        run.contains(
            "class-map match-any VOICE\n match dscp ef\n!\npolicy-map WAN\n class VOICE\n  priority 128\n class class-default\n  bandwidth percent 20\n!\n"
        ),
        "{run}"
    );
    assert!(
        run.contains(" speed auto\n service-policy output WAN\n!\n"),
        "{run}"
    );
    let shown = typed(&mut lab, c, "sh policy-map int g0/1 out");
    assert!(shown.contains("Service-policy output: WAN"), "{shown}");
}

/// Everything the QoS command line accepts, in one configuration.
const EVERYTHING: &str = "\
ip access-list extended GUEST-NET
 permit ip 10.1.0.0 0.0.0.255 any
access-list 10 permit 10.1.0.0 0.0.0.255
class-map match-any VOICE
 description Calls and their signalling
 match dscp ef cs5
 match ip dscp 44
class-map match-all GUEST
 match access-group name GUEST-NET
class-map match-any LEGACY
 match access-group 10
 match any
policy-map PARENT
 description Shape to what the provider sells
policy-map CHILD
 description Queues behind the shaper
 class VOICE
  priority percent 10
 class GUEST
  bandwidth 500
  police cir 2000000 conform-action transmit exceed-action drop
  set dscp cs1
 class class-default
  bandwidth percent 20
 class LEGACY
  police 64000
policy-map PARENT
 class class-default
  shape average 5000000
  service-policy CHILD
policy-map STRICT
 class VOICE
  priority
policy-map INGRESS
 class GUEST
  police 3000000
  set dscp 9
interface GigabitEthernet0/1
 service-policy output PARENT
interface GigabitEthernet0/0
 service-policy input INGRESS
 service-policy output STRICT";

#[test]
fn a_policy_survives_a_round_trip_through_show_running_config() {
    let (mut lab, [_, _, r1, _, _]) = network();
    cfg(&mut lab, r1, EVERYTHING);
    let run = lab.running_config(r1).unwrap();
    // Written the way a device writes it: names for code points, the default
    // class last, and a policy before the policy that uses it.
    let expected = "\
class-map match-any VOICE
 description Calls and their signalling
 match dscp cs5 ef
 match ip dscp 44
class-map match-all GUEST
 match access-group name GUEST-NET
class-map match-any LEGACY
 match access-group 10
 match any
!
policy-map CHILD
 description Queues behind the shaper
 class VOICE
  priority percent 10
 class GUEST
  bandwidth 500
  police cir 2000000 conform-action transmit exceed-action drop
  set dscp cs1
 class LEGACY
  police 64000
 class class-default
  bandwidth percent 20
policy-map PARENT
 description Shape to what the provider sells
 class class-default
  shape average 5000000
  service-policy CHILD
policy-map STRICT
 class VOICE
  priority
policy-map INGRESS
 class GUEST
  police 3000000
  set dscp 9
!
interface GigabitEthernet0/0
";
    assert!(run.contains(expected), "{run}");
    assert!(
        run.contains(" service-policy input INGRESS\n service-policy output STRICT\n!\n"),
        "{run}"
    );

    // A factory-fresh router booted from that text renders the same text.
    let spec = LabSpec {
        name: "copy".into(),
        nodes: vec![NodeSpec {
            name: "R1".into(),
            model: "router".into(),
            startup_config: Some(run.clone()),
            ..NodeSpec::default()
        }],
        ..LabSpec::default()
    };
    let copy = Lab::from_spec(&spec).unwrap();
    let id = copy.node_by_name("R1").unwrap();
    assert_eq!(copy.running_config(id).unwrap(), run);

    // And it survives `write memory` and a power cycle.
    exec(&mut lab, r1, "write memory");
    lab.set_power(r1, false);
    lab.set_power(r1, true);
    lab.settle(30 * SECOND);
    assert_eq!(lab.running_config(r1).unwrap(), run);
}

#[test]
fn what_the_drop_model_cannot_do_is_refused_not_swallowed() {
    let (mut lab, [_, _, r1, _, _]) = network();
    cfg(&mut lab, r1, EVERYTHING);
    let before = lab.running_config(r1).unwrap();
    let refused = |lab: &mut Lab, text: &str, says: &str| {
        let lines: Vec<String> = text.lines().map(str::to_string).collect();
        let outcome = lab.configure(r1, &lines).unwrap();
        let last = outcome.lines.last().unwrap();
        assert!(last.error, "{text:?} was accepted: {outcome:#?}");
        assert!(
            last.output.contains(says),
            "{text:?} said {:?}",
            last.output
        );
    };
    // The reference quotes this one.
    refused(
        &mut lab,
        "policy-map CHILD\n class NOSUCH",
        "% class map NOSUCH not configured",
    );
    refused(
        &mut lab,
        "interface g0/2\n service-policy output NOSUCH",
        "% policy map NOSUCH not configured",
    );
    refused(
        &mut lab,
        "policy-map CHILD\n class class-default\n service-policy NOSUCH",
        "% policy map NOSUCH not configured",
    );
    // A policy cannot contain itself, directly or through another.
    refused(
        &mut lab,
        "policy-map CHILD\n class class-default\n service-policy PARENT",
        "cannot be applied inside CHILD",
    );
    refused(
        &mut lab,
        "policy-map CHILD\n class class-default\n service-policy CHILD",
        "cannot be applied inside CHILD",
    );
    // Queues exist only where traffic leaves.
    refused(
        &mut lab,
        "interface g0/2\n service-policy input PARENT",
        "not supported in an input",
    );
    refused(
        &mut lab,
        "policy-map INGRESS\n class GUEST\n shape average 1000000",
        "not supported in an input",
    );
    refused(
        &mut lab,
        "policy-map INGRESS\n class GUEST\n service-policy CHILD",
        "not supported in an input",
    );
    // Flow traffic is counted on physical ports.
    refused(
        &mut lab,
        "interface Loopback0\n service-policy output STRICT",
        "not supported on Loopback0",
    );
    refused(
        &mut lab,
        "interface g0/3.10\n service-policy output STRICT",
        "not supported on GigabitEthernet0/3.10",
    );
    // What the reference rules out.
    refused(
        &mut lab,
        "policy-map CHILD\n class VOICE\n bandwidth 100",
        "cannot be configured",
    );
    refused(
        &mut lab,
        "policy-map CHILD\n class GUEST\n priority 100",
        "cannot be configured",
    );
    refused(
        &mut lab,
        "policy-map CHILD\n class class-default\n priority 100",
        "default class cannot be a priority class",
    );
    // A second policy in the same direction, and removing what is in use.
    refused(
        &mut lab,
        "interface g0/1\n service-policy output STRICT",
        "Policy map PARENT is already attached",
    );
    refused(
        &mut lab,
        "no policy-map PARENT",
        "%Policy map PARENT is being used",
    );
    refused(
        &mut lab,
        "no policy-map CHILD",
        "%Policy map CHILD is being used",
    );
    refused(
        &mut lab,
        "no class-map VOICE",
        "%Class-map VOICE is being used",
    );
    // Real commands the model has nothing behind are refused like a typo.
    for text in [
        "class-map match-any X\n match protocol http",
        "class-map match-any X\n match dscp 64",
        "class-map match-any X\n match dscp ef af11 af12 af13 af21 af22 af23 af31 af32",
        "class-map class-default",
        "policy-map CHILD\n class GUEST\n random-detect",
        "policy-map CHILD\n class GUEST\n fair-queue",
        "policy-map CHILD\n class GUEST\n queue-limit 128",
        "policy-map CHILD\n class GUEST\n bandwidth remaining percent 10",
        "policy-map CHILD\n class GUEST\n bandwidth percent 101",
        "policy-map CHILD\n class GUEST\n shape peak 1000000",
        "policy-map CHILD\n class GUEST\n police 2000000 conform-action set-dscp-transmit af11",
        "policy-map CHILD\n class GUEST\n police 100",
        "policy-map CHILD\n class GUEST\n set dscp purple",
        "policy-map CHILD\n class GUEST\n set cos 5",
        "interface g0/2\n service-policy WAN",
    ] {
        refused(&mut lab, text, "% Invalid input detected at '^' marker.");
    }
    refused(
        &mut lab,
        "policy-map CHILD\n class GUEST\n bandwidth",
        "% Incomplete command.",
    );
    // Nothing of all that changed the configuration, apart from the empty
    // class map the invalid matches were tried in and the two logical
    // interfaces a policy was refused on.
    cfg(
        &mut lab,
        r1,
        "no class-map X\nno interface Loopback0\nno interface g0/3.10",
    );
    assert_eq!(lab.running_config(r1).unwrap(), before);

    // Detached first, a policy and its classes can go.
    cfg(
        &mut lab,
        r1,
        "interface g0/1\n no service-policy output PARENT\ninterface g0/0\n no service-policy output\nno policy-map PARENT\nno policy-map CHILD\nno policy-map STRICT\nno class-map VOICE",
    );
    let after = lab.running_config(r1).unwrap();
    assert!(
        !after.contains("PARENT") && !after.contains("VOICE"),
        "{after}"
    );
    assert!(
        after.contains("policy-map INGRESS\n class GUEST\n"),
        "{after}"
    );
}

// ---------------------------------------------------------------- the model

#[test]
fn a_device_with_no_policy_answers_show_policy_map_interface_with_nothing() {
    let (mut lab, [_, sw, r1, _, _]) = network();
    flow(&mut lab, "bulk", 1_000_000, None);
    lab.advance(5 * SECOND);
    for node in [r1, sw] {
        assert_eq!(exec(&mut lab, node, "show policy-map interface"), "");
        assert_eq!(exec(&mut lab, node, "show policy-map"), "");
    }
    assert_eq!(exec(&mut lab, r1, "show policy-map interface g0/1"), "");
    // A policy that is defined but attached nowhere has no statistics either.
    cfg(
        &mut lab,
        r1,
        "policy-map IDLE\n class class-default\n  police 8000",
    );
    assert_eq!(exec(&mut lab, r1, "show policy-map interface"), "");
    assert!(exec(&mut lab, r1, "show policy-map").starts_with("  Policy Map IDLE\n"));
    // An interface the device does not have is an error, not silence.
    assert!(exec(&mut lab, r1, "show policy-map interface g9/9").contains("% Invalid input"));
}

#[test]
fn a_policer_drops_what_exceeds_its_rate_and_the_traffic_beyond_it_is_thinner() {
    let (mut lab, [_, _, r1, r2, _]) = network();
    cfg(&mut lab, r1, POLICED_USERS);
    let before = interface(&lab, r2, "GigabitEthernet0/1");
    let id = flow(&mut lab, "bulk", 5_000_000, None);
    lab.advance(10 * SECOND);

    let shown = exec(&mut lab, r1, "show policy-map interface");
    let users = class_block(&shown, "USERS", 0);
    // Everything offered is classified; 2 of the 5 Mbit/s conform.
    let (offered, conformed) = (octets(5_000_000, 10), octets(2_000_000, 10));
    assert_eq!(classified(&users), [offered / 800, offered], "{shown}");
    assert_eq!(
        numbers(&users, "conformed"),
        [conformed / 800, conformed],
        "{shown}"
    );
    assert_eq!(
        numbers(&users, "exceeded"),
        [(offered - conformed) / 800, offered - conformed],
        "{shown}"
    );
    assert!(users.contains("Match: access-group name USERS"), "{shown}");
    // Nothing fell into the default class.
    assert_eq!(classified(&class_block(&shown, "class-default", 0)), [0, 0]);

    // The next router sees what conformed, and so does the flow's receiver.
    let after = interface(&lab, r2, "GigabitEthernet0/1");
    let arrived = after.in_octets - before.in_octets;
    assert!(
        (conformed..conformed + 20_000).contains(&arrived),
        "{arrived}"
    );
    let state = lab.flows().into_iter().find(|f| f.0 == id).unwrap().2;
    assert!(state.active);
    assert_eq!(
        state.current_bps, 5_000_000,
        "the flow still offers its full rate"
    );
    assert_eq!(state.delivered_octets, conformed);
    // What comes back answers what arrived: a tenth of 2 Mbit/s.
    let view = lab
        .snapshot()
        .links
        .into_iter()
        .find(|l| l.bps_ab == 2_000_000)
        .unwrap();
    assert_eq!(view.bps_ba, 200_000);
    // A policer drop is the policy working: no interface counts it as a discard.
    for (node, name) in [(r1, "GigabitEthernet0/0"), (r1, "GigabitEthernet0/1")] {
        let counters = interface(&lab, node, name);
        assert_eq!(
            (counters.in_discards, counters.out_discards),
            (0, 0),
            "{name}"
        );
    }
}

#[test]
fn traffic_is_classified_by_its_marking_or_by_an_access_list_and_otherwise_falls_to_the_default() {
    let (mut lab, [_, _, r1, _, _]) = network();
    cfg(
        &mut lab,
        r1,
        "access-list 10 deny 10.1.0.99\naccess-list 10 permit 10.1.0.0 0.0.0.255\n\
         ip access-list extended WEB\n permit tcp any any eq www\n permit udp any any\n\
         ip access-list extended TO-PC2\n permit ip any host 10.2.0.10\n\
         class-map match-any VOICE\n match dscp ef cs5\n\
         class-map match-all WEB\n match access-group name WEB\n\
         class-map match-all MARKED-USERS\n match access-group 10\n match ip dscp af11\n\
         class-map match-any TO-PC2\n match access-group name TO-PC2\n\
         policy-map SORT\n class VOICE\n class WEB\n class MARKED-USERS\n class TO-PC2\n\
         interface g0/0\n service-policy input SORT",
    );
    flow(&mut lab, "voice", 800_000, Some(46));
    flow(&mut lab, "signalling", 80_000, Some(40));
    flow(&mut lab, "marked", 400_000, Some(10));
    flow(&mut lab, "unmarked", 1_600_000, None);
    lab.advance(10 * SECOND);
    let shown = exec(&mut lab, r1, "show policy-map interface g0/0 input");
    let bytes = |class: &str| classified(&class_block(&shown, class, 0))[1];
    // Two code points in one statement: either is a match.
    assert_eq!(bytes("VOICE"), octets(880_000, 10), "{shown}");
    // A flow is plain IP: an entry that asks for a protocol, with a port or
    // without, cannot see it.
    assert_eq!(bytes("WEB"), 0, "{shown}");
    // match-all: the address and the marking both have to fit.
    assert_eq!(bytes("MARKED-USERS"), octets(400_000, 10), "{shown}");
    // No marking stated: no `match dscp` applies, but an access list still does.
    assert_eq!(bytes("TO-PC2"), octets(1_600_000, 10), "{shown}");
    assert_eq!(bytes("class-default"), 0, "{shown}");
    assert!(
        class_block(&shown, "VOICE", 0).contains("Match: dscp cs5 (40) ef (46)"),
        "{shown}"
    );
    assert!(
        class_block(&shown, "MARKED-USERS", 0).contains("(match-all)"),
        "{shown}"
    );
    assert!(
        class_block(&shown, "MARKED-USERS", 0)
            .contains("Match: access-group 10\n      Match: ip dscp af11 (10)"),
        "{shown}"
    );

    // Take the access list away from under the class: it matches nothing, and
    // the unmarked flow falls through to the default class.
    cfg(&mut lab, r1, "no ip access-list extended TO-PC2");
    exec(&mut lab, r1, "clear counters");
    lab.advance(10 * SECOND);
    let shown = exec(&mut lab, r1, "show policy-map interface g0/0 input");
    let bytes = |class: &str| classified(&class_block(&shown, class, 0))[1];
    assert_eq!(bytes("TO-PC2"), 0, "{shown}");
    assert_eq!(bytes("class-default"), octets(1_600_000, 10), "{shown}");
}

#[test]
fn traffic_that_states_no_marking_is_not_taken_for_traffic_marked_zero() {
    let (mut lab, [_, _, r1, _, _]) = network();
    cfg(
        &mut lab,
        r1,
        "class-map match-any BEST-EFFORT\n match dscp default\n\
         policy-map SORT\n class BEST-EFFORT\n\
         interface g0/0\n service-policy input SORT",
    );
    // One flow says it is marked 0; the other says nothing about its marking.
    flow(&mut lab, "marked zero", 400_000, Some(0));
    flow(&mut lab, "says nothing", 1_600_000, None);
    lab.advance(10 * SECOND);
    let shown = exec(&mut lab, r1, "show policy-map interface g0/0");
    let best_effort = class_block(&shown, "BEST-EFFORT", 0);
    assert!(best_effort.contains("Match: dscp default (0)"), "{shown}");
    assert_eq!(classified(&best_effort)[1], octets(400_000, 10), "{shown}");
    assert_eq!(
        classified(&class_block(&shown, "class-default", 0))[1],
        octets(1_600_000, 10),
        "{shown}"
    );
}

#[test]
fn a_shaped_uplink_serves_voice_first_keeps_video_its_share_and_drops_the_default_class() {
    let (mut lab, [_, _, r1, r2, _]) = network();
    cfg(&mut lab, r1, SHAPED_UPLINK);
    let before = interface(&lab, r2, "GigabitEthernet0/1").in_octets;
    flow(&mut lab, "voice", 800_000, Some(46));
    flow(&mut lab, "video", 1_500_000, Some(34));
    flow(&mut lab, "bulk", 4_000_000, None);
    lab.advance(10 * SECOND);

    let shown = exec(&mut lab, r1, "show policy-map interface g0/1");
    // 6.3 Mbit/s is offered to a 5 Mbit/s shaper. Voice is served first and
    // video is inside its 40 % (2 Mbit/s): neither loses anything.
    let voice = class_block(&shown, "VOICE", 0);
    assert_eq!(classified(&voice), [1250, octets(800_000, 10)], "{shown}");
    assert_eq!(numbers(&voice, "Priority:"), [1000, 25000, 0], "{shown}");
    let video = class_block(&shown, "VIDEO", 0);
    assert_eq!(numbers(&video, "(queue depth"), [0, 0, 0], "{shown}");
    assert_eq!(
        numbers(&video, "(pkts output"),
        [2343, octets(1_500_000, 10)],
        "{shown}"
    );
    assert_eq!(numbers(&video, "bandwidth"), [40, 2000], "{shown}");
    // The default class gets what is left, 2.7 of its 4 Mbit/s; its queue
    // stands full and the 1.3 Mbit/s that do not fit are its drops.
    let default = class_block(&shown, "class-default", 1);
    let dropped = octets(1_300_000, 10);
    assert_eq!(
        classified(&default),
        [6250, octets(4_000_000, 10)],
        "{shown}"
    );
    assert_eq!(
        numbers(&default, "(queue depth"),
        [64, dropped / 800, 0],
        "{shown}"
    );
    assert_eq!(
        numbers(&default, "(pkts output"),
        [4218, octets(2_700_000, 10)],
        "{shown}"
    );
    // The shaped class above them counts all of it and sends exactly its rate.
    let shaped = class_block(&shown, "class-default", 0);
    assert_eq!(
        classified(&shaped),
        [9843, octets(6_300_000, 10)],
        "{shown}"
    );
    assert_eq!(
        numbers(&shaped, "(queue depth"),
        [64, dropped / 800, 0],
        "{shown}"
    );
    assert_eq!(
        numbers(&shaped, "(pkts output"),
        [7812, octets(5_000_000, 10)],
        "{shown}"
    );
    assert_eq!(
        numbers(&shaped, "shape (average)"),
        [5_000_000, 20_000, 20_000],
        "{shown}"
    );

    // A queue drop is congestion, and the interface says so too.
    let counters = interface(&lab, r1, "GigabitEthernet0/1");
    assert_eq!(counters.out_discards, dropped / 800);
    let arrived = interface(&lab, r2, "GigabitEthernet0/1").in_octets - before;
    let sent = octets(5_000_000, 10);
    assert!((sent..sent + 20_000).contains(&arrived), "{arrived}");
    let drops = exec(&mut lab, r1, "show interfaces g0/1 | include output drops");
    assert!(
        drops.contains(&format!("Total output drops: {}", dropped / 800)),
        "{drops}"
    );
}

#[test]
fn a_percentage_in_a_child_policy_is_of_the_shape_rate_above_it() {
    let (mut lab, [_, _, r1, _, _]) = network();
    cfg(&mut lab, r1, SHAPED_UPLINK);
    // Video wants twice its 40 % of the 5 Mbit/s shaper while the default
    // class wants more than the rest. Were the 40 % of the gigabit port,
    // video would be inside its guarantee and the default class would starve.
    flow(&mut lab, "video", 4_000_000, Some(34));
    flow(&mut lab, "bulk", 4_000_000, None);
    lab.advance(10 * SECOND);
    let shown = exec(&mut lab, r1, "show policy-map interface g0/1");
    let video = class_block(&shown, "VIDEO", 0);
    assert_eq!(numbers(&video, "bandwidth"), [40, 2000], "{shown}");
    assert_eq!(
        numbers(&video, "(pkts output"),
        [octets(2_000_000, 10) / 800, octets(2_000_000, 10)],
        "{shown}"
    );
    assert_eq!(
        numbers(&video, "(queue depth"),
        [64, octets(2_000_000, 10) / 800, 0],
        "{shown}"
    );
    // The default class has what nobody reserved: the other 3 Mbit/s.
    let default = class_block(&shown, "class-default", 1);
    assert_eq!(
        numbers(&default, "(pkts output"),
        [octets(3_000_000, 10) / 800, octets(3_000_000, 10)],
        "{shown}"
    );
}

#[test]
fn a_priority_class_is_held_to_its_rate_only_while_the_others_have_to_wait() {
    let (mut lab, [_, _, r1, _, _]) = network();
    cfg(&mut lab, r1, SHAPED_UPLINK);
    // 1.5 Mbit/s of voice against `priority 1000`, with room to spare.
    let voice = flow(&mut lab, "voice", 1_500_000, Some(46));
    lab.advance(10 * SECOND);
    let shown = exec(&mut lab, r1, "show policy-map interface g0/1");
    assert_eq!(
        numbers(&class_block(&shown, "VOICE", 0), "Priority:"),
        [1000, 25000, 0],
        "{shown}"
    );
    assert_eq!(lab.flows()[0].2.delivered_octets, octets(1_500_000, 10));

    // Now the shaper is full: voice is cut to its 1000 kbit/s, and the rest
    // of the 5 Mbit/s goes to the default class.
    flow(&mut lab, "bulk", 6_000_000, None);
    exec(&mut lab, r1, "clear counters");
    lab.advance(10 * SECOND);
    let shown = exec(&mut lab, r1, "show policy-map interface g0/1");
    let exceed = octets(500_000, 10) / 800;
    assert_eq!(
        numbers(&class_block(&shown, "VOICE", 0), "Priority:"),
        [1000, 25000, exceed],
        "{shown}"
    );
    // The queue all priority classes share shows what they sent and lost.
    let all = shown
        .split("queue stats for all priority classes:")
        .nth(1)
        .unwrap()
        .split("Class-map:")
        .next()
        .unwrap();
    assert_eq!(numbers(all, "(queue depth"), [0, exceed, 0], "{shown}");
    assert_eq!(
        numbers(all, "(pkts output"),
        [1562, octets(1_000_000, 10)],
        "{shown}"
    );
    let default = class_block(&shown, "class-default", 1);
    assert_eq!(
        numbers(&default, "(pkts output"),
        [6250, octets(4_000_000, 10)],
        "{shown}"
    );
    let _ = voice;
}

#[test]
fn an_interface_carries_no_more_than_its_speed_with_or_without_a_policy() {
    let (mut lab, [_, _, r1, r2, _]) = network();
    cfg(&mut lab, r1, "interface g0/1\n speed 10");
    lab.settle(30 * SECOND);
    let before = interface(&lab, r2, "GigabitEthernet0/1").in_octets;
    flow(&mut lab, "bulk", 16_000_000, None);
    lab.advance(10 * SECOND);
    // One queue, first in first out: 6 of the 16 Mbit/s are tail drops.
    let dropped = octets(6_000_000, 10) / 800;
    assert_eq!(
        interface(&lab, r1, "GigabitEthernet0/1").out_discards,
        dropped
    );
    let arrived = interface(&lab, r2, "GigabitEthernet0/1").in_octets - before;
    let sent = octets(10_000_000, 10);
    assert!((sent..sent + 20_000).contains(&arrived), "{arrived}");
    assert_eq!(lab.flows()[0].2.delivered_octets, sent);
    let link = lab
        .snapshot()
        .links
        .into_iter()
        .find(|l| l.bps_ab == 10_000_000);
    assert!(
        link.is_some(),
        "the link carries what the interface can send"
    );

    // With a policy that does not queue, the policy does its part and the
    // interface's own queue still drops what does not fit.
    cfg(
        &mut lab,
        r1,
        "policy-map CAP\n class class-default\n  police 12000000\ninterface g0/1\n service-policy output CAP",
    );
    exec(&mut lab, r1, "clear counters");
    lab.advance(10 * SECOND);
    let shown = exec(&mut lab, r1, "show policy-map interface");
    let default = class_block(&shown, "class-default", 0);
    assert_eq!(
        numbers(&default, "exceeded"),
        [octets(4_000_000, 10) / 800, octets(4_000_000, 10)]
    );
    assert!(
        !default.contains("queue"),
        "a class that only polices has no queue: {shown}"
    );
    assert_eq!(
        interface(&lab, r1, "GigabitEthernet0/1").out_discards,
        octets(2_000_000, 10) / 800
    );
}

#[test]
fn a_flow_of_any_rate_is_thinned_to_what_its_first_port_carries_and_wraps_no_counter() {
    let (mut lab, [h1, _, r1, _, _]) = network();
    cfg(&mut lab, r1, POLICED_USERS);
    cfg(&mut lab, r1, SHAPED_UPLINK);
    // Rates no network carries: across the path, to the sender's own
    // address (which crosses no port at all), and from a router to itself.
    let across = flow(&mut lab, "across", u64::MAX, Some(46));
    let mut add = |name: &str, src: &str, dst_ip: &str, pattern| {
        lab.add_flow(FlowSpec {
            name: name.into(),
            src: src.into(),
            dst_ip: dst_ip.into(),
            bps: u64::MAX,
            pattern,
            ..FlowSpec::default()
        })
        .unwrap()
    };
    let own = add(
        "to itself",
        "PC1",
        "10.1.0.10",
        nocturne_netsim::FlowPattern::Constant,
    );
    add(
        "router to itself",
        "R1",
        "10.0.0.1",
        nocturne_netsim::FlowPattern::Burst {
            on_s: 1,
            off_s: u32::MAX,
        },
    );
    add(
        "no period",
        "PC1",
        "10.2.0.10",
        nocturne_netsim::FlowPattern::Sine { period_s: 0 },
    );
    lab.advance(60 * SECOND);
    let state = |lab: &Lab, id: u32| lab.flows().into_iter().find(|f| f.0 == id).unwrap().2;
    assert_eq!(state(&lab, own).delivered_octets, u64::MAX);
    // PC1's port is 100 Mbit/s: that is all that ever reaches the switch,
    // and behind the policer and the shaper far less arrives.
    let sent = interface(&lab, h1, "Ethernet0").out_octets;
    assert!(sent <= octets(100_000_000, 60) + 100_000, "{sent}");
    assert!(state(&lab, across).active);
    assert!(state(&lab, across).delivered_octets <= octets(2_000_000, 60));
    let shown = exec(&mut lab, r1, "show policy-map interface");
    assert!(!shown.contains("NaN") && !shown.contains("inf"), "{shown}");
}

#[test]
fn set_dscp_re_marks_the_traffic_for_everything_behind_it() {
    let (mut lab, [_, _, r1, r2, _]) = network();
    cfg(
        &mut lab,
        r1,
        "class-map match-any ALL\n match any\npolicy-map MARK\n class ALL\n  set dscp ef\ninterface g0/0\n service-policy input MARK",
    );
    let by_marking = "class-map match-any VOICE\n match dscp ef\npolicy-map SEE\n class VOICE";
    cfg(
        &mut lab,
        r1,
        &format!("{by_marking}\ninterface g0/1\n service-policy output SEE"),
    );
    cfg(
        &mut lab,
        r2,
        &format!("{by_marking}\ninterface g0/1\n service-policy input SEE"),
    );
    flow(&mut lab, "plain", 800_000, None);
    lab.advance(10 * SECOND);
    let sent = octets(800_000, 10);
    let marked = exec(&mut lab, r1, "show policy-map interface g0/0");
    let all = class_block(&marked, "ALL", 0);
    assert!(
        all.contains("QoS Set\n        dscp ef\n          Packets marked 1250"),
        "{marked}"
    );
    // The same router's output policy and the next router's input policy
    // both see the new marking.
    for (node, command) in [
        (r1, "show policy-map interface g0/1 output"),
        (r2, "show policy-map interface g0/1 input"),
    ] {
        let shown = exec(&mut lab, node, command);
        assert_eq!(
            classified(&class_block(&shown, "VOICE", 0))[1],
            sent,
            "{shown}"
        );
        assert_eq!(
            classified(&class_block(&shown, "class-default", 0))[1],
            0,
            "{shown}"
        );
    }
    // The answers come back with the marking the flow itself carries: none.
    cfg(&mut lab, r2, "interface g0/1\n service-policy output SEE");
    exec(&mut lab, r2, "clear counters");
    lab.advance(10 * SECOND);
    let back = exec(&mut lab, r2, "show policy-map interface g0/1 output");
    assert_eq!(classified(&class_block(&back, "VOICE", 0))[1], 0, "{back}");
    assert_eq!(
        classified(&class_block(&back, "class-default", 0))[1],
        sent / 10,
        "{back}"
    );
}

#[test]
fn clear_counters_and_a_reload_start_the_policy_counters_over() {
    let (mut lab, [_, _, r1, _, _]) = network();
    cfg(&mut lab, r1, POLICED_USERS);
    cfg(&mut lab, r1, SHAPED_UPLINK);
    flow(&mut lab, "bulk", 8_000_000, None);
    lab.advance(10 * SECOND);
    let total = |lab: &mut Lab, command: &str| -> u64 {
        let shown = exec(lab, r1, command);
        shown
            .lines()
            .filter(|l| l.contains(" packets, ") && !l.contains("actions"))
            .map(|l| numbers(l, "")[1])
            .sum()
    };
    // 10 MB offered to the policer, 2.5 MB of it on to the shaper and its default class.
    assert_eq!(total(&mut lab, "show policy-map interface"), 15_000_000);
    assert!(interface(&lab, r1, "GigabitEthernet0/1").out_octets > 2_000_000);

    // One interface: its policy starts over, the other keeps counting.
    exec(&mut lab, r1, "clear counters g0/0");
    assert_eq!(total(&mut lab, "show policy-map interface g0/0"), 0);
    assert!(total(&mut lab, "show policy-map interface g0/1") > 0);
    let zeroed = exec(&mut lab, r1, "show policy-map interface g0/0");
    assert!(
        zeroed.contains("conformed 0 packets, 0 bytes; actions:")
            && zeroed.contains("exceeded 0 packets, 0 bytes; actions:")
            && zeroed.contains("5 minute offered rate 0 bps, drop rate 0 bps"),
        "{zeroed}"
    );
    // All of them.
    exec(&mut lab, r1, "clear counters");
    assert_eq!(total(&mut lab, "show policy-map interface"), 0);
    assert_eq!(interface(&lab, r1, "GigabitEthernet0/1").out_discards, 0);
    // And they count again from there.
    lab.advance(2 * SECOND);
    assert_eq!(
        total(&mut lab, "show policy-map interface g0/0"),
        octets(8_000_000, 2)
    );

    // Attaching a policy again starts it at zero; so does a reload.
    cfg(
        &mut lab,
        r1,
        "interface g0/0\n no service-policy input LIMIT-USERS\n service-policy input LIMIT-USERS",
    );
    assert_eq!(total(&mut lab, "show policy-map interface g0/0"), 0);
    lab.advance(2 * SECOND);
    exec(&mut lab, r1, "write memory");
    lab.set_power(r1, false);
    lab.set_power(r1, true);
    lab.settle(30 * SECOND);
    let reloaded = exec(&mut lab, r1, "show policy-map interface g0/0");
    assert!(
        reloaded.contains("Service-policy input: LIMIT-USERS"),
        "{reloaded}"
    );
    // The flow found its way again while the lab settled; what matters is
    // that the count starts from the reload, not from before it.
    let since: u64 = classified(&class_block(&reloaded, "USERS", 0))[1];
    let uptime = lab.metrics(r1).unwrap().uptime_s;
    assert!(since <= octets(8_000_000, uptime), "{since} in {uptime} s");
}

/// The exact text, so that anything written against it (the `qos` step of
/// the `cisco_ios` profile is) breaks loudly when the layout moves. The
/// layout itself is Cisco's: each line has its counterpart in the sample
/// output of the command reference (see `docs/dev/spec-netsim.md`).
#[test]
fn show_policy_map_interface_prints_a_flat_policy_with_a_policer_like_the_reference() {
    let (mut lab, [_, _, r1, _, _]) = network();
    cfg(&mut lab, r1, POLICED_USERS);
    flow(&mut lab, "bulk", 5_000_000, None);
    lab.advance(10 * SECOND);
    pretty_assertions::assert_eq!(exec(&mut lab, r1, "show policy-map interface"), FLAT);
}

#[test]
fn show_policy_map_interface_prints_a_shaped_policy_with_its_child_like_the_reference() {
    let (mut lab, [_, _, r1, _, _]) = network();
    cfg(&mut lab, r1, SHAPED_UPLINK);
    flow(&mut lab, "voice", 800_000, Some(46));
    flow(&mut lab, "video", 1_500_000, Some(34));
    flow(&mut lab, "bulk", 4_000_000, None);
    lab.advance(10 * SECOND);
    pretty_assertions::assert_eq!(
        exec(&mut lab, r1, "show policy-map interface"),
        HIERARCHICAL
    );
    // Asking for one interface or one direction gives the same or nothing.
    assert_eq!(
        exec(&mut lab, r1, "show policy-map interface g0/1"),
        HIERARCHICAL
    );
    assert_eq!(
        exec(&mut lab, r1, "show policy-map interface g0/1 output"),
        HIERARCHICAL
    );
    assert_eq!(
        exec(&mut lab, r1, "show policy-map interface g0/1 input"),
        ""
    );
    assert_eq!(exec(&mut lab, r1, "show policy-map interface g0/0"), "");
    // The configured form of the same policies.
    pretty_assertions::assert_eq!(
        exec(&mut lab, r1, "show policy-map"),
        concat!(
            "  Policy Map WAN-QUEUE\n",
            "    Class VOICE\n",
            "      priority 1000 (kbps)\n",
            "    Class VIDEO\n",
            "      bandwidth 40 (%)\n",
            "    Class class-default\n",
            "\n",
            "  Policy Map WAN-EDGE\n",
            "    Class class-default\n",
            "      Average Rate Traffic Shaping\n",
            "      cir 5000000 (bps)\n",
            "      service-policy WAN-QUEUE\n",
        )
    );
    assert_eq!(
        exec(&mut lab, r1, "show policy-map WAN-EDGE"),
        "  Policy Map WAN-EDGE\n    Class class-default\n      Average Rate Traffic Shaping\n      cir 5000000 (bps)\n      service-policy WAN-QUEUE\n"
    );
    assert_eq!(exec(&mut lab, r1, "show policy-map NOSUCH"), "");
}

/// Ten seconds of a 5 Mbit/s flow through `POLICED_USERS`.
const FLAT: &str = concat!(
    " GigabitEthernet0/0\n",
    "\n",
    "  Service-policy input: LIMIT-USERS\n",
    "\n",
    "    Class-map: USERS (match-all)\n",
    "      7812 packets, 6250000 bytes\n",
    "      5 minute offered rate 164000 bps, drop rate 98000 bps\n",
    "      Match: access-group name USERS\n",
    "      police:\n",
    "          cir 2000000 bps, bc 62500 bytes\n",
    "        conformed 3125 packets, 2500000 bytes; actions:\n",
    "          transmit\n",
    "        exceeded 4687 packets, 3750000 bytes; actions:\n",
    "          drop\n",
    "        conformed 65000 bps, exceed 98000 bps\n",
    "\n",
    "    Class-map: class-default (match-any)\n",
    "      0 packets, 0 bytes\n",
    "      5 minute offered rate 0 bps, drop rate 0 bps\n",
    "      Match: any\n",
);

/// Ten seconds of voice (0.8 Mbit/s, EF), video (1.5 Mbit/s, AF41) and
/// unmarked bulk traffic (4 Mbit/s) through `SHAPED_UPLINK`.
const HIERARCHICAL: &str = concat!(
    " GigabitEthernet0/1\n",
    "\n",
    "  Service-policy output: WAN-EDGE\n",
    "\n",
    "    Class-map: class-default (match-any)\n",
    "      9843 packets, 7875000 bytes\n",
    "      5 minute offered rate 206000 bps, drop rate 42000 bps\n",
    "      Match: any\n",
    "      Queueing\n",
    "      queue limit 64 packets\n",
    "      (queue depth/total drops/no-buffer drops) 64/2031/0\n",
    "      (pkts output/bytes output) 7812/6250000\n",
    "      shape (average) cir 5000000, bc 20000, be 20000\n",
    "      target shape rate 5000000\n",
    "\n",
    "      Service-policy : WAN-QUEUE\n",
    "\n",
    "        queue stats for all priority classes:\n",
    "\n",
    "          queue limit 64 packets\n",
    "          (queue depth/total drops/no-buffer drops) 0/0/0\n",
    "          (pkts output/bytes output) 1250/1000000\n",
    "\n",
    "        Class-map: VOICE (match-any)\n",
    "          1250 packets, 1000000 bytes\n",
    "          5 minute offered rate 26000 bps, drop rate 0 bps\n",
    "          Match: dscp ef (46)\n",
    "          Priority: 1000 kbps, burst bytes 25000, b/w exceed drops: 0\n",
    "\n",
    "        Class-map: VIDEO (match-any)\n",
    "          2343 packets, 1875000 bytes\n",
    "          5 minute offered rate 49000 bps, drop rate 0 bps\n",
    "          Match: dscp af41 (34)\n",
    "          Queueing\n",
    "          queue limit 64 packets\n",
    "          (queue depth/total drops/no-buffer drops) 0/0/0\n",
    "          (pkts output/bytes output) 2343/1875000\n",
    "          bandwidth 40% (2000 kbps)\n",
    "\n",
    "        Class-map: class-default (match-any)\n",
    "          6250 packets, 5000000 bytes\n",
    "          5 minute offered rate 131000 bps, drop rate 42000 bps\n",
    "          Match: any\n",
    "          queue limit 64 packets\n",
    "          (queue depth/total drops/no-buffer drops) 64/2031/0\n",
    "          (pkts output/bytes output) 4218/3375000\n",
);

#[test]
fn a_class_that_only_polices_waits_in_the_default_queue_and_its_drops_show_there() {
    let (mut lab, [_, _, r1, _, _]) = network();
    cfg(
        &mut lab,
        r1,
        "class-map match-any GOLD\n match dscp af41\nclass-map match-any CAPPED\n match dscp cs1\n\
         policy-map QUEUES\n class GOLD\n  bandwidth percent 50\n class CAPPED\n  police 3000000\n\
         policy-map UPLINK\n class class-default\n  shape average 4000000\n  service-policy QUEUES\n\
         interface g0/1\n service-policy output UPLINK",
    );
    flow(&mut lab, "gold", 1_000_000, Some(34));
    flow(&mut lab, "capped", 5_000_000, Some(8));
    flow(&mut lab, "plain", 2_000_000, None);
    lab.advance(10 * SECOND);
    let shown = exec(&mut lab, r1, "show policy-map interface");
    // Gold uses half of its 2 Mbit/s guarantee and loses nothing.
    let gold = class_block(&shown, "GOLD", 0);
    assert_eq!(numbers(&gold, "(queue depth"), [0, 0, 0], "{shown}");
    assert_eq!(
        numbers(&gold, "(pkts output"),
        [1562, octets(1_000_000, 10)],
        "{shown}"
    );
    // The capped class is policed from 5 to 3 Mbit/s. It has no queue of its
    // own, so it prints none.
    let capped = class_block(&shown, "CAPPED", 0);
    assert_eq!(
        numbers(&capped, "exceeded"),
        [3125, octets(2_000_000, 10)],
        "{shown}"
    );
    assert!(!capped.contains("queue"), "{shown}");
    // What is left for it and the default class is 3 Mbit/s for 5: they
    // share the default queue, which drops 2 Mbit/s of the two together and
    // sends on 3.
    let default = class_block(&shown, "class-default", 1);
    assert_eq!(
        classified(&default),
        [3125, octets(2_000_000, 10)],
        "{shown}"
    );
    assert_eq!(numbers(&default, "(queue depth"), [64, 3125, 0], "{shown}");
    assert_eq!(
        numbers(&default, "(pkts output"),
        [4687, octets(3_000_000, 10)],
        "{shown}"
    );
    // The class's own rates still say what became of its traffic: of 5
    // Mbit/s offered, 2 were policed away and 1.2 lost in the queue.
    lab.advance(3000 * SECOND);
    let settled = exec(&mut lab, r1, "show policy-map interface");
    let rates = numbers(&class_block(&settled, "CAPPED", 0), "5 minute offered rate");
    assert_eq!(rates[1], 4_999_000, "{settled}");
    assert!((3_190_000..=3_200_000).contains(&rates[2]), "{settled}");
}

#[test]
fn the_counters_are_the_same_however_the_host_slices_time() {
    let run = |steps: &[u64]| {
        let (mut lab, [_, _, r1, r2, _]) = network();
        cfg(&mut lab, r1, SHAPED_UPLINK);
        cfg(
            &mut lab,
            r2,
            "class-map match-any VOICE\n match dscp ef\npolicy-map IN\n class VOICE\n  police 64000\n  set dscp cs5\ninterface g0/1\n service-policy input IN",
        );
        // Rates that move, so every tick does different arithmetic.
        for (name, bps, dscp, pattern) in [
            (
                "voice",
                1_400_000,
                Some(46),
                nocturne_netsim::FlowPattern::Constant,
            ),
            (
                "video",
                2_500_000,
                Some(34),
                nocturne_netsim::FlowPattern::Sine { period_s: 17 },
            ),
            (
                "bulk",
                9_000_000,
                None,
                nocturne_netsim::FlowPattern::Burst { on_s: 3, off_s: 4 },
            ),
        ] {
            lab.add_flow(FlowSpec {
                name: name.into(),
                src: "PC1".into(),
                dst_ip: "10.2.0.10".into(),
                bps,
                pattern,
                dscp,
                ..FlowSpec::default()
            })
            .unwrap();
        }
        for step in steps {
            lab.advance(*step);
        }
        let mut transcript = exec(&mut lab, r1, "show policy-map interface");
        transcript.push_str(&exec(&mut lab, r2, "show policy-map interface"));
        transcript.push_str(&exec(&mut lab, r1, "show interfaces g0/1"));
        let metrics = serde_json::to_string(&lab.metrics(r1).unwrap()).unwrap();
        let flows = serde_json::to_string(&lab.flows()).unwrap();
        (transcript, metrics, flows)
    };
    let once = run(&[90 * SECOND]);
    // The same simulated time in uneven pieces, and the same run again.
    let pieces: Vec<u64> = (0..900)
        .map(|i| if i % 2 == 0 { 30_000 } else { 170_000 })
        .collect();
    assert_eq!(pieces.iter().sum::<u64>(), 90 * SECOND);
    let sliced = run(&pieces);
    pretty_assertions::assert_eq!(once.0, sliced.0);
    assert_eq!(once, sliced);
    assert_eq!(once, run(&[90 * SECOND]));
    // Something did happen: every kind of drop is in there.
    for needle in [
        "b/w exceed drops: 0",
        "exceeded 0 packets",
        "drops) 0/0/0\n      (pkts output",
    ] {
        assert!(
            !once.0.contains(needle),
            "nothing behind {needle:?}: {}",
            once.0
        );
    }
}

#[test]
fn a_lab_saved_before_flows_had_a_marking_loads_and_saves_unchanged() {
    // A lab file as they were written before the field existed.
    let saved = r#"{"name":"old","description":"","nodes":[
        {"name":"PC1","model":"pc","x":0.0,"y":0.0,"powered":true,"startup_config":null,
         "host":{"dhcp":false,"ip":"10.0.0.1","mask":"255.255.255.0","gateway":null,"dns":null,"extra_ips":[],"services":[]}},
        {"name":"PC2","model":"pc","x":0.0,"y":0.0,"powered":true,"startup_config":null,
         "host":{"dhcp":false,"ip":"10.0.0.2","mask":"255.255.255.0","gateway":null,"dns":null,"extra_ips":[],"services":[]}}],
        "links":[{"a":{"node":"PC1","interface":"Ethernet0"},"b":{"node":"PC2","interface":"Ethernet0"},"cable":"auto"}],
        "notes":[],
        "flows":[{"name":"copy","src":"PC1","dst_ip":"10.0.0.2","bps":1000000,"pattern":"constant","enabled":true}],
        "seed":7}"#;
    let spec: LabSpec = serde_json::from_str(saved).unwrap();
    assert_eq!(spec.flows[0].dscp, None);
    let mut lab = Lab::from_spec(&spec).unwrap();
    lab.settle(10 * SECOND);
    lab.advance(3 * SECOND);
    assert!(lab.flows()[0].2.active);
    // Saved again, the flow looks as it did: no marking is not written as one.
    let again = serde_json::to_value(lab.to_spec()).unwrap();
    assert_eq!(
        again["flows"],
        serde_json::from_str::<serde_json::Value>(saved).unwrap()["flows"]
    );

    // A marking that is set is kept through the lab file.
    let id = lab
        .add_flow(FlowSpec {
            name: "voice".into(),
            src: "PC1".into(),
            dst_ip: "10.0.0.2".into(),
            bps: 64_000,
            dscp: Some(46),
            ..FlowSpec::default()
        })
        .unwrap();
    assert_eq!(
        lab.flows().iter().find(|f| f.0 == id).unwrap().1.dscp,
        Some(46)
    );
    let text = serde_json::to_string(&lab.to_spec()).unwrap();
    assert!(text.contains(r#""enabled":true,"dscp":46}"#), "{text}");
    let reloaded = Lab::from_spec(&serde_json::from_str(&text).unwrap()).unwrap();
    assert_eq!(reloaded.to_spec().flows[1].dscp, Some(46));
    // Zero is a marking (`default`), not the absence of one.
    assert_eq!(
        serde_json::from_str::<FlowSpec>(r#"{"dscp":0}"#)
            .unwrap()
            .dscp,
        Some(0)
    );
    // A code point has six bits.
    let refused = lab.add_flow(FlowSpec {
        name: "bad".into(),
        src: "PC1".into(),
        dst_ip: "10.0.0.2".into(),
        dscp: Some(64),
        ..FlowSpec::default()
    });
    assert!(
        matches!(&refused, Err(LabError::InvalidSpec(why)) if why.contains("bad") && why.contains("64")),
        "{refused:?}"
    );
}

/// The policies of the reference's own hierarchical example (`show
/// policy-map interface FastEthernet0/0`, policies `test1` and `test2`), as
/// far as the simulator takes them: its `queue-limit 128` and `random-detect`
/// lines are refused here.
const REFERENCE_POLICIES: &str = "\
class-map match-all RT
 match ip dscp ef
class-map match-all BH
 match ip dscp af41
class-map match-all BL
 match ip dscp af21
policy-map test2
 class RT
  priority percent 20
 class BH
  bandwidth percent 40
 class BL
  bandwidth percent 35
policy-map test1
 class class-default
  shape average 1536000
  service-policy test2
interface GigabitEthernet0/1
 service-policy output test1";

/// The sample output printed for those policies in the "Cisco IOS Quality of
/// Service Solutions Command Reference", chapter "show mls qos through
/// wrr-queue threshold", `show policy-map interface`, the HQF example
/// (<https://www.cisco.com/c/en/us/td/docs/ios-xml/ios/qos/command/qos-cr-book/qos-s4.html>,
/// read 2026-11-22), typed in here with these differences and no others:
///
/// * the interface is the lab router's, not `FastEthernet0/0`;
/// * the counters are zero (the sample has 129 packets in `class-default`);
/// * `5 minute` where the sample's interface had `load-interval 30`;
/// * class `BH` has the default queue limit, 64, where the sample configures
///   128, and class `BL` is without its WRED table;
/// * spaces at line ends, the second empty line after class `RT` and the
///   empty line before the last class's queue are left out (the reference's
///   other hierarchical sample, `serial4/3`, has none there).
///
/// The numbers that are computed rather than configured are the sample's:
/// `bc 6144`, `307 kbps`, `burst bytes 7650`, `614 kbps`, `537 kbps`.
const REFERENCE_OUTPUT: &str = concat!(
    " GigabitEthernet0/1\n",
    "\n",
    "  Service-policy output: test1\n",
    "\n",
    "    Class-map: class-default (match-any)\n",
    "      0 packets, 0 bytes\n",
    "      5 minute offered rate 0 bps, drop rate 0 bps\n",
    "      Match: any\n",
    "      Queueing\n",
    "      queue limit 64 packets\n",
    "      (queue depth/total drops/no-buffer drops) 0/0/0\n",
    "      (pkts output/bytes output) 0/0\n",
    "      shape (average) cir 1536000, bc 6144, be 6144\n",
    "      target shape rate 1536000\n",
    "\n",
    "      Service-policy : test2\n",
    "\n",
    "        queue stats for all priority classes:\n",
    "\n",
    "          queue limit 64 packets\n",
    "          (queue depth/total drops/no-buffer drops) 0/0/0\n",
    "          (pkts output/bytes output) 0/0\n",
    "\n",
    "        Class-map: RT (match-all)\n",
    "          0 packets, 0 bytes\n",
    "          5 minute offered rate 0 bps, drop rate 0 bps\n",
    "          Match: ip dscp ef (46)\n",
    "          Priority: 20% (307 kbps), burst bytes 7650, b/w exceed drops: 0\n",
    "\n",
    "        Class-map: BH (match-all)\n",
    "          0 packets, 0 bytes\n",
    "          5 minute offered rate 0 bps, drop rate 0 bps\n",
    "          Match: ip dscp af41 (34)\n",
    "          Queueing\n",
    "          queue limit 64 packets\n",
    "          (queue depth/total drops/no-buffer drops) 0/0/0\n",
    "          (pkts output/bytes output) 0/0\n",
    "          bandwidth 40% (614 kbps)\n",
    "\n",
    "        Class-map: BL (match-all)\n",
    "          0 packets, 0 bytes\n",
    "          5 minute offered rate 0 bps, drop rate 0 bps\n",
    "          Match: ip dscp af21 (18)\n",
    "          Queueing\n",
    "          queue limit 64 packets\n",
    "          (queue depth/total drops/no-buffer drops) 0/0/0\n",
    "          (pkts output/bytes output) 0/0\n",
    "          bandwidth 35% (537 kbps)\n",
    "\n",
    "        Class-map: class-default (match-any)\n",
    "          0 packets, 0 bytes\n",
    "          5 minute offered rate 0 bps, drop rate 0 bps\n",
    "          Match: any\n",
    "          queue limit 64 packets\n",
    "          (queue depth/total drops/no-buffer drops) 0/0/0\n",
    "          (pkts output/bytes output) 0/0\n",
);

#[test]
fn the_policies_of_the_reference_s_example_print_as_the_reference_prints_them() {
    let (mut lab, [_, _, r1, _, _]) = network();
    cfg(&mut lab, r1, REFERENCE_POLICIES);
    pretty_assertions::assert_eq!(
        exec(&mut lab, r1, "show policy-map interface GigabitEthernet0/1"),
        REFERENCE_OUTPUT
    );
    // `show policy-map test1` and `test2`, the reference's HQF examples 1 and
    // 2 (the second without its queue-limit and WRED lines).
    assert_eq!(
        exec(&mut lab, r1, "show policy-map test1"),
        "  Policy Map test1\n    Class class-default\n      Average Rate Traffic Shaping\n      cir 1536000 (bps)\n      service-policy test2\n"
    );
    assert_eq!(
        exec(&mut lab, r1, "show policy-map test2"),
        "  Policy Map test2\n    Class RT\n      priority 20 (%)\n    Class BH\n      bandwidth 40 (%)\n    Class BL\n      bandwidth 35 (%)\n"
    );
    // The other default bursts the reference prints: `Priority: 32 kbps,
    // burst bytes 1500` (the `serial4/3` sample) and, in the older layout,
    // 128 kbit/s with a burst of 3200 bytes.
    cfg(
        &mut lab,
        r1,
        "policy-map test2\n class RT\n  priority 32\n class BH\n  no bandwidth\n  priority 128",
    );
    let shown = exec(&mut lab, r1, "show policy-map interface GigabitEthernet0/1");
    assert!(
        shown.contains("Priority: 32 kbps, burst bytes 1500, b/w exceed drops: 0"),
        "{shown}"
    );
    assert!(
        shown.contains("Priority: 128 kbps, burst bytes 3200, b/w exceed drops: 0"),
        "{shown}"
    );
}

#[test]
fn a_two_megabit_policer_offered_five_for_a_hundred_seconds_drops_three_fifths() {
    let (mut lab, [_, _, r1, _, _]) = network();
    cfg(
        &mut lab,
        r1,
        "policy-map CAP\n class class-default\n  police 2000000\ninterface g0/0\n service-policy input CAP",
    );
    let id = flow(&mut lab, "bulk", 5_000_000, None);
    lab.advance(100 * SECOND);
    let shown = exec(&mut lab, r1, "show policy-map interface");
    let class = class_block(&shown, "class-default", 0);
    // By hand: 5 Mbit/s for 100 s is 62.5 MB; 2 Mbit/s of it, 25 MB, conform
    // and three fifths, 37.5 MB, do not. A packet is 800 bytes.
    assert_eq!(classified(&class), [78_125, 62_500_000], "{shown}");
    assert_eq!(
        numbers(&class, "conformed"),
        [31_250, 25_000_000],
        "{shown}"
    );
    assert_eq!(numbers(&class, "exceeded"), [46_875, 37_500_000], "{shown}");
    let [_, offered] = classified(&class);
    assert_eq!(numbers(&class, "exceeded")[1] * 5, offered * 3);
    let state = lab.flows().into_iter().find(|f| f.0 == id).unwrap().2;
    assert_eq!(state.delivered_octets, 25_000_000);
}

#[test]
fn a_priority_class_with_a_policy_inside_shows_what_its_rate_turned_away() {
    let (mut lab, [_, _, r1, _, _]) = network();
    cfg(
        &mut lab,
        r1,
        "class-map match-any VOICE\n match dscp ef\n\
         policy-map INSIDE\n class class-default\n  police 4000000\n\
         policy-map UPLINK\n class VOICE\n  priority 500\n  service-policy INSIDE\n\
         interface g0/1\n speed 10\n service-policy output UPLINK",
    );
    lab.settle(30 * SECOND);
    // 13.5 Mbit/s for a 10 Mbit/s port: voice is held to its 500 kbit/s and
    // loses 1 Mbit/s, the default class gets the other 9.5 of its 12.
    flow(&mut lab, "voice", 1_500_000, Some(46));
    flow(&mut lab, "bulk", 12_000_000, None);
    lab.advance(10 * SECOND);
    let shown = exec(&mut lab, r1, "show policy-map interface g0/1");
    let turned_away = octets(1_000_000, 10) / 800;
    assert_eq!(
        numbers(&class_block(&shown, "VOICE", 0), "Priority:"),
        [500, 12_500, turned_away],
        "{shown}"
    );
    let all = shown
        .split("queue stats for all priority classes:")
        .nth(1)
        .unwrap()
        .split("Class-map:")
        .next()
        .unwrap();
    assert_eq!(numbers(all, "(queue depth"), [0, turned_away, 0], "{shown}");
    assert_eq!(
        numbers(all, "(pkts output"),
        [octets(500_000, 10) / 800, octets(500_000, 10)],
        "{shown}"
    );
    // Every packet the port did not send is in one of the two drop counters.
    let default = class_block(&shown, "class-default", 1);
    let queued = numbers(&default, "(queue depth")[1];
    assert_eq!(queued, octets(2_500_000, 10) / 800, "{shown}");
    assert_eq!(
        interface(&lab, r1, "GigabitEthernet0/1").out_discards,
        // 1562.5 and 3906.25 packets: the interface counts the sum whole.
        octets(3_500_000, 10) / 800
    );
}

#[test]
fn when_the_last_flow_is_gone_nothing_goes_on_showing_its_traffic() {
    let (mut lab, [_, _, r1, _, _]) = network();
    cfg(
        &mut lab,
        r1,
        "policy-map SLOW\n class class-default\n  shape average 1000000\ninterface g0/1\n service-policy output SLOW",
    );
    let id = flow(&mut lab, "bulk", 5_000_000, None);
    lab.advance(10 * SECOND);
    let busy = exec(&mut lab, r1, "show policy-map interface");
    let class = class_block(&busy, "class-default", 0);
    assert_eq!(numbers(&class, "(queue depth")[0], 64, "{busy}");
    assert!(lab.snapshot().links.iter().any(|l| l.bps_ab == 5_000_000));

    lab.remove_flow(id);
    lab.advance(2 * SECOND);
    // No traffic: no link carries any and no queue stands full. The counters
    // stay, as they do on a device whose traffic has stopped.
    for link in lab.snapshot().links {
        assert_eq!((link.bps_ab, link.bps_ba), (0, 0), "{link:?}");
    }
    let idle = exec(&mut lab, r1, "show policy-map interface");
    let class = class_block(&idle, "class-default", 0);
    assert_eq!(
        numbers(&class, "(queue depth"),
        [0, octets(4_000_000, 10) / 800, 0],
        "{idle}"
    );
    assert_eq!(classified(&class), [7812, octets(5_000_000, 10)], "{idle}");
    // The averages run down instead of standing still...
    let rate = |shown: &str| numbers(&class_block(shown, "class-default", 0), "5 minute")[1];
    lab.advance(300 * SECOND);
    let later = exec(&mut lab, r1, "show policy-map interface");
    assert!(
        rate(&later) > 0 && rate(&later) < rate(&idle) / 2,
        "{idle}\n{later}"
    );
    // ...to nothing, on the policy and on the interface alike.
    lab.advance(2 * 3600 * SECOND);
    let quiet = exec(&mut lab, r1, "show policy-map interface");
    assert!(
        quiet.contains("5 minute offered rate 0 bps, drop rate 0 bps"),
        "{quiet}"
    );
    let port = exec(&mut lab, r1, "show interfaces g0/1 | include 5 minute");
    assert!(
        port.contains("5 minute input rate 0 bits/sec")
            && port.contains("5 minute output rate 0 bits/sec"),
        "{port}"
    );
    // A new flow is counted from where the old one stopped.
    flow(&mut lab, "again", 800_000, None);
    lab.advance(10 * SECOND);
    let again = exec(&mut lab, r1, "show policy-map interface");
    assert_eq!(
        classified(&class_block(&again, "class-default", 0))[1],
        octets(5_000_000, 10) + octets(800_000, 10),
        "{again}"
    );
}

#[test]
fn a_policy_cannot_be_multiplied_into_more_classes_than_a_device_holds() {
    let (mut lab, [_, _, r1, _, _]) = network();
    // Forty classes, in a policy that is then applied inside each of the
    // forty classes of another: 40 + 40 x 40 classes once written out.
    let mut text = String::new();
    for n in 0..40 {
        text.push_str(&format!("class-map match-any C{n}\n match dscp {n}\n"));
    }
    text.push_str("policy-map LEAF\n");
    for n in 0..40 {
        text.push_str(&format!(" class C{n}\n  set dscp {n}\n"));
    }
    text.push_str("policy-map TREE\n");
    for n in 0..40 {
        text.push_str(&format!(" class C{n}\n  service-policy LEAF\n"));
    }
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    let outcome = lab.configure(r1, &lines).unwrap();
    let refused: Vec<_> = outcome.lines.iter().filter(|line| line.error).collect();
    // 40 + 24 x 40 = 1000 classes fit; the twenty-fifth child would make 1040.
    assert_eq!(refused.len(), 16, "{refused:#?}");
    assert!(
        refused.iter().all(|line| {
            line.line.trim() == "service-policy LEAF"
                && line.output.contains(
                    "% policy map TREE cannot be extended: a policy holds 1024 classes at most",
                )
        }),
        "{refused:#?}"
    );
    let run = lab.running_config(r1).unwrap();
    assert_eq!(run.matches("  service-policy LEAF\n").count(), 24, "{run}");

    // A class added to the inner policy is one more in every place it is
    // applied: 40 + 24 x 41 is 1024 and fits, one more does not.
    cfg(
        &mut lab,
        r1,
        "class-map match-any EXTRA\n match dscp 50\nclass-map match-any MORE\n match dscp 51\npolicy-map LEAF\n class EXTRA",
    );
    let outcome = lab
        .configure(
            r1,
            &[
                "policy-map LEAF".to_string(),
                " class MORE".to_string(),
                "  police 8000".to_string(),
            ],
        )
        .unwrap();
    assert!(
        outcome.lines[1].error
            && outcome.lines[1]
                .output
                .contains("holds 1024 classes at most"),
        "{outcome:#?}"
    );
    // The refused class was not opened, so its action did not land on the
    // class before it.
    assert!(outcome.lines[2].error, "{outcome:#?}");
    let run = lab.running_config(r1).unwrap();
    assert!(
        !run.contains("class MORE") && !run.contains("police 8000"),
        "{run}"
    );
    // What was accepted still works and still types back.
    cfg(&mut lab, r1, "interface g0/1\n service-policy output TREE");
    flow(&mut lab, "marked", 1_000_000, Some(7));
    lab.advance(5 * SECOND);
    let shown = exec(&mut lab, r1, "show policy-map interface g0/1");
    // Class C7 of the outer policy, and C7 of the policy inside it: the
    // eighth and ninth of that name, after those inside C0 to C6.
    for nth in [7, 8] {
        assert_eq!(
            classified(&class_block(&shown, "C7", nth))[1],
            octets(1_000_000, 5),
            "{nth}: {shown}"
        );
    }
    assert_eq!(classified(&class_block(&shown, "C7", 0))[1], 0, "{shown}");
    let run = lab.running_config(r1).unwrap();
    let copy = Lab::from_spec(&LabSpec {
        nodes: vec![NodeSpec {
            name: "R1".into(),
            model: "router".into(),
            startup_config: Some(run.clone()),
            ..NodeSpec::default()
        }],
        ..LabSpec::default()
    })
    .unwrap();
    let id = copy.node_by_name("R1").unwrap();
    assert!(copy.startup_errors(id).is_empty());
    assert_eq!(copy.running_config(id).unwrap(), run);
}

/// A tiny generator for the robustness test: the same sequence every run.
struct Dice(u64);

impl Dice {
    fn roll(&mut self, sides: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) as usize) % sides
    }

    fn pick<'a>(&mut self, from: &[&'a str]) -> &'a str {
        from[self.roll(from.len())]
    }
}

#[test]
fn no_sequence_of_qos_commands_panics_or_leaves_a_configuration_that_cannot_be_typed_back() {
    const NAMES: [&str; 8] = [
        "A",
        "B",
        "C",
        "D",
        "class-default",
        "match-any",
        "interface",
        "x y",
    ];
    const NUMBERS: [&str; 10] = [
        "0",
        "1",
        "7",
        "100",
        "8000",
        "64000",
        "3000000",
        "128000000001",
        "18446744073709551616",
        "-5",
    ];
    const DSCP: [&str; 9] = ["ef", "af41", "cs1", "default", "0", "46", "63", "64", "zz"];
    const PORTS: [&str; 7] = ["g0/0", "g0/1", "g0/2", "lo0", "g0/1.5", "vl1", "fa0/1"];
    let (mut lab, [_, _, r1, _, _]) = network();
    flow(&mut lab, "voice", 300_000, Some(46));
    flow(&mut lab, "video", 2_000_000, Some(34));
    flow(&mut lab, "scavenger", 700_000, Some(8));
    flow(&mut lab, "plain", 6_000_000, None);
    let mut dice = Dice(0x51ED_C0DE);
    let mut accepted = 0;
    for round in 0..120 {
        let mut lines: Vec<String> = Vec::new();
        for _ in 0..40 {
            let (name, number, dscp, port) = (
                dice.pick(&NAMES),
                dice.pick(&NUMBERS),
                dice.pick(&DSCP),
                dice.pick(&PORTS),
            );
            let line = match dice.roll(30) {
                0 => format!("class-map {name}"),
                1 => format!("class-map match-any {name}"),
                2 => format!("class-map match-all {name}"),
                3 => format!("match dscp {dscp}"),
                4 => format!("match ip dscp {dscp} {}", dice.pick(&DSCP)),
                5 => format!("match access-group {number}"),
                6 => format!("match access-group name {name}"),
                7 => "match any".to_string(),
                8 | 9 => format!("policy-map {name}"),
                10..=12 => format!("class {name}"),
                13 => "priority".to_string(),
                14 => format!("priority {number}"),
                15 => format!("priority percent {number}"),
                16 => format!("bandwidth {number}"),
                17 => format!("bandwidth percent {number}"),
                18 => format!("police {number}"),
                19 => format!("police cir {number} conform-action transmit exceed-action drop"),
                20 => format!("shape average {number}"),
                21 => format!("set dscp {dscp}"),
                22 | 23 => format!("service-policy {name}"),
                24 => format!("interface {port}"),
                25 => format!("service-policy input {name}"),
                26 => format!("service-policy output {name}"),
                27 => format!("description {name} {number}"),
                28 => "exit".to_string(),
                _ => format!("access-list 10 permit {}", dice.pick(&["any", "10.1.0.10"])),
            };
            lines.push(if dice.roll(6) == 0 {
                format!("no {line}")
            } else {
                line
            });
        }
        let outcome = lab.configure(r1, &lines).unwrap();
        accepted += outcome.lines.iter().filter(|l| !l.error).count();
        lab.advance(2 * SECOND);
        let shown = exec(&mut lab, r1, "show policy-map interface");
        assert!(
            !shown.contains("NaN") && !shown.contains("inf"),
            "round {round}:\n{shown}"
        );
        let _ = exec(&mut lab, r1, "show policy-map");
        if round % 10 == 9 {
            exec(&mut lab, r1, "clear counters");
            // Whatever state the commands left, the running configuration
            // must come back as it is when typed into a fresh device.
            let run = lab.running_config(r1).unwrap();
            let copy = Lab::from_spec(&LabSpec {
                nodes: vec![NodeSpec {
                    name: "R1".into(),
                    model: "router".into(),
                    startup_config: Some(run.clone()),
                    ..NodeSpec::default()
                }],
                ..LabSpec::default()
            })
            .unwrap();
            let id = copy.node_by_name("R1").unwrap();
            pretty_assertions::assert_eq!(copy.running_config(id).unwrap(), run, "round {round}");
        }
    }
    assert!(
        accepted > 1000,
        "the generator has to say something valid: {accepted}"
    );

    // The same for raw bytes typed into the new modes.
    let c = lab.console_open(r1).unwrap();
    typed(&mut lab, c, "enable");
    typed(&mut lab, c, "configure terminal");
    typed(&mut lab, c, "policy-map A");
    typed(&mut lab, c, "class class-default");
    for chunk in 0..200 {
        let bytes: Vec<u8> = (0..100).map(|_| dice.roll(256) as u8).collect();
        lab.console_input(c, &bytes);
        if chunk % 20 == 0 {
            typed(&mut lab, c, "\x03policy-map A");
            typed(&mut lab, c, "class class-default");
        }
        let _ = lab.console_output(c);
    }
    lab.advance(2 * SECOND);
    let _ = exec(&mut lab, r1, "show policy-map interface");
}
