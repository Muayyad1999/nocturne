//! Who connects to whom, where an address lives, and probes.

use nocturne_app::error::{AppError, AppResult};
use serde::Deserialize;
use serde_json::{Value, json};

use super::boundary;
use super::{DEVICE_ARG, Tool, ToolCtx, ToolOk, params, schema};
use crate::shape::{self, Table};

// ------------------------------------------------------ topology_neighbors

pub const TOPOLOGY_NEIGHBORS: Tool = Tool {
    name: "topology_neighbors",
    title: "Neighbours",
    description: "What a device is physically connected to, from LLDP/CDP and from what has \
                  been collected: the local interface, the neighbour and its interface. Use it \
                  to follow a path by hand, or to see what an interface would cut off before \
                  you propose shutting it.",
    brief: "What a device is connected to, interface by interface.",
    required: &[boundary::TOPOLOGY_NEIGHBORS],
    optional: &[],
    core: false,
    touches_device: false,
    parallel_safe: true,
    schema: || {
        schema(
            json!({"device": {"type": "string", "description": DEVICE_ARG}}),
            &["device"],
        )
    },
    run: |ctx, input| Box::pin(topology_neighbors(ctx, input)),
};

#[derive(Deserialize)]
struct DeviceParams {
    device: String,
}

async fn topology_neighbors(ctx: &ToolCtx, input: Value) -> AppResult<ToolOk> {
    let input: DeviceParams = params(input)?;
    let found = ctx
        .rpc(
            boundary::TOPOLOGY_NEIGHBORS,
            json!({"device_id": input.device}),
        )
        .await?;
    let links = found
        .get("links")
        .and_then(Value::as_array)
        .cloned()
        .or_else(|| found.as_array().cloned())
        .unwrap_or_default();
    let mut table = Table::new(
        &["local interface", "neighbour", "its interface", "how"],
        ctx.caps().rows,
    );
    for link in &links {
        table.push([
            shape::text_of(link, "local_interface"),
            format!(
                "{} {}",
                shape::text_of(link, "device"),
                shape::text_of(link, "address")
            ),
            shape::text_of(link, "remote_interface"),
            shape::text_of(link, "protocol"),
        ]);
    }
    Ok(ToolOk::new(
        format!(
            "{} neighbour(s) of `{}`.\n\n{}",
            links.len(),
            input.device,
            shape::data_block(&format!("neighbours of {}", input.device), &table.render())
        ),
        format!("{} neighbour(s)", links.len()),
    ))
}

// --------------------------------------------------------------- find_path

pub const FIND_PATH: Tool = Tool {
    name: "find_path",
    title: "Find a path",
    description: "The path packets would take between two devices or addresses, worked out \
                  from the routing, ARP and MAC tables Nocturne has collected. It stops \
                  honestly when the data runs out ('no route data collected for R3') instead \
                  of guessing — say so when it does.",
    brief: "The path between two devices or addresses, from collected routing data.",
    required: &[boundary::TOPOLOGY_PATH],
    optional: &[],
    core: false,
    touches_device: false,
    parallel_safe: true,
    schema: || {
        schema(
            json!({
                "from": {"type": "string", "description": "Device name, id or IP address."},
                "to": {"type": "string", "description": "Device name, id or IP address."}
            }),
            &["from", "to"],
        )
    },
    run: |ctx, input| Box::pin(find_path(ctx, input)),
};

#[derive(Deserialize)]
struct PathParams {
    from: String,
    to: String,
}

