//! The probes, against loopback and against servers the tests start.
//!
//! The rule being tested everywhere here is the one in `lib.rs`: a probe that
//! cannot run says so. Nothing in these tests accepts a plausible-looking
//! result in place of an answer — a ping this machine may not send must come
//! back as a refusal with a reason, and a TLS handshake against something that
//! is not TLS must come back as a failure, not as an empty certificate.

use std::time::Duration;

use nocturne_app::jobs::JobState;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::support;

/// How long a loopback probe may take before the test gives up on it.
const PATIENCE: Duration = Duration::from_secs(10);

#[tokio::test]
async fn a_ping_of_loopback_either_reports_or_says_it_could_not_run() {
    let lab = support::lab(&[]).await;
    let mut events = support::Events::new(&lab.app);
    let job = support::run_job(
        &lab.app,
        "probe.ping",
        json!({"host": "127.0.0.1", "count": 3, "interval_ms": 20, "timeout_ms": 500}),
    )
    .await
    .unwrap();
    let runs = lab
        .app
        .rpc("probe.history", json!({"kind": "ping"}))
        .await
        .unwrap();
    let run = &runs["runs"][0];

    match job.state {
        JobState::Succeeded => {
            let result = job.result.expect("a report");
            assert_eq!(result["sent"], 3);
            assert_eq!(result["address"], "127.0.0.1");
            assert_eq!(result["from"], "this computer");
            // It says how it was sent: the method the operating system allowed.
            assert!(
                ["icmp_datagram", "icmp_raw", "system_command"]
                    .contains(&result["method"].as_str().unwrap_or_default()),
                "{}",
                result["method"]
            );
            assert_eq!(result["echoes"].as_array().unwrap().len(), 3);
            // The echoes streamed as they came back, in sequence order.
            let echoes = events.drain("probe.ping.echo");
            assert!(!echoes.is_empty(), "no echo events");
            let sequence: Vec<u64> = echoes
                .iter()
                .map(|echo| echo["echo"]["seq"].as_u64().unwrap())
                .collect();
            let mut sorted = sequence.clone();
            sorted.sort_unstable();
            assert_eq!(sequence, sorted, "echoes arrived out of order");
            assert_eq!(echoes[0]["run_id"], run["id"]);

            assert_eq!(run["ok"], true);
            assert!(
                run["summary"].as_str().unwrap().contains("replies"),
                "{}",
                run["summary"]
            );
        }
        JobState::Failed => {
            // The machine may not send an echo request at all. That is an
            // answer about this machine, and it must not look like a result.
            let error = job.error.clone().unwrap_or_default();
            assert_eq!(run["ok"], false);
            assert_eq!(run["result"], Value::Null, "a refusal is not a report");
            assert!(
                [
                    "forbidden",
                    "unsupported",
                    "unavailable",
                    "not_found",
                    "timeout"
                ]
                .contains(&run["error_code"].as_str().unwrap_or_default()),
                "{run}"
            );
            assert!(!error.is_empty(), "a failure has to say why");
            eprintln!("this machine cannot send an echo request: {error}");
        }
        other => panic!("unexpected job state {other:?}"),
    }
}

