use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::process::Command;

#[test]
fn no_mode_flags_without_tty_prints_plain_snapshot() {
    Command::cargo_bin("o")
        .unwrap()
        .arg("--sample-ms=1")
        .assert()
        .success()
        .stdout(predicate::str::contains("CPU:"));
}
