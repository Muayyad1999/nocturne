//! Designing a network and trying it out in a lab before it touches
//! anything real.

use nocturne_app::error::AppResult;
use serde::Deserialize;
use serde_json::{Value, json};

use super::boundary;
use super::{Tool, ToolCtx, ToolOk, params, schema};
use crate::shape::{self, Table};
use crate::view::{Link, LinkKind};

// -------------------------------------------------------- design_network

pub const DESIGN_NETWORK: Tool = Tool {
    name: "design_network",
    title: "Design a network",
    description: "Turn requirements (how many users and sites, which services, what the \
                  constraints are) into a design: topology, addressing, VLANs, and the \
                  configuration each device would get. Nothing is created — it is a proposal \
                  to talk about.",
    brief: "Turn requirements into a network design. Nothing is created.",
    required: &[boundary::DESIGN_GENERATE],
    optional: &[],
    core: false,
    touches_device: false,
    parallel_safe: true,
    schema: || {
        schema(
            json!({
                "requirements": {
                    "type": "object",
                    "description": "The requirements as the design engine takes them: sites, users, services, addressing constraints, redundancy, vendor preference."
                }
            }),
            &["requirements"],
        )
    },
    run: |ctx, input| Box::pin(design_network(ctx, input)),
};

#[derive(Deserialize)]
struct DesignParams {
    requirements: Value,
}

async fn design_network(ctx: &ToolCtx, input: Value) -> AppResult<ToolOk> {
    let input: DesignParams = params(input)?;
    let design = ctx
        .rpc(boundary::DESIGN_GENERATE, input.requirements)
        .await?;
    Ok(ToolOk::new(
        shape::cap_text(&summarise_design(&design), ctx.caps().chars),
        "design",
    ))
}

fn summarise_design(design: &Value) -> String {
    let mut out = shape::fields(&[
        ("name", shape::text_of(design, "name")),
        ("summary", shape::text_of(design, "summary")),
    ]);
    for (label, key, columns) in [
        (
            "devices",
            "devices",
            vec!["name", "role", "model", "site", "management_address"],
        ),
        (
            "links",
            "links",
            vec!["from", "from_interface", "to", "to_interface"],
        ),
        ("vlans", "vlans", vec!["id", "name", "subnet", "gateway"]),
        ("subnets", "subnets", vec!["prefix", "purpose", "vlan"]),
    ] {
        let rows = design[key].as_array().cloned().unwrap_or_default();
        if rows.is_empty() {
            continue;
        }
        let mut table = Table::new(&columns, 50);
        for row in &rows {
            table.push(columns.iter().map(|column| shape::text_of(row, column)));
        }
        out.push_str(&format!("\n\n{label}:\n{}", table.render()));
    }
    if out.trim().is_empty() {
        out = shape::one_line(&design.to_string(), 2_000);
    }
    out
}

// ------------------------------------------------------------- create_lab

pub const CREATE_LAB: Tool = Tool {
    name: "create_lab",
    title: "Build a lab",
    description: "Build a design as a simulated lab, start it and check that everything can \
                  reach what it should. This is how a design is proved before anybody touches \
                  real hardware. Labs are simulated inside Nocturne; nothing is deployed to \
                  any machine.",
    brief: "Build a design as a simulated lab and check its connectivity.",
    required: &[boundary::DESIGN_CREATE_LAB],
    optional: &[],
    core: false,
    touches_device: false,
    parallel_safe: false,
    schema: || {
        schema(
            json!({
                "name": {"type": "string"},
                "requirements": {"type": "object", "description": "Requirements to design from, if you have no design yet."},
                "design": {"type": "object", "description": "A design from design_network."}
            }),
            &[],
        )
    },
    run: |ctx, input| Box::pin(create_lab(ctx, input)),
};

#[derive(Deserialize)]
struct CreateLabParams {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    requirements: Option<Value>,
    #[serde(default)]
    design: Option<Value>,
}

