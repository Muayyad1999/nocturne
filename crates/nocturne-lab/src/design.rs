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
}
