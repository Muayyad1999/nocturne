//! The TFTP server against the crate's own minimal test client (`support::tftp_client`):
//! reads, writes, option negotiation, block-number wraparound, path confinement,
//! retransmission behaviour and hostile datagrams.

mod support;

use std::net::SocketAddr;
use std::path::Path;
use std::time::{Duration, Instant};

use nocturne_netprobe::ProbeError;
use nocturne_netprobe::tftp::{TftpConfig, TftpEvent, TftpServer};
use support::tftp_client::{self as client, ClientError, ClientOptions, Received, Session};
use tokio::net::UdpSocket;
use tokio::sync::mpsc::UnboundedReceiver;

fn config(root: &Path) -> TftpConfig {
    TftpConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        root: root.to_path_buf(),
        allow_read: true,
        allow_write: true,
        allow_overwrite: false,
        timeout_ms: 2000,
        retries: 3,
        max_file_bytes: 64 * 1024 * 1024,
    }
}

async fn start(config: TftpConfig) -> (TftpServer, SocketAddr, UnboundedReceiver<TftpEvent>) {
    let (server, events) = TftpServer::start(config).await.unwrap();
    let address = server.local_addr();
    (server, address, events)
}

/// Bytes that make a misplaced or repeated block visible.
fn pattern(length: usize) -> Vec<u8> {
    (0..length)
        .map(|index| (index % 251) as u8 ^ (index / 251) as u8)
        .collect()
}

/// Waits for the next event that is not a progress report.
async fn next_event(events: &mut UnboundedReceiver<TftpEvent>) -> TftpEvent {
    loop {
        let event = tokio::time::timeout(Duration::from_secs(10), events.recv())
            .await
            .expect("an event within ten seconds")
            .expect("the server is running");
        if !matches!(event, TftpEvent::Progress { .. }) {
            return event;
        }
    }
}

#[tokio::test]
async fn reads_files_of_awkward_sizes() {
    let dir = tempfile::tempdir().unwrap();
    let (_server, address, mut events) = start(config(dir.path())).await;
    for size in [0usize, 1, 511, 512, 513, 1024, 5000] {
        let name = format!("file-{size}.bin");
        std::fs::write(dir.path().join(&name), pattern(size)).unwrap();
        let download = client::get(address, &name, &ClientOptions::default())
            .await
            .unwrap();
        assert_eq!(download.data, pattern(size), "size {size}");
        // A file that fills its last block exactly is followed by an empty block.
        assert_eq!(download.blocks, size as u64 / 512 + 1, "size {size}");
        assert_eq!(download.acknowledged, vec![], "no options were asked for");

        let TftpEvent::Started {
            id,
            peer,
            file,
            write,
            size: announced,
        } = next_event(&mut events).await
        else {
            panic!("expected Started");
        };
        assert!(peer.starts_with("127.0.0.1:"), "{peer}");
        assert_eq!(
            (file.as_str(), write, announced),
            (name.as_str(), false, Some(size as u64))
        );
        let finished = next_event(&mut events).await;
        assert!(
            matches!(finished, TftpEvent::Finished { id: done, bytes, .. } if done == id && bytes == size as u64),
            "{finished:?}"
        );
    }
}

