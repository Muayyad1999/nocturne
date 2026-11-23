//! What the rules remember between lines.
//!
//! One small record per (rule, device, instance): when the rule last spoke
//! for it, how many matches it has swallowed since, the lines it is counting
//! towards a threshold, and the alert it has open. The instance is the
//! interface, or the values of a threshold's `group_by` captures — which is
//! to say it comes out of text a device wrote, and on a UDP port anybody can
//! write that text. So a rule holds at most [`MAX_KEYS_PER_RULE`] records;
//! beyond that the ones touched longest ago are forgotten, and counted.
//!
//! Two clocks are kept apart on purpose. A line carries its own time, and
//! everything decided *when a line arrives* (is this inside the throttle
//! window, are these eight lines inside one minute) is arithmetic on line
//! times. What is decided *when no line arrives* (this alert has been quiet
//! for ten minutes) can only be measured on the wall clock. In the running
//! application the two are the same clock; keeping them apart is what lets
//! a line be replayed with its own time without a timer firing early.
//!
//! Nothing here is persisted: a restart starts every count again, and an
//! alert raised before it is no longer known to be open.
//!
//! An alert is only *waited on* when its rule can say that it is over — it
//! has an opposite event or a quiet period. One that has neither is the
//! monitor's from the moment it is raised: it is remembered like any other
//! record (another rule may follow it) and goes when the record goes idle,
//! instead of being counted as open for ever by something that will never
//! hear how it ended.

use std::collections::{HashMap, VecDeque};

/// How many (device, instance) records one rule may hold.
pub const MAX_KEYS_PER_RULE: usize = 10_000;
/// How much of a full rule is forgotten at once. Evicting one record per
/// new key would mean finding the oldest of ten thousand for every line of
/// a flood; a tenth at a time makes that one pass per thousand new keys.
const EVICT_AT_ONCE: usize = MAX_KEYS_PER_RULE / 10;
/// A record with no open alert is dropped after this long untouched, unless
/// its rule's own windows are longer.
pub const IDLE_MS: i64 = 3_600_000;
/// How many instances of one device one rule may speak for inside one
/// throttle window.
///
/// Throttling per instance means a second port is a second signal — and
/// that a sender who makes up a new port name in every line gets a signal,
/// a stored event and an alert for every line. A device does not have this
/// many things going wrong under one rule in one minute; past it the
/// matches are held back and counted, and each instance speaks, with its
/// count, on its next matching line after the window.
pub const MAX_SIGNALS_PER_WINDOW: u32 = 256;

/// (device id, or empty for a line no device claims; instance).
pub type Key = (String, String);

/// What a rule asks of its memory for one matching line.
#[derive(Clone, Copy, Debug)]
pub struct Ask<'a> {
    pub rule_id: &'a str,
    /// (count, window in milliseconds), when the rule has a threshold.
    pub threshold: Option<(u32, i64)>,
    pub throttle_ms: i64,
    /// Only count while this other rule's alert is open for the same key.
    pub after: Option<&'a str>,
}

/// What became of a matching line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The rule it depends on has no alert open for this key.
    NotAfter,
    /// Counted towards the threshold, which is not reached.
    Counting { have: usize },
    /// The rule already spoke for this key inside the throttle window, or
    /// for [`MAX_SIGNALS_PER_WINDOW`] other instances of the device.
    Suppressed,
    /// Raise the signal; it stands for `count` matching lines.
    Fire { count: u64 },
}

/// An alert a rule raised and has not cleared.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenAlert {
    pub rule_id: String,
    pub rule_name: String,
    pub dedup_key: String,
    pub title: String,
    pub device_id: Option<String>,
    pub instance: String,
    /// Wall-clock time of the last matching line.
    pub last_match_wall: i64,
    pub clear_after_ms: Option<i64>,
    /// Wall-clock end of the throttle window the alert was raised in.
    /// Nothing clears it before then: a port that goes down and up three
    /// times in ten seconds is one alert that clears once, not three alerts.
    pub hold_until_wall: i64,
    /// The opposite event arrived while the alert was held: the entry that
    /// said so and its time.
    pub recovered: Option<(i64, i64)>,
    /// The rule has an opposite event that can clear this alert.
    pub has_opposite: bool,
}

