//! Cross-checks against independent implementations.
//!
//! `tests/generate_vectors.py` produces thousands of random cases together
//! with the answers of Python's `ipaddress` module (for `subnet` and `vlsm`),
//! `passlib` (Cisco type 7) and a second implementation of the password
//! generator. A copy is vendored in `tests/data/vectors.json.gz`, so these
//! tests need no Python; when a suitable Python is installed, a fresh set
//! with a different seed is generated and checked as well.

use std::io::Read;
use std::path::PathBuf;
use std::process::Command;

use nocturne_tools::ToolError;
use nocturne_tools::secrets::{self, PasswordPolicy};
use nocturne_tools::subnet::{self, SubnetInfo};
use nocturne_tools::vlsm::{self, Plan, Request};
use pretty_assertions::assert_eq;
use serde_json::Value;

const VENDORED: &[u8] = include_bytes!("data/vectors.json.gz");

fn inflate(gz: &[u8]) -> Value {
    let mut json = String::new();
    flate2::read::GzDecoder::new(gz)
        .read_to_string(&mut json)
        .expect("the vector file is gzip-compressed UTF-8");
    serde_json::from_str(&json).expect("the vector file is JSON")
}

fn section<'a>(vectors: &'a Value, name: &str) -> &'a [Value] {
    vectors[name]
        .as_array()
        .unwrap_or_else(|| panic!("section `{name}` is missing"))
        .as_slice()
}

fn text<'a>(case: &'a Value, key: &str) -> &'a str {
    case[key]
        .as_str()
        .unwrap_or_else(|| panic!("`{key}` is not a string in {case}"))
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .unwrap_or_else(|| panic!("{value} is not a list"))
        .iter()
        .map(|item| {
            item.as_str()
                .unwrap_or_else(|| panic!("{item} is not a string"))
                .to_owned()
        })
        .collect()
}

/// Compare the crate with every case of a vector file; returns the number
/// of cases checked per section.
fn check(vectors: &Value) -> Vec<(&'static str, usize)> {
    vec![
        ("info", check_info(vectors)),
        ("validity", check_validity(vectors)),
        ("split", check_split(vectors)),
        ("summarize", check_summarize(vectors)),
        ("relations", check_relations(vectors)),
        ("ranges", check_ranges(vectors)),
        ("free", check_free(vectors)),
        ("vlsm", check_vlsm(vectors)),
        ("cisco7", check_cisco7(vectors)),
        ("passwords", check_passwords(vectors)),
    ]
}

fn check_info(vectors: &Value) -> usize {
    let cases = section(vectors, "info");
    for case in cases {
        let input = text(case, "input");
        let expected: SubnetInfo =
            serde_json::from_value(case["expect"].clone()).expect("an expected SubnetInfo");
        let actual =
            subnet::info(input).unwrap_or_else(|error| panic!("info({input:?}) failed: {error}"));
        assert_eq!(actual, expected, "info({input:?})");
    }
    cases.len()
}

fn check_validity(vectors: &Value) -> usize {
    let cases = section(vectors, "validity");
    for case in cases {
        let input = text(case, "input");
        let result = subnet::info(input);
        assert_eq!(
            result.is_ok(),
            case["valid"].as_bool().unwrap(),
            "info({input:?}) = {result:?}"
        );
    }
    cases.len()
}

fn check_split(vectors: &Value) -> usize {
    let cases = section(vectors, "split");
    for case in cases {
        let network = text(case, "network");
        let new_prefix = u8::try_from(case["new_prefix"].as_u64().unwrap()).unwrap();
        let limit = usize::try_from(case["limit"].as_u64().unwrap()).unwrap();
        let result = subnet::split(network, new_prefix, limit);
        if case["error"].as_bool() == Some(true) {
            assert!(
                matches!(result, Err(ToolError::Invalid { .. })),
                "split({network:?}, {new_prefix}) = {result:?}"
            );
            continue;
        }
        let result = result.unwrap_or_else(|error| {
            panic!("split({network:?}, {new_prefix}, {limit}) failed: {error}")
        });
        let cidrs: Vec<String> = result
            .subnets
            .iter()
            .map(|subnet| subnet.cidr.clone())
            .collect();
        assert_eq!(
            cidrs,
            strings(&case["cidrs"]),
            "split({network:?}, {new_prefix}, {limit})"
        );
        assert_eq!(
            result.total,
            text(case, "total"),
            "split({network:?}, {new_prefix}, {limit})"
        );
        assert_eq!(
            result.truncated,
            case["truncated"].as_bool().unwrap(),
            "split({network:?}, {new_prefix}, {limit})"
        );
        // Every subnet is described exactly as `info` would describe it.
        for described in &result.subnets {
            assert_eq!(described, &subnet::info(&described.cidr).unwrap());
        }
    }
    cases.len()
}

