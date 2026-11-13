//! The AI assistant: providers, chats, the tool boundary, redaction and autonomy.
//!
//! An engineer in the next chair who can read the whole network. It works
//! through the same RPC surface everything else does — it has no privileged
//! access of its own — and the set of methods its tools may reach is fixed in
//! [`tools::boundary`]. Everything a device said is untrusted text; everything
//! that comes back from a tool is redacted before the model sees it.
//!
//! Specification: `docs/dev/features/assistant.md` (conventions: `docs/dev/features/README.md`).

pub mod chats;
pub mod prompt;
pub mod providers;
pub mod redact;
pub mod settings;
pub mod shape;
pub mod tools;
pub mod turn;
pub mod usage;
pub mod view;

/// Register this feature's settings, state, RPC methods and background tasks.
pub fn init(builder: &mut nocturne_app::AppBuilder) {
    settings::declare(builder);
    builder.state(chats::ChatState::default());
    builder.state(providers::Override::default());
    builder.state(turn::TurnState::default());
    providers::register(builder);
    chats::register(builder);
    turn::register(builder);
    usage::register(builder);
    builder.on_start(|app| {
        // A turn that was running when the process stopped left its message
        // half-written; say so in the history rather than leaving a chat that
        // looks like it is still thinking.
        if let Err(err) = chats::repair_interrupted(app) {
            tracing::error!(%err, "interrupted assistant turns could not be repaired");
        }
    });
}
