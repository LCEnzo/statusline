// Plan usage windows from claude.ai's usage endpoint, the one Claude Code's /usage dialog
// reads. The bar takes from it what stdin lacks (the per-model weekly windows, such as
// Fable); hooks read the cache this module writes. At most one fetch per five minutes across
// every running status line. Only the login's access token is read: Claude Code refreshes it,
// and a refresh from here could sign its sessions out.

use chrono::{DateTime, SecondsFormat, TimeDelta, Utc};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

pub const FIVE_HOUR: &str = "5-hour";
pub const WEEKLY: &str = "weekly";
const FIVE_HOURS_SECS: i64 = 5 * 3600;
const WEEK_SECS: i64 = 7 * 86_400;

/// How old the cache may get before a fetch, and how long one fetch attempt keeps the others off.
const REFRESH_SECS: i64 = 300;
/// The `windows` read of Claude Code's own client: rate-limit windows without the spend store.
const URL: &str = "https://api.anthropic.com/api/oauth/usage?skip_spend=1";
const NETWORK_TIMEOUT: Duration = Duration::from_millis(1500);
/// The bar waits no longer than this, whatever the network does; it then draws the old cache.
const WAIT: Duration = Duration::from_millis(2000);

#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    pub name: String,
    pub used_percent: f64,
    pub resets_at: Option<DateTime<Utc>>,
    pub window_seconds: i64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Cache {
    /// When the windows were fetched; None until a fetch has succeeded.
    pub fetched_at: Option<DateTime<Utc>>,
    /// The last refresh failed, so the windows are as old as `fetched_at`.
    pub stale: bool,
    pub error: Option<String>,
    pub windows: Vec<Window>,
}

impl Cache {
    fn fresh(windows: Vec<Window>, now: DateTime<Utc>) -> Cache {
        Cache {
            fetched_at: Some(now),
            stale: false,
            error: None,
            windows,
        }
    }

    /// A failed refresh keeps the last windows and marks them stale.
    fn failed(old: Option<Cache>, error: String) -> Cache {
        let old = old.unwrap_or_default();
        Cache {
            stale: true,
            error: Some(error),
            ..old
        }
    }

    fn to_json(&self) -> String {
        let windows: Vec<Value> = self
            .windows
            .iter()
            .map(|w| {
                json!({
                    "name": w.name,
                    "used_percent": w.used_percent,
                    "resets_at": w.resets_at.map(stamp),
                    "window_seconds": w.window_seconds,
                })
            })
            .collect();
        let doc = json!({
            "version": 1,
            "fetched_at": self.fetched_at.map(stamp),
            "stale": self.stale,
            "error": self.error,
            "windows": windows,
        });
        serde_json::to_string_pretty(&doc).unwrap_or_default()
    }

    fn parse(text: &str) -> Option<Cache> {
        let v: Value = serde_json::from_str(text).ok()?;
        if v["version"] != 1 {
            return None;
        }
        let windows = v["windows"]
            .as_array()?
            .iter()
            .filter_map(|w| {
                Some(Window {
                    name: w["name"].as_str()?.to_owned(),
                    used_percent: w["used_percent"].as_f64()?,
                    resets_at: instant(&w["resets_at"]),
                    window_seconds: w["window_seconds"].as_i64()?,
                })
            })
            .collect();
        Some(Cache {
            fetched_at: instant(&v["fetched_at"]),
            stale: v["stale"].as_bool().unwrap_or(true),
            error: v["error"].as_str().map(str::to_owned),
            windows,
        })
    }
}

