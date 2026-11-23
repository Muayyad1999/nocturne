//! Quality of service: a marking in every form, and what voice calls cost.
//!
//! Both are `nocturne_tools::qos`. What this module adds is the reading of
//! the parameters — and with it the two refusals the engine cannot make,
//! because it never sees them: a marking with no word on what kind of number
//! it is, and a voice request with no number of calls. Neither has a default
//! worth the name. `46` is EF as a DSCP and an unrelated marking as a ToS
//! byte, and "one call" for a link sized for sixty is a wrong answer that
//! looks like a right one.

use nocturne_app::{App, AppError, AppResult, Router};
use nocturne_tools::qos::{self, Marking, MarkingForm, VoiceBandwidth, VoiceOptions};
use serde::Deserialize;
use serde_json::Value;

use crate::errors::tool;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct MarkingParams {
    /// The marking, as text or as a JSON number: `46`, `"ef"`, `"0xb8"`, `5`.
    pub value: Value,
    /// What `value` is: `dscp`, `tos`, `precedence` or `name`.
    #[serde(rename = "as")]
    pub form: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct VoiceParams {
    /// `g711`, `g729`, `g723.1-6.3`, … — the `voice_codecs` reference table.
    pub codec: String,
    /// Simultaneous calls.
    pub calls: Option<u32>,
    /// `ethernet`, `ppp`, `mpls`, … — the `voice_links` reference table — or
    /// `custom` with `l2_bytes`.
    pub link: String,
    /// Compressed RTP on the link.
    pub crtp: bool,
    /// Also estimate what voice-activity detection saves.
    pub vad: bool,
    /// Milliseconds of voice per packet, when not the codec's default.
    pub packet_ms: Option<u32>,
    /// Layer-2 bytes per packet, for the `custom` link only.
    pub l2_bytes: Option<u32>,
}

fn marking(params: MarkingParams) -> AppResult<Marking> {
    // A number arrives as a number from a caller that has one (the assistant,
    // a script) and as text from a form field; both are the same request.
    let value = match &params.value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        Value::Null => {
            return Err(AppError::invalid(
                "give the marking in `value`: a DSCP number or name, a ToS byte or an IP precedence",
            ));
        }
        _ => {
            return Err(AppError::invalid(
                "`value` is one marking, as text or a number",
            ));
        }
    };
    let Some(form) = params
        .form
        .as_deref()
        .map(str::trim)
        .filter(|form| !form.is_empty())
    else {
        return Err(AppError::invalid(
            "say what the value is in `as`: `dscp`, `tos`, `precedence` or `name`. The same number is a different marking as each",
        ));
    };
    let form: MarkingForm = form.parse().map_err(tool)?;
    qos::marking(&value, form).map_err(tool)
}

fn voice(params: VoiceParams) -> AppResult<VoiceBandwidth> {
    let Some(calls) = params.calls else {
        return Err(AppError::invalid(
            "give the number of simultaneous calls in `calls`",
        ));
    };
    qos::voice(
        &params.codec,
        calls,
        &params.link,
        &VoiceOptions {
            crtp: params.crtp,
            vad: params.vad,
            packet_ms: params.packet_ms,
            l2_bytes: params.l2_bytes,
        },
    )
    .map_err(tool)
}

pub fn register(router: &mut Router) {
    router.add(
        "tools.qos.marking",
        |_: App, params: MarkingParams| async move { marking(params) },
    );
    router.add(
        "tools.qos.voice",
        |_: App, params: VoiceParams| async move { voice(params) },
    );
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn params<T: serde::de::DeserializeOwned>(value: Value) -> T {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn a_marking_given_as_a_number_and_as_text_is_the_same_request() {
        let as_number = marking(params(json!({"value": 46, "as": "dscp"}))).unwrap();
        let as_text = marking(params(json!({"value": "46", "as": "dscp"}))).unwrap();
        assert_eq!(as_number, as_text);
        assert_eq!(as_number.name.as_deref(), Some("EF"));
        // A fraction is a number, but not a marking: the engine says so.
        let fraction = marking(params(json!({"value": 4.6, "as": "dscp"}))).unwrap_err();
        assert_eq!(fraction.code, "invalid");
        assert!(fraction.message.contains("4.6"), "{}", fraction.message);
        for not_a_marking in [json!(true), json!([46]), json!({"dscp": 46})] {
            let refused = marking(params(json!({"value": not_a_marking, "as": "dscp"})));
            assert_eq!(refused.unwrap_err().code, "invalid");
        }
    }

    #[test]
    fn a_number_with_no_word_on_what_it_is_gets_no_answer() {
        for silent in [
            json!({"value": 46}),
            json!({"value": 46, "as": ""}),
            json!({"value": 46, "as": "  "}),
        ] {
            let refused = marking(params(silent)).unwrap_err();
            assert_eq!(refused.code, "invalid");
            assert!(
                refused
                    .message
                    .contains("`dscp`, `tos`, `precedence` or `name`"),
                "{}",
                refused.message
            );
        }
        let missing = marking(params(json!({"as": "dscp"}))).unwrap_err();
        assert!(missing.message.contains("`value`"), "{}", missing.message);
    }

    #[test]
    fn a_voice_request_without_a_number_of_calls_is_not_assumed_to_be_one() {
        let refused = voice(params(json!({"codec": "g729", "link": "ppp"}))).unwrap_err();
        assert_eq!(refused.code, "invalid");
        assert!(refused.message.contains("`calls`"), "{}", refused.message);
        let one = voice(params(json!({"codec": "g729", "link": "ppp", "calls": 1}))).unwrap();
        assert_eq!((one.calls, one.per_call_bps), (1, 26_400));
    }
}
