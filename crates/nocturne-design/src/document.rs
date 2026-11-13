//! The plan as a Markdown document somebody can hand to an installer.

use std::fmt::Write as _;

use crate::config::{ADMIN_SECRET, ADMIN_USER, ENABLE_SECRET, PLACEHOLDER_PREFIX, SNMP_RO};
use crate::design::{Design, DeviceRole, Expect};

/// Render the design as Markdown: addressing, VLANs, cabling, hardware,
/// the checks, and the reasoning behind all of it.
pub fn document(design: &Design) -> String {
    let mut out = String::new();
    let plan = &design.plan;
    let _ = writeln!(out, "# {}\n", title(design));
    let _ = writeln!(out, "{}\n", design.lab.description);

    // -- passwords, first, because they are the one thing that must change --
    let _ = writeln!(out, "## Before you deploy this\n");
    let _ = writeln!(
        out,
        "Every secret in these configurations is a placeholder, not a password. Replace all of \
         them before anything is cabled to a live network:\n"
    );
    let _ = writeln!(out, "| Where | Placeholder |");
    let _ = writeln!(out, "|---|---|");
    let _ = writeln!(out, "| `enable secret` | `{ENABLE_SECRET}` |");
    let _ = writeln!(out, "| `username {ADMIN_USER}` | `{ADMIN_SECRET}` |");
    let _ = writeln!(out, "| SNMP read-only community | `{SNMP_RO}` |");
    let _ = writeln!(
        out,
        "\nAnything beginning `{PLACEHOLDER_PREFIX}` is one of these. Generate real secrets with \
         a password manager; do not reuse one across devices.\n"
    );

    // -- sites --------------------------------------------------------------
    let _ = writeln!(out, "## Sites\n");
    let _ = writeln!(out, "| Site | Users | Block | VLANs | Devices |");
    let _ = writeln!(out, "|---|---:|---|---|---:|");
    for site in &plan.sites {
        let _ = writeln!(
            out,
            "| {}{} | {} | `{}` | {} | {} |",
            site.name,
            if site.is_hub && plan.sites.len() > 1 {
                " (hub)"
            } else {
                ""
            },
            site.users,
            site.block,
            site.vlans
                .iter()
                .map(u16::to_string)
                .collect::<Vec<_>>()
                .join(", "),
            site.devices.len()
        );
    }

    // -- addressing ---------------------------------------------------------
    let _ = writeln!(out, "\n## VLANs and addressing\n");
    let _ = writeln!(
        out,
        "| Site | VLAN | Name | Purpose | Subnet | Gateway | DHCP pool | Planned | Capacity |"
    );
    let _ = writeln!(out, "|---|---:|---|---|---|---|---|---:|---:|");
    for vlan in &plan.vlans {
        let pool = if vlan.dhcp_first.is_empty() {
            "static".to_string()
        } else {
            format!("{} – {}", vlan.dhcp_first, vlan.dhcp_last)
        };
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | `{}` | {} | {} | {} | {} |",
            vlan.site,
            vlan.id,
            vlan.name,
            vlan.purpose.describe(),
            vlan.subnet,
            vlan.gateway,
            pool,
            vlan.hosts_needed,
            vlan.hosts_available
        );
    }

    // -- devices ------------------------------------------------------------
    let _ = writeln!(out, "\n## Devices\n");
    let _ = writeln!(
        out,
        "| Device | Site | Role | Model | Management | Loopback |"
    );
    let _ = writeln!(out, "|---|---|---|---|---|---|");
    for dev in plan
        .devices
        .iter()
        .filter(|d| d.role != DeviceRole::Host && d.role != DeviceRole::Internet)
    {
        let _ = writeln!(
            out,
            "| {} | {} | {} | `{}` | {} | {} |",
            dev.name,
            dev.site,
            dev.role.label(),
            dev.model,
            dash(&dev.mgmt_ip),
            dash(&dev.loopback)
        );
    }

    // -- cabling ------------------------------------------------------------
    let _ = writeln!(out, "\n## Cabling\n");
    let _ = writeln!(out, "| From | Port | To | Port | Kind | Subnet / carries |");
    let _ = writeln!(out, "|---|---|---|---|---|---|");
    for link in &plan.links {
        let detail = if link.subnet.is_empty() {
            link.carries.clone()
        } else {
            format!("`{}`", link.subnet)
        };
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {:?} | {} |",
            link.a,
            link.a_if,
            link.b,
            link.b_if,
            link.kind,
            dash(&detail)
        );
    }

    // -- hardware -----------------------------------------------------------
    let _ = writeln!(out, "\n## Hardware\n");
    let _ = writeln!(out, "| Class | Quantity | Ports | Notes |");
    let _ = writeln!(out, "|---|---:|---|---|");
    for line in &plan.bill_of_materials {
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} |",
            line.class, line.quantity, line.ports, line.notes
        );
    }
    let _ = writeln!(
        out,
        "\nThe classes are generic on purpose: no part numbers and no prices, because both \
         depend on who you buy from and when.\n"
    );

    // -- decisions ----------------------------------------------------------
    let _ = writeln!(out, "## Why it is built this way\n");
    for decision in &design.decisions {
        let where_ = if decision.site.is_empty() {
            String::new()
        } else {
            format!(" ({})", decision.site)
        };
        let _ = writeln!(
            out,
            "- **{}{}** — {}",
            decision.topic, where_, decision.text
        );
    }

    if !design.warnings.is_empty() {
        let _ = writeln!(out, "\n## Read this part twice\n");
        for warning in &design.warnings {
            let _ = writeln!(out, "- {warning}");
        }
    }

    // -- checks -------------------------------------------------------------
    let _ = writeln!(out, "\n## What a correct build satisfies\n");
    let _ = writeln!(
        out,
        "Each row was derived from the design and holds in the simulator. Repeat them on the \
         real network after the installation.\n"
    );
    let _ = writeln!(out, "| From | To | Probe | Expected | Why |");
    let _ = writeln!(out, "|---|---|---|---|---|");
    for check in &plan.checks {
        let _ = writeln!(
            out,
            "| {} | {} ({}) | {} | {} | {} |",
            check.from,
            check.to,
            check.to_name,
            match check.port {
                Some(port) => format!("TCP {port}"),
                None => "ping".to_string(),
            },
            match check.expect {
                Expect::Reachable => "reachable",
                Expect::Blocked => "blocked",
            },
            check.why
        );
    }

    // -- configurations -----------------------------------------------------
    let _ = writeln!(out, "\n## Configurations\n");
    for node in &design.lab.nodes {
        let Some(config) = &node.startup_config else {
            continue;
        };
        let _ = writeln!(out, "### {}\n", node.name);
        let _ = writeln!(out, "```\n{}```\n", ensure_newline(config));
    }
    out
}

fn title(design: &Design) -> String {
    if design.lab.name.is_empty() {
        "Network design".to_string()
    } else {
        design.lab.name.clone()
    }
}

fn dash(text: &str) -> String {
    if text.is_empty() {
        "—".to_string()
    } else {
        text.to_string()
    }
}

fn ensure_newline(text: &str) -> String {
    if text.ends_with('\n') || text.is_empty() {
        text.to_string()
    } else {
        format!("{text}\n")
    }
}