async fn create_lab(ctx: &ToolCtx, input: Value) -> AppResult<ToolOk> {
    let input: CreateLabParams = params(input)?;
    let mut request = json!({});
    if let Some(name) = &input.name {
        request["name"] = json!(name);
    }
    if let Some(design) = input.design {
        request["design"] = design;
    }
    if let Some(requirements) = input.requirements {
        request["requirements"] = requirements;
    }
    let lab = ctx.rpc(boundary::DESIGN_CREATE_LAB, request).await?;
    let lab_id = shape::text_of(&lab, "id");
    if !lab_id.is_empty() {
        ctx.link(Link::new(LinkKind::Lab, &lab_id));
    }
    ctx.audit(
        "assistant.lab.create",
        None,
        format!("Assistant built the lab `{}`", shape::text_of(&lab, "name")),
        json!({"lab_id": lab_id}),
    );
    Ok(ToolOk::new(
        shape::cap_text(
            &format!(
                "{}\n\n{}",
                shape::fields(&[
                    ("lab", shape::text_of(&lab, "name")),
                    ("id", lab_id.clone()),
                    (
                        "nodes",
                        lab["nodes"]
                            .as_array()
                            .map(Vec::len)
                            .unwrap_or_default()
                            .to_string()
                    ),
                    ("running", shape::tristate(lab.get("running"))),
                ]),
                connectivity_text(&lab["connectivity"], 50)
            ),
            ctx.caps().chars,
        ),
        format!("lab {}", shape::text_of(&lab, "name")),
    ))
}

// -------------------------------------------------------------- list_labs

pub const LIST_LABS: Tool = Tool {
    name: "list_labs",
    title: "Labs",
    description: "The simulated labs that exist, with how many nodes each has and whether it \
                  is running.",
    brief: "The simulated labs that exist.",
    required: &[boundary::LABS_LIST],
    optional: &[],
    core: false,
    touches_device: false,
    parallel_safe: true,
    schema: || schema(json!({}), &[]),
    run: |ctx, input| Box::pin(list_labs(ctx, input)),
};

async fn list_labs(ctx: &ToolCtx, _input: Value) -> AppResult<ToolOk> {
    let labs = ctx.rpc(boundary::LABS_LIST, json!({})).await?;
    let labs = labs
        .as_array()
        .cloned()
        .or_else(|| labs["labs"].as_array().cloned())
        .unwrap_or_default();
    let mut table = Table::new(
        &["name", "nodes", "links", "running", "updated", "id"],
        ctx.caps().rows,
    );
    for lab in &labs {
        table.push([
            shape::text_of(lab, "name"),
            shape::text_of(lab, "nodes"),
            shape::text_of(lab, "links"),
            shape::tristate(lab.get("running")),
            lab["updated_at"]
                .as_i64()
                .map(shape::iso)
                .unwrap_or_default(),
            shape::text_of(lab, "id"),
        ]);
    }
    Ok(ToolOk::new(
        format!("{} lab(s).\n\n{}", labs.len(), table.render()),
        format!("{} lab(s)", labs.len()),
    ))
}

// ------------------------------------------------------- lab_connectivity

pub const LAB_CONNECTIVITY: Tool = Tool {
    name: "lab_connectivity",
    title: "Lab connectivity",
    description: "Can everything in a lab reach everything it should? Returns the matrix and, \
                  for each pair that cannot, the reason. Run it before and after a change in a \
                  lab to show what the change did.",
    brief: "Which nodes of a lab can reach which, and why not.",
    required: &[boundary::LABS_CONNECTIVITY],
    optional: &[],
    core: false,
    touches_device: false,
    parallel_safe: true,
    schema: || {
        schema(
            json!({
                "lab_id": {"type": "string"},
                "nodes": {"type": "array", "items": {"type": "string"}, "description": "Only these nodes."}
            }),
            &["lab_id"],
        )
    },
    run: |ctx, input| Box::pin(lab_connectivity(ctx, input)),
};

#[derive(Deserialize)]
struct ConnectivityParams {
    lab_id: String,
    #[serde(default)]
    nodes: Option<Vec<String>>,
}