fn check_summarize(vectors: &Value) -> usize {
    let cases = section(vectors, "summarize");
    for case in cases {
        let inputs = strings(&case["inputs"]);
        assert_eq!(
            subnet::summarize(&inputs).unwrap(),
            strings(&case["summary"]),
            "summarize({inputs:?})"
        );
        let supernet = subnet::supernet(&inputs);
        if case["supernet_error"].as_bool() == Some(true) {
            assert!(
                matches!(supernet, Err(ToolError::Invalid { .. })),
                "supernet({inputs:?}) = {supernet:?}"
            );
        } else {
            assert_eq!(
                supernet.as_deref(),
                Ok(text(case, "supernet")),
                "supernet({inputs:?})"
            );
        }
    }
    cases.len()
}

fn check_relations(vectors: &Value) -> usize {
    let cases = section(vectors, "relations");
    for case in cases {
        let (a, b) = (text(case, "a"), text(case, "b"));
        assert_eq!(
            subnet::contains(a, b),
            Ok(case["a_contains_b"].as_bool().unwrap()),
            "contains({a:?}, {b:?})"
        );
        let overlaps = case["overlaps"].as_bool().unwrap();
        assert_eq!(
            subnet::overlaps(a, b),
            Ok(overlaps),
            "overlaps({a:?}, {b:?})"
        );
        assert_eq!(
            subnet::overlaps(b, a),
            Ok(overlaps),
            "overlaps({b:?}, {a:?})"
        );
    }
    cases.len()
}

fn check_ranges(vectors: &Value) -> usize {
    let cases = section(vectors, "ranges");
    for case in cases {
        let (first, last) = (text(case, "first"), text(case, "last"));
        let result = subnet::range_to_cidrs(first, last);
        if case["error"].as_bool() == Some(true) {
            assert!(
                matches!(result, Err(ToolError::Invalid { .. })),
                "range_to_cidrs({first:?}, {last:?}) = {result:?}"
            );
        } else {
            assert_eq!(
                result,
                Ok(strings(&case["cidrs"])),
                "range_to_cidrs({first:?}, {last:?})"
            );
        }
    }
    cases.len()
}

fn check_free(vectors: &Value) -> usize {
    let cases = section(vectors, "free");
    for case in cases {
        let parent = text(case, "parent");
        let used = strings(&case["used"]);
        assert_eq!(
            subnet::free_space(parent, &used),
            Ok(strings(&case["free"])),
            "free_space({parent:?}, {used:?})"
        );

        let prefix = u8::try_from(case["prefix"].as_u64().unwrap()).unwrap();
        let next = subnet::next_free(parent, prefix, &used);
        match case["next_error"].as_str() {
            Some("invalid") => assert!(
                matches!(next, Err(ToolError::Invalid { .. })),
                "next_free({parent:?}, {prefix}) = {next:?}"
            ),
            Some("does_not_fit") => {
                assert!(
                    matches!(next, Err(ToolError::DoesNotFit(_))),
                    "next_free({parent:?}, {prefix}) = {next:?}"
                );
            }
            Some(other) => panic!("unknown error kind {other}"),
            None => {
                let expected = case["next"].as_str().map(str::to_owned);
                assert_eq!(
                    next,
                    Ok(expected),
                    "next_free({parent:?}, {prefix}, {used:?})"
                );
            }
        }
    }
    cases.len()
}

fn check_vlsm(vectors: &Value) -> usize {
    let cases = section(vectors, "vlsm");
    for case in cases {
        let base = text(case, "base");
        let requests: Vec<Request> = case["requests"]
            .as_array()
            .unwrap()
            .iter()
            .map(|request| Request {
                name: request[0].as_str().unwrap().to_owned(),
                hosts: request[1].as_u64().unwrap(),
            })
            .collect();
        let p2p31 = case["p2p31"].as_bool().unwrap();
        let result = vlsm::plan(base, &requests, p2p31);
        let context = format!("plan({base:?}, {requests:?}, {p2p31})");
        let Some(kind) = case["error"].as_str() else {
            let expected: Plan =
                serde_json::from_value(case["plan"].clone()).expect("an expected Plan");
            assert_eq!(result, Ok(expected), "{context}");
            continue;
        };
        // The error names the request that could not be placed.
        let index = usize::try_from(case["request"].as_u64().unwrap()).unwrap();
        let name = requests[index].name.trim();
        let label = if name.is_empty() {
            format!("#{}", index + 1)
        } else {
            format!("`{name}`")
        };
        let error = result.expect_err(&context);
        match kind {
            "invalid" => assert!(
                matches!(error, ToolError::Invalid { .. }),
                "{context} = {error:?}"
            ),
            "does_not_fit" => assert!(
                matches!(error, ToolError::DoesNotFit(_)),
                "{context} = {error:?}"
            ),
            other => panic!("unknown error kind {other}"),
        }
        assert!(
            error.to_string().contains(&label),
            "{context}: `{error}` should name {label}"
        );
    }
    cases.len()
}

