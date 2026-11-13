//! Combining the tables of several templates (`TextTable.extend`).
//!
//! A few index rows name more than one template for a command. `clitable`
//! parses the output with each of them and adds the columns of the later
//! tables to the first one, matching rows by the values declared with the
//! `Key` option (or by position when there are none).

use crate::value::{Record, Value};

/// The tables of the templates listed for one command cannot be combined.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("cannot merge tables: key column '{key}' is missing")]
pub struct MergeError {
    key: String,
}

/// Adds the columns `table` lacks from `other`.
///
/// `header` is the header of `table` and grows by the added columns. A row
/// receives the cells of the first row of `other` with the same `keys`; rows
/// without a partner keep empty cells.
pub(crate) fn extend(
    table: &mut [Record],
    header: &mut Vec<String>,
    other: &[Record],
    other_header: &[String],
    keys: &[String],
) -> Result<(), MergeError> {
    let missing = |key: &String| MergeError { key: key.clone() };
    if let Some(key) = keys.iter().find(|key| !header.contains(key)) {
        return Err(missing(key));
    }
    let added: Vec<&String> = other_header
        .iter()
        .filter(|column| !header.contains(column))
        .collect();
    if added.is_empty() {
        return Ok(());
    }
    for row in table.iter_mut() {
        for &column in &added {
            row.insert(column.clone(), Value::Text(String::new()));
        }
    }
    header.extend(added.iter().map(|&column| column.clone()));

    let copy = |row: &mut Record, source: &Record| {
        for &column in &added {
            if let Some(value) = source.get(column) {
                row.insert(column.clone(), value.clone());
            }
        }
    };
    if keys.is_empty() {
        for (row, source) in table.iter_mut().zip(other) {
            copy(row, source);
        }
        return Ok(());
    }
    if !table.is_empty()
        && !other.is_empty()
        && let Some(key) = keys.iter().find(|key| !other_header.contains(key))
    {
        return Err(missing(key));
    }
    for row in table.iter_mut() {
        let partner = other
            .iter()
            .find(|source| keys.iter().all(|key| row.get(key) == source.get(key)));
        if let Some(source) = partner {
            copy(row, source);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(header: &[&str], rows: &[&[&str]]) -> (Vec<String>, Vec<Record>) {
        let header: Vec<String> = header.iter().map(|name| (*name).to_owned()).collect();
        let rows = rows
            .iter()
            .map(|row| {
                header
                    .iter()
                    .cloned()
                    .zip(row.iter().map(|cell| Value::from(*cell)))
                    .collect()
            })
            .collect();
        (header, rows)
    }

    fn cells(rows: &[Record]) -> Vec<Vec<&str>> {
        rows.iter()
            .map(|row| row.values().map(Value::as_str).collect())
            .collect()
    }

    // The expected tables below are what textfsm.texttable.TextTable.extend produces.

    #[test]
    fn merges_on_keys() {
        let (mut header, mut rows) = table(
            &["SWITCH", "ROLE"],
            &[&["1", "Active"], &["2", "Member"], &["3", "Member"]],
        );
        let (other_header, other) = table(
            &["SWITCH", "PORT", "ROLE"],
            &[&["3", "Gi3", "x"], &["1", "Gi1", "y"], &["1", "Gi9", "z"]],
        );
        extend(
            &mut rows,
            &mut header,
            &other,
            &other_header,
            &["SWITCH".to_owned()],
        )
        .unwrap();
        assert_eq!(header, ["SWITCH", "ROLE", "PORT"]);
        assert_eq!(
            cells(&rows),
            [
                ["1", "Active", "Gi1"],
                ["2", "Member", ""],
                ["3", "Member", "Gi3"]
            ]
        );
    }

    #[test]
    fn merges_by_position_without_keys() {
        let (mut header, mut rows) = table(&["A"], &[&["1"], &["2"], &["3"]]);
        let (other_header, other) = table(&["B"], &[&["x"], &["y"]]);
        extend(&mut rows, &mut header, &other, &other_header, &[]).unwrap();
        assert_eq!(header, ["A", "B"]);
        assert_eq!(cells(&rows), [["1", "x"], ["2", "y"], ["3", ""]]);
    }

    #[test]
    fn nothing_to_add() {
        let (mut header, mut rows) = table(&["A", "B"], &[&["1", "2"]]);
        let (other_header, other) = table(&["B", "A"], &[&["9", "9"]]);
        extend(
            &mut rows,
            &mut header,
            &other,
            &other_header,
            &["A".to_owned()],
        )
        .unwrap();
        assert_eq!(header, ["A", "B"]);
        assert_eq!(cells(&rows), [["1", "2"]]);
    }

    #[test]
    fn keys_must_exist() {
        let (mut header, mut rows) = table(&["A"], &[&["1"]]);
        let (other_header, other) = table(&["K", "B"], &[&["1", "x"]]);
        let error = extend(
            &mut rows,
            &mut header,
            &other,
            &other_header,
            &["K".to_owned()],
        );
        assert_eq!(
            error,
            Err(MergeError {
                key: "K".to_owned()
            })
        );

        // A key the second table lacks only matters once rows are compared.
        let (mut header, mut rows) = table(&["K", "A"], &[&["1", "a"]]);
        let (other_header, other) = table(&["B"], &[&["x"]]);
        let keys = ["K".to_owned()];
        assert!(extend(&mut rows, &mut header, &other, &other_header, &keys).is_err());
        let (mut header, mut rows) = table(&["K", "A"], &[&["1", "a"]]);
        extend(&mut rows, &mut header, &[], &other_header, &keys).unwrap();
        assert_eq!(cells(&rows), [["1", "a", ""]]);
    }
}