async fn lab_connectivity(ctx: &ToolCtx, input: Value) -> AppResult<ToolOk> {
    let input: ConnectivityParams = params(input)?;
    let result = ctx
        .rpc(
            boundary::LABS_CONNECTIVITY,
            json!({"lab_id": input.lab_id, "nodes": input.nodes}),
        )
        .await?;
    ctx.link(Link::new(LinkKind::Lab, &input.lab_id));
    Ok(ToolOk::new(
        connectivity_text(&result, ctx.caps().rows),
        format!(
            "{}% reachable",
            result["reachable_pct"].as_f64().unwrap_or_default().round()
        ),
    ))
}

/// The pairs that cannot reach each other, which is the part worth reading.
fn connectivity_text(result: &Value, limit: usize) -> String {
    if !result.is_object() {
        return String::new();
    }
    let nodes: Vec<String> = result["nodes"]
        .as_array()
        .map(|nodes| nodes.iter().map(shape::scalar_text).collect())
        .unwrap_or_default();
    let matrix = result["matrix"].as_array().cloned().unwrap_or_default();
    let mut failures = Vec::new();
    let mut pairs = 0usize;
    for (row, from) in matrix.iter().zip(nodes.iter()) {
        for (cell, to) in row
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .zip(nodes.iter())
        {
            if from == to {
                continue;
            }
            pairs += 1;
            if cell["ok"].as_bool() != Some(true) {
                failures.push(format!("{from} → {to}: {}", shape::text_of(cell, "reason")));
            }
        }
    }
    let mut out = format!(
        "{} node(s), {pairs} pair(s), {}% reachable.",
        nodes.len(),
        result["reachable_pct"].as_f64().unwrap_or_default().round()
    );
    if failures.is_empty() {
        out.push_str(" Everything reaches everything.");
    } else {
        out.push_str("\nCannot reach:");
        for failure in failures.iter().take(limit) {
            out.push_str(&format!("\n  {failure}"));
        }
        if failures.len() > limit {
            out.push_str(&format!("\n  [{} more]", failures.len() - limit));
        }
    }
    out
}

// -------------------------------------------------------------- lab_probe

pub const LAB_PROBE: Tool = Tool {
    name: "lab_probe",
    title: "Probe in a lab",
    description: "Ping (or open a TCP port) between two nodes of a lab, and get the answer at \
                  once. Use it to prove one specific path a design is supposed to allow — or \
                  to prove one it is supposed to block.",
    brief: "Ping or port-check between two lab nodes.",
    required: &[boundary::LABS_PROBE],
    optional: &[],
    core: false,
    touches_device: false,
    parallel_safe: true,
    schema: || {
        schema(
            json!({
                "lab_id": {"type": "string"},
                "from": {"type": "string", "description": "Node name."},
                "to": {"type": "string", "description": "Node name or IP address."},
                "kind": {"type": "string", "enum": ["ping", "tcp"]},
                "port": {"type": "integer", "minimum": 1, "maximum": 65535}
            }),
            &["lab_id", "from", "to"],
        )
    },
    run: |ctx, input| Box::pin(lab_probe(ctx, input)),
};

#[derive(Deserialize)]
struct LabProbeParams {
    lab_id: String,
    from: String,
    to: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    port: Option<u16>,
}

async fn lab_probe(ctx: &ToolCtx, input: Value) -> AppResult<ToolOk> {
    let input: LabProbeParams = params(input)?;
    let kind = input.kind.unwrap_or_else(|| "ping".into());
    let result = ctx
        .rpc(
            boundary::LABS_PROBE,
            json!({"lab_id": input.lab_id, "from": input.from, "to": input.to,
                   "kind": kind, "port": input.port}),
        )
        .await?;
    ctx.link(Link::new(LinkKind::Lab, &input.lab_id));
    let ok = result["ok"].as_bool() == Some(true);
    Ok(ToolOk::new(
        format!(
            "{} from {} to {}: {}\n{}",
            kind,
            input.from,
            input.to,
            if ok { "reachable" } else { "not reachable" },
            shape::fields(&[
                ("reason", shape::text_of(&result, "reason")),
                ("detail", shape::text_of(&result, "detail")),
                (
                    "rtt ms",
                    result["rtt_ms"]
                        .as_f64()
                        .map(|rtt| format!("{rtt:.2}"))
                        .unwrap_or_default()
                ),
            ])
        ),
        if ok { "reachable" } else { "not reachable" },
    ))
}
