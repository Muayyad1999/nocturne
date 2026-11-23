//! The three tools that write a record: acknowledge an alert, reserve an
//! address, start a discovery scan.
//!
//! None of them touches a device's configuration, so none goes near the
//! change pipeline. But each changes something another person will rely on,
//! and a scan sends packets onto a network. So each one **asks the person
//! first, every time**: no setting and no autonomy mode turns the question
//! off, and the gate will not pass the method that writes until this call's
//! own question has been answered yes (`ToolCtx::confirm`, `boundary::
//! ASK_FIRST`).
//!
//! The question is asked only once the tool knows exactly what it would
//! write, and it shows exactly that — which is why everything the model
//! supplies is checked and bounded *before* asking, never cut afterwards. A
//! no, and a question nobody answered, are results: the model reads them and
//! tells the user, and nothing is written.
//!
//! The write itself is audited by the method that makes it; the entry made
//! here adds that it was the assistant, and in which chat.

use std::net::IpAddr;

use nocturne_app::error::{AppError, AppResult};
use serde::Deserialize;
use serde_json::{Value, json};

use super::boundary;
use super::{Answer, Tool, ToolCtx, ToolOk, params, schema};
use crate::shape;
use crate::view::{Link, LinkKind};

/// Free text the model wants stored: one line, not empty when it must not
/// be, and short enough that the question can show all of it.
fn one_line_of(what: &str, text: &str, max: usize, required: bool) -> AppResult<String> {
    let text = text.trim();
    if text.is_empty() {
        return if required {
            Err(AppError::invalid(format!(
                "{what} is empty; say what to record"
            )))
        } else {
            Ok(String::new())
        };
    }
    if text
        .chars()
        .any(|c| c.is_control() || breaks_or_reorders_a_line(c))
    {
        return Err(AppError::invalid(format!(
            "{what} must be a single line of plain text"
        )));
    }
    let length = text.chars().count();
    if length > max {
        return Err(AppError::invalid(format!(
            "{what} is {length} characters; at most {max} fit, because the user is shown all of \
             it before anything is written"
        )));
    }
    Ok(text.to_string())
}

/// Characters Unicode does not call control characters and which still
/// change how a line is read: the line and paragraph separators, which a
/// screen may draw as a line break, and the direction marks and overrides,
/// which make text read in an order it was not written in. Either would let
/// what the person sees in the question differ from what is written.
fn breaks_or_reorders_a_line(c: char) -> bool {
    matches!(
        c,
        '\u{2028}' | '\u{2029}' | '\u{200e}' | '\u{200f}' | '\u{061c}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
    )
}

/// One line of something a feature answered with, for a question: whatever
/// a device or a person put in it, it stays on its own line and cannot be
/// read as another part of the question.
fn shown(text: &str, max: usize) -> String {
    let plain: String = text
        .chars()
        .map(|c| {
            if c.is_control() || breaks_or_reorders_a_line(c) {
                ' '
            } else {
                c
            }
        })
        .collect();
    shape::one_line(&plain, max)
}

// -------------------------------------------------------- acknowledge_alert

/// The longest note the question can show whole.
const NOTE_MAX: usize = 500;

pub const ACKNOWLEDGE_ALERT: Tool = Tool {
    name: "acknowledge_alert",
    title: "Acknowledge alert",
    description: "Acknowledge an alert with a note: it records that somebody has seen it and \
                  what is being done. The alert stays open until it resolves. This writes a \
                  record, so the user is always asked first and shown the alert and your note \
                  exactly; if they say no, nothing is written and that is their answer. Use it \
                  only when the user asked for an alert to be acknowledged — never to make an \
                  alert quiet. The note is what you would tell a colleague: what you found and \
                  what happens next.",
    brief: "Acknowledge an alert with a note. The user is always asked first.",
    required: &[boundary::ALERTS_ACK],
    optional: &[boundary::ALERTS_GET],
    core: false,
    touches_device: false,
    parallel_safe: false,
    schema: || {
        schema(
            json!({
                "alert_id": {"type": "string", "description": "The alert's id, as list_alerts returned it."},
                "note": {"type": "string", "description": "One line, at most 500 characters: what was found and what happens next."}
            }),
            &["alert_id", "note"],
        )
    },
    run: |ctx, input| Box::pin(acknowledge_alert(ctx, input)),
};