#[tokio::test]
async fn a_ping_until_cancelled_stops_when_the_job_is_cancelled() {
    let lab = support::lab(&[]).await;
    let started = lab
        .app
        .rpc(
            "probe.ping",
            json!({"host": "127.0.0.1", "count": 0, "interval_ms": 20, "timeout_ms": 300}),
        )
        .await
        .unwrap();
    let job_id = started["job_id"].as_str().unwrap().to_string();

    // Give it long enough to send a few echoes, then stop it. A continuous
    // ping that ignored the cancel would run for the hour the settings allow.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // While it is still running, its history record must name the job that is
    // carrying it out. `history::begin` saves the record before `jobs::start`
    // has returned an id, so without a second save once the id is known this
    // is `null` for as long as the probe runs and is only corrected when it
    // ends — which is to say, it is wrong at exactly the moment somebody
    // wants to follow the probe or stop it.
    let running = lab
        .app
        .rpc("probe.history", json!({"kind": "ping"}))
        .await
        .unwrap()["runs"][0]
        .clone();
    assert_eq!(
        running["finished_at"],
        Value::Null,
        "this is meant to be a record of a probe still running: {running}"
    );
    assert_eq!(
        running["job_id"].as_str(),
        Some(job_id.as_str()),
        "a running probe's record should name its job: {running}"
    );
    assert!(
        running["method"].is_string(),
        "and the method it settled on: {running}"
    );

    let cancelled = lab
        .app
        .rpc("jobs.cancel", json!({"id": job_id}))
        .await
        .unwrap();
    let job = tokio::time::timeout(
        Duration::from_secs(5),
        nocturne_app::jobs::wait(&lab.app, &job_id),
    )
    .await
    .expect("a cancelled ping stops promptly")
    .unwrap();
    assert!(job.state.is_finished());
    assert_eq!(cancelled["cancelled"], true);

    let run = lab
        .app
        .rpc("probe.history", json!({"kind": "ping"}))
        .await
        .unwrap()["runs"][0]
        .clone();
    assert!(run["finished_at"].is_number(), "{run}");
    assert_ne!(run["ok"], Value::Null, "the record is not left running");
}

#[tokio::test]
async fn a_ping_until_cancelled_stops_itself_when_the_settings_say_it_has_run_long_enough() {
    let lab = support::lab(&[]).await;
    // One second is the shortest the settings allow; a run that ignored the
    // cap would go on for the hour the default allows.
    lab.app
        .rpc(
            "settings.set",
            json!({"section": "toolbox", "value": {"probe": {"max_run_s": 1}}}),
        )
        .await
        .unwrap();
    let job = support::run_job(
        &lab.app,
        "probe.ping",
        json!({"host": "127.0.0.1", "count": 0, "interval_ms": 50, "timeout_ms": 200}),
    )
    .await
    .unwrap();
    assert!(job.state.is_finished(), "it stopped on its own");
    let run = lab
        .app
        .rpc("probe.history", json!({"kind": "ping"}))
        .await
        .unwrap()["runs"][0]
        .clone();
    assert_eq!(run["job_id"], job.id, "the record points at its job");
    assert!(run["finished_at"].is_number());
}

#[tokio::test]
async fn a_port_check_finds_the_listener_the_test_opened_and_reads_its_banner() {
    let lab = support::lab(&[]).await;
    let mut events = support::Events::new(&lab.app);
    let open = support::banner_listener("SSH-2.0-OpenSSH_9.6 test\r\n").await;
    let closed = support::closed_port().await;

    let job = support::run_job(
        &lab.app,
        "probe.ports",
        json!({"host": "127.0.0.1", "ports": format!("{open},{closed}"), "timeout_ms": 1500}),
    )
    .await
    .unwrap();
    assert_eq!(job.state, JobState::Succeeded);
    let result = job.result.unwrap();
    assert_eq!(result["scanned"], 2);
    assert_eq!(result["open"], json!([open]));
    assert_eq!(result["from"], "this computer");

    let ports = result["ports"].as_array().unwrap();
    let listener = ports.iter().find(|port| port["port"] == open).unwrap();
    assert_eq!(listener["state"], "open");
    assert_eq!(
        listener["banner"].as_str().unwrap(),
        "SSH-2.0-OpenSSH_9.6 test"
    );
    let shut = ports.iter().find(|port| port["port"] == closed).unwrap();
    assert!(
        shut["state"] == "closed" || shut["state"] == "filtered",
        "{shut}"
    );

    // Only open ports are streamed, and the result has the full list.
    let streamed = events.drain("probe.port");
    assert_eq!(streamed.len(), 1, "{streamed:?}");
    assert_eq!(streamed[0]["port"]["port"], open);

    // The cap on how many ports one check may try is enforced before any
    // socket is opened.
    let refused = lab
        .app
        .rpc(
            "probe.ports",
            json!({"host": "127.0.0.1", "ports": "1-9000"}),
        )
        .await
        .unwrap_err();
    assert_eq!(refused.code, "invalid");
    assert!(refused.message.contains("at most"), "{}", refused.message);
}