impl OpenAlert {
    /// Whether the rule that raised it can ever say it is over. An alert
    /// that is not clearable is closed by a person, in the monitor, and
    /// nothing here hears of it.
    pub fn clearable(&self) -> bool {
        self.has_opposite || self.clear_after_ms.is_some()
    }
}

/// What became of a rule's opposite event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Opposite {
    /// The alert was open and is to be cleared now.
    Cleared(OpenAlert),
    /// The alert is open but inside its throttle window: remembered, and
    /// cleared by the sweep when the window ends unless it matches again.
    Held,
    /// This process knows of no open alert for the key (it may have been
    /// raised before a restart): ask the monitor to clear it anyway.
    Unknown,
    /// Asked already inside this window, or asked for
    /// [`MAX_SIGNALS_PER_WINDOW`] other instances of the device in it;
    /// nothing to do.
    Nothing,
}

#[derive(Debug, Default)]
struct Instance {
    /// Order of last use, for forgetting the oldest first.
    touched_seq: u64,
    touched_wall: i64,
    idle_ms: i64,
    /// Line time at which the rule last spoke for this key.
    window_started_at: Option<i64>,
    /// Matching lines not yet reported in a signal.
    unreported: u64,
    /// Line times counted towards the threshold.
    hits: VecDeque<i64>,
    open: Option<OpenAlert>,
    /// Line time of the last clear sent without knowing of an open alert.
    blind_clear_at: Option<i64>,
}

impl Instance {
    /// An alert is open that its rule will clear: the record has to stay
    /// until it does.
    fn waited_on(&self) -> bool {
        self.open.as_ref().is_some_and(OpenAlert::clearable)
    }
}

/// Why the sweep cleared an alert.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// Its opposite arrived (now, or while it was held).
    Opposite,
    /// No matching line for `clear_after_s`.
    Quiet,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::Opposite => "opposite",
            Reason::Quiet => "quiet",
        }
    }
}

/// How often one rule has spoken for one device in the current window.
#[derive(Debug, Default)]
struct Budget {
    started_at: i64,
    fired: u32,
}

impl Budget {
    /// Take one of the window's [`MAX_SIGNALS_PER_WINDOW`], starting a new
    /// window when the last one is over. `false` when none is left.
    fn spend(&mut self, ts: i64, window_ms: i64) -> bool {
        // A line dated before the window began (the clock was set back) can
        // only be measured against a window of its own.
        if self.fired == 0 || ts - self.started_at >= window_ms || ts < self.started_at {
            *self = Budget {
                started_at: ts,
                fired: 0,
            };
        }
        if self.fired >= MAX_SIGNALS_PER_WINDOW {
            return false;
        }
        self.fired += 1;
        true
    }
}

/// What the memory holds and has let go, for `logs.status`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// Records held.
    pub tracked: u64,
    /// Records forgotten at the per-rule cap.
    pub forgotten: u64,
    /// Alerts a rule raised and is waiting to clear.
    pub open: u64,
    /// Matches, and opposites nobody here knew an alert for, held back at
    /// the per-device cap.
    pub held_back: u64,
}

#[derive(Debug, Default)]
pub struct Memory {
    rules: HashMap<String, HashMap<Key, Instance>>,
    /// (rule, device) → how often the rule spoke for the device.
    budgets: HashMap<(String, String), Budget>,
    /// (rule, device) → how often the monitor was asked to clear an alert
    /// this process does not know of.
    blind: HashMap<(String, String), Budget>,
    seq: u64,
    forgotten: u64,
    held_back: u64,
}