async fn find_path(ctx: &ToolCtx, input: Value) -> AppResult<ToolOk> {
    let input: PathParams = params(input)?;
    let found = ctx
        .rpc(
            boundary::TOPOLOGY_PATH,
            json!({"from": input.from, "to": input.to}),
        )
        .await?;
    let hops = found["hops"].as_array().cloned().unwrap_or_default();
    let mut table = Table::new(
        &["#", "node", "in", "out", "via", "detail"],
        ctx.caps().rows,
    );
    for (index, hop) in hops.iter().enumerate() {
        table.push([
            (index + 1).to_string(),
            shape::text_of(hop, "node"),
            shape::text_of(hop, "in_interface"),
            shape::text_of(hop, "out_interface"),
            shape::text_of(hop, "via"),
            shape::text_of(hop, "detail"),
        ]);
    }
    let complete = found["complete"].as_bool() == Some(true);
    let stopped = shape::text_of(&found, "stopped_because");
    Ok(ToolOk::new(
        format!(
            "{} hop(s) from `{}` to `{}`. {}\n\n{}",
            hops.len(),
            input.from,
            input.to,
            if complete {
                "The path is complete.".to_string()
            } else {
                format!("The path is incomplete: {stopped}")
            },
            shape::data_block("the path, from collected routing data", &table.render())
        ),
        format!("{} hop(s)", hops.len()),
    ))
}

// ------------------------------------------------------------- locate_host

pub const LOCATE_HOST: Tool = Tool {
    name: "locate_host",
    title: "Find a host",
    description: "Which switch port an IP address, MAC address or hostname is plugged into, \
                  from the collected ARP and MAC tables. Returns every place the address was \
                  seen, with the edge port first and the trunks behind it, and when each \
                  sighting was.",
    brief: "Which switch port an IP or MAC address is plugged into.",
    required: &[boundary::LOCATE_HOST],
    optional: &[],
    core: false,
    touches_device: false,
    parallel_safe: true,
    schema: || {
        schema(
            json!({"query": {"type": "string", "description": "An IP address, a MAC address in any format, or a hostname."}}),
            &["query"],
        )
    },
    run: |ctx, input| Box::pin(locate_host(ctx, input)),
};

#[derive(Deserialize)]
struct QueryParams {
    query: String,
}

async fn locate_host(ctx: &ToolCtx, input: Value) -> AppResult<ToolOk> {
    let input: QueryParams = params(input)?;
    let found = ctx
        .rpc(boundary::LOCATE_HOST, json!({"query": input.query}))
        .await?;
    let locations = found["locations"].as_array().cloned().unwrap_or_default();
    let best = found["best"].as_u64().map(|index| index as usize);
    let mut table = Table::new(
        &["", "device", "interface", "vlan", "kind", "seen"],
        ctx.caps().rows,
    );
    for (index, location) in locations.iter().enumerate() {
        table.push([
            if Some(index) == best { "→" } else { "" }.to_string(),
            shape::text_of(location, "device"),
            shape::text_of(location, "interface"),
            shape::text_of(location, "vlan"),
            shape::text_of(location, "kind"),
            location["seen_at"]
                .as_i64()
                .map(shape::iso)
                .unwrap_or_default(),
        ]);
    }
    Ok(ToolOk::new(
        format!(
            "{}\n{}\n\n{}",
            shape::fields(&[
                ("ip", shape::text_of(&found, "ip")),
                ("mac", shape::text_of(&found, "mac")),
                ("vendor", shape::text_of(&found, "vendor")),
            ]),
            found["how"]
                .as_array()
                .map(|how| how
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("; "))
                .unwrap_or_default(),
            shape::data_block("where the address was seen", &table.render())
        ),
        format!("{} location(s)", locations.len()),
    ))
}

// ------------------------------------------------------------- ipam_search

pub const IPAM_SEARCH: Tool = Tool {
    name: "ipam_search",
    title: "Search addressing",
    description: "Search the address plan: an IP finds its address record and the prefix that \
                  contains it; a name or MAC fragment finds matching addresses; a VLAN number \
                  finds the VLAN. Use it before you suggest an address, so you suggest one that \
                  is actually free and in the right subnet.",
    brief: "Search the address plan for an IP, a name or a VLAN.",
    required: &[boundary::IPAM_SEARCH],
    optional: &[],
    core: false,
    touches_device: false,
    parallel_safe: true,
    schema: || schema(json!({"query": {"type": "string"}}), &["query"]),
    run: |ctx, input| Box::pin(ipam_search(ctx, input)),
};

