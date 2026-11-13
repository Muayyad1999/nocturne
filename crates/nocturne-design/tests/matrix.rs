//! The requirements matrix: every design in it boots and proves itself.
//!
//! The matrix is sharded across several test functions so `cargo test` runs
//! them in parallel; see `common::matrix` for what it covers and why the
//! largest user counts are kept to the single-site cases.

mod common;

use common::{matrix, run_case};

const SHARDS: usize = 16;

fn shard(n: usize) {
    let cases = matrix(n, SHARDS);
    assert!(!cases.is_empty(), "shard {n} is empty");
    for case in &cases {
        run_case(case);
    }
}

#[test]
fn shard_0() {
    shard(0);
}
#[test]
fn shard_1() {
    shard(1);
}
#[test]
fn shard_2() {
    shard(2);
}
#[test]
fn shard_3() {
    shard(3);
}
#[test]
fn shard_4() {
    shard(4);
}
#[test]
fn shard_5() {
    shard(5);
}
#[test]
fn shard_6() {
    shard(6);
}
#[test]
fn shard_7() {
    shard(7);
}
#[test]
fn shard_8() {
    shard(8);
}
#[test]
fn shard_9() {
    shard(9);
}
#[test]
fn shard_10() {
    shard(10);
}
#[test]
fn shard_11() {
    shard(11);
}
#[test]
fn shard_12() {
    shard(12);
}
#[test]
fn shard_13() {
    shard(13);
}
#[test]
fn shard_14() {
    shard(14);
}
#[test]
fn shard_15() {
    shard(15);
}

#[test]
fn the_matrix_covers_what_it_claims_to() {
    let all: Vec<_> = (0..SHARDS).flat_map(|s| matrix(s, SHARDS)).collect();
    let users: Vec<u32> = all
        .iter()
        .flat_map(|c| c.requirements.sites.iter().map(|s| s.users))
        .collect();
    assert!(users.contains(&5) && users.contains(&2000), "{users:?}");
    for sites in 1..=4 {
        assert!(
            all.iter().any(|c| c.requirements.sites.len() == sites),
            "no case with {sites} site(s)"
        );
    }
    assert!(all.len() > 150, "only {} cases", all.len());
}
