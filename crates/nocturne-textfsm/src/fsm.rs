//! Execution of a compiled template (`TextFSM.ParseText` in the reference).
//!
//! The reference keeps, per value, the last assigned match and option-specific
//! state, and lets every option react to assign / clear / save events in the
//! order the options were declared. [`Slot`] models exactly that, because
//! several observable details fall out of it:
//!
//! * A rule assigns *every* named group of its regex, including groups that
//!   took no part in the match (optional parts, other alternatives). Such a
//!   group assigns "nothing": it erases the pending value, resets the
//!   `Filldown` memory and adds an empty item to a `List`.
//! * `Required` looks at the last assigned match, and for `Required,List` it
//!   does so before the list replaces it.
//! * A matched empty string counts as a value when deciding whether a row is
//!   empty, but not for `Required`.

use crate::error::RunError;
use crate::template::{LineOp, RecordOp, Template, Transition, ValueOption, ValueSpec};
use crate::text;
use crate::value::{Record, Value};

pub(crate) fn run(template: &Template, input: &str) -> Result<Vec<Record>, RunError> {
    let mut table = Table::new(&template.values);
    let mut state = &template.states[template.start];
    'lines: for line in text::split_lines(input) {
        for rule in &state.rules {
            let captures = rule
                .regex
                .match_start(line)
                .map_err(|error| RunError::Regex {
                    rule_line: rule.line,
                    message: error.to_string(),
                })?;
            let Some(captures) = captures else { continue };
            for &(group, value) in &rule.assignments {
                table.assign(value, captures.get(group));
            }
            match rule.record_op {
                RecordOp::NoRecord => {}
                RecordOp::Record => table.record(),
                RecordOp::Clear => table.clear(),
                RecordOp::Clearall => table.clear_all(),
            }
            match &rule.line_op {
                LineOp::Continue => continue,
                LineOp::Error(message) => {
                    return Err(RunError::ErrorAction {
                        message: message.clone(),
                        rule_line: rule.line,
                        input_line: line.to_owned(),
                    });
                }
                LineOp::Next => {}
            }
            match rule.transition {
                None => {}
                Some(Transition::State(next)) => state = &template.states[next],
                Some(Transition::End) => return Ok(table.into_records()),
                Some(Transition::Eof) => break 'lines,
            }
            break;
        }
    }
    if template.records_at_end {
        table.record();
    }
    Ok(table.into_records())
}

/// The rows recorded so far and the row being built.
struct Table<'t> {
    slots: Vec<Slot<'t>>,
    rows: Vec<Vec<Cell>>,
}

/// The run-time state of one value.
struct Slot<'t> {
    spec: &'t ValueSpec,
    /// What the next record would contain.
    pending: Pending,
    /// `Filldown`: the last assigned match, restored after a record.
    remembered: Option<String>,
    /// `List`: the matches since the last record.
    items: Vec<Option<String>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum Pending {
    #[default]
    Nothing,
    Text(String),
    List(Vec<Option<String>>),
}

impl Pending {
    /// Python truthiness: set, and not empty.
    fn is_set(&self) -> bool {
        match self {
            Pending::Nothing => false,
            Pending::Text(text) => !text.is_empty(),
            Pending::List(items) => !items.is_empty(),
        }
    }

    /// Whether this contributes nothing to a row: unset, or a list without
    /// items. A matched empty string does count.
    fn is_absent(&self) -> bool {
        match self {
            Pending::Nothing => true,
            Pending::Text(_) => false,
            Pending::List(items) => items.is_empty(),
        }
    }
}

/// A cell of a recorded row. `List` items are `None` where a rule matched
/// without the value's group taking part.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Cell {
    Text(String),
    List(Vec<Option<String>>),
}

impl Cell {
    fn is_set(&self) -> bool {
        match self {
            Cell::Text(text) => !text.is_empty(),
            Cell::List(items) => !items.is_empty(),
        }
    }
}