impl Memory {
    /// The record of a key, made if need be — forgetting the oldest tenth
    /// of the rule's records first when the rule is full.
    fn slot(&mut self, rule_id: &str, key: &Key, wall: i64) -> &mut Instance {
        self.seq += 1;
        let seq = self.seq;
        let records = self.rules.entry(rule_id.to_string()).or_default();
        if records.len() >= MAX_KEYS_PER_RULE && !records.contains_key(key) {
            // Records with an alert that is waited on go last: forgetting
            // one of those leaves an alert nothing will clear.
            let mut order: Vec<(bool, u64, Key)> = records
                .iter()
                .map(|(key, record)| (record.waited_on(), record.touched_seq, key.clone()))
                .collect();
            order.select_nth_unstable_by(EVICT_AT_ONCE - 1, |left, right| {
                (left.0, left.1).cmp(&(right.0, right.1))
            });
            for (_, _, old) in order.into_iter().take(EVICT_AT_ONCE) {
                records.remove(&old);
            }
            self.forgotten += EVICT_AT_ONCE as u64;
        }
        let record = records.entry(key.clone()).or_default();
        record.touched_seq = seq;
        record.touched_wall = wall;
        record
    }

    /// Whether `rule_id` has an alert open for `key`.
    pub fn is_open(&self, rule_id: &str, key: &Key) -> bool {
        self.rules
            .get(rule_id)
            .and_then(|records| records.get(key))
            .is_some_and(|record| record.open.is_some())
    }

    /// A line matched: count it, and say whether the rule speaks.
    pub fn matched(&mut self, ask: &Ask<'_>, key: &Key, ts: i64, wall: i64) -> Verdict {
        if let Some(other) = ask.after
            && !self.is_open(other, key)
        {
            return Verdict::NotAfter;
        }
        {
            let record = self.slot(ask.rule_id, key, wall);
            let window = ask.threshold.map(|(_, window)| window).unwrap_or_default();
            record.idle_ms = IDLE_MS.max(ask.throttle_ms).max(window);
            if let Some(open) = &mut record.open {
                // Still happening: the quiet period starts again, and an
                // opposite seen a moment ago no longer stands.
                open.last_match_wall = wall;
                open.recovered = None;
            }
            let lines = match ask.threshold {
                Some((count, window)) if count > 1 => {
                    while record
                        .hits
                        .front()
                        .is_some_and(|first| ts - first >= window)
                    {
                        record.hits.pop_front();
                    }
                    record.hits.push_back(ts);
                    if record.hits.len() < count as usize {
                        return Verdict::Counting {
                            have: record.hits.len(),
                        };
                    }
                    // Reached: these lines are spoken for, and the next
                    // signal needs as many again.
                    let lines = record.hits.len() as u64;
                    record.hits.clear();
                    lines
                }
                _ => 1,
            };
            record.unreported += lines;
            if record
                .window_started_at
                .is_some_and(|started| ts - started < ask.throttle_ms)
            {
                return Verdict::Suppressed;
            }
        }
        let budget = self
            .budgets
            .entry((ask.rule_id.to_string(), key.0.clone()))
            .or_default();
        if !budget.spend(ts, ask.throttle_ms) {
            // Not spoken, so the instance's own window has not started: it
            // speaks on its next match once the device's window is over,
            // and its count still holds this line.
            self.held_back += 1;
            return Verdict::Suppressed;
        }
        let record = self
            .rules
            .get_mut(ask.rule_id)
            .and_then(|records| records.get_mut(key))
            .expect("the record was touched a moment ago");
        record.window_started_at = Some(ts);
        Verdict::Fire {
            count: std::mem::take(&mut record.unreported),
        }
    }

    /// The rule raised an alert for this key.
    pub fn opened(&mut self, key: &Key, alert: OpenAlert, wall: i64) {
        let rule_id = alert.rule_id.clone();
        self.slot(&rule_id, key, wall).open = Some(alert);
    }

