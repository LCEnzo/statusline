// The relay's file as the binary meets it: the bar reads STATUSLINE_USAGE_FILE and draws.
// Nothing here may touch the network, and a missing or broken file is never an error.

use chrono::{SecondsFormat, TimeDelta, Utc};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn stamp(t: chrono::DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// The relay's file: weekly and Fable (when `fable`), resetting `reset_in` from now.
fn relay_file(stale: bool, fable: bool, reset_in: TimeDelta) -> Value {
    let resets = stamp(Utc::now() + reset_in);
    let mut windows = vec![
        json!({"name": "weekly", "used_percent": 41.0, "resets_at": resets, "window_seconds": 604800}),
    ];
    if fable {
        windows.push(json!({"name": "Fable", "used_percent": 32.0, "resets_at": resets, "window_seconds": 604800}));
    }
    json!({"version": 1, "fetched_at": stamp(Utc::now()), "stale": stale,
           "error": null, "windows": windows})
}

/// Runs the status line with stdin carrying a weekly number (`stdin_weekly`) or none, and the
/// usage file at `file`. Returns stdout with colour codes removed.
fn run(file: &Path, stdin_weekly: bool) -> String {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .ancestors()
        .last()
        .unwrap()
        .to_owned();
    let mut input = json!({
        "model": {"display_name": "Opus 5.5", "id": "claude-opus-5-5"},
        "cwd": root.join("statusline-test-no-such-dir"),
    });
    if stdin_weekly {
        input["rate_limits"] = json!({"seven_day": {
            "used_percentage": 43,
            "resets_at": (Utc::now() + TimeDelta::days(3)).timestamp()}});
    }
    let mut child = Command::new(env!("CARGO_BIN_EXE_statusline"))
        .env("STATUSLINE_USAGE_FILE", file)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.to_string().as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    let mut plain = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            chars.by_ref().find(|&c| c == 'm');
        } else {
            plain.push(c);
        }
    }
    plain
}

fn with_file(name: &str, content: &Value, stdin_weekly: bool) -> String {
    let d = dir(name);
    let f = d.join("usage.json");
    std::fs::write(&f, content.to_string()).unwrap();
    run(&f, stdin_weekly)
}

#[test]
fn a_fresh_file_adds_fable_and_stdin_keeps_the_weekly_number() {
    let line = with_file("fresh", &relay_file(false, true, TimeDelta::days(3)), true);
    assert!(line.contains(" W 43% ("), "{line}");
    assert!(line.contains(" F 32%"), "{line}");
    assert!(!line.contains("F 32%?"), "{line}");
    assert!(!line.contains("Fable"), "{line}");
    assert!(!line.contains("W 41%"), "{line}");
}

#[test]
fn a_stale_file_marks_fable_but_not_stdins_number() {
    let line = with_file("stale", &relay_file(true, true, TimeDelta::days(3)), true);
    assert!(line.contains(" F 32%?"), "{line}");
    assert!(line.contains(" W 43% ("), "{line}");
    assert!(!line.contains("W 43%?"), "{line}");
}

#[test]
fn without_fable_in_the_file_there_is_no_f() {
    let line = with_file("no-fable", &relay_file(false, false, TimeDelta::days(3)), true);
    assert!(line.contains(" W 43% ("), "{line}");
    assert!(!line.contains(" F "), "{line}");
}

#[test]
fn a_missing_or_broken_file_draws_the_bar_without_file_segments() {
    let d = dir("absent");
    let line = run(&d.join("nothing-here.json"), true);
    assert!(line.contains(" W 43% ("), "{line}");
    assert!(!line.contains(" F "), "{line}");
    assert!(!line.to_lowercase().contains("error"), "{line}");
    let broken = d.join("broken.json");
    std::fs::write(&broken, "{ not json").unwrap();
    assert_eq!(run(&broken, true), line);
}

#[test]
fn the_file_fills_the_weekly_window_when_stdin_lacks_it() {
    let line = with_file("fills", &relay_file(false, true, TimeDelta::days(3)), false);
    assert!(line.contains(" W 41% ("), "{line}");
    assert!(line.contains(" F 32%"), "{line}");
}

#[test]
fn a_passed_reset_hides_fable_and_a_future_one_shows_it() {
    let past = with_file("passed", &relay_file(false, true, TimeDelta::seconds(-5)), true);
    assert!(!past.contains(" F "), "{past}");
    assert!(past.contains(" W 43% ("), "{past}");
    let soon = with_file("soon", &relay_file(false, true, TimeDelta::seconds(120)), true);
    assert!(soon.contains(" F 32%"), "{soon}");
}