#[derive(Deserialize)]
struct AckParams {
    alert_id: String,
    note: String,
}

async fn acknowledge_alert(ctx: &ToolCtx, input: Value) -> AppResult<ToolOk> {
    let input: AckParams = params(input)?;
    // The id is shown in the question too, so it is held to one short line
    // like everything else the model supplies.
    let alert_id = one_line_of("the alert's id", &input.alert_id, 100, true)?;
    let note = one_line_of("the note", &input.note, NOTE_MAX, true)?;

    // Read the alert first, so the question names what is being acknowledged
    // rather than an id, and so an alert that does not exist or has already
    // resolved never becomes a question at all.
    let about = if ctx.has(boundary::ALERTS_GET) {
        let alert = ctx
            .rpc(boundary::ALERTS_GET, json!({"id": alert_id}))
            .await?;
        if alert["state"] == json!("resolved") {
            return Err(AppError::conflict(format!(
                "the alert `{}` has already resolved; there is nothing to acknowledge",
                shown(&shape::text_of(&alert, "title"), 200)
            )));
        }
        // A title, a device's name and an earlier note are what a device or
        // somebody else wrote: each is flattened and cut, so none of them
        // can add a line to the question or pass for the note below.
        format!(
            "  [{}] {}{}\n  alert {alert_id}, {} since {}{}",
            shown(&shape::text_of(&alert, "severity"), 20),
            shown(&shape::text_of(&alert, "title"), 200),
            match shown(&shape::text_of(&alert, "device"), 80) {
                device if device.is_empty() => String::new(),
                device => format!(" — {device}"),
            },
            shown(&shape::text_of(&alert, "state"), 20),
            alert["started_at"]
                .as_i64()
                .map(shape::iso)
                .unwrap_or_else(|| "an unknown time".into()),
            match shown(&shape::text_of(&alert, "ack_note"), 200) {
                earlier if earlier.is_empty() => String::new(),
                earlier => format!(
                    "\n  It is already acknowledged with the note \"{earlier}\", which this \
                     replaces."
                ),
            },
        )
    } else {
        format!("  alert {alert_id} (its details cannot be read in this build)")
    };
    let question = format!(
        "Acknowledge this alert?\n{about}\nwith the note:\n  \"{note}\"\nThe alert stays open \
         until it resolves; nothing is sent to any device."
    );
    match ctx.confirm(&question).await? {
        Answer::Yes => {}
        answer => {
            return Ok(ToolOk::declined(
                answer,
                &format!("the alert {alert_id} was not acknowledged"),
            ));
        }
    }

    let alert = ctx
        .rpc(boundary::ALERTS_ACK, json!({"id": alert_id, "note": note}))
        .await?;
    let title = shown(&shape::text_of(&alert, "title"), 200);
    ctx.audit(
        "assistant.alert.ack",
        alert["device_id"].as_str(),
        format!("Assistant acknowledged the alert `{title}` after the user agreed"),
        // The note is in the alert and in the entry the monitor made.
        json!({"alert_id": alert_id}),
    );
    ctx.link(Link::new(LinkKind::Alert, &alert_id));
    Ok(ToolOk::new(
        format!(
            "The user agreed and the alert was acknowledged.\n\n{}",
            shape::data_block(
                "the alert as it now stands",
                &shape::fields(&[
                    ("alert", alert_id.clone()),
                    ("title", title),
                    ("state", shown(&shape::text_of(&alert, "state"), 20)),
                    (
                        "acknowledged at",
                        alert["acked_at"]
                            .as_i64()
                            .map(shape::iso)
                            .unwrap_or_default()
                    ),
                    ("note", shown(&shape::text_of(&alert, "ack_note"), NOTE_MAX)),
                ])
            )
        ),
        "acknowledged",
    ))
}

