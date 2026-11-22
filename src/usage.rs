// Plan usage windows as the `usage-relay` mod writes them to ~/.claude/usage-relay/usage.json.
// This program only reads that file and draws; it never fetches. A redraw must not wait on
// the network, so everything slow lives in the mod. The bar takes from the file what stdin
// lacks (the per-model weekly windows, such as Fable).

use chrono::{DateTime, Utc};
use serde_json::Value;
use std::path::PathBuf;

pub const FIVE_HOUR: &str = "5-hour";
pub const WEEKLY: &str = "weekly";

#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    pub name: String,
    pub used_percent: f64,
    pub resets_at: Option<DateTime<Utc>>,
    pub window_seconds: i64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Cache {
    /// The relay's last refresh failed, so the windows are as old as `fetched_at`.
    pub stale: bool,
    pub windows: Vec<Window>,
}

impl Cache {
    /// Version 1 of the relay's file. Another version, or a file of another shape, is not read.
    /// A window missing a field is skipped; a missing `stale` counts as stale.
    pub fn parse(text: &str) -> Option<Cache> {
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
            stale: v["stale"].as_bool().unwrap_or(true),
            windows,
        })
    }
}

fn instant(v: &Value) -> Option<DateTime<Utc>> {
    let t = DateTime::parse_from_rfc3339(v.as_str()?).ok()?;
    Some(t.with_timezone(&Utc))
}

fn file_path() -> Option<PathBuf> {
    match std::env::var_os("STATUSLINE_USAGE_FILE").filter(|p| !p.is_empty()) {
        Some(p) => Some(PathBuf::from(p)),
        None => Some(
            std::env::home_dir()?
                .join(".claude")
                .join("usage-relay")
                .join("usage.json"),
        ),
    }
}

/// The relay's file, or None when it is missing or unreadable.
pub fn load() -> Option<Cache> {
    Cache::parse(&std::fs::read_to_string(file_path()?).ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn the_relays_file_is_read_as_documented() {
        let text = r#"{"version": 1, "fetched_at": "2026-11-22T10:15:03Z", "stale": false,
            "error": null, "windows": [
              {"name": "weekly", "used_percent": 41.0, "resets_at": "2026-11-27T19:59:59+02:00", "window_seconds": 604800},
              {"name": "Fable", "used_percent": 0, "resets_at": null, "window_seconds": 604800}]}"#;
        assert_eq!(
            Cache::parse(text),
            Some(Cache {
                stale: false,
                windows: vec![
                    Window {
                        name: WEEKLY.into(),
                        used_percent: 41.0,
                        resets_at: Some(utc("2026-11-27T17:59:59Z")),
                        window_seconds: 604_800
                    },
                    Window {
                        name: "Fable".into(),
                        used_percent: 0.0,
                        resets_at: None,
                        window_seconds: 604_800
                    },
                ]
            })
        );
    }

    #[test]
    fn a_missing_stale_flag_counts_as_stale_and_true_is_stale() {
        let stale = |s: &str| {
            Cache::parse(&format!(r#"{{"version": 1, {s} "windows": []}}"#))
                .unwrap()
                .stale
        };
        assert!(stale(""));
        assert!(stale(r#""stale": true,"#));
        assert!(!stale(r#""stale": false,"#));
        assert!(stale(r#""stale": "no","#));
    }

    #[test]
    fn a_window_missing_a_field_is_skipped_and_a_bad_time_is_none() {
        let text = r#"{"version": 1, "stale": false, "windows": [
            {"used_percent": 1, "window_seconds": 1},
            {"name": "a", "window_seconds": 1},
            {"name": "b", "used_percent": 1},
            {"name": "c", "used_percent": 1, "window_seconds": 1, "resets_at": "next friday"}]}"#;
        let c = Cache::parse(text).unwrap();
        assert_eq!(c.windows.len(), 1);
        assert_eq!(c.windows[0].name, "c");
        assert_eq!(c.windows[0].resets_at, None);
    }

    #[test]
    fn a_file_of_another_version_or_shape_is_not_read() {
        for text in [
            r#"{"version": 2, "windows": []}"#,
            r#"{"windows": []}"#,
            r#"{"version": 1}"#,
            r#"{"version": 1, "windows": {}}"#,
            "[]",
            "",
            "not json",
        ] {
            assert_eq!(Cache::parse(text), None, "{text:?}");
        }
    }
}
