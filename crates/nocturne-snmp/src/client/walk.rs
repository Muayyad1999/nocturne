//! Subtree walking and table pivoting.
//!
//! A walk follows one or more subtrees at once. With GETBULK every subtree is a repeater, so
//! a response interleaves them — `a1 b1 c1 a2 b2 c2 …` — and each subtree forms its own stream
//! of successive OIDs. For a table that means a response carries whole rows, and a walk of N
//! columns costs about as many round trips as a walk of one. SNMPv1 does the same with one
//! multi-varbind GETNEXT per row.

use std::collections::BTreeMap;

use indexmap::IndexMap;

use super::Client;
use crate::error::status;
use crate::{Oid, SnmpError, Value, VarBind};

/// A subtree that is still being walked.
struct Stream {
    /// Which of the caller's roots this stream belongs to.
    root: usize,
    /// The last OID seen; the next request continues after it.
    cursor: Oid,
}

impl Client {
    /// Walks every subtree in `roots`, collecting at most `limit` bindings for each.
    ///
    /// A stream ends when the agent leaves its subtree, signals the end of the MIB, or
    /// returns an OID that is not greater than the previous one (which would loop forever).
    pub(super) async fn walk_subtrees(
        &mut self,
        roots: &[Oid],
        limit: usize,
    ) -> Result<Vec<Vec<VarBind>>, SnmpError> {
        let mut found: Vec<Vec<VarBind>> = vec![Vec::new(); roots.len()];
        if limit == 0 {
            return Ok(found);
        }
        let mut streams: Vec<Stream> = roots
            .iter()
            .enumerate()
            .map(|(root, oid)| Stream {
                root,
                cursor: oid.clone(),
            })
            .collect();
        let mut repetitions = self.max_repetitions;

        while !streams.is_empty() {
            let cursors: Vec<Oid> = streams.iter().map(|stream| stream.cursor.clone()).collect();
            let binds = if self.is_v1() {
                match self.get_next(&cursors).await {
                    Ok(binds) => binds,
                    // SNMPv1 has no endOfMibView: running off the end fails the request with
                    // noSuchName, and error-index names the stream that fell off.
                    Err(SnmpError::Agent { status, index, .. }) if status == status::NO_SUCH_NAME => {
                        match (index as usize).checked_sub(1) {
                            Some(slot) if slot < streams.len() => {
                                streams.remove(slot);
                            }
                            // No usable index: nothing tells the streams apart, so stop.
                            _ => streams.clear(),
                        }
                        continue;
                    }
                    Err(e) => return Err(e),
                }
            } else {
                // Never ask for more rows than the fullest stream may still take.
                let room = streams
                    .iter()
                    .map(|stream| limit - found[stream.root].len())
                    .max()
                    .unwrap_or(1);
                let asked = repetitions.min(u32::try_from(room).unwrap_or(u32::MAX));
                let mut granted = asked;
                let binds = self.bulk(&cursors, 0, &mut granted).await?;
                if granted < asked {
                    // The agent found the request tooBig; stay at the size it could answer.
                    repetitions = granted;
                }
                binds
            };

            if binds.is_empty() {
                // Nothing more to say for any stream.
                break;
            }
            let width = streams.len();
            let mut ended = vec![false; width];
            for (position, bind) in binds.into_iter().enumerate() {
                let slot = position % width;
                if ended[slot] {
                    continue;
                }
                let stream = &mut streams[slot];
                let in_subtree = bind.oid.starts_with(&roots[stream.root]);
                if bind.value.is_exception() || !in_subtree || bind.oid <= stream.cursor {
                    ended[slot] = true;
                    continue;
                }
                stream.cursor = bind.oid.clone();
                let collected = &mut found[stream.root];
                collected.push(bind);
                if collected.len() >= limit {
                    ended[slot] = true;
                }
            }
            let mut ended = ended.into_iter();
            streams.retain(|_| !ended.next().unwrap_or(false));
        }
        Ok(found)
    }
}