fn check_cisco7(vectors: &Value) -> usize {
    let cases = section(vectors, "cisco7");
    for case in cases {
        let (plain, encoded) = (text(case, "plain"), text(case, "encoded"));
        let salt = u8::try_from(case["salt"].as_u64().unwrap()).unwrap();
        assert_eq!(
            secrets::cisco7_decode(encoded).as_deref(),
            Ok(plain),
            "cisco7_decode({encoded:?})"
        );
        let ours = secrets::cisco7_encode(plain, salt);
        if salt <= 15 {
            assert_eq!(
                ours.as_deref(),
                Ok(encoded),
                "cisco7_encode({plain:?}, {salt})"
            );
        } else {
            assert!(ours.is_err(), "IOS does not generate salt {salt}");
        }
    }
    cases.len()
}

fn check_passwords(vectors: &Value) -> usize {
    let cases = section(vectors, "passwords");
    for case in cases {
        let flag = |key: &str| case[key].as_bool().unwrap();
        let policy = PasswordPolicy {
            length: usize::try_from(case["length"].as_u64().unwrap()).unwrap(),
            upper: flag("upper"),
            lower: flag("lower"),
            digits: flag("digits"),
            symbols: flag("symbols"),
            avoid_ambiguous: flag("avoid_ambiguous"),
        };
        let seed: [u8; 32] = hex::decode(text(case, "seed")).unwrap().try_into().unwrap();
        assert_eq!(
            secrets::generate_password(&policy, &seed).as_deref(),
            Ok(text(case, "password")),
            "{policy:?}"
        );
    }
    cases.len()
}

#[test]
fn vendored_vectors_from_python_ipaddress_and_passlib() {
    let vectors = inflate(VENDORED);
    let counts = check(&vectors);
    // "A few thousand random inputs", and no section silently empty.
    let minimums = [2000, 1000, 500, 500, 1000, 500, 500, 500, 300, 100];
    for ((name, count), minimum) in counts.iter().zip(minimums) {
        assert!(*count >= minimum, "section `{name}` has only {count} cases");
    }
    assert!(
        vectors["meta"]["passlib"].is_string(),
        "the vendored vectors are generated with passlib installed"
    );
}

/// A scratch directory that is removed again, also when the test fails.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("nocturne-tools-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&path).expect("a scratch directory");
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn fresh_vectors_from_the_installed_python() {
    let scratch = Scratch::new("vectors");
    let out = scratch.0.join("vectors.json");
    let generator = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/generate_vectors.py");
    let run = |python: &str| {
        Command::new(python)
            // -I: isolated mode, so nothing in the working directory can shadow a module.
            .arg("-I")
            .arg(&generator)
            .args(["--seed", "4242", "--count", "400", "--out"])
            .arg(&out)
            .output()
    };
    let Ok(output) = run("python3").or_else(|_| run("python")) else {
        eprintln!("SKIPPED: no Python interpreter found; only the vendored vectors were checked");
        return;
    };
    if output.status.code() == Some(3) {
        eprintln!(
            "SKIPPED: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return;
    }
    assert!(
        output.status.success(),
        "generate_vectors.py failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let vectors: Value = serde_json::from_slice(&std::fs::read(&out).unwrap()).unwrap();
    let counts = check(&vectors);
    for (name, count) in &counts {
        // passlib is optional for the generator.
        assert!(*count > 0 || *name == "cisco7", "section `{name}` is empty");
    }
    eprintln!(
        "checked against Python {}: {counts:?}",
        vectors["meta"]["python"]
    );
}

/// `NOCTURNE_TOOLS_VECTORS=/path/to/vectors.json[.gz] cargo test` checks a
/// vector file of any size made with `generate_vectors.py --seed N --count N`.
#[test]
fn vectors_named_in_the_environment() {
    let Some(path) = std::env::var_os("NOCTURNE_TOOLS_VECTORS") else {
        eprintln!("SKIPPED: NOCTURNE_TOOLS_VECTORS is not set");
        return;
    };
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.to_string_lossy()));
    let vectors = if bytes.starts_with(&[0x1f, 0x8b]) {
        inflate(&bytes)
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    eprintln!("checked {}: {:?}", path.to_string_lossy(), check(&vectors));
}
