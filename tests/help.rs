use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::process::Command;

#[test]
fn help_shows_usage_and_examples() {
    let mut cmd = Command::cargo_bin("o").unwrap();
    cmd.arg("--help");
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("Usage:"))
        .stdout(predicate::str::contains("Examples:"))
        .stdout(predicate::str::contains("--plain"))
        .stdout(predicate::str::contains("--json"))
        .stdout(predicate::str::contains("--watch"));
}
