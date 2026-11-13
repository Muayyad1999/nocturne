//! The interface table: one row per device and port, filtered, sorted and
//! paged in SQL because a campus has a hundred thousand of them.

use nocturne_app::{App, AppError, AppResult};
use rusqlite::{Row, params};
use serde::Deserialize;

use crate::model::IfStatus;

/// Replace everything known about one device's interfaces.
pub fn replace(app: &App, device_id: &str, interfaces: &[IfStatus]) -> AppResult<()> {
    app.store().transaction(|tx| {
        tx.execute(
            "DELETE FROM monitor_interfaces WHERE device_id = ?1",
            params![device_id],
        )?;
        let mut insert = tx.prepare_cached(
            "INSERT OR REPLACE INTO monitor_interfaces
             (device_id, device, name, description, admin_up, oper_up, speed_bps,
              in_bps, out_bps, util_pct, in_err_ps, out_err_ps, in_disc_ps, out_disc_ps,
              uplink, lab, at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
        )?;
        for interface in interfaces {
            insert.execute(params![
                device_id,
                interface.device,
                interface.name,
                interface.description,
                interface.admin_up,
                interface.oper_up,
                interface.speed_bps.map(|speed| speed as i64),
                interface.in_bps,
                interface.out_bps,
                interface.util_pct,
                interface.in_err_ps,
                interface.out_err_ps,
                interface.in_disc_ps,
                interface.out_disc_ps,
                interface.uplink,
                interface.lab,
                interface.at,
            ])?;
        }
        Ok(())
    })
}

pub fn forget(app: &App, device_id: &str) -> AppResult<()> {
    app.store().with(|conn| {
        conn.execute(
            "DELETE FROM monitor_interfaces WHERE device_id = ?1",
            params![device_id],
        )
    })?;
    Ok(())
}

/// Everything recorded for one device, in name order.
pub fn of_device(app: &App, device_id: &str) -> AppResult<Vec<IfStatus>> {
    query(
        app,
        &InterfaceQuery {
            device_id: Some(device_id.to_string()),
            limit: Some(10_000),
            ..InterfaceQuery::default()
        },
    )
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct InterfaceQuery {
    pub device_id: Option<String>,
    /// Free text over the device name, the interface name and its description.
    pub filter: Option<String>,
    /// Only interfaces that want looking at: down while admin up, taking
    /// errors, or above 90 % utilisation.
    pub problem_only: bool,
    /// `name` (default), `device`, `util`, `in_bps`, `out_bps`, `errors`.
    pub sort: Option<String>,
    pub limit: Option<u32>,
    /// How many rows to skip, as a decimal string: pass the number of rows
    /// already received. A page shorter than `limit` is the last one.
    pub cursor: Option<String>,
}

fn order_by(sort: Option<&str>) -> &'static str {
    match sort.unwrap_or("name") {
        "device" => "device COLLATE NOCASE, name COLLATE NOCASE",
        "util" => "util_pct IS NULL, util_pct DESC, name COLLATE NOCASE",
        "in_bps" => "in_bps IS NULL, in_bps DESC, name COLLATE NOCASE",
        "out_bps" => "out_bps IS NULL, out_bps DESC, name COLLATE NOCASE",
        "errors" => "COALESCE(in_err_ps, 0) + COALESCE(out_err_ps, 0) DESC, name COLLATE NOCASE",
        _ => "device COLLATE NOCASE, name COLLATE NOCASE",
    }
}

const PROBLEM: &str = "(admin_up = 1 AND oper_up = 0) \
     OR COALESCE(in_err_ps, 0) > 0 OR COALESCE(out_err_ps, 0) > 0 \
     OR COALESCE(util_pct, 0) >= 90";

pub fn query(app: &App, query: &InterfaceQuery) -> AppResult<Vec<IfStatus>> {
    let limit = query.limit.unwrap_or(200).clamp(1, 5_000) as i64;
    let offset = match &query.cursor {
        Some(cursor) if !cursor.is_empty() => cursor
            .parse::<i64>()
            .map_err(|_| AppError::invalid("the cursor is the number of rows already received"))?,
        _ => 0,
    };
    let text = query
        .filter
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(|text| format!("%{}%", text.replace('%', "\\%").replace('_', "\\_")));

    let mut sql = String::from(
        "SELECT device_id, device, name, description, admin_up, oper_up, speed_bps,
                in_bps, out_bps, util_pct, in_err_ps, out_err_ps, in_disc_ps, out_disc_ps,
                uplink, lab, at
         FROM monitor_interfaces WHERE 1 = 1",
    );
    if query.device_id.is_some() {
        sql.push_str(" AND device_id = :device_id");
    }
    if text.is_some() {
        sql.push_str(
            " AND (device LIKE :text ESCAPE '\\' OR name LIKE :text ESCAPE '\\' \
               OR description LIKE :text ESCAPE '\\')",
        );
    }
    if query.problem_only {
        sql.push_str(&format!(" AND ({PROBLEM})"));
    }
    sql.push_str(&format!(
        " ORDER BY {} LIMIT :limit OFFSET :offset",
        order_by(query.sort.as_deref())
    ));

    app.store().with(|conn| {
        let mut statement = conn.prepare_cached(&sql)?;
        let mut bound: Vec<(&str, &dyn rusqlite::ToSql)> =
            vec![(":limit", &limit), (":offset", &offset)];
        if let Some(device_id) = &query.device_id {
            bound.push((":device_id", device_id));
        }
        if let Some(text) = &text {
            bound.push((":text", text));
        }
        let rows = statement.query_map(bound.as_slice(), row_to_status)?;
        rows.collect()
    })
}

fn row_to_status(row: &Row<'_>) -> rusqlite::Result<IfStatus> {
    Ok(IfStatus {
        device_id: row.get(0)?,
        device: row.get(1)?,
        name: row.get(2)?,
        description: row.get(3)?,
        admin_up: row.get(4)?,
        oper_up: row.get(5)?,
        speed_bps: row.get::<_, Option<i64>>(6)?.map(|speed| speed as u64),
        in_bps: row.get(7)?,
        out_bps: row.get(8)?,
        util_pct: row.get(9)?,
        in_err_ps: row.get(10)?,
        out_err_ps: row.get(11)?,
        in_disc_ps: row.get(12)?,
        out_disc_ps: row.get(13)?,
        uplink: row.get(14)?,
        lab: row.get(15)?,
        at: row.get(16)?,
    })
}

/// How many interfaces are in a problem state, for `monitor.status`.
pub fn problem_count(app: &App) -> AppResult<u64> {
    app.store().with(|conn| {
        conn.query_row(
            &format!("SELECT COUNT(*) FROM monitor_interfaces WHERE {PROBLEM}"),
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|count| count as u64)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Harness;

    fn sample(device: &str, name: &str, util: Option<f64>, oper: Option<bool>) -> IfStatus {
        IfStatus {
            device: device.into(),
            name: name.into(),
            admin_up: Some(true),
            oper_up: oper,
            util_pct: util,
            at: 10,
            ..IfStatus::default()
        }
    }

    #[tokio::test]
    async fn interfaces_are_replaced_filtered_sorted_and_paged() {
        let harness = Harness::new().await;
        let id = harness
            .device("SW1", "10.0.0.1", serde_json::json!({}))
            .await;
        replace(
            &harness.app,
            &id,
            &[
                sample("SW1", "Gi0/1", Some(95.0), Some(true)),
                sample("SW1", "Gi0/2", Some(1.0), Some(false)),
                sample("SW1", "Gi0/3", None, Some(true)),
            ],
        )
        .unwrap();
        let all = of_device(&harness.app, &id).unwrap();
        assert_eq!(
            all.iter().map(|row| row.name.as_str()).collect::<Vec<_>>(),
            ["Gi0/1", "Gi0/2", "Gi0/3"]
        );

        // Replacing drops what is no longer there.
        replace(
            &harness.app,
            &id,
            &[sample("SW1", "Gi0/1", Some(95.0), Some(true))],
        )
        .unwrap();
        assert_eq!(of_device(&harness.app, &id).unwrap().len(), 1);

        replace(
            &harness.app,
            &id,
            &[
                sample("SW1", "Gi0/1", Some(95.0), Some(true)),
                sample("SW1", "Gi0/2", Some(1.0), Some(false)),
                sample("SW1", "Gi0/3", Some(2.0), Some(true)),
            ],
        )
        .unwrap();
        let problems = query(
            &harness.app,
            &InterfaceQuery {
                problem_only: true,
                ..InterfaceQuery::default()
            },
        )
        .unwrap();
        assert_eq!(
            problems
                .iter()
                .map(|row| row.name.as_str())
                .collect::<Vec<_>>(),
            ["Gi0/1", "Gi0/2"],
            "95 % busy and admin-up-but-down are both problems"
        );
        assert_eq!(problem_count(&harness.app).unwrap(), 2);

        let busiest = query(
            &harness.app,
            &InterfaceQuery {
                sort: Some("util".into()),
                limit: Some(1),
                ..InterfaceQuery::default()
            },
        )
        .unwrap();
        assert_eq!(busiest[0].name, "Gi0/1");

        let page = query(
            &harness.app,
            &InterfaceQuery {
                limit: Some(2),
                cursor: Some("2".into()),
                ..InterfaceQuery::default()
            },
        )
        .unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].name, "Gi0/3");

        forget(&harness.app, &id).unwrap();
        assert!(of_device(&harness.app, &id).unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_filter_matches_the_name_and_the_description_and_a_bad_cursor_is_refused() {
        let harness = Harness::new().await;
        let id = harness
            .device("SW1", "10.0.0.1", serde_json::json!({}))
            .await;
        replace(
            &harness.app,
            &id,
            &[
                IfStatus {
                    description: "uplink to CORE".into(),
                    ..sample("SW1", "Gi0/1", None, Some(true))
                },
                sample("SW1", "Gi0/2", None, Some(true)),
            ],
        )
        .unwrap();
        let found = query(
            &harness.app,
            &InterfaceQuery {
                filter: Some("uplink".into()),
                ..InterfaceQuery::default()
            },
        )
        .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "Gi0/1");

        let error = query(
            &harness.app,
            &InterfaceQuery {
                cursor: Some("later".into()),
                ..InterfaceQuery::default()
            },
        )
        .unwrap_err();
        assert_eq!(error.code, "invalid");
    }
}