#[tokio::test]
async fn negotiates_block_size_transfer_size_and_timeout() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("image.bin"), pattern(100_000)).unwrap();
    let (_server, address, _events) = start(config(dir.path())).await;
    let granted = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect()
    };

    let options = ClientOptions {
        blksize: Some(1428),
        tsize: true,
        timeout: Some(2),
    };
    let download = client::get(address, "image.bin", &options).await.unwrap();
    assert_eq!(download.data, pattern(100_000));
    assert_eq!(
        download.acknowledged,
        granted(&[("blksize", "1428"), ("timeout", "2"), ("tsize", "100000")])
    );
    assert_eq!(download.blocks, 100_000 / 1428 + 1);

    // The largest block size TFTP allows: the whole file in two datagrams.
    //
    // Only where the host will carry a datagram that size. Linux's loopback
    // takes 64 KiB; macOS refuses anything over `net.inet.udp.maxdgram`,
    // 9216 by default, with EMSGSIZE before it ever reaches the wire. That
    // is the host's limit and not this server's, and what the server does
    // with the option — grants it, below — is asserted either way.
    if host_sends_datagram(65_464).await {
        let jumbo = ClientOptions {
            blksize: Some(65_464),
            ..ClientOptions::default()
        };
        let download = client::get(address, "image.bin", &jumbo).await.unwrap();
        assert_eq!((download.data.len(), download.blocks), (100_000, 2));
    } else {
        eprintln!(
            "skipping the 65464-byte block size: this host will not send a datagram that large"
        );
    }

    // More than the maximum is granted as the maximum.
    let greedy = ClientOptions {
        blksize: Some(100_000),
        ..ClientOptions::default()
    };
    let download = client::get(address, "image.bin", &greedy).await.unwrap();
    assert_eq!(download.acknowledged, granted(&[("blksize", "65464")]));
    assert_eq!(download.data, pattern(100_000));

    // The smallest legal block size.
    let tiny = ClientOptions {
        blksize: Some(8),
        ..ClientOptions::default()
    };
    std::fs::write(dir.path().join("small.bin"), pattern(100)).unwrap();
    let download = client::get(address, "small.bin", &tiny).await.unwrap();
    assert_eq!((download.data, download.blocks), (pattern(100), 13));

    // Below the legal minimum the option is not acknowledged and the default applies.
    let illegal = ClientOptions {
        blksize: Some(4),
        ..ClientOptions::default()
    };
    let download = client::get(address, "small.bin", &illegal).await.unwrap();
    assert_eq!(download.acknowledged, vec![]);
    assert_eq!(download.blocks, 1);

    // Unknown options and odd spellings.
    let mut session = Session::open(address).await;
    let options = [
        ("windowsize", "16".to_owned()),
        ("BlkSize", "1024".to_owned()),
        ("tsize", "0".to_owned()),
    ];
    session
        .send(&client::request(
            client::RRQ,
            "image.bin",
            "OCTET",
            &options,
        ))
        .await;
    assert_eq!(
        session.receive().await.unwrap(),
        Received::OptionAck(granted(&[("blksize", "1024"), ("tsize", "100000")]))
    );
}

#[tokio::test]
async fn block_numbers_wrap_around_for_large_transfers() {
    let dir = tempfile::tempdir().unwrap();
    // 66 000 blocks of 8 bytes: past the 65 535 a 16-bit block number can count.
    let content = pattern(8 * 66_000);
    std::fs::write(dir.path().join("big.bin"), &content).unwrap();
    let mut settings = config(dir.path());
    settings.timeout_ms = 5000;
    let (_server, address, _events) = start(settings).await;
    let options = ClientOptions {
        blksize: Some(8),
        tsize: true,
        ..ClientOptions::default()
    };

    let download = client::get(address, "big.bin", &options).await.unwrap();
    assert_eq!(download.blocks, 66_001);
    assert!(download.data == content, "the downloaded bytes differ");

    let upload = client::put(address, "big-copy.bin", &content, &options)
        .await
        .unwrap();
    assert_eq!(upload.blocks, 66_001);
    // The server acknowledges the last block before the file is renamed into place.
    for _ in 0..100 {
        if dir.path().join("big-copy.bin").exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        std::fs::read(dir.path().join("big-copy.bin")).unwrap() == content,
        "the uploaded bytes differ"
    );
}

/// Waits until an upload's Finished (or Failed) event arrives: the file is in place by then.
async fn upload_outcome(events: &mut UnboundedReceiver<TftpEvent>) -> TftpEvent {
    loop {
        match next_event(events).await {
            TftpEvent::Started { .. } => continue,
            outcome => return outcome,
        }
    }
}

