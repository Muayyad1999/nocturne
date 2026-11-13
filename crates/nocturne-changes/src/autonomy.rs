//! What the AI assistant may apply by itself.
//!
//! `changes.assistant_apply` is the only apply the assistant's tool set can
//! call, and this is the gate in front of it. The rule is not "the model is
//! confident": it is the autonomy setting, and on top of it a list of things
//! that always need a human whatever the setting says — a protected device, a
//! protected resource, a high or critical risk, or a device type nobody has
//! checked against real hardware.

use nocturne_app::error::AppError;
use nocturne_app::{App, inventory};

use crate::model::{Change, RiskLevel};
use crate::settings::{Autonomy, autonomy};

fn refuse(change: &Change, reason: String) -> AppError {
    AppError::forbidden(format!(
        "{reason} Ask the person you are working with to look at the change `{}` in the Changes screen and approve it there; they can see the exact commands, the predicted difference and the risk.",
        change.title
    ))
    .with_details(serde_json::json!({"change_id": change.id, "needs": "human_approval"}))
}

/// May the assistant apply this change without a human? Returns the value to
/// record as `approval.by`.
pub fn decide(app: &App, change: &Change) -> Result<String, AppError> {
    let mode = autonomy(app);
    let mut lab_only = true;
    for target in &change.targets {
        let device = inventory::get(app, &target.device_id)?;
        let Some(plan) = &target.plan else {
            return Err(AppError::conflict(format!(
                "`{}` has no plan yet; plan the change before applying it",
                device.name
            )));
        };
        let is_lab = device.lab.is_some();
        lab_only &= is_lab;

        if device.protected {
            return Err(refuse(
                change,
                format!(
                    "`{}` is a protected device, so no change to it is ever applied without a human.",
                    device.name
                ),
            ));
        }
        if !plan.protected_hits.is_empty() {
            return Err(refuse(
                change,
                format!(
                    "the change touches protected resources on `{}` ({}), which always needs a human to confirm it by name.",
                    device.name,
                    plan.protected_hits
                        .iter()
                        .map(|hit| hit.subject.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        }
        if plan.risk.level >= RiskLevel::High {
            return Err(refuse(
                change,
                format!(
                    "the change is {} risk on `{}` ({}).",
                    plan.risk.level.as_str(),
                    device.name,
                    plan.risk.reasons.first().map(String::as_str).unwrap_or("")
                ),
            ));
        }
        if plan.maturity != "verified" {
            return Err(refuse(
                change,
                format!(
                    "the device type of `{}` (`{}`) has not been verified against real hardware, so Nocturne will not apply to it unattended.",
                    device.name, plan.profile
                ),
            ));
        }
        if mode == Autonomy::LowRisk && !is_lab && plan.risk.level != RiskLevel::Low {
            return Err(refuse(
                change,
                format!(
                    "autonomy is `low_risk` and the change is {} risk on `{}`, which is a real device.",
                    plan.risk.level.as_str(),
                    device.name
                ),
            ));
        }
    }

    match mode {
        Autonomy::Draft => Err(refuse(
            change,
            "autonomy is set to `draft`, so the assistant never applies changes by itself.".into(),
        )),
        Autonomy::Lab if !lab_only => Err(refuse(
            change,
            "autonomy is set to `lab`, so the assistant only applies changes to lab devices, and this change has real ones.".into(),
        )),
        _ => Ok(format!("policy:{}", mode.as_str())),
    }
}