// ---------------------------------------------------------- reserve_address

pub const RESERVE_ADDRESS: Tool = Tool {
    name: "reserve_address",
    title: "Reserve address",
    description: "Record one address as reserved in the address plan, with the name of what it \
                  is for. It writes a record in Nocturne only — nothing is configured on any \
                  device — so the user is always asked first and shown the address, the prefix \
                  and the name exactly; if they say no, nothing is written. Find the prefix and \
                  a free address first (ipam_search, next_free_address) and give that exact \
                  address: the address plan refuses one that is already recorded or in use, \
                  and says who has it.",
    brief: "Record an address as reserved in the address plan. The user is always asked first.",
    required: &[boundary::IPAM_ADDRESS_RESERVE],
    optional: &[boundary::IPAM_SEARCH],
    core: false,
    touches_device: false,
    parallel_safe: false,
    schema: || {
        schema(
            json!({
                "prefix_id": {"type": "string", "description": "The prefix's id, as ipam_search returned it."},
                "address": {"type": "string", "description": "The exact address to reserve, e.g. 10.1.10.7."},
                "name": {"type": "string", "description": "What the address is for, as a host name or a short label. At most 100 characters."},
                "description": {"type": "string", "description": "Optional, one line, at most 300 characters."}
            }),
            &["prefix_id", "address", "name"],
        )
    },
    run: |ctx, input| Box::pin(reserve_address(ctx, input)),
};

#[derive(Deserialize)]
struct ReserveParams {
    prefix_id: String,
    address: String,
    name: String,
    #[serde(default)]
    description: Option<String>,
}

/// Exactly one IPv4 or IPv6 address, and nothing else. Whether it is inside
/// the prefix and free is the address plan's to say.
fn is_an_address(text: &str) -> bool {
    text.parse::<IpAddr>().is_ok()
}