fn file_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn writes_files_and_announces_them() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("backups")).unwrap();
    let (_server, address, mut events) = start(config(dir.path())).await;

    for (name, size, options) in [
        ("sw1.cfg", 3000usize, ClientOptions::default()),
        (
            "backups/core-r1.cfg",
            70_000,
            ClientOptions {
                blksize: Some(1400),
                tsize: true,
                timeout: Some(3),
            },
        ),
        ("empty.cfg", 0, ClientOptions::default()),
        ("exact.bin", 1024, ClientOptions::default()),
    ] {
        let announces_size = options.tsize;
        let upload = client::put(address, name, &pattern(size), &options)
            .await
            .unwrap();
        if announces_size {
            assert!(
                upload
                    .acknowledged
                    .contains(&("tsize".to_owned(), size.to_string())),
                "{:?}",
                upload.acknowledged
            );
        }
        let TftpEvent::Started {
            id,
            file,
            write,
            size: announced,
            ..
        } = next_event(&mut events).await
        else {
            panic!("expected Started");
        };
        assert_eq!((file.as_str(), write), (name, true));
        assert_eq!(announced, announces_size.then_some(size as u64));
        let finished = next_event(&mut events).await;
        assert!(
            matches!(finished, TftpEvent::Finished { id: done, bytes, .. } if done == id && bytes == size as u64),
            "{name}: {finished:?}"
        );
        assert_eq!(
            std::fs::read(dir.path().join(name)).unwrap(),
            pattern(size),
            "{name}"
        );
    }
    // No temporary files are left behind.
    assert_eq!(
        file_names(dir.path()),
        ["backups", "empty.cfg", "exact.bin", "sw1.cfg"]
    );
    assert_eq!(file_names(&dir.path().join("backups")), ["core-r1.cfg"]);
}

#[tokio::test]
async fn existing_files_are_only_replaced_when_allowed() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("running.cfg"), "original").unwrap();
    let (_server, address, mut events) = start(config(dir.path())).await;
    let error = client::put(
        address,
        "running.cfg",
        b"replacement",
        &ClientOptions::default(),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&error, ClientError::Server { code: 6, .. }),
        "{error:?}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("running.cfg")).unwrap(),
        "original"
    );
    let denied = next_event(&mut events).await;
    assert!(
        matches!(&denied, TftpEvent::Denied { file, reason, .. } if file == "running.cfg" && reason.contains("already exists")),
        "{denied:?}"
    );

    let (_server, address, mut events) = start(TftpConfig {
        allow_overwrite: true,
        ..config(dir.path())
    })
    .await;
    client::put(
        address,
        "running.cfg",
        b"replacement",
        &ClientOptions::default(),
    )
    .await
    .unwrap();
    assert!(matches!(
        upload_outcome(&mut events).await,
        TftpEvent::Finished { bytes: 11, .. }
    ));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("running.cfg")).unwrap(),
        "replacement"
    );
}

#[tokio::test]
async fn uploads_larger_than_the_limit_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (_server, address, mut events) = start(TftpConfig {
        max_file_bytes: 1000,
        ..config(dir.path())
    })
    .await;

    // Announced size over the limit: refused before any data moves.
    let announced = ClientOptions {
        tsize: true,
        ..ClientOptions::default()
    };
    let error = client::put(address, "big.bin", &pattern(5000), &announced)
        .await
        .unwrap_err();
    assert!(
        matches!(&error, ClientError::Server { code: 3, .. }),
        "{error:?}"
    );
    assert!(matches!(
        next_event(&mut events).await,
        TftpEvent::Denied { .. }
    ));

    // Not announced: cut off when the limit is crossed, and nothing is left on disk.
    let error = client::put(
        address,
        "big.bin",
        &pattern(5000),
        &ClientOptions::default(),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&error, ClientError::Server { code: 3, .. }),
        "{error:?}"
    );
    let failed = upload_outcome(&mut events).await;
    assert!(
        matches!(&failed, TftpEvent::Failed { error, .. } if error.contains("limit of 1000 bytes")),
        "{failed:?}"
    );
    assert_eq!(file_names(dir.path()), Vec::<String>::new());

    // Exactly at the limit is fine.
    client::put(
        address,
        "fits.bin",
        &pattern(1000),
        &ClientOptions::default(),
    )
    .await
    .unwrap();
    assert!(matches!(
        upload_outcome(&mut events).await,
        TftpEvent::Finished { bytes: 1000, .. }
    ));
}