#[tokio::test]
async fn a_probe_can_be_aimed_at_a_device_by_name_and_the_result_names_it() {
    let lab = support::lab(&["SW1"]).await;
    let open = support::banner_listener("hello\r\n").await;
    let id = support::device_at(&lab.app, "EDGE1", "127.0.0.1").await;

    let job = support::run_job(
        &lab.app,
        "probe.ports",
        json!({"device": "EDGE1", "ports": open.to_string(), "timeout_ms": 1500}),
    )
    .await
    .unwrap();
    let result = job.result.unwrap();
    assert_eq!(result["host"], "127.0.0.1");
    assert_eq!(result["device"]["name"], "EDGE1");
    assert_eq!(result["device"]["id"], id);
    assert_eq!(result["device"]["role"], "access");
    assert_eq!(result["open"], json!([open]));
    // The job is attached to the device, so it shows on the device's screen.
    assert_eq!(job.device_ids, vec![id.clone()]);

    let run = lab
        .app
        .rpc("probe.history", json!({"device_id": id}))
        .await
        .unwrap();
    assert_eq!(run["total"], 1);
    assert_eq!(run["runs"][0]["device"]["name"], "EDGE1");

    // A device that has no address cannot be probed, and says so.
    let no_address = lab.app.rpc("probe.ping", json!({"device": "SW1"})).await;
    let refused = no_address.unwrap_err();
    assert_eq!(refused.code, "invalid");
    assert!(
        refused.message.contains("no address"),
        "{}",
        refused.message
    );

    // And one of the two has to be given.
    assert_eq!(
        lab.app
            .rpc(
                "probe.ports",
                json!({"host": "127.0.0.1", "device": "EDGE1"})
            )
            .await
            .unwrap_err()
            .code,
        "invalid"
    );
    assert_eq!(
        lab.app
            .rpc("probe.ports", json!({}))
            .await
            .unwrap_err()
            .code,
        "invalid"
    );
    assert_eq!(
        lab.app
            .rpc("probe.ping", json!({"device": "no such device"}))
            .await
            .unwrap_err()
            .code,
        "not_found"
    );
}

#[tokio::test]
async fn a_dns_lookup_asks_the_server_it_was_given_and_recognises_the_answer() {
    let lab = support::lab(&[]).await;
    let port = support::dns_responder([10, 0, 0, 2]).await;
    support::device_at(&lab.app, "CORE1", "10.0.0.2").await;

    let answer = tokio::time::timeout(
        PATIENCE,
        lab.app.rpc(
            "probe.dns",
            json!({"name": "core1.lab.example", "kind": "A", "server": format!("127.0.0.1:{port}")}),
        ),
    )
    .await
    .expect("the responder answers")
    .unwrap();

    assert_eq!(answer["rcode"], "NOERROR");
    assert_eq!(answer["records"][0]["data"], "10.0.0.2");
    assert_eq!(answer["records"][0]["kind"], "A");
    assert_eq!(answer["from"], "this computer");
    // The address in the answer is one Nocturne manages, and it says whose.
    assert_eq!(answer["devices"][0]["name"], "CORE1");

    let run = lab
        .app
        .rpc("probe.history", json!({"kind": "dns"}))
        .await
        .unwrap();
    assert_eq!(run["runs"][0]["ok"], true);
    assert!(
        run["runs"][0]["summary"]
            .as_str()
            .unwrap()
            .contains("NOERROR"),
        "{}",
        run["runs"][0]["summary"]
    );

    // A name that is not a name is refused before anything is sent.
    assert_eq!(
        lab.app
            .rpc("probe.dns", json!({"name": "not a name; reload"}))
            .await
            .unwrap_err()
            .code,
        "invalid"
    );
}

