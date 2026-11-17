//! Auto-login: answering the login prompts of a console, once, and never
//! typing a password at anything that is not a login prompt.
//!
//! The consoles here are scripted `Connection::pair` devices, so the whole
//! path is exercised — settings, the credential, the profile's prompt
//! patterns, the session pump — not just the state machine.

mod common;

use std::time::Duration;

use common::{Events, SECRET, Script, open_device, scripted, session_id};
use serde_json::json;

/// Longer than the quarter second a prompt has to stand still, with room for
/// a loaded machine.
const SETTLED: Duration = Duration::from_millis(1500);

/// How long the quiet device below stays quiet.
///
/// The test watches part of that silence and asserts that nothing was typed
/// into it. That only means anything while the silence lasts, so it has to be
/// much longer than the window being watched: with the two a factor of two
/// apart, a sleep that overshot — which a runner with every core busy does —
/// ran past the end of the silence, the real login dialogue began, the
/// password was typed exactly as it should be, and the test called that a
/// leak.
const SILENCE: Duration = Duration::from_secs(6);

/// Everything the console was told, once it has had time to be told it.
async fn lines_after_settling(heard: &common::Heard) -> Vec<String> {
    tokio::time::sleep(SETTLED).await;
    heard.lines()
}

#[tokio::test]
async fn the_user_name_and_password_are_answered_once_each() {
    let lab = scripted(
        &[(
            "CON1",
            Script {
                banner: "\r\nUser Access Verification\r\n\r\n".into(),
                ..Script::default()
            },
        )],
        true,
    )
    .await;
    let app = &lab.app;
    let mut events = Events::new(app);
    let id = session_id(&open_device(app, &lab.id("CON1")).await.expect("open"));

    // The session reaches the command prompt without anybody typing.
    events.wait_for(&id, "SW1>").await;
    assert_eq!(lab.heard("CON1").lines(), ["admin", SECRET]);

    // Nothing else is answered afterwards: the session is the user's.
    app.rpc("terminal.input", json!({"session_id": id, "data": "\r"}))
        .await
        .expect("input");
    assert_eq!(
        lines_after_settling(&lab.heard("CON1")).await,
        ["admin", SECRET, ""]
    );

    // And the password is in no event the UI ever sees.
    events.drain();
    assert!(!events.any_payload_contains(SECRET), "the password leaked");
}