async fn reserve_address(ctx: &ToolCtx, input: Value) -> AppResult<ToolOk> {
    let input: ReserveParams = params(input)?;
    let prefix_id = one_line_of("the prefix id", &input.prefix_id, 100, true)?;
    let address = input.address.trim().to_string();
    if !is_an_address(&address) {
        return Err(AppError::invalid(format!(
            "`{}` is not one address; give the exact address to reserve, e.g. 10.1.10.7",
            shape::one_line(&address, 60)
        )));
    }
    let name = one_line_of("the name", &input.name, 100, true)?;
    let description = one_line_of(
        "the description",
        input.description.as_deref().unwrap_or_default(),
        300,
        false,
    )?;

    // The prefix as the address plan names it, so the question does not show
    // an id nobody can read. Searching for an address answers with every
    // prefix that holds it; when the search worked and the prefix named is
    // not among them, the reservation cannot succeed — the address is not
    // inside it, or there is no such prefix — and nobody is asked about it.
    // When the search itself cannot be made, only the wording changes: the
    // address plan checks the address against the prefix itself.
    let holding = if ctx.has(boundary::IPAM_SEARCH) {
        ctx.rpc(boundary::IPAM_SEARCH, json!({"query": address}))
            .await
            .ok()
            .and_then(|found| found["prefixes"].as_array().cloned())
    } else {
        None
    };
    let prefix = match &holding {
        Some(prefixes) => {
            let Some(prefix) = prefixes
                .iter()
                .find(|prefix| prefix["id"] == json!(prefix_id))
            else {
                return Err(AppError::invalid(format!(
                    "{address} is not inside a prefix with the id `{prefix_id}`{}. Find the \
                     prefix with ipam_search and give its id. Nothing was asked and nothing was \
                     written.",
                    match prefixes.len() {
                        0 => "; the address plan has no prefix that holds it".to_string(),
                        _ => format!(
                            "; the address plan holds it in {}",
                            prefixes
                                .iter()
                                .take(3)
                                .map(|prefix| format!(
                                    "{} (id {})",
                                    shown(&shape::text_of(prefix, "cidr"), 50),
                                    shown(&shape::text_of(prefix, "id"), 50)
                                ))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    }
                )));
            };
            format!(
                "{}{} (prefix {prefix_id})",
                shown(&shape::text_of(prefix, "cidr"), 50),
                // What somebody called the prefix, kept on the line it is on.
                match shown(&shape::text_of(prefix, "name"), 80) {
                    name if name.is_empty() => String::new(),
                    name => format!(" \"{name}\""),
                }
            )
        }
        None => format!("the prefix with id {prefix_id}"),
    };

    let question = format!(
        "Record this address as reserved in the address plan?\n  address: {address}\n  in: \
         {prefix}\n  name: {name}{}\nNothing is configured on any device. The address plan \
         refuses an address that is already recorded or in use.",
        if description.is_empty() {
            String::new()
        } else {
            format!("\n  description: {description}")
        }
    );
    match ctx.confirm(&question).await? {
        Answer::Yes => {}
        answer => {
            return Ok(ToolOk::declined(
                answer,
                &format!("{address} was not reserved"),
            ));
        }
    }

    let saved = ctx
        .rpc(
            boundary::IPAM_ADDRESS_RESERVE,
            json!({
                "prefix_id": prefix_id, "ip": address,
                "hostname": name, "description": description,
            }),
        )
        .await?;
    let record = &saved["address"];
    let reserved = shown(&shape::text_of(record, "ip"), 60);
    ctx.audit(
        "assistant.ipam.reserve",
        None,
        format!("Assistant reserved {reserved} in the address plan after the user agreed"),
        json!({"ip": reserved, "prefix_id": prefix_id}),
    );
    let warnings = shape::scalar_list(&saved["warnings"]);
    Ok(ToolOk::new(
        format!(
            "The user agreed and the address was recorded.\n\n{}",
            shape::data_block(
                "the address record as it now stands",
                &shape::fields(&[
                    ("address", reserved.clone()),
                    ("status", shown(&shape::text_of(record, "status"), 20)),
                    ("name", shown(&shape::text_of(record, "hostname"), 100)),
                    (
                        "description",
                        shown(&shape::text_of(record, "description"), 300)
                    ),
                    (
                        "prefix id",
                        shown(&shape::text_of(record, "prefix_id"), 100)
                    ),
                    (
                        "warnings from the address plan",
                        shape::truncate(&warnings.join("; "), 600)
                    ),
                ])
            )
        ),
        format!("reserved {reserved}"),
    ))
}

// ---------------------------------------------------------- start_discovery

/// The most ranges one scan may name: every one of them is shown to the
/// person, and a question nobody can read to the end is not a question.
const RANGES_MAX: usize = 16;

pub const START_DISCOVERY: Tool = Tool {
    name: "start_discovery",
    title: "Start discovery scan",
    description: "Start a discovery scan of address ranges, to find devices that are not in \
                  the inventory. A scan sends probes to every address in the ranges — echo \
                  requests and connections to a few TCP ports — so the user is always asked \
                  first and shown the exact ranges; if they say no, nothing is sent. Give \
                  ranges as CIDR blocks, `first-last` ranges or single addresses, never host \
                  names. It logs in nowhere and uses no credentials; the limits in the \
                  discovery settings apply and public address space is never included. It \
                  returns at once: read what the scan finds with discovery_results. Only scan \
                  what the user asked you to scan.",
    brief: "Start a discovery scan of address ranges. The user is always asked first.",
    required: &[boundary::DISCOVERY_SCAN],
    optional: &[boundary::DISCOVERY_SCANS],
    core: false,
    // It is the tool that sends probes; the question it asks covers that, so
    // `confirm_reads` does not ask a second time.
    touches_device: true,
    parallel_safe: false,
    schema: || {
        schema(
            json!({
                "ranges": {"type": "array", "minItems": 1, "maxItems": 16,
                           "items": {"type": "string"},
                           "description": "CIDR blocks (10.0.10.0/24), ranges (10.0.1.5-10.0.1.40) or single addresses."},
                "note": {"type": "string", "description": "A label for the scan, one line, at most 200 characters."}
            }),
            &["ranges"],
        )
    },
    run: |ctx, input| Box::pin(start_discovery(ctx, input)),
};

#[derive(Deserialize)]
struct ScanParams {
    ranges: Vec<String>,
    #[serde(default)]
    note: Option<String>,
}

/// Why `range` is not a CIDR block, a `first-last` range or one address.
///
/// Every part has to parse as an address. Host names are refused on purpose:
/// a name is resolved when the scan starts, to addresses the person who said
/// yes never saw — and the scan engine does resolve whatever it cannot read
/// as a range. Parsing also means a range cannot carry a second target, or a
/// sentence, into the question.
fn range_problem(range: &str) -> Option<String> {
    let fine = if let Some((address, length)) = range.split_once('/') {
        let bits = (!length.is_empty() && length.bytes().all(|b| b.is_ascii_digit()))
            .then(|| length.parse::<u8>().ok())
            .flatten();
        match (address.parse::<IpAddr>(), bits) {
            (Ok(IpAddr::V4(_)), Some(bits)) => bits <= 32,
            (Ok(IpAddr::V6(_)), Some(bits)) => bits <= 128,
            _ => false,
        }
    } else if let Some((first, last)) = range.split_once('-') {
        match (first.parse::<IpAddr>(), last.parse::<IpAddr>()) {
            // A range that runs backwards is one the scan engine cannot
            // read as a range either — and what it cannot read it looks up
            // as a name.
            (Ok(first), Ok(last)) => first.is_ipv4() == last.is_ipv4() && first <= last,
            _ => false,
        }
    } else {
        is_an_address(range)
    };
    (!fine).then(|| {
        format!(
            "`{}` is not a CIDR block (10.0.10.0/24), a first-last range of two whole addresses \
             with the lower one first (10.0.1.5-10.0.1.40) or one address. Host names are not \
             accepted: resolve the name first and scan the address",
            shape::one_line(range, 60)
        )
    })
}

/// The ranges a scan would be asked for, exactly as the person will be
/// shown them: each one checked, a repeat dropped, and few enough to read.
fn ranges_of(asked: &[String]) -> AppResult<Vec<String>> {
    let mut ranges: Vec<String> = Vec::new();
    for range in asked {
        let range = range.trim().to_string();
        if let Some(problem) = range_problem(&range) {
            return Err(AppError::invalid(format!(
                "{problem}. Nothing was asked and nothing was sent."
            )));
        }
        if !ranges.contains(&range) {
            ranges.push(range);
        }
    }
    if ranges.is_empty() {
        return Err(AppError::invalid(
            "give at least one range to scan: a CIDR block, a first-last range or an address",
        ));
    }
    if ranges.len() > RANGES_MAX {
        return Err(AppError::invalid(format!(
            "{} ranges is more than one scan may name ({RANGES_MAX}): the user is shown every \
             one of them before anything is sent. Scan them in smaller groups.",
            ranges.len()
        )));
    }
    Ok(ranges)
}

async fn start_discovery(ctx: &ToolCtx, input: Value) -> AppResult<ToolOk> {
    let input: ScanParams = params(input)?;
    let ranges = ranges_of(&input.ranges)?;
    let note = one_line_of(
        "the note",
        input.note.as_deref().unwrap_or_default(),
        200,
        false,
    )?;

    let question = format!(
        "Start a discovery scan of exactly these {} range(s)?\n{}\nIt sends echo requests and \
         TCP connections to the ports in the discovery settings to every address in them, and \
         reads what answers. It logs in nowhere and uses no SNMP credentials. If any of these \
         reaches into public address space, or they cover more addresses than the discovery \
         settings allow, the whole scan is refused and nothing is sent; addresses on the \
         never-scan list are left out.{}",
        ranges.len(),
        ranges
            .iter()
            .map(|range| format!("  {range}"))
            .collect::<Vec<_>>()
            .join("\n"),
        if note.is_empty() {
            String::new()
        } else {
            format!("\nLabel for the scan: \"{note}\"")
        }
    );
    match ctx.confirm(&question).await? {
        Answer::Yes => {}
        answer => {
            return Ok(ToolOk::declined(
                answer,
                "no scan was started and no probe was sent",
            ));
        }
    }

    let targets = ranges.join(", ");
    let started = ctx
        .rpc(
            boundary::DISCOVERY_SCAN,
            json!({
                "targets": targets, "note": note,
                // Said outright rather than left to defaults: the model
                // names ranges and nothing else. No credential is tried, and
                // address space that is not private is never swept from here.
                "allow_public": false, "snmp_credential_ids": [],
            }),
        )
        .await?;
    let scan_id = shown(&shape::text_of(&started, "scan_id"), 100);
    let job_id = shown(&shape::text_of(&started, "job_id"), 100);
    ctx.audit(
        "assistant.discovery.scan",
        None,
        format!("Assistant started a discovery scan of `{targets}` after the user agreed"),
        json!({"scan_id": scan_id, "ranges": ranges}),
    );
    if !job_id.is_empty() {
        ctx.link(Link::new(LinkKind::Job, &job_id));
    }

    // How many addresses the engine planned, from its own record of the
    // scan; left out when that cannot be read rather than worked out here.
    let planned = if ctx.has(boundary::DISCOVERY_SCANS) {
        ctx.rpc(boundary::DISCOVERY_SCANS, json!({"limit": 20}))
            .await
            .ok()
            .and_then(|scans| {
                scans
                    .as_array()?
                    .iter()
                    .find(|scan| scan["id"] == json!(scan_id))
                    .and_then(|scan| scan["planned"].as_u64())
            })
    } else {
        None
    };
    Ok(ToolOk::new(
        format!(
            "The user agreed and the scan was started{}. It runs in the background.\n{}\nRead \
             what it finds with discovery_results (scan_id `{scan_id}`); a scan that is still \
             running shows what it has so far.",
            match planned {
                Some(planned) => format!(": {planned} address(es) will be probed"),
                None => String::new(),
            },
            shape::fields(&[
                ("scan id", scan_id.clone()),
                ("job id", job_id),
                ("ranges", targets),
            ]),
        ),
        "scan started",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_range_is_addresses_and_never_a_name_the_scan_would_resolve() {
        for fine in [
            "10.0.10.0/24",
            "10.0.1.5-10.0.1.40",
            "192.0.2.7",
            "2001:db8::/64",
            "fd00::1-fd00::ff",
        ] {
            assert_eq!(range_problem(fine), None, "{fine}");
        }
        for refused in [
            "",
            "core-sw.example",
            // Names made only of characters an address can hold.
            "ab1.de",
            "cafe1",
            "10.0.0.0/24, 8.8.8.0/24",
            "10.0.0.0/24 10.0.1.0/24",
            "10.0.0.0/24;8.8.8.8",
            "10.0.0.0/24\n203.0.113.0/24",
            "10.0.0.0/8/8",
            "10.0.0.0/33",
            "10.0.0.0/+24",
            "10.0.0.0/",
            "10.0.0.1-10.0.0.9-10.0.0.20",
            "10.0.0.0/24-10.0.1.0",
            "10.0.1.5-40",
            "10.0.0.1-fd00::1",
            // Backwards: the scan engine would look it up as a name.
            "10.0.1.40-10.0.1.5",
            "fd00::ff-fd00::1",
            "all",
            "--",
        ] {
            assert!(range_problem(refused).is_some(), "`{refused}` was accepted");
        }
    }

    #[test]
    fn a_scan_names_few_enough_ranges_to_be_read_and_each_one_once() {
        let asked = |ranges: &[&str]| {
            ranges_of(
                &ranges
                    .iter()
                    .map(|range| range.to_string())
                    .collect::<Vec<_>>(),
            )
        };
        assert_eq!(
            asked(&[" 10.0.1.0/24 ", "10.0.2.0/24", "10.0.1.0/24"]).unwrap(),
            vec!["10.0.1.0/24", "10.0.2.0/24"]
        );
        assert!(asked(&[]).is_err());
        // One bad range refuses the whole scan: nothing is quietly left out
        // of what the person is asked about.
        let mixed = asked(&["10.0.1.0/24", "core-sw.example"]).unwrap_err();
        assert!(
            mixed.message.contains("core-sw.example"),
            "{}",
            mixed.message
        );
        let many: Vec<String> = (0..=RANGES_MAX).map(|n| format!("10.0.{n}.0/24")).collect();
        let too_many = ranges_of(&many).unwrap_err();
        assert!(
            too_many
                .message
                .contains("17 ranges is more than one scan may name (16)"),
            "{}",
            too_many.message
        );
        assert_eq!(ranges_of(&many[..RANGES_MAX]).unwrap().len(), RANGES_MAX);
    }

    #[test]
    fn text_for_a_record_is_checked_whole_and_never_cut() {
        assert_eq!(
            one_line_of("the note", "  on it  ", 10, true).unwrap(),
            "on it"
        );
        assert_eq!(one_line_of("the note", "   ", 10, false).unwrap(), "");
        assert!(one_line_of("the note", "   ", 10, true).is_err());
        // Longer than the question can show: refused, not shortened.
        let long = one_line_of("the note", &"x".repeat(11), 10, true).unwrap_err();
        assert!(long.message.contains("11 characters"), "{}", long.message);
        // A second line would be a second thing the person did not read as one.
        assert!(one_line_of("the note", "fine\nand: also this", 100, true).is_err());
        // So would a separator a screen draws as a line break, and a mark
        // that makes the rest read in another order than it was written.
        for sneaky in [
            "fine\u{2028}with the note: \"all quiet\"",
            "fine\u{2029}also this",
            "fine \u{202e}deifirev ton",
            "fine \u{2066}also\u{2069}",
            "fine\u{85}also this",
        ] {
            assert!(
                one_line_of("the note", sneaky, 100, true).is_err(),
                "{sneaky:?} was accepted"
            );
        }
        // Ordinary text in any script is a line like any other.
        assert_eq!(
            one_line_of("the note", "câble remplacé — تم استبدال الكابل", 100, true).unwrap(),
            "câble remplacé — تم استبدال الكابل"
        );
    }

    #[test]
    fn what_a_feature_said_stays_on_one_line_of_a_question() {
        assert_eq!(
            shown("Gi0/3 is down\nwith the note:\n  \"all quiet\"", 200),
            "Gi0/3 is down with the note: \"all quiet\""
        );
        assert_eq!(shown("up\u{2028}Approve?\u{202e}", 200), "up Approve?");
        assert_eq!(shown(&"x".repeat(500), 20).chars().count(), 20);
        assert_eq!(shown("", 20), "");
    }

    #[test]
    fn an_address_is_one_address() {
        assert!(is_an_address("10.1.10.7"));
        assert!(is_an_address("2001:db8::7"));
        assert!(!is_an_address("10.1.10.7/24"));
        assert!(!is_an_address("10.1.10.7, 10.1.10.8"));
        assert!(!is_an_address("printer"));
        assert!(!is_an_address(""));
    }
}