async fn ipam_search(ctx: &ToolCtx, input: Value) -> AppResult<ToolOk> {
    let input: QueryParams = params(input)?;
    let found = ctx
        .rpc(boundary::IPAM_SEARCH, json!({"query": input.query}))
        .await?;
    let mut sections = Vec::new();
    for (label, key, columns) in [
        (
            "prefixes",
            "prefixes",
            vec!["prefix", "description", "vlan", "site", "id"],
        ),
        (
            "addresses",
            "addresses",
            vec!["address", "name", "device", "interface", "status"],
        ),
        ("vlans", "vlans", vec!["vlan_id", "name", "site", "id"]),
    ] {
        let rows = found[key].as_array().cloned().unwrap_or_default();
        if rows.is_empty() {
            continue;
        }
        let mut table = Table::new(&columns, ctx.caps().rows);
        for row in &rows {
            table.push(columns.iter().map(|column| shape::text_of(row, column)));
        }
        sections.push(format!(
            "{label}:\n{}",
            shape::data_block(&format!("address plan: {label}"), &table.render())
        ));
    }
    let body = if sections.is_empty() {
        format!("Nothing in the address plan matches `{}`.", input.query)
    } else {
        sections.join("\n\n")
    };
    Ok(ToolOk::new(body, "address plan"))
}

// ------------------------------------------------------- next_free_address

pub const NEXT_FREE_ADDRESS: Tool = Tool {
    name: "next_free_address",
    title: "Next free address",
    description: "Free addresses in a prefix from the address plan, skipping the network and \
                  broadcast addresses, the gateway, any DHCP range and anything recently seen \
                  in an ARP table. Needs the prefix's id, which ipam_search returns.",
    brief: "Free addresses in a prefix (by prefix id from ipam_search).",
    required: &[boundary::IPAM_NEXT_ADDRESS],
    optional: &[],
    core: false,
    touches_device: false,
    parallel_safe: true,
    schema: || {
        schema(
            json!({
                "prefix_id": {"type": "string", "description": "Prefix id, as ipam_search returned it."},
                "count": {"type": "integer", "minimum": 1, "maximum": 50}
            }),
            &["prefix_id"],
        )
    },
    run: |ctx, input| Box::pin(next_free_address(ctx, input)),
};

#[derive(Deserialize)]
struct NextAddressParams {
    prefix_id: String,
    #[serde(default)]
    count: Option<u32>,
}

async fn next_free_address(ctx: &ToolCtx, input: Value) -> AppResult<ToolOk> {
    let input: NextAddressParams = params(input)?;
    let found = ctx
        .rpc(
            boundary::IPAM_NEXT_ADDRESS,
            json!({"prefix_id": input.prefix_id, "count": input.count.unwrap_or(1)}),
        )
        .await?;
    Ok(ToolOk::new(
        shape::cap_text(&pretty(&found), ctx.caps().chars),
        "free addresses",
    ))
}

// ------------------------------------------------------------- subnet_calc

pub const SUBNET_CALC: Tool = Tool {
    name: "subnet_calc",
    title: "Subnet calculator",
    description: "Work out a subnet: network and broadcast address, mask, wildcard, usable \
                  range and host count. Takes `10.0.4.0/22`, `10.0.4.7 255.255.252.0` or an \
                  address with a wildcard. Use it instead of doing the arithmetic yourself — \
                  an off-by-one in a mask is a real outage.",
    brief: "Subnet arithmetic: network, broadcast, mask, wildcard, usable range, host count.",
    required: &[boundary::TOOLS_SUBNET],
    optional: &[],
    core: true,
    touches_device: false,
    parallel_safe: true,
    schema: || {
        schema(
            json!({"input": {"type": "string", "description": "`10.0.4.0/22`, `10.0.4.7 255.255.252.0`, or an address and a wildcard mask."}}),
            &["input"],
        )
    },
    run: |ctx, input| Box::pin(subnet_calc(ctx, input)),
};

#[derive(Deserialize)]
struct SubnetParams {
    input: String,
}

async fn subnet_calc(ctx: &ToolCtx, input: Value) -> AppResult<ToolOk> {
    let input: SubnetParams = params(input)?;
    let result = ctx
        .rpc(boundary::TOOLS_SUBNET, json!({"input": input.input}))
        .await?;
    Ok(ToolOk::new(pretty(&result), input.input))
}