/// Turns walked columns into rows: index suffix → column OID → value.
///
/// Rows are ordered by index (the order the agent serves them in) and cut to the first
/// `limit`; cells keep the order of `columns`.
pub(super) fn pivot(
    columns: &[Oid],
    walked: Vec<Vec<VarBind>>,
    limit: usize,
) -> IndexMap<Vec<u32>, IndexMap<Oid, Value>> {
    let mut rows: BTreeMap<Vec<u32>, IndexMap<Oid, Value>> = BTreeMap::new();
    for (column, binds) in columns.iter().zip(walked) {
        for bind in binds {
            if let Some(index) = bind.oid.suffix(column) {
                rows.entry(index.to_vec())
                    .or_default()
                    .insert(column.clone(), bind.value);
            }
        }
    }
    rows.into_iter().take(limit).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn oid(s: &str) -> Oid {
        s.parse().unwrap()
    }

    fn bind(s: &str, value: i64) -> VarBind {
        VarBind::new(oid(s), Value::Integer(value))
    }

    #[test]
    fn pivot_orders_rows_by_index_and_cells_by_column() {
        let columns = [oid("1.3.6.1.2.1.2.2.1.2"), oid("1.3.6.1.2.1.2.2.1.8")];
        let walked = vec![
            // Deliberately out of order, with a row (10) the second column lacks.
            vec![
                bind("1.3.6.1.2.1.2.2.1.2.10", 110),
                bind("1.3.6.1.2.1.2.2.1.2.2", 102),
                bind("1.3.6.1.2.1.2.2.1.2.1", 101),
            ],
            vec![
                bind("1.3.6.1.2.1.2.2.1.8.1", 801),
                bind("1.3.6.1.2.1.2.2.1.8.2", 802),
                bind("1.3.6.1.2.1.2.2.1.8.3", 803),
            ],
        ];
        let table = pivot(&columns, walked.clone(), usize::MAX);
        let indexes: Vec<&Vec<u32>> = table.keys().collect();
        assert_eq!(indexes, [&vec![1], &vec![2], &vec![3], &vec![10]]);
        assert_eq!(
            table[&vec![1u32]].iter().collect::<Vec<_>>(),
            [
                (&columns[0], &Value::Integer(101)),
                (&columns[1], &Value::Integer(801)),
            ]
        );
        // Sparse rows only hold the cells that exist.
        assert_eq!(table[&vec![3u32]].len(), 1);
        assert_eq!(table[&vec![3u32]][&columns[1]], Value::Integer(803));
        assert_eq!(table[&vec![10u32]].keys().collect::<Vec<_>>(), [&columns[0]]);

        // The limit keeps the first rows in index order, complete.
        let limited = pivot(&columns, walked, 2);
        assert_eq!(limited.keys().collect::<Vec<_>>(), [&vec![1], &vec![2]]);
        assert_eq!(limited[&vec![2u32]].len(), 2);
    }

    #[test]
    fn pivot_handles_multi_part_indexes_and_foreign_bindings() {
        let columns = [oid("1.3.6.1.2.1.4.22.1.2")];
        let walked = vec![vec![
            bind("1.3.6.1.2.1.4.22.1.2.2.10.0.0.9", 1),
            bind("1.3.6.1.2.1.4.22.1.2.2.10.0.0.10", 2),
            bind("1.3.6.1.2.1.4.22.1.2.1.192.168.1.1", 3),
            // Not under the column at all: ignored.
            bind("1.3.6.1.2.1.4.22.1.3.1.192.168.1.1", 4),
        ]];
        let table = pivot(&columns, walked, usize::MAX);
        let indexes: Vec<Vec<u32>> = table.keys().cloned().collect();
        assert_eq!(
            indexes,
            [
                vec![1, 192, 168, 1, 1],
                vec![2, 10, 0, 0, 9],
                vec![2, 10, 0, 0, 10],
            ]
        );
        assert!(pivot(&columns, vec![vec![]], usize::MAX).is_empty());
        assert!(pivot(&[], vec![], 5).is_empty());
    }
}