#[tokio::test]
async fn nothing_outside_the_root_can_be_read_or_written() {
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.txt"), "enable secret 5 $1$abc").unwrap();
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("configs")).unwrap();
    std::fs::write(dir.path().join("configs/sw1.cfg"), "hostname sw1").unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(
            outside.path().join("secret.txt"),
            dir.path().join("link-to-secret"),
        )
        .unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("link-to-outside")).unwrap();
    }
    let (_server, address, mut events) = start(TftpConfig {
        allow_overwrite: true,
        ..config(dir.path())
    })
    .await;
    let secret = outside.path().join("secret.txt").display().to_string();
    let relative_escape = format!(
        "../{}/secret.txt",
        outside.path().file_name().unwrap().to_string_lossy()
    );

    let mut attempts: Vec<String> = [
        "/etc/passwd",
        "../secret.txt",
        "configs/../../secret.txt",
        "configs\\..\\..\\secret.txt",
        "..\\..\\..\\..\\windows\\win.ini",
        "C:\\windows\\win.ini",
        "\\\\server\\share\\file",
        "NUL",
        "con.txt",
        "configs/COM1",
        "",
        "configs",
        ".",
        "..",
    ]
    .map(str::to_owned)
    .to_vec();
    attempts.push(secret.clone());
    attempts.push(relative_escape);
    if cfg!(unix) {
        attempts.push("link-to-secret".to_owned());
        attempts.push("link-to-outside/secret.txt".to_owned());
    }
    for name in &attempts {
        let read = client::get(address, name, &ClientOptions::default())
            .await
            .unwrap_err();
        assert!(
            matches!(&read, ClientError::Server { code: 2, .. }),
            "read {name:?}: {read:?}"
        );
        let event = next_event(&mut events).await;
        assert!(
            matches!(&event, TftpEvent::Denied { .. }),
            "read {name:?}: {event:?}"
        );

        let write = client::put(
            address,
            name,
            b"overwritten by an attacker",
            &ClientOptions::default(),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(&write, ClientError::Server { code: 2, .. }),
            "write {name:?}: {write:?}"
        );
        assert!(matches!(
            next_event(&mut events).await,
            TftpEvent::Denied { .. }
        ));
    }
    assert_eq!(
        std::fs::read_to_string(&secret).unwrap(),
        "enable secret 5 $1$abc"
    );
    assert_eq!(file_names(outside.path()), ["secret.txt"]);

    // What is inside the root still works, and a missing file is "not found", not "denied access".
    let download = client::get(address, "configs/sw1.cfg", &ClientOptions::default())
        .await
        .unwrap();
    assert_eq!(download.data, b"hostname sw1");
    let missing = client::get(address, "configs/sw2.cfg", &ClientOptions::default())
        .await
        .unwrap_err();
    assert_eq!(
        missing,
        ClientError::Server {
            code: 1,
            message: "file not found".to_owned()
        }
    );
}

#[tokio::test]
async fn reading_and_writing_can_be_switched_off() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("sw1.cfg"), "hostname sw1").unwrap();

    let read_only = TftpConfig {
        allow_write: false,
        ..config(dir.path())
    };
    let (_server, address, _events) = start(read_only).await;
    assert!(
        client::get(address, "sw1.cfg", &ClientOptions::default())
            .await
            .is_ok()
    );
    let error = client::put(address, "new.cfg", b"x", &ClientOptions::default())
        .await
        .unwrap_err();
    assert!(
        matches!(&error, ClientError::Server { code: 2, message } if message.contains("uploads are disabled")),
        "{error:?}"
    );

    let write_only = TftpConfig {
        allow_read: false,
        ..config(dir.path())
    };
    let (_server, address, _events) = start(write_only).await;
    assert!(
        client::put(address, "new.cfg", b"x", &ClientOptions::default())
            .await
            .is_ok()
    );
    let error = client::get(address, "sw1.cfg", &ClientOptions::default())
        .await
        .unwrap_err();
    assert!(
        matches!(&error, ClientError::Server { code: 2, message } if message.contains("downloads are disabled")),
        "{error:?}"
    );
}