// --------------------------------------------------------------- vlsm_plan

pub const VLSM_PLAN: Tool = Tool {
    name: "vlsm_plan",
    title: "Address plan",
    description: "Split a block into subnets of the sizes you need (variable-length subnet \
                  masking): give the base prefix and a name and host count per subnet, and get \
                  the allocation back with what is left over.",
    brief: "Split a prefix into subnets of given sizes.",
    required: &[boundary::TOOLS_VLSM],
    optional: &[],
    core: false,
    touches_device: false,
    parallel_safe: true,
    schema: || {
        schema(
            json!({
                "base": {"type": "string", "description": "The block to carve up, e.g. 10.20.0.0/16."},
                "sizes": {
                    "type": "array", "minItems": 1, "maxItems": 100,
                    "items": {
                        "type": "object",
                        "properties": {"name": {"type": "string"}, "hosts": {"type": "integer", "minimum": 1}},
                        "required": ["name", "hosts"],
                        "additionalProperties": false
                    }
                }
            }),
            &["base", "sizes"],
        )
    },
    run: |ctx, input| Box::pin(vlsm_plan(ctx, input)),
};

#[derive(Deserialize)]
struct VlsmParams {
    base: String,
    sizes: Vec<Value>,
}

async fn vlsm_plan(ctx: &ToolCtx, input: Value) -> AppResult<ToolOk> {
    let input: VlsmParams = params(input)?;
    let result = ctx
        .rpc(
            boundary::TOOLS_VLSM,
            json!({"base": input.base, "sizes": input.sizes}),
        )
        .await?;
    Ok(ToolOk::new(
        shape::cap_text(&pretty(&result), ctx.caps().chars),
        format!("{} subnet(s) of {}", input.sizes.len(), input.base),
    ))
}

// -------------------------------------------------------------------- ping

pub const PING: Tool = Tool {
    name: "ping",
    title: "Ping",
    description: "Ping a host from the machine Nocturne runs on. Answers 'can we reach it from \
                  here' — which is a different question from whether a switch can reach it \
                  (that is device_ping). Returns loss and round-trip times.",
    brief: "Ping a host from this machine: loss and round-trip times.",
    required: &[boundary::PROBE_PING, boundary::JOBS_GET],
    optional: &[],
    core: true,
    touches_device: false,
    parallel_safe: true,
    schema: || {
        schema(
            json!({
                "host": {"type": "string", "description": "Address or hostname."},
                "count": {"type": "integer", "minimum": 1, "maximum": 20}
            }),
            &["host"],
        )
    },
    run: |ctx, input| Box::pin(ping(ctx, input)),
};

#[derive(Deserialize)]
struct PingParams {
    host: String,
    #[serde(default)]
    count: Option<u32>,
}

async fn ping(ctx: &ToolCtx, input: Value) -> AppResult<ToolOk> {
    let input: PingParams = params(input)?;
    let started = ctx
        .rpc(
            boundary::PROBE_PING,
            json!({"host": input.host, "count": input.count.unwrap_or(4).clamp(1, 20)}),
        )
        .await?;
    let report = job_result(ctx, &started, "the ping").await?;
    let loss = report["loss_pct"].as_f64().unwrap_or(100.0);
    Ok(ToolOk::new(
        shape::fields(&[
            ("target", shape::text_of(&report, "target")),
            ("address", shape::text_of(&report, "address")),
            (
                "sent/received",
                format!(
                    "{}/{}",
                    report["sent"].as_u64().unwrap_or_default(),
                    report["received"].as_u64().unwrap_or_default()
                ),
            ),
            ("loss %", format!("{loss:.0}")),
            (
                "rtt min/avg/max ms",
                format!(
                    "{}/{}/{}",
                    millis(&report["min_ms"]),
                    millis(&report["avg_ms"]),
                    millis(&report["max_ms"])
                ),
            ),
        ]),
        format!("{}% loss", loss.round()),
    ))
}