fn stamp(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn instant(v: &Value) -> Option<DateTime<Utc>> {
    let t = DateTime::parse_from_rfc3339(v.as_str()?).ok()?;
    Some(t.with_timezone(&Utc))
}

/// The windows in the endpoint's answer. The 5-hour and weekly windows are the top-level
/// objects Claude Code's /usage reads; per-model weekly windows exist only in `limits[]`,
/// told apart by `kind`, as Claude Code does, never by label. A window without a number is absent.
pub fn parse_answer(body: &str) -> Result<Vec<Window>, String> {
    let v: Value = serde_json::from_str(body).map_err(|_| "answer is not JSON".to_string())?;
    if !v.is_object() {
        return Err("answer is not a JSON object".into());
    }
    let mut out = Vec::new();
    for (key, name, secs) in [
        ("five_hour", FIVE_HOUR, FIVE_HOURS_SECS),
        ("seven_day", WEEKLY, WEEK_SECS),
    ] {
        if let Some(pct) = v[key]["utilization"].as_f64() {
            out.push(Window {
                name: name.into(),
                used_percent: pct,
                resets_at: instant(&v[key]["resets_at"]),
                window_seconds: secs,
            });
        }
    }
    for item in v["limits"].as_array().into_iter().flatten() {
        if item["kind"] != "weekly_scoped" {
            continue;
        }
        let name = item["scope"]["model"]["display_name"]
            .as_str()
            .filter(|n| !n.is_empty());
        let (Some(name), Some(pct)) = (name, item["percent"].as_f64()) else {
            continue;
        };
        out.push(Window {
            name: name.into(),
            used_percent: pct,
            resets_at: instant(&item["resets_at"]),
            window_seconds: WEEK_SECS,
        });
    }
    Ok(out)
}

/// Whether `then` lies within one refresh interval of `now`, on either side. A time slightly
/// ahead is another process's clock read a moment later; one far ahead means the clock moved back.
fn is_recent(then: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    (now - then).abs() < TimeDelta::seconds(REFRESH_SECS)
}

/// The access token from Claude Code's credentials file, if it has not expired.
/// Errors never quote the file.
fn access_token(credentials: &str, now: DateTime<Utc>) -> Result<String, String> {
    let v: Value =
        serde_json::from_str(credentials).map_err(|_| "credentials file unreadable".to_string())?;
    let oauth = &v["claudeAiOauth"];
    let token = oauth["accessToken"]
        .as_str()
        .filter(|t| !t.is_empty())
        .ok_or("no claude.ai login")?;
    // Milliseconds. Without one the call is made, and a 401 marks the cache stale.
    if let Some(expires_ms) = oauth["expiresAt"].as_i64() {
        if expires_ms <= now.timestamp_millis() {
            return Err("token expired".into());
        }
    }
    Ok(token.to_owned())
}

/// Takes the fetch mark when no fetch started within the refresh interval. The mark is left in
/// place afterwards: its mtime is when the last attempt started, which holds every other status
/// line off for the interval, failed attempts included. Two processes reclaiming an old mark in
/// the same instant can both fetch; that costs one extra request.
fn claim(mark: &Path, now: DateTime<Utc>) -> bool {
    let create = || {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(mark)
            .is_ok()
    };
    if create() {
        return true;
    }
    let started = std::fs::metadata(mark).and_then(|m| m.modified());
    match started {
        Ok(t) if is_recent(t.into(), now) => false,
        _ => {
            let _ = std::fs::remove_file(mark);
            create()
        }
    }
}

/// Readers see the old file or the new one, never a partial write.
fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

fn cache_dir() -> PathBuf {
    match std::env::var_os("STATUSLINE_CACHE_DIR").filter(|d| !d.is_empty()) {
        Some(d) => PathBuf::from(d),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("cache"),
    }
}

fn credentials_path() -> Option<PathBuf> {
    let dir = match std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty()) {
        Some(d) => PathBuf::from(d),
        None => std::env::home_dir()?.join(".claude"),
    };
    Some(dir.join(".credentials.json"))
}

fn fetch(token: &str) -> Result<String, String> {
    use ureq::tls::{RootCerts, TlsConfig};
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(NETWORK_TIMEOUT))
        .http_status_as_error(false)
        .tls_config(
            TlsConfig::builder()
                .root_certs(RootCerts::PlatformVerifier)
                .build(),
        )
        .build()
        .into();
    let mut resp = agent
        .get(URL)
        .header("Authorization", format!("Bearer {token}"))
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("Content-Type", "application/json")
        .header(
            "User-Agent",
            concat!("statusline/", env!("CARGO_PKG_VERSION")),
        )
        .call()
        .map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    if status != 200 {
        return Err(format!("HTTP {status}"));
    }
    resp.body_mut().read_to_string().map_err(|e| e.to_string())
}

fn refresh(now: DateTime<Utc>) -> Result<Vec<Window>, String> {
    let path = credentials_path().ok_or("no home directory")?;
    let credentials =
        std::fs::read_to_string(path).map_err(|_| "no claude.ai login".to_string())?;
    let token = access_token(&credentials, now)?;
    parse_answer(&fetch(&token)?)
}

/// The cache as this redraw should draw it, with a fetch running when one is due.
pub struct Usage {
    cached: Option<Cache>,
    refreshed: Option<Receiver<Cache>>,
    deadline: Instant,
}