#[tokio::test]
async fn many_transfers_run_at_once_each_on_its_own_port() {
    let dir = tempfile::tempdir().unwrap();
    for index in 0..40usize {
        std::fs::write(
            dir.path().join(format!("f{index}.bin")),
            pattern(20_000 + index),
        )
        .unwrap();
    }
    let (_server, address, mut events) = start(config(dir.path())).await;
    let mut tasks = tokio::task::JoinSet::new();
    for index in 0..40usize {
        tasks.spawn(async move {
            if index % 2 == 0 {
                let download =
                    client::get(address, &format!("f{index}.bin"), &ClientOptions::default())
                        .await
                        .unwrap();
                assert_eq!(download.data, pattern(20_000 + index));
            } else {
                client::put(
                    address,
                    &format!("up{index}.bin"),
                    &pattern(15_000 + index),
                    &ClientOptions::default(),
                )
                .await
                .unwrap();
            }
        });
    }
    while let Some(result) = tasks.join_next().await {
        result.unwrap();
    }
    let mut finished = 0;
    let mut peers = std::collections::HashSet::new();
    while finished < 40 {
        match next_event(&mut events).await {
            TftpEvent::Started { peer, .. } => {
                peers.insert(peer);
            }
            TftpEvent::Finished { .. } => finished += 1,
            other => panic!("unexpected event {other:?}"),
        }
    }
    assert_eq!(peers.len(), 40);
    for index in (1..40usize).step_by(2) {
        assert_eq!(
            std::fs::read(dir.path().join(format!("up{index}.bin"))).unwrap(),
            pattern(15_000 + index)
        );
    }
}

#[tokio::test]
async fn a_silent_client_gets_retransmissions_and_is_then_given_up() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("three-blocks.bin"), pattern(1200)).unwrap();
    let settings = TftpConfig {
        timeout_ms: 150,
        retries: 2,
        ..config(dir.path())
    };
    let (_server, address, mut events) = start(settings).await;

    let mut session = Session::open(address).await;
    session
        .send(&client::request(
            client::RRQ,
            "three-blocks.bin",
            "octet",
            &[],
        ))
        .await;
    let first = session.receive().await.unwrap();
    assert_eq!(
        first,
        Received::Data {
            block: 1,
            data: pattern(1200)[..512].to_vec()
        }
    );
    // No acknowledgement: the same block comes again after the timeout.
    let sent = Instant::now();
    let again = session.receive().await.unwrap();
    assert_eq!(again, first);
    assert!(
        sent.elapsed() >= Duration::from_millis(120),
        "{:?}",
        sent.elapsed()
    );
    // Now answer, and the transfer carries on.
    session.send(&client::ack(1)).await;
    assert!(matches!(
        session.receive().await.unwrap(),
        Received::Data { block: 2, .. }
    ));
    // Go silent for good: 1 + 2 retransmissions, then the server gives up.
    let mut copies = 0;
    while let Some(Ok(Received::Data { block: 2, .. })) =
        session.receive_within(Duration::from_millis(600)).await
    {
        copies += 1;
    }
    assert_eq!(copies, 2, "two retransmissions after the first copy");
    assert!(matches!(
        next_event(&mut events).await,
        TftpEvent::Started { .. }
    ));
    let failed = next_event(&mut events).await;
    assert!(
        matches!(&failed, TftpEvent::Failed { error, .. } if error.contains("stopped answering")),
        "{failed:?}"
    );
}