fn millis(value: &Value) -> String {
    value
        .as_f64()
        .map(|value| format!("{value:.1}"))
        .unwrap_or_else(|| "-".into())
}

// -------------------------------------------------------------- traceroute

pub const TRACEROUTE: Tool = Tool {
    name: "traceroute",
    title: "Traceroute",
    description: "Trace the path from this machine to a host, hop by hop, with round-trip \
                  times. A hop with no reply is a router that does not answer, not necessarily \
                  a break.",
    brief: "Trace the path from this machine to a host.",
    required: &[boundary::PROBE_TRACEROUTE, boundary::JOBS_GET],
    optional: &[],
    core: false,
    touches_device: false,
    parallel_safe: true,
    schema: || {
        schema(
            json!({
                "host": {"type": "string"},
                "max_hops": {"type": "integer", "minimum": 1, "maximum": 64}
            }),
            &["host"],
        )
    },
    run: |ctx, input| Box::pin(traceroute(ctx, input)),
};

#[derive(Deserialize)]
struct TraceParams {
    host: String,
    #[serde(default)]
    max_hops: Option<u32>,
}

async fn traceroute(ctx: &ToolCtx, input: Value) -> AppResult<ToolOk> {
    let input: TraceParams = params(input)?;
    let started = ctx
        .rpc(
            boundary::PROBE_TRACEROUTE,
            json!({"host": input.host, "max_hops": input.max_hops.unwrap_or(30)}),
        )
        .await?;
    let report = job_result(ctx, &started, "the traceroute").await?;
    let hops = report["hops"].as_array().cloned().unwrap_or_default();
    let mut table = Table::new(&["ttl", "address", "hostname", "rtt ms", "note"], 64);
    for hop in &hops {
        table.push([
            hop["ttl"].as_u64().unwrap_or_default().to_string(),
            shape::text_of(hop, "address"),
            shape::text_of(hop, "hostname"),
            hop["rtts_ms"]
                .as_array()
                .map(|rtts| rtts.iter().map(millis).collect::<Vec<_>>().join(" "))
                .unwrap_or_default(),
            shape::text_of(hop, "note"),
        ]);
    }
    let reached = report["reached"].as_bool() == Some(true);
    Ok(ToolOk::new(
        format!(
            "{} to {} ({}): {}\n\n{}",
            if reached { "Reached" } else { "Did not reach" },
            shape::text_of(&report, "target"),
            shape::text_of(&report, "address"),
            format_args!("{} hop(s)", hops.len()),
            table.render()
        ),
        format!("{} hop(s)", hops.len()),
    ))
}

// -------------------------------------------------------------- port_check

pub const PORT_CHECK: Tool = Tool {
    name: "port_check",
    title: "Check ports",
    description: "Check whether TCP ports on a host are open, closed or filtered, from this \
                  machine. Use it for 'is SSH open on it', 'is the management plane reachable'.",
    brief: "Check whether TCP ports on a host are open.",
    required: &[boundary::PROBE_PORTS, boundary::JOBS_GET],
    optional: &[],
    core: false,
    touches_device: false,
    parallel_safe: true,
    schema: || {
        schema(
            json!({
                "host": {"type": "string"},
                "ports": {"type": "string", "description": "`22,443`, a range `1-1024`, or `common` / `top100`."}
            }),
            &["host", "ports"],
        )
    },
    run: |ctx, input| Box::pin(port_check(ctx, input)),
};

#[derive(Deserialize)]
struct PortParams {
    host: String,
    ports: String,
}

