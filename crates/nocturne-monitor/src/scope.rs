//! Which devices a rule or a maintenance window is about.

use nocturne_app::model::Device;
use nocturne_app::{App, AppResult, inventory};

use crate::model::Scope;

fn any_equal(wanted: &[String], value: &str) -> bool {
    wanted
        .iter()
        .any(|candidate| candidate.trim().eq_ignore_ascii_case(value.trim()))
}

/// Whether a device is in scope.
///
/// `lab_gate` is on for alert rules — a simulated device is only alerted on
/// when the rule asks for it — and off for maintenance windows, which mute
/// whatever they cover.
pub fn matches(scope: &Scope, device: &Device, lab_gate: bool) -> bool {
    if lab_gate && device.lab.is_some() && !scope.include_lab {
        return false;
    }
    if !scope.device_ids.is_empty() && !scope.device_ids.iter().any(|id| id == &device.id) {
        return false;
    }
    if !scope.sites.is_empty() && !any_equal(&scope.sites, &device.site) {
        return false;
    }
    if !scope.roles.is_empty() && !any_equal(&scope.roles, &device.role) {
        return false;
    }
    if !scope.tags.is_empty() && !device.tags.iter().any(|tag| any_equal(&scope.tags, tag)) {
        return false;
    }
    true
}

/// Whether a scope names nothing in particular, and so covers everything.
pub fn is_everything(scope: &Scope) -> bool {
    scope.sites.is_empty()
        && scope.roles.is_empty()
        && scope.tags.is_empty()
        && scope.device_ids.is_empty()
}

/// Every device a scope covers.
pub fn devices(app: &App, scope: &Scope, lab_gate: bool) -> AppResult<Vec<Device>> {
    Ok(inventory::list(app, &inventory::DeviceFilter::default())?
        .into_iter()
        .map(|row| row.device)
        .filter(|device| matches(scope, device, lab_gate))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nocturne_app::model::LabRef;

    fn device() -> Device {
        Device {
            id: "dev1".into(),
            name: "SW1".into(),
            site: "HQ".into(),
            role: "access".into(),
            tags: vec!["floor-2".into()],
            ..Device::default()
        }
    }

    #[test]
    fn an_empty_scope_covers_every_real_device() {
        let scope = Scope::default();
        assert!(is_everything(&scope));
        assert!(matches(&scope, &device(), true));
    }

    #[test]
    fn each_facet_narrows_and_they_all_have_to_agree() {
        let mut scope = Scope {
            sites: vec!["hq".into()],
            ..Scope::default()
        };
        assert!(matches(&scope, &device(), true), "sites ignore case");
        scope.roles = vec!["core".into()];
        assert!(!matches(&scope, &device(), true));
        scope.roles = vec!["access".into()];
        scope.tags = vec!["FLOOR-2".into()];
        assert!(matches(&scope, &device(), true));
        scope.device_ids = vec!["other".into()];
        assert!(!matches(&scope, &device(), true));
    }

    #[test]
    fn lab_devices_are_left_alone_unless_a_rule_opts_in() {
        let mut lab = device();
        lab.lab = Some(LabRef {
            lab_id: "lab1".into(),
            node: "SW1".into(),
        });
        let mut scope = Scope::default();
        assert!(!matches(&scope, &lab, true));
        // A maintenance window covers it anyway.
        assert!(matches(&scope, &lab, false));
        scope.include_lab = true;
        assert!(matches(&scope, &lab, true));
    }
}