#[tokio::test]
async fn a_banner_that_mentions_a_password_is_not_a_password_prompt() {
    let lab = scripted(
        &[
            // The word is in the middle of a line that ends in the real
            // user-name prompt, all in one write.
            (
                "INLINE",
                Script {
                    banner: "\r\nUnauthorised use is an offence. Password: policy is 90 days.\r\n"
                        .into(),
                    ..Script::default()
                },
            ),
            // The banner's last line ends with the word and the device then
            // falls silent for much longer than a prompt has to stand still —
            // long enough that the window the test watches during cannot run
            // past the end of the silence on a machine whose sleeps overshoot.
            (
                "PAUSED",
                Script {
                    banner: "\r\nNOTICE: rotate your Password:\r\n".into(),
                    after_pause: Some((SILENCE, String::new())),
                    ..Script::default()
                },
            ),
        ],
        true,
    )
    .await;
    let app = &lab.app;
    let mut events = Events::new(app);

    let inline = session_id(&open_device(app, &lab.id("INLINE")).await.expect("open"));
    events.wait_for(&inline, "SW1>").await;
    assert_eq!(lab.heard("INLINE").lines(), ["admin", SECRET]);

    let paused = session_id(&open_device(app, &lab.id("PAUSED")).await.expect("open"));
    // While the device is quiet after that banner, nothing may be typed. The
    // window is watched throughout rather than only at the end, so the first
    // thing typed is caught whenever in the silence it happens — and it is
    // comfortably shorter than the silence, so reaching the end of it is not
    // a race with the login dialogue starting.
    let watch_until = tokio::time::Instant::now() + SETTLED;
    while tokio::time::Instant::now() < watch_until {
        assert!(
            lab.heard("PAUSED").lines().is_empty(),
            "answered a banner: {:?}",
            lab.heard("PAUSED").lines()
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    events.wait_for(&paused, "SW1>").await;
    assert_eq!(lab.heard("PAUSED").lines(), ["admin", SECRET]);
}

#[tokio::test]
async fn a_console_that_echoes_is_answered_exactly_once() {
    let lab = scripted(
        &[(
            "CON1",
            Script {
                echo: true,
                ..Script::default()
            },
        )],
        true,
    )
    .await;
    let app = &lab.app;
    let mut events = Events::new(app);
    let id = session_id(&open_device(app, &lab.id("CON1")).await.expect("open"));

    events.wait_for(&id, "SW1>").await;
    // The echo of the answered user-name prompt is not mistaken for a new one.
    assert_eq!(
        lines_after_settling(&lab.heard("CON1")).await,
        ["admin", SECRET]
    );
    assert_eq!(lab.heard("CON1").count("admin"), 1);
    assert_eq!(lab.heard("CON1").count(SECRET), 1);
}

#[tokio::test]
async fn a_console_that_asks_twice_is_answered_once() {
    let lab = scripted(
        &[(
            "CON1",
            Script {
                refusals: 1,
                ..Script::default()
            },
        )],
        true,
    )
    .await;
    let app = &lab.app;
    let mut events = Events::new(app);
    let id = session_id(&open_device(app, &lab.id("CON1")).await.expect("open"));

    // The device refuses the first attempt and asks again. Guessing on would
    // spend the account's second attempt, so the session is handed over.
    events.wait_for(&id, "% Login invalid").await;
    assert_eq!(
        lines_after_settling(&lab.heard("CON1")).await,
        ["admin", SECRET]
    );

    // The user can still finish the login by hand.
    app.rpc(
        "terminal.input",
        json!({"session_id": id, "data": "operator\r"}),
    )
    .await
    .expect("input");
    assert_eq!(
        lines_after_settling(&lab.heard("CON1")).await,
        ["admin", SECRET, "operator"]
    );
}

#[tokio::test]
async fn a_password_prompt_that_is_not_a_login_is_left_alone() {
    for banner in [
        "\r\nEnter new password: ",
        "\r\nYour password has expired.\r\nNew Password: ",
        "\r\nRetype password: ",
        "\r\nEnter enable password: ",
    ] {
        let lab = scripted(
            &[(
                "CON1",
                Script {
                    banner: banner.to_string(),
                    listen_only: true,
                    ..Script::default()
                },
            )],
            true,
        )
        .await;
        let app = &lab.app;
        let mut events = Events::new(app);
        let id = session_id(&open_device(app, &lab.id("CON1")).await.expect("open"));
        events.wait_for(&id, "password").await;
        assert_eq!(
            lines_after_settling(&lab.heard("CON1")).await,
            Vec::<String>::new(),
            "answered {banner:?}"
        );
    }
}

#[tokio::test]
async fn a_command_prompt_ends_the_watch_before_any_password_is_typed() {
    // A console that is already logged in, then asks for the enable password.
    let lab = scripted(
        &[(
            "CON1",
            Script {
                banner: "\r\nSW1>".into(),
                after_pause: Some((SETTLED, "enable\r\nPassword: ".into())),
                listen_only: true,
                ..Script::default()
            },
        )],
        true,
    )
    .await;
    let app = &lab.app;
    let mut events = Events::new(app);
    let id = session_id(&open_device(app, &lab.id("CON1")).await.expect("open"));
    events.wait_for(&id, "Password: ").await;
    assert_eq!(
        lines_after_settling(&lab.heard("CON1")).await,
        Vec::<String>::new()
    );
}

#[tokio::test]
async fn what_the_user_types_takes_the_session_over() {
    let lab = scripted(
        &[(
            "CON1",
            Script {
                banner: "\r\n".into(),
                after_pause: Some((SETTLED, String::new())),
                ..Script::default()
            },
        )],
        true,
    )
    .await;
    let app = &lab.app;
    let mut events = Events::new(app);
    let id = session_id(&open_device(app, &lab.id("CON1")).await.expect("open"));

    // The user gets there first; the login prompts are theirs from now on.
    app.rpc(
        "terminal.input",
        json!({"session_id": id, "data": "operator\r"}),
    )
    .await
    .expect("input");
    events.wait_for(&id, "Password: ").await;
    assert_eq!(
        lines_after_settling(&lab.heard("CON1")).await,
        ["operator"],
        "auto-login typed after the user had"
    );
}

#[tokio::test]
async fn a_bare_return_to_wake_a_console_does_not_count_as_typing() {
    // The console is not listening yet, as a serial console that has to be
    // woken is not, so the Return is swallowed rather than taken for a login.
    let lab = scripted(
        &[(
            "CON1",
            Script {
                after_pause: Some((Duration::from_millis(400), String::new())),
                wake_sink: true,
                ..Script::default()
            },
        )],
        true,
    )
    .await;
    let app = &lab.app;
    let mut events = Events::new(app);
    let id = session_id(&open_device(app, &lab.id("CON1")).await.expect("open"));
    app.rpc("terminal.input", json!({"session_id": id, "data": "\r"}))
        .await
        .expect("input");
    events.wait_for(&id, "SW1>").await;
    assert_eq!(lab.heard("CON1").lines(), ["admin", SECRET]);
    assert!(
        lab.heard("CON1")
            .bytes
            .load(std::sync::atomic::Ordering::Acquire)
            > 0
    );
}

#[tokio::test]
async fn nothing_is_typed_without_a_credential_or_with_the_setting_off() {
    let without = scripted(&[("CON1", Script::default())], false).await;
    let mut events = Events::new(&without.app);
    let id = session_id(
        &open_device(&without.app, &without.id("CON1"))
            .await
            .expect("open"),
    );
    events.wait_for(&id, "Username: ").await;
    assert_eq!(
        lines_after_settling(&without.heard("CON1")).await,
        Vec::<String>::new()
    );

    let off = scripted(&[("CON1", Script::default())], true).await;
    off.app
        .rpc(
            "settings.set",
            json!({"section": "terminal", "value": {"auto_login": false}}),
        )
        .await
        .expect("settings");
    let mut events = Events::new(&off.app);
    let id = session_id(&open_device(&off.app, &off.id("CON1")).await.expect("open"));
    events.wait_for(&id, "Username: ").await;
    assert_eq!(
        lines_after_settling(&off.heard("CON1")).await,
        Vec::<String>::new()
    );
}
