//! The `design.*` RPC surface: requirements in, a whole network out.
//!
//! `nocturne-design` is a pure engine that knows nothing about the
//! application, and what it produces is a [`LabSpec`](nocturne_netsim::LabSpec)
//! — so this is where it belongs. The feature that already owns the simulator
//! can boot a design the moment it is generated, which is the only honest way
//! to hand somebody a network they did not design themselves: not a drawing
//! and a promise, but a network they can log into and test before anything is
//! ordered or cabled.

use nocturne_app::{App, AppBuilder, AppError, AppResult, NoParams};
use nocturne_design::{DesignError, Requirements};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::api::create_record;
use crate::model::LabView;

#[derive(Deserialize)]
struct RequirementsParams {
    #[serde(default)]
    requirements: Requirements,
}

#[derive(Deserialize)]
struct ExportParams {
    /// The requirements to design from. A design is a pure function of these
    /// (`nocturne_design::generate` reads no clock and no randomness), so the
    /// document is reproduced rather than carried back and forth: the lab
    /// inside a `Design` holds every configuration line of every device, and
    /// `design.generate` deliberately does not return it.
    #[serde(default)]
    requirements: Requirements,
    #[serde(default)]
    format: ExportFormat,
}

#[derive(Deserialize, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
enum ExportFormat {
    /// The whole design as one document: what to change before deploying,
    /// the sites, the addressing plan, the links, the bill of materials, the
    /// tests to run, every device's configuration, and why it is built this
    /// way.
    #[default]
    Markdown,
    /// Just the configurations, one per device, for handing to a deployment
    /// tool.
    Configs,
}

#[derive(Deserialize)]
struct CreateLabParams {
    #[serde(default)]
    requirements: Requirements,
    /// A name for the lab; the design's project name is used when absent.
    #[serde(default)]
    name: Option<String>,
}

/// Every problem at once, in the words the engine chose, because a person
/// fixing requirements wants the whole list and not the first complaint.
fn refuse(error: DesignError) -> AppError {
    let problems = error.problems();
    match problems.len() {
        0 => AppError::invalid(error.to_string()),
        1 => AppError::invalid(problems[0].clone()),
        _ => AppError::invalid(problems.join("; ")),
    }
}

fn generate(params: RequirementsParams) -> AppResult<Value> {
    let design = nocturne_design::generate(&params.requirements).map_err(refuse)?;
    let document = nocturne_design::document(&design);
    Ok(json!({
        "plan": design.plan,
        "decisions": design.decisions,
        "warnings": design.warnings,
        "document": document,
        // The lab is not returned: it holds every configuration line of every
        // device and is large. `design.create_lab` builds it in one call.
        "lab": {
            "name": design.lab.name,
            "nodes": design.lab.nodes.len(),
            "links": design.lab.links.len(),
        },
    }))
}

fn create_lab(app: &App, params: CreateLabParams) -> AppResult<LabView> {
    let design = nocturne_design::generate(&params.requirements).map_err(refuse)?;
    let mut spec = design.lab;
    if let Some(name) = params.name.filter(|name| !name.trim().is_empty()) {
        spec.name = name.trim().to_string();
    }
    spec.name = crate::model::unique_name(app, &spec.name)?;
    create_record(app, spec, "design.create_lab")
}

fn export(params: ExportParams) -> AppResult<Value> {
    let design = nocturne_design::generate(&params.requirements).map_err(refuse)?;
    if params.format == ExportFormat::Markdown {
        return Ok(json!({
            "format": "markdown",
            "name": design.lab.name,
            "document": nocturne_design::document(&design),
        }));
    }
    // A host has addressing and services, not a command line, so it has no
    // configuration to export. Saying which devices those are is the
    // difference between a complete export and one that quietly left
    // something out.
    let mut configs = serde_json::Map::new();
    let mut without = Vec::new();
    for node in &design.lab.nodes {
        match node.startup_config.as_deref() {
            Some(config) => {
                configs.insert(node.name.clone(), Value::String(config.to_string()));
            }
            None => without.push(json!({"node": node.name, "model": node.model})),
        }
    }
    Ok(json!({
        "format": "configs",
        "name": design.lab.name,
        "configs": configs,
        // Named, not dropped: these are the nodes with no command line.
        "without_a_configuration": without,
    }))
}

pub fn init(builder: &mut AppBuilder) {
    let router = &mut builder.router;
    router.add("design.examples", |_app: App, _: NoParams| async move {
        let examples: Vec<Value> = nocturne_design::examples()
            .into_iter()
            .map(|(name, requirements)| json!({"name": name, "requirements": requirements}))
            .collect();
        Ok(examples)
    });
    router.add(
        "design.validate",
        |_app: App, params: RequirementsParams| async move {
            let problems = nocturne_design::validate(&params.requirements);
            Ok(json!({"ok": problems.is_empty(), "problems": problems}))
        },
    );
    router.add(
        "design.generate",
        |_app: App, params: RequirementsParams| async move { generate(params) },
    );
    router.add(
        "design.create_lab",
        |app: App, params: CreateLabParams| async move { create_lab(&app, params) },
    );
    router.add(
        "design.export",
        |_app: App, params: ExportParams| async move { export(params) },
    );
}
