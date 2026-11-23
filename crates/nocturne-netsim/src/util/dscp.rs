//! DSCP code points and the names a command line uses for them.

/// The named code points of RFC 2474/2597/3246, as IOS spells them.
const NAMES: &[(u8, &str)] = &[
    (0, "default"),
    (8, "cs1"),
    (10, "af11"),
    (12, "af12"),
    (14, "af13"),
    (16, "cs2"),
    (18, "af21"),
    (20, "af22"),
    (22, "af23"),
    (24, "cs3"),
    (26, "af31"),
    (28, "af32"),
    (30, "af33"),
    (32, "cs4"),
    (34, "af41"),
    (36, "af42"),
    (38, "af43"),
    (40, "cs5"),
    (46, "ef"),
    (48, "cs6"),
    (56, "cs7"),
];

/// `ef`, `af41`, `cs3`, `default`, or a number from 0 to 63.
pub(crate) fn parse(text: &str) -> Option<u8> {
    if let Some((value, _)) = NAMES
        .iter()
        .find(|(_, name)| name.eq_ignore_ascii_case(text))
    {
        return Some(*value);
    }
    let digits = !text.is_empty() && text.len() <= 2 && text.bytes().all(|b| b.is_ascii_digit());
    digits
        .then(|| text.parse::<u8>().ok())
        .flatten()
        .filter(|value| *value <= 63)
}

pub(crate) fn name(value: u8) -> Option<&'static str> {
    NAMES.iter().find(|(v, _)| *v == value).map(|(_, n)| *n)
}

/// How a configuration writes the value: by name where it has one.
pub(crate) fn config_text(value: u8) -> String {
    name(value).map_or_else(|| value.to_string(), str::to_string)
}

/// How statistics write the value: `ef (46)`, or the bare number.
pub(crate) fn show_text(value: u8) -> String {
    name(value).map_or_else(|| value.to_string(), |n| format!("{n} ({value})"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_numbers_are_the_same_code_point() {
        assert_eq!(parse("ef"), Some(46));
        assert_eq!(parse("EF"), Some(46));
        assert_eq!(parse("46"), Some(46));
        assert_eq!(parse("af41"), Some(34));
        assert_eq!(parse("default"), Some(0));
        assert_eq!(parse("63"), Some(63));
        assert_eq!(parse("64"), None);
        assert_eq!(parse("af44"), None);
        assert_eq!(parse(""), None);
        assert_eq!(parse("-1"), None);
        assert_eq!(config_text(46), "ef");
        assert_eq!(config_text(5), "5");
        assert_eq!(show_text(34), "af41 (34)");
        assert_eq!(show_text(5), "5");
        for value in 0..=63 {
            assert_eq!(parse(&config_text(value)), Some(value));
        }
    }
}