impl<'t> Table<'t> {
    fn new(values: &'t [ValueSpec]) -> Table<'t> {
        let slots = values
            .iter()
            .map(|spec| Slot {
                spec,
                pending: Pending::Nothing,
                remembered: None,
                items: Vec::new(),
            })
            .collect();
        Table {
            slots,
            rows: Vec::new(),
        }
    }

    /// A rule matched: `matched` is what the value's group captured, `None` if
    /// the group did not take part in the match.
    fn assign(&mut self, value: usize, matched: Option<&str>) {
        let slot = &mut self.slots[value];
        slot.pending = matched.map_or(Pending::Nothing, |text| Pending::Text(text.to_owned()));
        for option in &slot.spec.options {
            match option {
                ValueOption::Filldown => slot.remembered = matched.map(str::to_owned),
                ValueOption::List => slot.items.push(matched.map(str::to_owned)),
                ValueOption::Fillup => {
                    // Copy the match up the table until a row already has this value.
                    let Some(text) = matched.filter(|text| !text.is_empty()) else {
                        continue;
                    };
                    for row in self.rows.iter_mut().rev() {
                        if row[value].is_set() {
                            break;
                        }
                        row[value] = Cell::Text(text.to_owned());
                    }
                }
                ValueOption::Required | ValueOption::Key => {}
            }
        }
    }

    /// The `Clear` operator, also applied after every record: values are
    /// dropped, except that `Filldown` restores its last match.
    fn clear(&mut self) {
        for slot in &mut self.slots {
            if slot.spec.options.contains(&ValueOption::Filldown) {
                slot.pending = slot
                    .remembered
                    .clone()
                    .map_or(Pending::Nothing, Pending::Text);
            } else {
                slot.pending = Pending::Nothing;
                slot.items.clear();
            }
        }
    }

    /// The `Clearall` operator: forget everything, `Filldown` values included.
    fn clear_all(&mut self) {
        for slot in &mut self.slots {
            slot.pending = Pending::Nothing;
            slot.remembered = None;
            slot.items.clear();
        }
    }

    /// The `Record` operator: append the pending row if it is well formed.
    fn record(&mut self) {
        let mut row = Vec::with_capacity(self.slots.len());
        let mut complete = true;
        'slots: for slot in &mut self.slots {
            for option in &slot.spec.options {
                match option {
                    ValueOption::Required if !slot.pending.is_set() => {
                        complete = false;
                        break 'slots;
                    }
                    ValueOption::List => slot.pending = Pending::List(slot.items.clone()),
                    _ => {}
                }
            }
            row.push(slot.pending.clone());
        }
        if !complete {
            // A missing required value discards the pending row.
            self.clear();
            return;
        }
        // An entirely empty row is not recorded (and nothing needs clearing).
        if row.iter().all(Pending::is_absent) {
            return;
        }
        let cells = row.into_iter().map(|pending| match pending {
            Pending::Nothing => Cell::Text(String::new()),
            Pending::Text(text) => Cell::Text(text),
            Pending::List(items) => Cell::List(items),
        });
        self.rows.push(cells.collect());
        self.clear();
    }

    fn into_records(self) -> Vec<Record> {
        let names: Vec<&str> = self
            .slots
            .iter()
            .map(|slot| slot.spec.name.as_str())
            .collect();
        self.rows
            .into_iter()
            .map(|row| {
                names
                    .iter()
                    .zip(row)
                    .map(|(name, cell)| ((*name).to_owned(), Value::from(cell)))
                    .collect()
            })
            .collect()
    }
}

impl From<Cell> for Value {
    fn from(cell: Cell) -> Value {
        match cell {
            Cell::Text(text) => Value::Text(text),
            // The reference hands such items to the caller as Python's `None`;
            // ntc-templates turns them into the string "None".
            Cell::List(items) => Value::List(
                items
                    .into_iter()
                    .map(|item| item.unwrap_or_else(|| "None".to_owned()))
                    .collect(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use crate::{RunError, Template, Value};

    const TEMPLATE: &str = "\
Value Filldown,Required GROUP (\\S+)
Value List MEMBER (\\S+)

Start
  ^group ${GROUP}
  ^member ${MEMBER}
  ^end -> Record
  ^unsupported -> Error \"cannot parse this\"
  ^. -> Error
";

    #[test]
    fn records_rows_keyed_by_value_name() {
        let template = Template::parse(TEMPLATE).unwrap();
        let rows = template
            .run("group a\nmember x\nmember y\nend\nend\n")
            .unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].keys().collect::<Vec<_>>(), ["GROUP", "MEMBER"]);
        assert_eq!(rows[0]["GROUP"], Value::from("a"));
        assert_eq!(rows[0]["MEMBER"].as_list(), ["x", "y"]);
        assert_eq!(rows[1]["MEMBER"], Value::List(Vec::new()));
        // The Filldown value produces the row recorded at the end of the input.
        assert_eq!(rows[2], rows[1]);
        // A run does not leave anything behind.
        assert!(template.run("").unwrap().is_empty());
    }

    #[test]
    fn error_rules_stop_the_run() {
        let template = Template::parse(TEMPLATE).unwrap();
        let error = template.run("group a\nunsupported\n").unwrap_err();
        assert_eq!(
            error,
            RunError::ErrorAction {
                message: Some("cannot parse this".to_owned()),
                rule_line: 8,
                input_line: "unsupported".to_owned(),
            }
        );
        assert_eq!(
            error.to_string(),
            "Error: cannot parse this. Rule Line: 8. Input Line: unsupported."
        );
        let error = template.run("group a\n?\n").unwrap_err();
        assert_eq!(
            error.to_string(),
            "State Error raised. Rule Line: 9. Input Line: ?"
        );
    }
}
