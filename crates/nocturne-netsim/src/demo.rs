//! Ready-made labs: the campus reference network and small teaching labs.

use crate::api::LabSpec;

/// The reference campus network (placeholder until tier 2).
pub fn demo_campus() -> LabSpec {
    LabSpec { name: "Campus".to_string(), ..LabSpec::default() }
}

/// Small teaching labs (placeholder until tier 2).
pub fn examples() -> Vec<LabSpec> {
    Vec::new()
}