async fn port_check(ctx: &ToolCtx, input: Value) -> AppResult<ToolOk> {
    let input: PortParams = params(input)?;
    let started = ctx
        .rpc(
            boundary::PROBE_PORTS,
            json!({"host": input.host, "ports": input.ports}),
        )
        .await?;
    let report = job_result(ctx, &started, "the port check").await?;
    let results = report
        .as_array()
        .cloned()
        .or_else(|| report["ports"].as_array().cloned())
        .unwrap_or_default();
    let mut table = Table::new(
        &["port", "state", "service", "rtt ms", "banner"],
        ctx.caps().rows,
    );
    for result in &results {
        table.push([
            result["port"].as_u64().unwrap_or_default().to_string(),
            shape::text_of(result, "state"),
            shape::text_of(result, "service"),
            millis(&result["rtt_ms"]),
            shape::one_line(&shape::text_of(result, "banner"), 80),
        ]);
    }
    let open = results
        .iter()
        .filter(|result| shape::text_of(result, "state") == "open")
        .count();
    Ok(ToolOk::new(
        format!(
            "{} port(s) checked on {}, {open} open.\n\n{}",
            results.len(),
            input.host,
            table.render()
        ),
        format!("{open} open"),
    ))
}

// -------------------------------------------------------------- dns_lookup

pub const DNS_LOOKUP: Tool = Tool {
    name: "dns_lookup",
    title: "DNS lookup",
    description: "Look up a name (or an address, for a reverse lookup). Says which server \
                  answered and what the response code was: NXDOMAIN and 'no such record' are \
                  different answers and both matter.",
    brief: "Look up a DNS name or reverse-look-up an address.",
    required: &[boundary::PROBE_DNS],
    optional: &[],
    core: false,
    touches_device: false,
    parallel_safe: true,
    schema: || {
        schema(
            json!({
                "name": {"type": "string", "description": "A name, or an IP address for a PTR lookup."},
                "kind": {"type": "string", "description": "A, AAAA, PTR, CNAME, MX, NS, TXT, SRV, SOA, CAA."},
                "server": {"type": "string", "description": "Ask this resolver instead of the system one."}
            }),
            &["name"],
        )
    },
    run: |ctx, input| Box::pin(dns_lookup(ctx, input)),
};

#[derive(Deserialize)]
struct DnsParams {
    name: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    server: Option<String>,
}

async fn dns_lookup(ctx: &ToolCtx, input: Value) -> AppResult<ToolOk> {
    let input: DnsParams = params(input)?;
    let answer = ctx
        .rpc(
            boundary::PROBE_DNS,
            json!({"name": input.name, "kind": input.kind, "server": input.server}),
        )
        .await?;
    let records = answer["records"].as_array().cloned().unwrap_or_default();
    let mut table = Table::new(&["name", "kind", "ttl", "data"], ctx.caps().rows);
    for record in &records {
        table.push([
            shape::text_of(record, "name"),
            shape::text_of(record, "kind"),
            record["ttl"].as_u64().unwrap_or_default().to_string(),
            shape::text_of(record, "data"),
        ]);
    }
    Ok(ToolOk::new(
        format!(
            "{} for {}: {} ({} record(s)), answered by {} in {:.0} ms.\n\n{}",
            shape::text_of(&answer, "kind"),
            shape::text_of(&answer, "query"),
            shape::text_of(&answer, "rcode"),
            records.len(),
            shape::text_of(&answer, "server"),
            answer["elapsed_ms"].as_f64().unwrap_or_default(),
            table.render()
        ),
        shape::text_of(&answer, "rcode"),
    ))
}

// ------------------------------------------------------------- tls_inspect

pub const TLS_INSPECT: Tool = Tool {
    name: "tls_inspect",
    title: "Inspect TLS",
    description: "Look at the TLS certificate a host presents: who it is for, who issued it, \
                  when it expires, whether the chain is trusted and whether it matches the \
                  name. Works on an expired or self-signed certificate too — that is what it \
                  is for.",
    brief: "The TLS certificate a host presents, and when it expires.",
    required: &[boundary::PROBE_TLS],
    optional: &[],
    core: false,
    touches_device: false,
    parallel_safe: true,
    schema: || {
        schema(
            json!({
                "host": {"type": "string"},
                "port": {"type": "integer", "minimum": 1, "maximum": 65535},
                "sni": {"type": "string"}
            }),
            &["host"],
        )
    },
    run: |ctx, input| Box::pin(tls_inspect(ctx, input)),
};

#[derive(Deserialize)]
struct TlsParams {
    host: String,
    #[serde(default)]
    port: Option<u16>,
    #[serde(default)]
    sni: Option<String>,
}

