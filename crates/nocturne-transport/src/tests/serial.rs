//! Serial connections, over a pseudo-terminal standing in for the cable.

#![cfg(unix)]

use std::io::{Read, Write};
use std::time::Duration;

use serialport::{SerialPort, TTYPort};

use super::support::{Tap, soon, tap};
use crate::{Connection, FlowControl, Parity, SerialParams, TransportKind, connect_serial};

/// A pseudo-terminal pair: the test holds the "device" end, the path names
/// the end `connect_serial` opens.
fn cable() -> (TTYPort, String) {
    let (mut device, port) = TTYPort::pair().expect("cannot create a pseudo-terminal");
    device.set_timeout(Duration::from_secs(10)).unwrap();
    let path = port.name().expect("the pseudo-terminal has no name");
    drop(port);
    (device, path)
}

/// Opens the far end of the cable, or `None` where a pseudo-terminal cannot
/// stand in for a serial port.
///
/// This whole file rests on that substitution. Linux allows the far end of a
/// pty pair to be opened by its path and driven with serial line settings;
/// macOS refuses with ENOTTY — "Not a typewriter" — so the stand-in does not
/// hold there and six tests would fail for one reason that is not about
/// `connect_serial` at all. Serial ports themselves are not emulated by
/// anybody's CI, so this is as close as the suite gets on any platform.
async fn open(path: &str) -> Option<Tap<Connection>> {
    let params = SerialParams {
        port: path.to_owned(),
        baud: 115_200,
        ..SerialParams::default()
    };
    match soon(connect_serial(&params)).await {
        Ok(connection) => Some(tap(connection)),
        Err(error) => {
            eprintln!(
                "skipping: a pseudo-terminal cannot be opened as a serial port here ({error})"
            );
            None
        }
    }
}

/// Reads exactly `count` bytes from the device end.
async fn device_read(device: TTYPort, count: usize) -> (TTYPort, Vec<u8>) {
    soon(tokio::task::spawn_blocking(move || {
        let mut device = device;
        let mut received = vec![0u8; count];
        device.read_exact(&mut received).unwrap();
        (device, received)
    }))
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn data_flows_both_ways() {
    let (mut device, path) = cable();
    let Some(mut conn) = open(&path).await else {
        return;
    };
    assert_eq!(conn.info().kind, Some(TransportKind::Serial));
    assert_eq!(conn.info().peer, path);

    device.write_all(b"Router>").unwrap();
    assert_eq!(conn.until(b">").await, b"Router>");

    conn.send("enable\r").await.unwrap();
    let (device, received) = device_read(device, 7).await;
    assert_eq!(received, b"enable\r");

    // Raw bytes pass unchanged in both directions.
    let binary: Vec<u8> = (0..=255).collect();
    conn.send(binary.clone()).await.unwrap();
    let (mut device, received) = device_read(device, 256).await;
    assert_eq!(received, binary);
    device.write_all(&binary).unwrap();
    assert_eq!(conn.exactly(256).await, binary);

    // A resize means nothing to a serial line and must not disturb it.
    conn.resize(132, 50).await.unwrap();
    conn.send("x").await.unwrap();
    let (_device, received) = device_read(device, 1).await;
    assert_eq!(received, b"x");
    conn.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn break_holds_the_line_for_a_quarter_second() {
    let (device, path) = cable();
    let Some(conn) = open(&path).await else {
        return;
    };
    assert!(conn.supports_break());

    let started = std::time::Instant::now();
    conn.send_break().await.unwrap();
    // The break is carried out by the writing thread: what is sent next
    // arrives only once it is over.
    conn.send("after").await.unwrap();
    let (_device, received) = device_read(device, 5).await;
    assert_eq!(received, b"after");
    assert!(
        started.elapsed() >= Duration::from_millis(240),
        "{:?}",
        started.elapsed()
    );
    conn.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_releases_the_port() {
    let (mut device, path) = cable();
    let Some(conn) = open(&path).await else {
        return;
    };
    // While it is open, the port is ours alone.
    let params = SerialParams {
        port: path.clone(),
        ..SerialParams::default()
    };
    let busy = soon(connect_serial(&params)).await.unwrap_err();
    assert_eq!(busy.code(), "io", "{busy}");

    conn.close().await;
    // No waiting, no retry: closed means released.
    let Some(mut again) = open(&path).await else {
        return;
    };
    device.write_all(b"still here").unwrap();
    assert_eq!(again.until(b"here").await, b"still here");

    let (writer, reader) = again.inner.split();
    let mut reader = tap(reader);
    writer.close().await;
    let (_, reason) = reader.until_closed().await;
    assert_eq!(reason, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unplugging_the_device_ends_the_session() {
    let (device, path) = cable();
    let Some(mut conn) = open(&path).await else {
        return;
    };
    drop(device);
    let (_, reason) = conn.until_closed().await;
    let reason = reason.unwrap();
    assert!(reason.starts_with("serial port error"), "{reason}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn line_settings_are_applied() {
    let (_device, path) = cable();
    let params = SerialParams {
        port: path,
        baud: 19_200,
        data_bits: 7,
        parity: Parity::Even,
        stop_bits: 2,
        flow: FlowControl::Software,
    };
    let Ok(conn) = soon(connect_serial(&params)).await else {
        eprintln!("skipping: a pseudo-terminal cannot be opened as a serial port here");
        return;
    };
    soon(conn.close()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_burst_of_output_is_not_lost() {
    const TOTAL: usize = 200_000;
    let (device, path) = cable();
    let Some(mut conn) = open(&path).await else {
        return;
    };
    let writer = tokio::task::spawn_blocking(move || {
        let mut device = device;
        let data: Vec<u8> = (0..TOTAL).map(|index| (index % 251) as u8).collect();
        device.write_all(&data).unwrap();
        device
    });
    let received = conn.exactly(TOTAL).await;
    for (index, byte) in received.iter().enumerate() {
        assert_eq!(*byte, (index % 251) as u8, "byte {index} is wrong");
    }
    let _device = soon(writer).await.unwrap();
    conn.close().await;
}
