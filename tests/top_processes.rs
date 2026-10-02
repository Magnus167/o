use assert_cmd::prelude::*;
use std::process::Command;

fn json(args: &[&str]) -> serde_json::Value {
    let out = Command::cargo_bin("o")
        .unwrap()
        .args(["-j", "--sample-ms=1"])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success());
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn json_without_top_has_no_processes_key() {
    assert!(json(&[]).get("processes").is_none());
}

#[test]
fn json_top_3_lists_three_processes() {
    let v = json(&["--top", "3", "--sort", "mem"]);
    let procs = v["processes"].as_array().expect("processes array");
    assert_eq!(procs.len(), 3);
    for p in procs {
        assert!(p["pid"].is_u64(), "{p}");
        assert!(p["name"].is_string(), "{p}");
        assert!(p["cpu_percent"].is_number(), "{p}");
        assert!(p["mem_bytes"].is_u64(), "{p}");
    }
}