async fn tls_inspect(ctx: &ToolCtx, input: Value) -> AppResult<ToolOk> {
    let input: TlsParams = params(input)?;
    let report = ctx
        .rpc(
            boundary::PROBE_TLS,
            json!({"host": input.host, "port": input.port.unwrap_or(443), "sni": input.sni}),
        )
        .await?;
    let leaf = report["chain"].get(0).cloned().unwrap_or(Value::Null);
    Ok(ToolOk::new(
        shape::fields(&[
            (
                "host",
                format!(
                    "{}:{}",
                    shape::text_of(&report, "host"),
                    report["port"].as_u64().unwrap_or_default()
                ),
            ),
            ("protocol", shape::text_of(&report, "protocol")),
            ("cipher", shape::text_of(&report, "cipher")),
            ("subject", shape::text_of(&leaf, "subject")),
            ("issuer", shape::text_of(&leaf, "issuer")),
            (
                "names",
                leaf["sans"]
                    .as_array()
                    .map(|sans| {
                        sans.iter()
                            .filter_map(Value::as_str)
                            .take(8)
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default(),
            ),
            ("valid until", shape::text_of(&leaf, "not_after")),
            (
                "days left",
                report["expires_in_days"]
                    .as_i64()
                    .map(|days| days.to_string())
                    .unwrap_or_default(),
            ),
            ("trusted", shape::tristate(report.get("trusted"))),
            ("trust problem", shape::text_of(&report, "trust_error")),
            (
                "name matches",
                shape::tristate(report.get("hostname_matches")),
            ),
        ]),
        format!(
            "{} days left",
            report["expires_in_days"].as_i64().unwrap_or_default()
        ),
    ))
}

// ------------------------------------------------------------------ shared

/// Probes answer with a job id; wait for it and hand back its result.
async fn job_result(ctx: &ToolCtx, started: &Value, what: &str) -> AppResult<Value> {
    let job_id = shape::text_of(started, "job_id");
    if job_id.is_empty() {
        // A build where the probe answers directly.
        return Ok(started.clone());
    }
    let job = ctx.wait_for_job(&job_id, what).await?;
    let result = job["result"].clone();
    if result.is_null() {
        return Err(AppError::unavailable(format!("{what} returned nothing.")));
    }
    Ok(result)
}

/// A JSON result with no shape of its own, as indented key/value text.
fn pretty(value: &Value) -> String {
    fn walk(value: &Value, indent: usize, out: &mut String) {
        let pad = " ".repeat(indent);
        match value {
            Value::Object(map) => {
                for (key, item) in map {
                    match item {
                        Value::Object(_) | Value::Array(_) => {
                            out.push_str(&format!("{pad}{key}:\n"));
                            walk(item, indent + 2, out);
                        }
                        _ => out.push_str(&format!("{pad}{key}: {}\n", scalar(item))),
                    }
                }
            }
            Value::Array(items) => {
                for item in items {
                    match item {
                        Value::Object(_) | Value::Array(_) => {
                            out.push_str(&format!("{pad}-\n"));
                            walk(item, indent + 2, out);
                        }
                        _ => out.push_str(&format!("{pad}- {}\n", scalar(item))),
                    }
                }
            }
            other => out.push_str(&format!("{pad}{}\n", scalar(other))),
        }
    }
    let mut out = String::new();
    walk(value, 0, &mut out);
    out.trim_end().to_string()
}

fn scalar(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => "-".into(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unshaped_results_read_as_indented_text() {
        let rendered = pretty(&json!({
            "network": "10.0.4.0", "mask": "255.255.252.0", "hosts": 1022,
            "usable": {"first": "10.0.4.1", "last": "10.0.7.254"},
            "notes": ["private", null]
        }));
        assert_eq!(
            rendered,
            "network: 10.0.4.0\nmask: 255.255.252.0\nhosts: 1022\nusable:\n  first: 10.0.4.1\n  last: 10.0.7.254\nnotes:\n  - private\n  - -"
        );
    }
}