#[tokio::test]
async fn duplicate_acknowledgements_do_not_duplicate_data() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("three-blocks.bin"), pattern(1200)).unwrap();
    let (_server, address, _events) = start(TftpConfig {
        timeout_ms: 5000,
        ..config(dir.path())
    })
    .await;

    let mut session = Session::open(address).await;
    session
        .send(&client::request(
            client::RRQ,
            "three-blocks.bin",
            "octet",
            &[],
        ))
        .await;
    assert!(matches!(
        session.receive().await.unwrap(),
        Received::Data { block: 1, .. }
    ));
    // The "sorcerer's apprentice" trigger: the same acknowledgement twice.
    session.send(&client::ack(1)).await;
    session.send(&client::ack(1)).await;
    assert!(matches!(
        session.receive().await.unwrap(),
        Received::Data { block: 2, .. }
    ));
    assert!(
        session
            .receive_within(Duration::from_millis(300))
            .await
            .is_none(),
        "block 2 must be sent once, not once per acknowledgement"
    );
    session.send(&client::ack(2)).await;
    let last = session.receive().await.unwrap();
    assert_eq!(
        last,
        Received::Data {
            block: 3,
            data: pattern(1200)[1024..].to_vec()
        }
    );
}

#[tokio::test]
async fn a_lost_acknowledgement_of_an_upload_is_sent_again() {
    let dir = tempfile::tempdir().unwrap();
    // A long retransmission timeout on purpose: the test below asserts that a
    // repeated block is acknowledged *at once* rather than after the server's
    // own timer, so the two have to be far enough apart that a busy machine
    // cannot be mistaken for the timer. At 1 s they were only 500 ms apart.
    let (_server, address, mut events) = start(TftpConfig {
        timeout_ms: 8_000,
        ..config(dir.path())
    })
    .await;
    let content = pattern(700);

    let mut session = Session::open(address).await;
    session
        .send(&client::request(client::WRQ, "upload.bin", "octet", &[]))
        .await;
    assert_eq!(session.receive().await.unwrap(), Received::Ack { block: 0 });
    session.send(&client::data(1, &content[..512])).await;
    assert_eq!(session.receive().await.unwrap(), Received::Ack { block: 1 });
    // Pretend the acknowledgement was lost and repeat the block: it is acknowledged again
    // right away, and it is not written twice.
    let repeated = Instant::now();
    session.send(&client::data(1, &content[..512])).await;
    assert_eq!(session.receive().await.unwrap(), Received::Ack { block: 1 });
    // Well inside the 8 s retransmission timeout: an acknowledgement that
    // waited for the timer could not land here.
    assert!(
        repeated.elapsed() < Duration::from_secs(4),
        "{:?}",
        repeated.elapsed()
    );

    session.send(&client::data(2, &content[512..])).await;
    assert_eq!(session.receive().await.unwrap(), Received::Ack { block: 2 });
    // The same for the final block, which the server waits around for.
    session.send(&client::data(2, &content[512..])).await;
    assert_eq!(session.receive().await.unwrap(), Received::Ack { block: 2 });

    assert!(matches!(
        upload_outcome(&mut events).await,
        TftpEvent::Finished { bytes: 700, .. }
    ));
    assert_eq!(
        std::fs::read(dir.path().join("upload.bin")).unwrap(),
        content
    );
}

#[tokio::test]
async fn a_client_error_ends_the_transfer() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("file.bin"), pattern(5000)).unwrap();
    let (_server, address, mut events) = start(config(dir.path())).await;
    let mut session = Session::open(address).await;
    session
        .send(&client::request(client::RRQ, "file.bin", "octet", &[]))
        .await;
    assert!(matches!(
        session.receive().await.unwrap(),
        Received::Data { block: 1, .. }
    ));
    // "Disk full" from the client.
    session.send(b"\x00\x05\x00\x03Disk full\x00").await;
    assert!(matches!(
        next_event(&mut events).await,
        TftpEvent::Started { .. }
    ));
    let failed = next_event(&mut events).await;
    assert!(
        matches!(&failed, TftpEvent::Failed { error, .. } if error.contains("Disk full") && error.contains("error 3")),
        "{failed:?}"
    );
}