#[tokio::test]
async fn whois_follows_the_server_it_is_pointed_at_and_summarises_the_answer() {
    let lab = support::lab(&[]).await;
    let port = support::whois_server(
        "NetRange:       10.0.0.0 - 10.255.255.255\nCIDR:           10.0.0.0/8\nNetName:        LAB-NET\nOrgName:        Example Industries\nCountry:        GB\nOrgAbuseEmail:  abuse@example.com\n",
    )
    .await;

    let answer = tokio::time::timeout(
        PATIENCE,
        lab.app.rpc(
            "probe.whois",
            json!({"query": "10.0.0.1", "server": format!("127.0.0.1:{port}")}),
        ),
    )
    .await
    .expect("the server answers")
    .unwrap();

    assert!(answer["text"].as_str().unwrap().contains("LAB-NET"));
    let summary: Vec<(String, String)> = serde_json::from_value(answer["summary"].clone()).unwrap();
    assert!(
        summary
            .iter()
            .any(|(field, value)| field == "netname" && value == "LAB-NET"),
        "{summary:?}"
    );
    assert_eq!(answer["from"], "this computer");

    let run = lab
        .app
        .rpc("probe.history", json!({"kind": "whois"}))
        .await
        .unwrap();
    assert_eq!(run["runs"][0]["ok"], true);

    assert_eq!(
        lab.app
            .rpc("probe.whois", json!({"query": ""}))
            .await
            .unwrap_err()
            .code,
        "invalid"
    );
}

