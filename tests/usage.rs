// The usage cache as the binary meets it: a fresh cache is drawn without a fetch, an old one
// is refreshed once per interval, and a refresh that cannot run keeps the old windows, marked
// stale. Every case here fails before the network: no login, or an expired one.

use chrono::{SecondsFormat, TimeDelta, Utc};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// A fresh directory for one test's cache and login.
fn dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn stamp(t: chrono::DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// A cache holding the weekly and Fable windows of the endpoint's real answer, fetched
/// `age` ago, resetting three days from now.
fn seed_cache(dir: &Path, age: TimeDelta) {
    let resets = stamp(Utc::now() + TimeDelta::days(3));
    let cache = json!({
        "version": 1,
        "fetched_at": stamp(Utc::now() - age),
        "stale": false,
        "error": null,
        "windows": [
            {"name": "weekly", "used_percent": 41.0, "resets_at": resets, "window_seconds": 604800},
            {"name": "Fable", "used_percent": 32.0, "resets_at": resets, "window_seconds": 604800},
        ],
    });
    std::fs::write(dir.join("usage.json"), cache.to_string()).unwrap();
}

fn read_cache(dir: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(dir.join("usage.json")).unwrap()).unwrap()
}

/// Runs the statusline with `dir` as both its cache and its Claude config directory, and
/// returns stdout with colour codes removed.
fn run(dir: &Path) -> String {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .ancestors()
        .last()
        .unwrap()
        .to_owned();
    let input = json!({
        "model": {"display_name": "Opus 5.5", "id": "claude-opus-5-5"},
        "cwd": root.join("statusline-test-no-such-dir"),
        "rate_limits": {"seven_day": {
            "used_percentage": 43,
            "resets_at": (Utc::now() + TimeDelta::days(3)).timestamp(),
        }},
    })
    .to_string();
    let mut child = Command::new(env!("CARGO_BIN_EXE_statusline"))
        .env("CLAUDE_CONFIG_DIR", dir)
        .env("STATUSLINE_CACHE_DIR", dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
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

#[test]
fn a_fresh_cache_is_drawn_without_a_fetch() {
    let d = dir("usage-fresh");
    seed_cache(&d, TimeDelta::minutes(4));
    let line = run(&d);
    assert!(
        line.contains(" W 43% ("),
        "stdin's weekly number wins: {line}"
    );
    assert!(line.contains(" Fable 32% ("), "{line}");
    assert!(!d.join("usage.fetch").exists(), "no fetch was due");
    assert_eq!(read_cache(&d)["stale"], false);
}

#[test]
fn an_old_cache_without_a_login_keeps_its_windows_marked_stale() {
    let d = dir("usage-no-login");
    seed_cache(&d, TimeDelta::minutes(6));
    let line = run(&d);
    assert!(line.contains(" Fable 32%? ("), "{line}");
    assert!(
        line.contains(" W 43% ("),
        "stdin's number is never stale: {line}"
    );
    let cache = read_cache(&d);
    assert_eq!(cache["stale"], true);
    assert_eq!(cache["error"], "no claude.ai login");
    assert_eq!(cache["windows"][1]["name"], "Fable");
    assert_eq!(cache["windows"][1]["used_percent"], 32.0);
}

#[test]
fn an_expired_token_is_not_sent() {
    let d = dir("usage-expired");
    seed_cache(&d, TimeDelta::minutes(6));
    let expired = Utc::now().timestamp_millis() - 1000;
    let creds = json!({"claudeAiOauth": {"accessToken": "not-a-token", "expiresAt": expired}});
    std::fs::write(d.join(".credentials.json"), creds.to_string()).unwrap();
    run(&d);
    assert_eq!(read_cache(&d)["error"], "token expired");
}

#[test]
fn after_an_attempt_the_next_status_line_waits_out_the_interval() {
    let d = dir("usage-throttle");
    seed_cache(&d, TimeDelta::minutes(6));
    run(&d);
    assert_eq!(read_cache(&d)["error"], "no claude.ai login");
    // A second attempt would write its error over this one.
    let mut cache = read_cache(&d);
    cache["error"] = json!("left alone");
    std::fs::write(d.join("usage.json"), cache.to_string()).unwrap();
    let line = run(&d);
    assert_eq!(read_cache(&d)["error"], "left alone");
    assert!(line.contains(" Fable 32%? ("), "{line}");
}