    /// The rule's opposite event arrived for this key.
    pub fn opposite(
        &mut self,
        rule_id: &str,
        key: &Key,
        throttle_ms: i64,
        entry_id: i64,
        ts: i64,
        wall: i64,
    ) -> Opposite {
        let known = self.rules.get(rule_id).and_then(|records| records.get(key));
        if !known.is_some_and(|record| record.open.is_some()) {
            // Nothing here raised an alert for this key. The monitor may
            // still hold one from before a restart, so it is asked — once a
            // window per key, and for only so many keys of one device in a
            // window: the key comes out of the line, and a sender who makes
            // up a neighbour per line must not get a call per line.
            if known
                .and_then(|record| record.blind_clear_at)
                .is_some_and(|asked| ts - asked < throttle_ms)
            {
                return Opposite::Nothing;
            }
            let budget = self
                .blind
                .entry((rule_id.to_string(), key.0.clone()))
                .or_default();
            if !budget.spend(ts, throttle_ms) {
                // No record is made either: a key that was not passed on
                // has nothing to remember, and is passed on by its next
                // line after the window.
                self.held_back += 1;
                return Opposite::Nothing;
            }
            let record = self.slot(rule_id, key, wall);
            record.idle_ms = record.idle_ms.max(IDLE_MS).max(throttle_ms);
            record.blind_clear_at = Some(ts);
            return Opposite::Unknown;
        }
        let record = self.slot(rule_id, key, wall);
        record.idle_ms = record.idle_ms.max(IDLE_MS).max(throttle_ms);
        let held = record
            .window_started_at
            .is_some_and(|started| ts - started < throttle_ms);
        match &mut record.open {
            Some(open) if held => {
                open.recovered = Some((entry_id, ts));
                Opposite::Held
            }
            _ => match record.open.take() {
                Some(open) => Opposite::Cleared(open),
                None => Opposite::Nothing,
            },
        }
    }

    /// What is due on the wall clock: alerts whose opposite arrived while
    /// they were held and whose window is over, and alerts that have been
    /// quiet for their `clear_after_s`. Idle records are dropped on the way.
    pub fn sweep(&mut self, wall: i64) -> Vec<(OpenAlert, Reason)> {
        let mut due = Vec::new();
        for records in self.rules.values_mut() {
            for record in records.values_mut() {
                let reason = match &record.open {
                    Some(open) if wall < open.hold_until_wall => None,
                    Some(open) if open.recovered.is_some() => Some(Reason::Opposite),
                    Some(open)
                        if open
                            .clear_after_ms
                            .is_some_and(|after| wall - open.last_match_wall >= after) =>
                    {
                        Some(Reason::Quiet)
                    }
                    _ => None,
                };
                if let Some(reason) = reason
                    && let Some(open) = record.open.take()
                {
                    due.push((open, reason));
                }
            }
            records.retain(|_, record| {
                record.waited_on() || wall - record.touched_wall < record.idle_ms
            });
        }
        self.rules.retain(|_, records| !records.is_empty());
        due
    }

    /// Forget everything a rule remembers (it was deleted).
    pub fn forget_rule(&mut self, rule_id: &str) {
        self.rules.remove(rule_id);
        self.budgets.retain(|(rule, _), _| rule != rule_id);
        self.blind.retain(|(rule, _), _| rule != rule_id);
    }

