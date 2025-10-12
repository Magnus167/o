use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::process::Command;

#[test]
fn json_single_outputs_valid_json() {
    let mut cmd = Command::cargo_bin("o").unwrap();
    cmd.args(["-j", "--sample-ms=1"]);
    let assert = cmd.assert().success();
    // Basic shape checks
    assert
        .stdout(predicate::str::contains("{"))
        .stdout(predicate::str::contains("cpu_percent"))
        .stdout(predicate::str::contains("mem_used"))
        .stdout(predicate::str::contains("mem_total"))
        .stdout(predicate::str::contains("swap_used"))
        .stdout(predicate::str::contains("swap_total"))
        .stdout(predicate::str::contains("uptime_secs"))
        .stdout(predicate::str::contains("load_avg"))
        .stdout(predicate::str::contains("disk_read_bps"))
        .stdout(predicate::str::contains("disk_write_bps"));
}

#[test]
fn plain_once_contains_expected_sections() {
    let mut cmd = Command::cargo_bin("o").unwrap();
    cmd.env("COLUMNS", "80");
    cmd.args(["--plain", "-o", "--sample-ms=1", "--color=never"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("o - system summary"))
        .stdout(predicate::str::contains("CPU:"))
        .stdout(predicate::str::contains("Mem:"))
        .stdout(predicate::str::contains("Swap:"))
        .stdout(predicate::str::contains("Disk:"))
        .stdout(predicate::str::contains("Up:"))
        .stdout(predicate::str::contains("Load:"));
}