impl Usage {
    pub fn start(now: DateTime<Utc>) -> Usage {
        let deadline = Instant::now() + WAIT;
        let dir = cache_dir();
        let path = dir.join("usage.json");
        let cached = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| Cache::parse(&t));
        let due = !cached
            .as_ref()
            .and_then(|c| c.fetched_at)
            .is_some_and(|t| is_recent(t, now));
        if !due || std::fs::create_dir_all(&dir).is_err() || !claim(&dir.join("usage.fetch"), now) {
            return Usage {
                cached,
                refreshed: None,
                deadline,
            };
        }
        let (tx, rx) = mpsc::channel();
        let old = cached.clone();
        std::thread::spawn(move || {
            let next = match refresh(now) {
                Ok(windows) => Cache::fresh(windows, Utc::now()),
                Err(e) => Cache::failed(old, e),
            };
            if std::env::var_os("STATUSLINE_DEBUG").is_some() {
                eprintln!("usage refresh: {:?}", next.error);
            }
            let _ = write_atomic(&path, &next.to_json());
            let _ = tx.send(next);
        });
        Usage {
            cached,
            refreshed: Some(rx),
            deadline,
        }
    }

    /// The refreshed cache if it arrives in time, else the one read at the start.
    pub fn finish(self) -> Option<Cache> {
        let wait = self.deadline.saturating_duration_since(Instant::now());
        self.refreshed
            .and_then(|rx| rx.recv_timeout(wait).ok())
            .or(self.cached)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn window(name: &str, pct: f64, resets: Option<&str>, secs: i64) -> Window {
        Window {
            name: name.into(),
            used_percent: pct,
            resets_at: resets.map(utc),
            window_seconds: secs,
        }
    }

    /// The endpoint's answer on 22.11.2026, trimmed of the null-valued keys that change
    /// nothing; values as they came.
    const REAL_ANSWER: &str = r#"{
        "five_hour": null,
        "seven_day": {"utilization": 41.0, "resets_at": "2026-11-27T17:59:59.890436+00:00",
                      "limit_dollars": null, "used_dollars": null, "remaining_dollars": null,
                      "locked_reason": null},
        "seven_day_oauth_apps": null, "seven_day_opus": null, "seven_day_sonnet": null,
        "extra_usage": null,
        "limits": [
            {"kind": "weekly_all", "group": "weekly", "percent": 41, "severity": "normal",
             "resets_at": "2026-11-27T17:59:59.890436+00:00", "scope": null, "is_active": true},
            {"kind": "weekly_scoped", "group": "weekly", "percent": 32, "severity": "normal",
             "resets_at": "2026-11-27T17:59:59.890668+00:00",
             "scope": {"model": {"id": null, "display_name": "Fable"}, "surface": null},
             "is_active": false}
        ],
        "spend": null
    }"#;

    #[test]
    fn real_answer_gives_the_weekly_and_fable_windows_and_no_five_hour() {
        assert_eq!(
            parse_answer(REAL_ANSWER).unwrap(),
            vec![
                window(WEEKLY, 41.0, Some("2026-11-27T17:59:59.890436Z"), 604_800),
                window("Fable", 32.0, Some("2026-11-27T17:59:59.890668Z"), 604_800),
            ]
        );
    }

    #[test]
    fn a_five_hour_window_is_five_hours_long() {
        let body =
            r#"{"five_hour": {"utilization": 12.5, "resets_at": "2026-11-22T15:00:00+00:00"}}"#;
        assert_eq!(
            parse_answer(body).unwrap(),
            vec![window(
                FIVE_HOUR,
                12.5,
                Some("2026-11-22T15:00:00Z"),
                18_000
            )]
        );
    }

    #[test]
    fn resets_at_offsets_become_utc_and_bad_times_become_none() {
        let body = r#"{
            "seven_day": {"utilization": 1, "resets_at": "2026-11-27T19:59:59+02:00"},
            "limits": [{"kind": "weekly_scoped", "percent": 2, "resets_at": "next friday",
                        "scope": {"model": {"display_name": "Fable"}}}]
        }"#;
        assert_eq!(
            parse_answer(body).unwrap(),
            vec![
                window(WEEKLY, 1.0, Some("2026-11-27T17:59:59Z"), 604_800),
                window("Fable", 2.0, None, 604_800),
            ]
        );
    }

    #[test]
    fn windows_without_a_number_are_absent_and_zero_is_a_number() {
        let cases = [
            (r#"{}"#, vec![]),
            (
                r#"{"five_hour": null, "seven_day": null, "limits": null}"#,
                vec![],
            ),
            (
                r#"{"seven_day": {"utilization": null, "resets_at": "2026-11-27T17:59:59Z"}}"#,
                vec![],
            ),
            (
                r#"{"seven_day": {"resets_at": "2026-11-27T17:59:59Z"}}"#,
                vec![],
            ),
            (r#"{"limits": {}}"#, vec![]),
            (r#"{"limits": [3, null, "x"]}"#, vec![]),
            (
                r#"{"seven_day": {"utilization": 0, "resets_at": null}}"#,
                vec![window(WEEKLY, 0.0, None, 604_800)],
            ),
            (
                r#"{"limits": [{"kind": "weekly_scoped", "percent": 0, "resets_at": null,
                                "scope": {"model": {"display_name": "Fable"}}}]}"#,
                vec![window("Fable", 0.0, None, 604_800)],
            ),
        ];
        for (body, want) in cases {
            assert_eq!(parse_answer(body).unwrap(), want, "{body}");
        }
    }

    #[test]
    fn limits_rows_are_told_apart_by_kind_and_need_a_model_name_and_percent() {
        let row = |kind: &str, scope: &str, percent: &str| {
            format!(
                r#"{{"limits": [{{"kind": "{kind}", "percent": {percent}, "scope": {scope}}}]}}"#
            )
        };
        let fable = r#"{"model": {"display_name": "Fable"}}"#;
        let skipped = [
            row("weekly_all", fable, "5"),
            row("session", fable, "5"),
            row("weekly_scoped", "null", "5"),
            row("weekly_scoped", r#"{"model": null}"#, "5"),
            row("weekly_scoped", r#"{"model": {"display_name": null}}"#, "5"),
            row("weekly_scoped", r#"{"model": {"display_name": ""}}"#, "5"),
            row("weekly_scoped", fable, "null"),
        ];
        for body in &skipped {
            assert_eq!(parse_answer(body).unwrap(), vec![], "{body}");
        }
        assert_eq!(
            parse_answer(&row("weekly_scoped", fable, "5")).unwrap(),
            vec![window("Fable", 5.0, None, 604_800)]
        );
    }

    #[test]
    fn an_answer_that_is_not_a_json_object_is_an_error() {
        for body in ["", "<html>", "null", "[]", "41"] {
            assert!(parse_answer(body).is_err(), "{body:?}");
        }
    }

    #[test]
    fn recent_means_within_five_minutes_on_either_side() {
        let now = utc("2026-11-22T12:00:00Z");
        let at = |secs: i64| now - TimeDelta::seconds(secs);
        assert!(is_recent(at(0), now));
        assert!(is_recent(now - TimeDelta::milliseconds(299_999), now));
        assert!(!is_recent(at(300), now));
        assert!(!is_recent(at(3600), now));
        assert!(is_recent(at(-1), now));
        assert!(is_recent(at(-299), now));
        assert!(!is_recent(at(-300), now));
    }

    #[test]
    fn the_token_is_used_until_its_expiry_in_milliseconds() {
        let now = utc("2026-11-22T12:00:00Z");
        let ms = now.timestamp_millis();
        let creds = |expires: &str| {
            format!(
                r#"{{"claudeAiOauth": {{"accessToken": "tok-123", "refreshToken": "r", "expiresAt": {expires}}}}}"#
            )
        };
        assert_eq!(
            access_token(&creds(&(ms + 1).to_string()), now),
            Ok("tok-123".into())
        );
        assert_eq!(
            access_token(&creds(&ms.to_string()), now),
            Err("token expired".into())
        );
        assert_eq!(
            access_token(&creds(&(ms - 1000).to_string()), now),
            Err("token expired".into())
        );
        // An hour ahead written in seconds is long past in milliseconds.
        let seconds = (now.timestamp() + 3600).to_string();
        assert_eq!(
            access_token(&creds(&seconds), now),
            Err("token expired".into())
        );
        assert_eq!(access_token(&creds("null"), now), Ok("tok-123".into()));
    }

    #[test]
    fn a_missing_or_unreadable_login_is_an_error_that_quotes_nothing() {
        let now = utc("2026-11-22T12:00:00Z");
        for creds in [
            "",
            "{",
            r#"{"mcpOAuth": {}}"#,
            r#"{"claudeAiOauth": {"refreshToken": "secret-r"}}"#,
            r#"{"claudeAiOauth": {"accessToken": "", "refreshToken": "secret-r"}}"#,
        ] {
            let err = access_token(creds, now).unwrap_err();
            assert!(!err.contains("secret"), "{err}");
        }
    }

    /// A fresh directory under target/ for one test.
    fn scratch(name: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("unit-scratch")
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn one_claim_per_refresh_interval() {
        let mark = scratch("claim").join("usage.fetch");
        let now = Utc::now();
        assert!(claim(&mark, now), "no mark yet");
        assert!(!claim(&mark, now), "a fetch just started");
        let set_age = |secs: u64| {
            let t = std::time::SystemTime::now() - Duration::from_secs(secs);
            std::fs::File::options()
                .write(true)
                .open(&mark)
                .unwrap()
                .set_modified(t)
                .unwrap();
        };
        set_age(298);
        assert!(!claim(&mark, Utc::now()), "started 298 s ago");
        set_age(302);
        assert!(claim(&mark, Utc::now()), "started 302 s ago");
        assert!(
            !claim(&mark, Utc::now()),
            "the reclaim restarts the interval"
        );
    }

    #[test]
    fn write_atomic_replaces_the_file_and_leaves_no_temp_file() {
        let dir = scratch("atomic");
        let path = dir.join("usage.json");
        write_atomic(&path, "one").unwrap();
        write_atomic(&path, "two").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "two");
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["usage.json"]);
    }

    #[test]
    fn a_failed_refresh_keeps_the_old_windows_and_marks_them_stale() {
        let old = Cache::fresh(
            vec![window("Fable", 32.0, Some("2026-11-27T17:59:59Z"), 604_800)],
            utc("2026-11-22T10:00:00Z"),
        );
        let next = Cache::failed(Some(old.clone()), "HTTP 401".into());
        assert_eq!(next.windows, old.windows);
        assert_eq!(next.fetched_at, old.fetched_at);
        assert!(next.stale);
        assert_eq!(next.error.as_deref(), Some("HTTP 401"));

        let first = Cache::failed(None, "timeout".into());
        assert_eq!(first.windows, vec![]);
        assert_eq!(first.fetched_at, None);
        assert!(first.stale);
    }

    /// The keys a hook reads. Renaming one breaks the reader, so it fails here first.
    #[test]
    fn the_cache_file_keeps_its_contract() {
        let cache = Cache::fresh(
            vec![window(
                "Fable",
                32.0,
                Some("2026-11-27T17:59:59.890668Z"),
                604_800,
            )],
            utc("2026-11-22T10:15:03.5Z"),
        );
        let v: Value = serde_json::from_str(&cache.to_json()).unwrap();
        assert_eq!(
            v,
            json!({
                "version": 1,
                "fetched_at": "2026-11-22T10:15:03Z",
                "stale": false,
                "error": null,
                "windows": [{
                    "name": "Fable",
                    "used_percent": 32.0,
                    "resets_at": "2026-11-27T17:59:59Z",
                    "window_seconds": 604800,
                }],
            })
        );
    }

    #[test]
    fn a_cache_of_another_version_or_shape_is_not_read() {
        assert_eq!(Cache::parse(r#"{"version": 2, "windows": []}"#), None);
        assert_eq!(Cache::parse(r#"{"windows": []}"#), None);
        assert_eq!(Cache::parse(r#"{"version": 1}"#), None);
        assert_eq!(Cache::parse("not json"), None);
    }

    /// Random caches with names that need escaping, fractional percents and absent times
    /// come back from the file unchanged. Times are whole seconds, as the file stores them.
    #[test]
    fn random_caches_survive_the_file() {
        let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let names = [
            "Fable",
            WEEKLY,
            FIVE_HOUR,
            "Opus \"4\"",
            "Ćirilica Ж",
            "a\\b\nc",
            "",
        ];
        let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        for _ in 0..500 {
            let time = |n: u64| {
                (!n.is_multiple_of(4)).then(|| base + TimeDelta::seconds((n % 40_000_000) as i64))
            };
            let windows = (0..next() % 5)
                .map(|_| Window {
                    name: names[(next() % names.len() as u64) as usize].into(),
                    used_percent: (next() % 15_001) as f64 / 100.0,
                    resets_at: time(next()),
                    window_seconds: [18_000, 604_800][(next() % 2) as usize],
                })
                .collect();
            let cache = Cache {
                fetched_at: time(next()),
                stale: next() % 2 == 0,
                error: (next() % 2 == 0).then(|| "HTTP 5\"03\"".to_string()),
                windows,
            };
            assert_eq!(Cache::parse(&cache.to_json()), Some(cache.clone()));
        }
    }
}
