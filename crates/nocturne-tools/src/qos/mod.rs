//! Quality of service: what a marking means, and what a voice call costs.
//!
//! [`marking`] reads one marking — a DSCP value or name, a ToS byte or an IP
//! precedence — and writes it in every other form. [`voice`] works out the
//! bandwidth of voice calls over a link from the size of one packet and the
//! number of packets a second, and shows that arithmetic.
//!
//! The DSCP names come from the reference table in [`crate::reference`]; the
//! codec and link figures come from the two Cisco documents named in
//! [`SOURCE_PER_CALL`] and [`SOURCE_SRND`], and the tests reproduce the tables
//! those documents publish.

mod marking;
mod voice;

pub use marking::{Marking, MarkingForm, marking};
pub use voice::{
    MAX_CALLS, MAX_L2_BYTES, MAX_PACKET_MS, PacketPart, SOURCE_PER_CALL, SOURCE_SRND,
    VAD_SAVING_PCT, VadEstimate, VoiceBandwidth, VoiceCodec, VoiceLink, VoiceOptions, voice,
    voice_codecs, voice_links,
};