#[tokio::test]
async fn an_http_check_times_every_phase_of_a_real_request() {
    let lab = support::lab(&[]).await;
    let port = support::http_server(
        "HTTP/1.1 200 OK\r\nServer: test\r\nContent-Type: text/plain\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
    )
    .await;

    let report = tokio::time::timeout(
        PATIENCE,
        lab.app.rpc(
            "probe.http",
            json!({"url": format!("http://127.0.0.1:{port}/health"), "timeout_ms": 5000}),
        ),
    )
    .await
    .expect("the server answers")
    .unwrap();

    assert_eq!(report["status"], 200);
    assert_eq!(report["reason"], "OK");
    assert_eq!(report["body_bytes"], 5);
    assert_eq!(report["body_preview"], "hello");
    assert!(report["total_ms"].as_f64().unwrap() >= 0.0);
    assert!(
        report["headers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|header| header[0].as_str().unwrap().eq_ignore_ascii_case("server")),
    );
    assert_eq!(report["from"], "this computer");

    let run = lab
        .app
        .rpc("probe.history", json!({"kind": "http"}))
        .await
        .unwrap();
    assert!(
        run["runs"][0]["summary"]
            .as_str()
            .unwrap()
            .starts_with("200 OK"),
        "{}",
        run["runs"][0]["summary"]
    );

    // Nothing is listening: a failure with a reason, and a record of it.
    let closed = support::closed_port().await;
    let failed = lab
        .app
        .rpc(
            "probe.http",
            json!({"url": format!("http://127.0.0.1:{closed}/")}),
        )
        .await
        .unwrap_err();
    assert!(
        ["unavailable", "timeout"].contains(&failed.code),
        "{failed:?}"
    );
    let runs = lab
        .app
        .rpc("probe.history", json!({"kind": "http"}))
        .await
        .unwrap();
    assert_eq!(runs["runs"][0]["ok"], false);
    assert_eq!(runs["runs"][0]["result"], Value::Null);
}

#[tokio::test]
async fn an_ntp_query_reports_the_servers_own_numbers() {
    let lab = support::lab(&[]).await;
    let port = support::ntp_responder().await;

    let report = tokio::time::timeout(
        PATIENCE,
        lab.app.rpc(
            "probe.ntp",
            json!({"server": format!("127.0.0.1:{port}"), "timeout_ms": 3000}),
        ),
    )
    .await
    .expect("the responder answers")
    .unwrap();

    assert_eq!(report["stratum"], 2);
    assert_eq!(report["version"], 4);
    assert_eq!(report["leap"], "none");
    // The responder claims 10.0.0.1 upstream, 1/16 s root delay.
    assert_eq!(report["reference"], "10.0.0.1");
    assert_eq!(report["root_delay_ms"], 62.5);
    assert!(
        report["offset_ms"].as_f64().unwrap().abs() < 5000.0,
        "{report}"
    );
    assert_eq!(report["from"], "this computer");

    let run = lab
        .app
        .rpc("probe.history", json!({"kind": "ntp"}))
        .await
        .unwrap();
    assert!(
        run["runs"][0]["summary"]
            .as_str()
            .unwrap()
            .contains("stratum 2"),
        "{}",
        run["runs"][0]["summary"]
    );

    // A port with no NTP server behind it is a refusal, not a time.
    let closed = support::closed_port().await;
    let failed = lab
        .app
        .rpc(
            "probe.ntp",
            json!({"server": format!("127.0.0.1:{closed}"), "timeout_ms": 700}),
        )
        .await
        .unwrap_err();
    assert!(
        ["unavailable", "timeout"].contains(&failed.code),
        "{failed:?}"
    );
}

#[tokio::test]
async fn a_tls_inspection_of_something_that_is_not_tls_fails_rather_than_inventing_a_certificate() {
    let lab = support::lab(&[]).await;
    let port = support::silent_listener().await;

    let failed = tokio::time::timeout(
        PATIENCE,
        lab.app.rpc(
            "probe.tls",
            json!({"host": "127.0.0.1", "port": port, "timeout_ms": 2000}),
        ),
    )
    .await
    .expect("it gives up")
    .unwrap_err();
    assert!(
        ["unavailable", "timeout", "invalid"].contains(&failed.code),
        "{failed:?}"
    );

    let runs = lab
        .app
        .rpc("probe.history", json!({"kind": "tls"}))
        .await
        .unwrap();
    let run = &runs["runs"][0];
    assert_eq!(run["ok"], false);
    assert_eq!(run["result"], Value::Null);
    assert!(run["error"].as_str().unwrap().len() > 5, "{run}");

    assert_eq!(
        lab.app
            .rpc("probe.tls", json!({"host": "127.0.0.1", "port": 0}))
            .await
            .unwrap_err()
            .code,
        "invalid"
    );
}

#[tokio::test]
async fn a_sweep_finds_loopback_through_the_port_it_was_told_about() {
    let lab = support::lab(&[]).await;
    let mut events = support::Events::new(&lab.app);
    let open = support::banner_listener("hello\r\n").await;
    support::device_at(&lab.app, "LOOP1", "127.0.0.1").await;

    let job = support::run_job(
        &lab.app,
        "probe.sweep",
        json!({"targets": "127.0.0.1", "ports": open.to_string(), "timeout_ms": 1500, "resolve_names": false}),
    )
    .await
    .unwrap();
    assert_eq!(job.state, JobState::Succeeded);
    let result = job.result.unwrap();
    assert_eq!(result["planned"], 1);
    assert_eq!(result["alive"], 1);
    let host = &result["hosts"][0];
    assert_eq!(host["address"], "127.0.0.1");
    assert!(
        host["via"] == "icmp" || host["via"] == format!("tcp/{open}"),
        "{host}"
    );
    // The sweep says whose address it found.
    assert_eq!(host["device"]["name"], "LOOP1");

    let streamed = events.drain("probe.sweep.host");
    assert_eq!(streamed.len(), 1);
    assert_eq!(streamed[0]["host"]["address"], "127.0.0.1");
    assert_eq!(streamed[0]["device"]["name"], "LOOP1");

    let run = lab
        .app
        .rpc("probe.history", json!({"kind": "sweep"}))
        .await
        .unwrap();
    assert!(
        run["runs"][0]["summary"]
            .as_str()
            .unwrap()
            .contains("1 of 1 addresses"),
        "{}",
        run["runs"][0]["summary"]
    );
}

#[tokio::test]
async fn a_sweep_of_public_space_needs_the_settings_and_the_request_to_agree() {
    let lab = support::lab(&[]).await;
    // Asked for without the installation allowing it: refused outright.
    let refused = lab
        .app
        .rpc(
            "probe.sweep",
            json!({"targets": "8.8.8.0/30", "allow_public": true}),
        )
        .await
        .unwrap_err();
    assert_eq!(refused.code, "forbidden");
    assert!(refused.message.contains("Settings"), "{}", refused.message);

    // Not asked for at all: also refused, and the refusal says what to ask for.
    let quiet = lab
        .app
        .rpc("probe.sweep", json!({"targets": "8.8.8.0/30"}))
        .await
        .unwrap_err();
    assert_eq!(quiet.code, "forbidden");
    assert_eq!(quiet.details.unwrap()["needs"], "allow_public");

    // With the settings changed, the request decides — and nothing was sent
    // before it was allowed, which is why these refusals are not jobs.
    lab.app
        .rpc(
            "settings.set",
            json!({"section": "toolbox", "value": {"sweep": {"allow_public": true, "max_hosts": 4}}}),
        )
        .await
        .unwrap();
    let too_many = lab
        .app
        .rpc("probe.sweep", json!({"targets": "10.0.0.0/24"}))
        .await
        .unwrap_err();
    assert_eq!(too_many.code, "invalid");
    assert!(
        too_many.message.contains('4'),
        "the cap is named: {}",
        too_many.message
    );
}

#[tokio::test]
async fn this_machines_own_network_is_reported_and_says_which_machine_it_is() {
    let lab = support::lab(&[]).await;
    let report = lab.app.rpc("probe.local", Value::Null).await.unwrap();
    assert_eq!(report["from"], "this computer");
    let interfaces = report["interfaces"].as_array().unwrap();
    assert!(
        interfaces
            .iter()
            .any(|interface| interface["is_loopback"] == true),
        "no loopback interface: {interfaces:?}"
    );
    assert!(report["neighbours"].is_array() && report["gateways"].is_array());

    // The same question from a browser is about the server, and says so.
    let server = support::server_mode();
    assert_eq!(
        server.rpc("probe.local", Value::Null).await.unwrap()["from"],
        "the Nocturne server"
    );
}

#[tokio::test]
async fn wake_on_lan_checks_the_address_before_it_sends_anything() {
    let lab = support::lab(&[]).await;
    assert_eq!(
        lab.app
            .rpc("probe.wol", json!({"mac": "not a mac"}))
            .await
            .unwrap_err()
            .code,
        "invalid"
    );
    let multicast = lab
        .app
        .rpc("probe.wol", json!({"mac": "01:00:5e:00:00:01"}))
        .await
        .unwrap_err();
    assert_eq!(multicast.code, "invalid");
    assert!(
        multicast.message.contains("multicast"),
        "{}",
        multicast.message
    );

    // Sending may or may not be allowed on this machine; either way the
    // record says what happened, and the address is normalised.
    match lab
        .app
        .rpc("probe.wol", json!({"mac": "0000.0c12.3456"}))
        .await
    {
        Ok(result) => {
            assert_eq!(result["mac"], "00:00:0c:12:34:56");
            assert!(result["sent_to"].is_array());
        }
        Err(error) => eprintln!("this machine cannot broadcast: {error}"),
    }
    let runs = lab
        .app
        .rpc("probe.history", json!({"kind": "wol"}))
        .await
        .unwrap();
    assert_eq!(runs["total"], 1);
    assert_ne!(runs["runs"][0]["ok"], Value::Null);
}
