// `statusline --navigator` joins the navigator segment that contrib/nav-wrapper.sh used to.
// The fake navigator is examples/fake_navigator.rs, which `cargo test` builds.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn fake_navigator() -> PathBuf {
    let bin = Path::new(env!("CARGO_BIN_EXE_statusline"));
    let fake = bin
        .parent()
        .unwrap()
        .join("examples")
        .join(format!("fake_navigator{}", std::env::consts::EXE_SUFFIX));
    assert!(
        fake.is_file(),
        "{} missing: run the whole `cargo test`",
        fake.display()
    );
    fake
}

/// A fresh record directory for one test.
fn record_dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A payload whose cwd is outside any repository, so no git segment varies between runs.
fn payload() -> String {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .ancestors()
        .last()
        .unwrap()
        .to_owned();
    let cwd = root.join("statusline-test-no-such-dir");
    serde_json::json!({
        "model": {"display_name": "Opus 5.5", "id": "claude-opus-5-5"},
        "cwd": cwd,
        "session_id": "0f3c2a9e-test",
    })
    .to_string()
}

/// Runs the statusline and returns its stdout; `envs` of None removes the variable.
fn run(args: &[&str], envs: &[(&str, Option<&str>)], input: &str) -> String {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_statusline"));
    cmd.args(args)
        .env_remove("NAVIGATOR_BIN")
        .env_remove("COLUMNS")
        .env_remove("FAKE_NAV_OUT")
        .env_remove("FAKE_NAV_RECORD")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    for (k, v) in envs {
        match v {
            Some(v) => cmd.env(k, v),
            None => cmd.env_remove(k),
        };
    }
    let mut child = cmd.spawn().unwrap();
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap()
}

/// Visible width, derived here rather than from the crate: drop CSI sequences, count chars.
fn visible_width(s: &str) -> i64 {
    let mut n = 0;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) && c != '[' {
                    break;
                }
            }
        } else {
            n += 1;
        }
    }
    n
}

#[test]
fn without_the_flag_navigator_is_never_run() {
    let rec = record_dir("no_flag");
    let fake = fake_navigator();
    let input = payload();
    let base = run(&[], &[], &input);
    let out = run(
        &[],
        &[
            ("NAVIGATOR_BIN", Some(fake.to_str().unwrap())),
            ("FAKE_NAV_RECORD", Some(rec.to_str().unwrap())),
            ("COLUMNS", Some("200")),
        ],
        &input,
    );
    assert_eq!(out, base);
    assert!(
        !rec.join("args.txt").exists(),
        "navigator ran without --navigator"
    );
}

#[test]
fn the_flag_joins_navigator_with_the_payload_and_the_remaining_width() {
    let rec = record_dir("joined");
    let fake = fake_navigator();
    let input = payload();
    let base = run(&[], &[], &input);
    let base_line = base.trim_end_matches(['\r', '\n']);
    let out = run(
        &["--navigator"],
        &[
            ("NAVIGATOR_BIN", Some(fake.to_str().unwrap())),
            ("FAKE_NAV_RECORD", Some(rec.to_str().unwrap())),
            ("COLUMNS", Some("200")),
        ],
        &input,
    );
    assert_eq!(
        out.trim_end_matches(['\r', '\n']),
        format!("{base_line}  [nav]")
    );
    assert_eq!(
        std::fs::read(rec.join("stdin.bin")).unwrap(),
        input.as_bytes()
    );
    let args = std::fs::read_to_string(rec.join("args.txt")).unwrap();
    let width = 200 - visible_width(base_line) - 2;
    assert_eq!(args, format!("statusline\n--color\n256\n--width\n{width}"));
}

#[test]
fn an_unusable_columns_passes_no_width() {
    for columns in [None, Some("0100"), Some("auto")] {
        let rec = record_dir("no_width");
        let fake = fake_navigator();
        run(
            &["--navigator"],
            &[
                ("NAVIGATOR_BIN", Some(fake.to_str().unwrap())),
                ("FAKE_NAV_RECORD", Some(rec.to_str().unwrap())),
                ("COLUMNS", columns),
            ],
            &payload(),
        );
        let args = std::fs::read_to_string(rec.join("args.txt")).unwrap();
        assert_eq!(args, "statusline\n--color\n256", "COLUMNS={columns:?}");
    }
}

#[test]
fn a_missing_or_silent_navigator_leaves_the_base_alone() {
    let input = payload();
    let base = run(&[], &[], &input);
    let missing = run(
        &["--navigator"],
        &[("NAVIGATOR_BIN", Some("C:/no/such/navigator.exe"))],
        &input,
    );
    assert_eq!(missing, base);
    let fake = fake_navigator();
    let silent = run(
        &["--navigator"],
        &[
            ("NAVIGATOR_BIN", Some(fake.to_str().unwrap())),
            ("FAKE_NAV_OUT", Some("")),
        ],
        &input,
    );
    assert_eq!(silent, base);
}