    pub fn counts(&self) -> Counts {
        let tracked: usize = self.rules.values().map(HashMap::len).sum();
        let open = self
            .rules
            .values()
            .flat_map(HashMap::values)
            .filter(|record| record.waited_on())
            .count();
        Counts {
            tracked: tracked as u64,
            forgotten: self.forgotten,
            open: open as u64,
            held_back: self.held_back,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(instance: &str) -> Key {
        ("dev1".to_string(), instance.to_string())
    }

    fn ask(threshold: Option<(u32, i64)>) -> Ask<'static> {
        Ask {
            rule_id: "r1",
            threshold,
            throttle_ms: 60_000,
            after: None,
        }
    }

    fn alert(instance: &str, wall: i64, clear_after_ms: Option<i64>) -> OpenAlert {
        OpenAlert {
            rule_id: "r1".into(),
            rule_name: "Rule".into(),
            dedup_key: format!("logs:r1:dev1:{instance}"),
            title: "Something".into(),
            device_id: Some("dev1".into()),
            instance: instance.into(),
            last_match_wall: wall,
            clear_after_ms,
            hold_until_wall: wall + 60_000,
            recovered: None,
            // As for a rule with an opposite event, unless a test says not.
            has_opposite: true,
        }
    }

    #[test]
    fn the_nth_line_inside_the_window_fires_and_the_count_starts_again() {
        let mut memory = Memory::default();
        let rule = ask(Some((3, 10_000)));
        assert_eq!(
            memory.matched(&rule, &key("a"), 1_000, 1_000),
            Verdict::Counting { have: 1 }
        );
        assert_eq!(
            memory.matched(&rule, &key("a"), 2_000, 2_000),
            Verdict::Counting { have: 2 }
        );
        assert_eq!(
            memory.matched(&rule, &key("a"), 3_000, 3_000),
            Verdict::Fire { count: 3 }
        );
        assert_eq!(
            memory.matched(&rule, &key("a"), 4_000, 4_000),
            Verdict::Counting { have: 1 },
            "the three that fired are spoken for"
        );
    }

    #[test]
    fn lines_spread_wider_than_the_window_never_add_up() {
        let mut memory = Memory::default();
        let rule = ask(Some((3, 10_000)));
        for at in [0, 6_000, 12_000, 18_000, 24_000] {
            assert!(
                matches!(
                    memory.matched(&rule, &key("a"), at, at),
                    Verdict::Counting { .. }
                ),
                "a line every six seconds is never three in ten"
            );
        }
    }

    #[test]
    fn a_line_a_whole_window_after_the_first_no_longer_counts_with_it() {
        // Three in ten seconds. The window is half open: a line exactly ten
        // seconds after another is outside that one's window.
        let rule = ask(Some((3, 10_000)));

        let mut edge = Memory::default();
        edge.matched(&rule, &key("a"), 1_000, 1_000);
        edge.matched(&rule, &key("a"), 2_000, 2_000);
        assert_eq!(
            edge.matched(&rule, &key("a"), 11_000, 11_000),
            Verdict::Counting { have: 2 },
            "the first line is exactly a window old and has left it"
        );
        assert_eq!(
            edge.matched(&rule, &key("a"), 11_999, 11_999),
            Verdict::Fire { count: 3 },
            "the window slides: the second line is still inside it"
        );

        let mut inside = Memory::default();
        inside.matched(&rule, &key("a"), 1_000, 1_000);
        inside.matched(&rule, &key("a"), 2_000, 2_000);
        assert_eq!(
            inside.matched(&rule, &key("a"), 10_999, 10_999),
            Verdict::Fire { count: 3 },
            "a millisecond short of the window is inside it"
        );
    }

    #[test]
    fn a_threshold_of_one_fires_on_the_line_itself() {
        let mut memory = Memory::default();
        assert_eq!(
            memory.matched(&ask(Some((1, 10_000))), &key("a"), 1_000, 1_000),
            Verdict::Fire { count: 1 }
        );
    }

    #[test]
    fn each_group_is_counted_on_its_own() {
        let mut memory = Memory::default();
        let rule = ask(Some((2, 10_000)));
        assert_eq!(
            memory.matched(&rule, &key("a"), 1_000, 1_000),
            Verdict::Counting { have: 1 }
        );
        assert_eq!(
            memory.matched(&rule, &key("b"), 1_500, 1_500),
            Verdict::Counting { have: 1 },
            "another group's line does not complete this one's count"
        );
        assert_eq!(
            memory.matched(&rule, &key("a"), 2_000, 2_000),
            Verdict::Fire { count: 2 }
        );
    }

    #[test]
    fn a_full_rule_forgets_its_oldest_keys_and_counts_them() {
        let mut memory = Memory::default();
        let rule = ask(Some((2, 10_000)));
        for index in 0..MAX_KEYS_PER_RULE + 1 {
            memory.matched(&rule, &key(&format!("k{index}")), 1_000, 1_000);
        }
        let counts = memory.counts();
        assert_eq!(counts.forgotten, EVICT_AT_ONCE as u64);
        assert_eq!(
            counts.tracked,
            (MAX_KEYS_PER_RULE - EVICT_AT_ONCE + 1) as u64
        );
        // The first keys went; one of them starts counting from nothing.
        assert_eq!(
            memory.matched(&rule, &key("k0"), 1_100, 1_100),
            Verdict::Counting { have: 1 },
            "a forgotten key has lost its count"
        );
        // The newest stayed and completes its count.
        let newest = key(&format!("k{}", MAX_KEYS_PER_RULE));
        assert_eq!(
            memory.matched(&rule, &newest, 1_200, 1_200),
            Verdict::Fire { count: 2 }
        );
    }

    #[test]
    fn fifty_thousand_made_up_keys_leave_ten_thousand_records_and_a_count() {
        let mut memory = Memory::default();
        let rule = ask(Some((8, 60_000)));
        let mut most = 0;
        for index in 0..50_000 {
            let made_up = key(&format!("10.9.{}.{}", index / 250, index % 250));
            memory.matched(&rule, &made_up, 1_000, 1_000);
            // Looked at where it is fullest: just before each forgetting.
            if index % 1_000 == 999 {
                most = most.max(memory.counts().tracked);
            }
        }
        assert!(
            most <= MAX_KEYS_PER_RULE as u64,
            "the rule never held more than its cap: {most}"
        );
        let counts = memory.counts();
        assert_eq!(counts.tracked, MAX_KEYS_PER_RULE as u64);
        assert_eq!(
            counts.forgotten, 40_000,
            "every key that was let go is in the count"
        );
        assert_eq!(counts.tracked + counts.forgotten, 50_000);
    }

    #[test]
    fn a_key_with_an_alert_open_outlives_the_ones_without() {
        let mut memory = Memory::default();
        let rule = ask(None);
        memory.matched(&rule, &key("held"), 1_000, 1_000);
        memory.opened(&key("held"), alert("held", 1_000, None), 1_000);
        for index in 0..MAX_KEYS_PER_RULE {
            memory.matched(&rule, &key(&format!("k{index}")), 1_000, 1_000);
        }
        assert!(
            memory.is_open("r1", &key("held")),
            "the oldest key was kept because something still depends on it"
        );
    }

    #[test]
    fn an_opposite_inside_the_window_waits_and_a_new_match_cancels_it() {
        let mut memory = Memory::default();
        let rule = ask(None);
        assert_eq!(
            memory.matched(&rule, &key("a"), 1_000, 1_000),
            Verdict::Fire { count: 1 }
        );
        memory.opened(&key("a"), alert("a", 1_000, None), 1_000);

        assert_eq!(
            memory.opposite("r1", &key("a"), 60_000, 7, 5_000, 5_000),
            Opposite::Held
        );
        assert!(
            memory.sweep(30_000).is_empty(),
            "nothing clears inside the window"
        );
        // It went down again before the window ended: still open.
        assert_eq!(
            memory.matched(&rule, &key("a"), 40_000, 40_000),
            Verdict::Suppressed
        );
        assert!(memory.sweep(61_000).is_empty());
        assert!(memory.is_open("r1", &key("a")));

        // And up again, and this time nothing follows.
        assert_eq!(
            memory.opposite("r1", &key("a"), 60_000, 9, 45_000, 45_000),
            Opposite::Held
        );
        let cleared = memory.sweep(61_000);
        assert_eq!(cleared.len(), 1);
        assert_eq!(cleared[0].1, Reason::Opposite);
        assert_eq!(cleared[0].0.recovered, Some((9, 45_000)));
        assert!(!memory.is_open("r1", &key("a")));
    }

    #[test]
    fn an_opposite_after_the_window_clears_at_once() {
        let mut memory = Memory::default();
        let rule = ask(None);
        memory.matched(&rule, &key("a"), 1_000, 1_000);
        memory.opened(&key("a"), alert("a", 1_000, None), 1_000);
        let outcome = memory.opposite("r1", &key("a"), 60_000, 7, 62_000, 62_000);
        assert!(matches!(outcome, Opposite::Cleared(_)), "{outcome:?}");
        // The next occurrence is a new alert, raised at once.
        assert_eq!(
            memory.matched(&rule, &key("a"), 63_000, 63_000),
            Verdict::Fire { count: 1 }
        );
    }

    #[test]
    fn an_opposite_for_an_alert_nobody_remembers_is_passed_on_once_a_window() {
        let mut memory = Memory::default();
        assert_eq!(
            memory.opposite("r1", &key("a"), 60_000, 1, 1_000, 1_000),
            Opposite::Unknown
        );
        assert_eq!(
            memory.opposite("r1", &key("a"), 60_000, 2, 2_000, 2_000),
            Opposite::Nothing,
            "a flood of `up` lines is not a flood of calls"
        );
        assert_eq!(
            memory.opposite("r1", &key("a"), 60_000, 3, 62_000, 62_000),
            Opposite::Unknown
        );
    }

    #[test]
    fn opposites_for_made_up_keys_are_passed_on_for_only_so_many_of_a_device_in_a_window() {
        let mut memory = Memory::default();
        let mut asked = 0;
        for index in 0..MAX_SIGNALS_PER_WINDOW + 40 {
            let neighbour = key(&format!("192.0.2.{index}"));
            if memory.opposite("r1", &neighbour, 60_000, 1, 1_000, 1_000) == Opposite::Unknown {
                asked += 1;
            }
        }
        assert_eq!(asked, MAX_SIGNALS_PER_WINDOW);
        let counts = memory.counts();
        assert_eq!(counts.held_back, 40);
        assert_eq!(
            counts.tracked,
            u64::from(MAX_SIGNALS_PER_WINDOW),
            "a key that was not passed on leaves nothing behind"
        );

        // Another device's neighbours are not held back by this one's flood.
        let elsewhere = ("dev2".to_string(), "192.0.2.1".to_string());
        assert_eq!(
            memory.opposite("r1", &elsewhere, 60_000, 2, 1_000, 1_000),
            Opposite::Unknown
        );
        // And after the window a held-back key is passed on.
        let late = key(&format!("192.0.2.{}", MAX_SIGNALS_PER_WINDOW + 5));
        assert_eq!(
            memory.opposite("r1", &late, 60_000, 3, 62_000, 62_000),
            Opposite::Unknown
        );
    }

    #[test]
    fn an_alert_its_rule_cannot_clear_is_not_waited_on() {
        let mut memory = Memory::default();
        let rule = ask(None);
        memory.matched(&rule, &key("by-hand"), 1_000, 1_000);
        let by_hand = OpenAlert {
            has_opposite: false,
            ..alert("by-hand", 1_000, None)
        };
        assert!(!by_hand.clearable());
        memory.opened(&key("by-hand"), by_hand, 1_000);

        assert_eq!(
            memory.counts().open,
            0,
            "nothing here will ever hear how it ended, so it is not counted as open"
        );
        assert!(
            memory.is_open("r1", &key("by-hand")),
            "a rule that follows this one still sees it"
        );
        memory.sweep(1_000 + IDLE_MS + 1);
        assert_eq!(
            memory.counts().tracked,
            0,
            "and its record goes when it is idle, like any other"
        );
        assert!(!memory.is_open("r1", &key("by-hand")));
    }

    #[test]
    fn a_quiet_alert_clears_after_its_period_and_a_match_restarts_the_clock() {
        let mut memory = Memory::default();
        let rule = ask(None);
        memory.matched(&rule, &key("a"), 1_000, 1_000);
        memory.opened(&key("a"), alert("a", 1_000, Some(600_000)), 1_000);

        assert!(memory.sweep(500_000).is_empty());
        // Seen again at 400 s (suppressed or not, it is still happening).
        memory.matched(&rule, &key("a"), 400_000, 400_000);
        assert!(
            memory.sweep(700_000).is_empty(),
            "ten minutes since the first line, five since the last"
        );
        let cleared = memory.sweep(1_000_001);
        assert_eq!(cleared.len(), 1);
        assert_eq!(cleared[0].1, Reason::Quiet);
    }

    #[test]
    fn a_rule_that_follows_another_only_counts_while_that_one_is_open() {
        let mut memory = Memory::default();
        let follower = Ask {
            rule_id: "r2",
            threshold: None,
            throttle_ms: 60_000,
            after: Some("r1"),
        };
        assert_eq!(
            memory.matched(&follower, &key("a"), 1_000, 1_000),
            Verdict::NotAfter
        );
        memory.matched(&ask(None), &key("a"), 2_000, 2_000);
        memory.opened(&key("a"), alert("a", 2_000, None), 2_000);
        assert_eq!(
            memory.matched(&follower, &key("b"), 3_000, 3_000),
            Verdict::NotAfter,
            "open for another instance is not open for this one"
        );
        assert_eq!(
            memory.matched(&follower, &key("a"), 3_000, 3_000),
            Verdict::Fire { count: 1 }
        );
    }

    #[test]
    fn idle_records_are_dropped_and_open_ones_are_not() {
        let mut memory = Memory::default();
        let rule = ask(None);
        memory.matched(&rule, &key("idle"), 1_000, 1_000);
        memory.matched(&rule, &key("open"), 1_000, 1_000);
        memory.opened(&key("open"), alert("open", 1_000, None), 1_000);
        memory.sweep(1_000 + IDLE_MS + 1);
        let counts = memory.counts();
        assert_eq!((counts.tracked, counts.forgotten, counts.open), (1, 0, 1));
    }

    #[test]
    fn one_rule_speaks_for_only_so_many_instances_of_a_device_in_a_window() {
        let mut memory = Memory::default();
        let rule = ask(None);
        let mut fired = 0;
        for index in 0..MAX_SIGNALS_PER_WINDOW + 10 {
            let port = key(&format!("made-up-port-{index}"));
            if matches!(
                memory.matched(&rule, &port, 1_000, 1_000),
                Verdict::Fire { .. }
            ) {
                fired += 1;
            }
        }
        assert_eq!(fired, MAX_SIGNALS_PER_WINDOW);
        assert_eq!(memory.counts().held_back, 10);

        // Another device is not held back by this one's storm.
        let elsewhere = ("dev2".to_string(), "Gi1/0/1".to_string());
        assert_eq!(
            memory.matched(&rule, &elsewhere, 1_000, 1_000),
            Verdict::Fire { count: 1 }
        );

        // After the window a held-back instance speaks, and its count
        // still holds the line that was held back.
        let late = key(&format!("made-up-port-{}", MAX_SIGNALS_PER_WINDOW + 5));
        assert_eq!(
            memory.matched(&rule, &late, 62_000, 62_000),
            Verdict::Fire { count: 2 }
        );
    }
}