#[tokio::test]
async fn unsupported_modes_and_stray_packets_get_the_right_errors() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("file.bin"), "content").unwrap();
    let (_server, address, _events) = start(config(dir.path())).await;

    let mut session = Session::open(address).await;
    session
        .send(&client::request(client::RRQ, "file.bin", "mail", &[]))
        .await;
    let error = session.receive().await.unwrap_err();
    assert!(
        matches!(&error, ClientError::Server { code: 4, .. }),
        "{error:?}"
    );

    // netascii is accepted and served as octet.
    let mut session = Session::open(address).await;
    session
        .send(&client::request(client::RRQ, "file.bin", "NetASCII", &[]))
        .await;
    assert_eq!(
        session.receive().await.unwrap(),
        Received::Data {
            block: 1,
            data: b"content".to_vec()
        }
    );

    // DATA and ACK sent to the request port belong to no transfer.
    for stray in [client::data(1, b"stray"), client::ack(7)] {
        let mut session = Session::open(address).await;
        session.send(&stray).await;
        let error = session.receive().await.unwrap_err();
        assert!(
            matches!(&error, ClientError::Server { code: 5, .. }),
            "{error:?}"
        );
    }

    // A request cut short.
    let mut session = Session::open(address).await;
    session.send(b"\x00\x01file.bin\x00oct").await;
    let error = session.receive().await.unwrap_err();
    assert!(
        matches!(&error, ClientError::Server { code: 4, .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn hostile_datagrams_do_not_disturb_the_server() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("file.bin"), pattern(3000)).unwrap();
    let (_server, address, _events) = start(config(dir.path())).await;
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();

    let mut state = 0x243f_6a88_85a3_08d3u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for round in 0..20_000u32 {
        let mut junk = vec![0u8; (next() % 600) as usize];
        for byte in &mut junk {
            *byte = next() as u8;
        }
        // Most of them carry a real opcode so that they get past the first check.
        if junk.len() >= 2 && round % 3 != 0 {
            junk[0] = 0;
            junk[1] = (round % 8) as u8;
        }
        socket.send_to(&junk, address).await.unwrap();
        if round % 512 == 0 {
            // Let the server keep up: its replies fill our receive buffer otherwise.
            let mut sink = [0u8; 2048];
            while socket.try_recv_from(&mut sink).is_ok() {}
            tokio::task::yield_now().await;
        }
    }
    // Truncations of a valid request, and oversized ones.
    let valid = client::request(
        client::RRQ,
        "file.bin",
        "octet",
        &[("blksize", "1024".to_owned())],
    );
    for cut in 0..valid.len() {
        socket.send_to(&valid[..cut], address).await.unwrap();
    }
    // A name far past any datagram's worth. Where the host refuses to send
    // something this big at all (macOS, EMSGSIZE) there is no hostile
    // datagram to test the server's nerve against, and the rest of the
    // hostile cases below still run.
    let huge_name = "a".repeat(60_000);
    let _ = socket
        .send_to(
            &client::request(client::RRQ, &huge_name, "octet", &[]),
            address,
        )
        .await;
    socket
        .send_to(
            &client::request(
                client::WRQ,
                "x",
                "octet",
                &[
                    ("blksize", "99999999999999999999".to_owned()),
                    ("tsize", "-1".to_owned()),
                    ("timeout", "0".to_owned()),
                ],
            ),
            address,
        )
        .await
        .unwrap();

    // Stop generating work for it: every refusal it sent us is sitting in our
    // receive buffer, and on Windows each one we never read becomes an ICMP
    // port-unreachable and then a receive error on *its* socket.
    let mut sink = [0u8; 2048];
    while socket.try_recv_from(&mut sink).is_ok() {}

    // The server is still there and still correct. "Still there" is the
    // claim, not "answers instantly": twenty thousand datagrams are a real
    // backlog, and on a slower machine working through them takes longer than
    // the ten seconds of patience the test client has — which is exactly how
    // this test failed on Windows while passing on Linux. Asking once and
    // demanding an answer inside that window tested the machine's speed. So
    // ask until it answers or the deadline passes, which still fails if the
    // server is genuinely stuck rather than merely behind.
    let began = std::time::Instant::now();
    let patience = Duration::from_secs(90);
    let download = loop {
        match client::get(address, "file.bin", &ClientOptions::default()).await {
            Ok(download) => break download,
            Err(error) => assert!(
                began.elapsed() < patience,
                "the server never came back after the flood ({:?} after {:?})",
                error,
                began.elapsed()
            ),
        }
    };
    assert_eq!(download.data, pattern(3000));
}

#[tokio::test]
async fn stopping_releases_the_port_and_removes_partial_uploads() {
    let dir = tempfile::tempdir().unwrap();
    let (server, address, mut events) = start(config(dir.path())).await;

    // An upload that is in the middle when the server stops.
    let mut session = Session::open(address).await;
    session
        .send(&client::request(client::WRQ, "half.bin", "octet", &[]))
        .await;
    assert_eq!(session.receive().await.unwrap(), Received::Ack { block: 0 });
    session.send(&client::data(1, &pattern(512))).await;
    assert_eq!(session.receive().await.unwrap(), Received::Ack { block: 1 });
    assert!(matches!(
        next_event(&mut events).await,
        TftpEvent::Started { .. }
    ));
    assert_eq!(
        file_names(dir.path()).len(),
        1,
        "the partial upload is a hidden temporary file"
    );
    assert!(!dir.path().join("half.bin").exists());

    server.stop().await;
    assert_eq!(
        file_names(dir.path()),
        Vec::<String>::new(),
        "the partial upload is removed"
    );
    // The port is free again at once.
    let rebound = UdpSocket::bind(address).await;
    assert!(rebound.is_ok(), "{rebound:?}");
    drop(rebound);

    // Dropping a server stops it as well.
    let (server, address, _events) = start(config(dir.path())).await;
    drop(server);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(UdpSocket::bind(address).await.is_ok());
}

#[tokio::test]
async fn a_busy_or_privileged_port_is_explained() {
    let dir = tempfile::tempdir().unwrap();
    let (_server, address, _events) = start(config(dir.path())).await;
    let error = TftpServer::start(TftpConfig {
        bind: address,
        ..config(dir.path())
    })
    .await
    .unwrap_err();
    assert!(matches!(&error, ProbeError::Io(_)), "{error:?}");
    assert!(
        error.to_string().contains(&address.to_string())
            && error.to_string().contains("already using it"),
        "{error}"
    );

    // Port 69 needs privileges; when this test runs without them the error must say what to do.
    match TftpServer::start(TftpConfig {
        bind: "127.0.0.1:69".parse().unwrap(),
        ..config(dir.path())
    })
    .await
    {
        Err(ProbeError::Permission(message)) => {
            assert!(
                message.contains("1024") && message.contains("CAP_NET_BIND_SERVICE"),
                "{message}"
            );
        }
        // Root (or a machine that allows low ports), or a TFTP server already on port 69.
        Ok((server, _events)) => server.stop().await,
        Err(ProbeError::Io(_)) => {}
        Err(other) => panic!("unexpected error {other:?}"),
    }
}

/// Whether this host will send a UDP datagram of `bytes` at all.
///
/// Linux's loopback carries a 64 KiB datagram; macOS refuses anything over
/// `net.inet.udp.maxdgram` — 9216 by default — with EMSGSIZE at the sender,
/// so a test that needs one that big is testing the host rather than this
/// code. Asked by trying, because the limit is a sysctl and not worth
/// reading.
async fn host_sends_datagram(bytes: usize) -> bool {
    let Ok(socket) = tokio::net::UdpSocket::bind("127.0.0.1:0").await else {
        return false;
    };
    let Ok(itself) = socket.local_addr() else {
        return false;
    };
    socket.send_to(&vec![0u8; bytes], itself).await.is_ok()
}
