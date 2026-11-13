//! The policy packs that ship with Nocturne.
//!
//! They are data (`policies/*.yaml`), read once and held in memory. A
//! built-in is read-only: it can be disabled, and it can be duplicated into a
//! policy of one's own, but its rules cannot be edited in place — a rule that
//! quietly changed under a name people trust would be worse than no rule.
//!
//! Every rule states in its `rationale` exactly which configuration line it
//! looks for, because on both platforms a setting at its default may not be
//! printed at all. A rule that would need to know something the text does not
//! say is not shipped.

use std::sync::LazyLock;

use super::model::Policy;

const PACKS: [(&str, &str); 2] = [
    ("ios_hardening", include_str!("policies/ios_hardening.yaml")),
    (
        "routeros_hardening",
        include_str!("policies/routeros_hardening.yaml"),
    ),
];

static POLICIES: LazyLock<Vec<Policy>> = LazyLock::new(|| {
    PACKS
        .iter()
        .map(|(name, yaml)| {
            let mut policy: Policy = serde_yaml_ng::from_str(yaml)
                .unwrap_or_else(|err| panic!("built-in policy `{name}` must parse: {err}"));
            policy.builtin = true;
            policy.enabled = true;
            policy.updated_at = 0;
            policy
        })
        .collect()
});

/// Every built-in policy, as shipped.
pub fn all() -> &'static [Policy] {
    &POLICIES
}

pub fn get(id: &str) -> Option<&'static Policy> {
    POLICIES.iter().find(|policy| policy.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compliance::validate;

    #[test]
    fn every_built_in_policy_is_well_formed() {
        assert_eq!(all().len(), PACKS.len());
        let mut ids: Vec<&str> = Vec::new();
        for policy in all() {
            assert!(policy.builtin && policy.enabled, "{}", policy.id);
            assert!(
                policy.id.starts_with("builtin."),
                "a built-in id must be recognisable: {}",
                policy.id
            );
            assert!(!policy.name.is_empty() && !policy.description.is_empty());
            assert!(!policy.scope.is_empty(), "{} has no scope", policy.id);
            assert!(!ids.contains(&policy.id.as_str()));
            ids.push(&policy.id);

            let problems = validate::problems(policy);
            assert!(problems.is_empty(), "{}: {problems:?}", policy.id);
            let mut rule_ids: Vec<&str> = Vec::new();
            for rule in &policy.rules {
                assert!(!rule.id.is_empty() && !rule.title.is_empty(), "{policy:?}");
                assert!(
                    rule.rationale.len() > 80,
                    "{} says too little about why it is right",
                    rule.id
                );
                assert!(
                    !rule_ids.contains(&rule.id.as_str()),
                    "duplicate rule id {}",
                    rule.id
                );
                rule_ids.push(&rule.id);
            }
        }
    }
}
