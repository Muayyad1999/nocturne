//! What one check produced in one scan: its findings, and — just as
//! important — which devices it could actually decide for.
//!
//! "Nothing found" and "not checked" are different answers, and the whole
//! feature is only trustworthy if it can tell them apart. A check that cannot
//! decide for a device calls [`CheckReport::skip`] with the reason; a check
//! that decided calls [`CheckReport::ok`]. Only a device marked `ok` counts
//! as a clean scan for the resolve logic, so a device that drops out of
//! collection never silently resolves its own open findings.
//!
//! The same holds one level down. A check that decided for a device may still
//! have been unable to judge one thing on it — one class of a QoS policy
//! measured too briefly since a failed read — and says so with
//! [`CheckReport::undecided`]: a finding about that thing is then neither
//! seen again nor counted clean.

use std::collections::{BTreeMap, BTreeSet};

use crate::model::{Finding, insight_id};

/// One check's outcome over the devices of a scan.
#[derive(Clone, Debug, Default)]
pub struct CheckReport {
    pub check: &'static str,
    pub findings: Vec<Finding>,
    evaluated: BTreeSet<String>,
    skipped: BTreeMap<String, &'static str>,
    /// Insight ids of the subjects it could not judge.
    undecided: BTreeSet<String>,
}

impl CheckReport {
    pub fn new(check: &'static str) -> CheckReport {
        CheckReport {
            check,
            ..CheckReport::default()
        }
    }

    /// The check reached a verdict for this device.
    pub fn ok(&mut self, device_id: &str) {
        if !self.evaluated.contains(device_id) {
            self.evaluated.insert(device_id.to_string());
        }
    }

    /// The check could not decide for this device, and why. The first reason
    /// wins: it is the one closest to the missing data.
    pub fn skip(&mut self, device_id: &str, reason: &'static str) {
        if !self.skipped.contains_key(device_id) {
            self.skipped.insert(device_id.to_string(), reason);
        }
    }

    /// The check could not judge `subject` on this device in this scan,
    /// whatever it decided about the rest of the device. An open finding
    /// about it stays as it is: a scan that could not look is not a scan in
    /// which the problem was gone.
    pub fn undecided(&mut self, device_id: &str, subject: &str) {
        self.undecided
            .insert(insight_id(self.check, device_id, subject));
    }

    /// The insight ids of the subjects this check could not judge.
    pub fn undecided_ids(&self) -> &BTreeSet<String> {
        &self.undecided
    }

    /// Record a finding. Its device, and every device it names as related,
    /// have evidently been evaluated.
    pub fn found(&mut self, finding: Finding) {
        self.ok(&finding.device_id);
        for related in &finding.related {
            let device_id = related.device_id.clone();
            self.evaluated.insert(device_id);
        }
        self.findings.push(finding);
    }

    /// Devices this check decided for.
    pub fn evaluated(&self) -> &BTreeSet<String> {
        &self.evaluated
    }

    /// Devices it could not decide for, with the reason — a device that was
    /// evaluated at all (one interface decided, another not) counts as
    /// evaluated and is not listed here.
    pub fn skips(&self) -> impl Iterator<Item = (&str, &'static str)> {
        self.skipped
            .iter()
            .filter(|(device_id, _)| !self.evaluated.contains(*device_id))
            .map(|(device_id, reason)| (device_id.as_str(), *reason))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_that_was_decided_for_is_not_also_reported_as_skipped() {
        let mut report = CheckReport::new("duplex_mismatch");
        report.skip("a", "no interface data");
        report.ok("a");
        report.skip("b", "no interface data");
        report.skip("b", "no link");
        assert_eq!(
            report.skips().collect::<Vec<_>>(),
            [("b", "no interface data")]
        );
        assert_eq!(report.evaluated().len(), 1);
    }

    #[test]
    fn a_subject_that_could_not_be_judged_is_held_by_its_insight_id_and_decides_nothing() {
        let mut report = CheckReport::new("qos_class_dropping");
        report.undecided("a", "Gi0/0 out WAN/BULK");
        let finding = Finding::new("qos_class_dropping", "a", "Gi0/0 out WAN/BULK", "x");
        assert!(report.undecided_ids().contains(&finding.id()));
        assert!(
            report.evaluated().is_empty() && report.skips().count() == 0,
            "the device itself is neither decided nor skipped by it"
        );
    }

    #[test]
    fn a_finding_marks_both_ends_evaluated() {
        let mut report = CheckReport::new("duplex_mismatch");
        report.found(
            Finding::new("duplex_mismatch", "a", "Gi0/1", "half/full").related("b", "SW2", "Gi0/2"),
        );
        assert!(report.evaluated().contains("a") && report.evaluated().contains("b"));
    }
}
