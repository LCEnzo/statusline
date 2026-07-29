// Claude Code statusline. Receives session JSON on stdin, prints one
// ANSI-colored line on stdout. Reads .git directly where possible so a
// refresh costs at most two git subprocesses (status; diffstat when dirty).

use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

const RESET: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";
const BOLD_RED: &str = "\x1b[1;31m";
const CYAN: &str = "\x1b[36m";
const MAGENTA: &str = "\x1b[35m";
const DIM_BLUE: &str = "\x1b[2;34m";
const DIM_CYAN: &str = "\x1b[2;36m";
const ORANGE: &str = "\x1b[38;5;208m";

/// A statusline segment: ANSI text plus its visible (printable) width.
#[derive(Default)]
struct Seg {
    text: String,
    width: usize,
}

impl Seg {
    fn push(&mut self, color: &str, visible: &str) {
        if !color.is_empty() {
            self.text.push_str(color);
        }
        self.text.push_str(visible);
        if !color.is_empty() {
            self.text.push_str(RESET);
        }
        self.width += visible.chars().count();
    }
}

fn get_str<'a>(v: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut cur = v;
    for k in path {
        cur = cur.get(k)?;
    }
    cur.as_str().filter(|s| !s.is_empty() && *s != "null")
}

fn get_f64(v: &Value, path: &[&str]) -> Option<f64> {
    let mut cur = v;
    for k in path {
        cur = cur.get(k)?;
    }
    cur.as_f64()
}

fn usage_color(pct: i64) -> &'static str {
    if pct >= 90 {
        BOLD_RED
    } else if pct >= 80 {
        RED
    } else if pct >= 50 {
        CYAN
    } else {
        GREEN
    }
}

struct Repo {
    /// Per-worktree gitdir: contains HEAD. For linked worktrees this is
    /// <main>/.git/worktrees/<name>, discovered via the `.git` file.
    gitdir: PathBuf,
    /// Shared gitdir: contains logs/refs/stash. Resolved via `commondir`.
    commondir: PathBuf,
}

fn find_repo(start: &Path) -> Option<Repo> {
    for dir in start.ancestors() {
        let dotgit = dir.join(".git");
        if dotgit.is_dir() {
            return Some(Repo {
                commondir: dotgit.clone(),
                gitdir: dotgit,
            });
        }
        if dotgit.is_file() {
            if let Some(repo) = worktree_repo(dir, &dotgit) {
                return Some(repo);
            }
            // Unreadable or malformed .git file — keep walking; an
            // enclosing repo may still exist above it.
        }
    }
    None
}

/// Resolve a `.git` pointer file (linked worktree or submodule) to its gitdir.
fn worktree_repo(dir: &Path, dotgit: &Path) -> Option<Repo> {
    let content = std::fs::read_to_string(dotgit).ok()?;
    let target = content.strip_prefix("gitdir:")?.trim();
    let mut gitdir = PathBuf::from(target);
    if gitdir.is_relative() {
        gitdir = dir.join(gitdir);
    }
    let commondir = match std::fs::read_to_string(gitdir.join("commondir")) {
        Ok(c) => {
            let cd = PathBuf::from(c.trim());
            if cd.is_relative() {
                gitdir.join(cd)
            } else {
                cd
            }
        }
        Err(_) => gitdir.clone(),
    };
    Some(Repo { gitdir, commondir })
}

fn branch_name(gitdir: &Path) -> Option<String> {
    let head = std::fs::read_to_string(gitdir.join("HEAD")).ok()?;
    let head = head.trim();
    if let Some(r) = head.strip_prefix("ref:") {
        let r = r.trim();
        Some(r.strip_prefix("refs/heads/").unwrap_or(r).to_string())
    } else {
        // Detached HEAD: show the abbreviated commit.
        Some(head.chars().take(8).collect())
    }
}

#[derive(Default)]
struct GitStatus {
    ahead: i64,
    behind: i64,
    tracked_dirty: usize,
    untracked: usize,
}

fn git_status(cwd: &Path) -> Option<GitStatus> {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["--no-optional-locks", "status", "--porcelain=v2", "--branch"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut st = GitStatus::default();
    for line in text.lines() {
        if let Some(ab) = line.strip_prefix("# branch.ab ") {
            let mut it = ab.split_whitespace();
            st.ahead = it
                .next()
                .and_then(|a| a.trim_start_matches('+').parse().ok())
                .unwrap_or(0);
            st.behind = it
                .next()
                .and_then(|b| b.trim_start_matches('-').parse().ok())
                .unwrap_or(0);
        } else if line.starts_with('1') || line.starts_with('2') || line.starts_with('u') {
            st.tracked_dirty += 1;
        } else if line.starts_with('?') {
            st.untracked += 1;
        }
    }
    Some(st)
}

/// (insertions, deletions) of worktree+index vs HEAD. None on error or no changes.
fn diffstat(cwd: &Path) -> Option<(u64, u64)> {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["--no-optional-locks", "diff", "--shortstat", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    if text.trim().is_empty() {
        return None;
    }
    let (mut add, mut del) = (0u64, 0u64);
    for part in text.split(',') {
        let part = part.trim();
        let num: u64 = part
            .split(' ')
            .next()
            .and_then(|n| n.parse().ok())
            .unwrap_or(0);
        if part.contains("insertion") {
            add = num;
        } else if part.contains("deletion") {
            del = num;
        }
    }
    Some((add, del))
}

fn stash_count(commondir: &Path) -> usize {
    std::fs::read_to_string(commondir.join("logs").join("refs").join("stash"))
        .map(|s| s.lines().count())
        .unwrap_or(0)
}

#[cfg(windows)]
fn conout_width() -> Option<usize> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::Console::{
        GetConsoleScreenBufferInfo, CONSOLE_SCREEN_BUFFER_INFO,
    };
    let con = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("CONOUT$")
        .ok()?;
    let mut info: CONSOLE_SCREEN_BUFFER_INFO = unsafe { std::mem::zeroed() };
    let ok = unsafe { GetConsoleScreenBufferInfo(con.as_raw_handle() as _, &mut info) };
    if ok == 0 {
        return None;
    }
    Some((info.srWindow.Right - info.srWindow.Left + 1) as usize)
}

/// Parent pids of this process, nearest first, via a Toolhelp snapshot.
#[cfg(windows)]
fn ancestor_pids(max: usize) -> Vec<u32> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE {
        return Vec::new();
    }
    let mut parent = std::collections::HashMap::new();
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    if unsafe { Process32FirstW(snap, &mut entry) } != 0 {
        loop {
            parent.insert(entry.th32ProcessID, entry.th32ParentProcessID);
            if unsafe { Process32NextW(snap, &mut entry) } == 0 {
                break;
            }
        }
    }
    unsafe { CloseHandle(snap) };
    let mut out = Vec::new();
    let mut pid = std::process::id();
    while out.len() < max {
        match parent.get(&pid) {
            Some(&pp) if pp != 0 && pp != pid && !out.contains(&pp) => {
                out.push(pp);
                pid = pp;
            }
            _ => break,
        }
    }
    out
}

#[cfg(windows)]
fn term_width() -> Option<usize> {
    use windows_sys::Win32::System::Console::{AttachConsole, FreeConsole};
    // The console this process is spawned into is typically a hidden helper
    // conhost with a default-sized buffer that says nothing about the user's
    // terminal. The real console belongs to an ancestor (the terminal-attached
    // CLI process), so walk the process tree and take the console of the
    // farthest ancestor we can attach to, falling back to our own.
    let debug = std::env::var_os("STATUSLINE_DEBUG").is_some();
    let mut width = conout_width();
    if debug {
        eprintln!("own console width: {width:?}");
    }
    for pid in ancestor_pids(10) {
        unsafe { FreeConsole() };
        if unsafe { AttachConsole(pid) } != 0 {
            if let Some(w) = conout_width() {
                if debug {
                    eprintln!("ancestor {pid} console width: {w}");
                }
                width = Some(w);
            }
        } else if debug {
            eprintln!("ancestor {pid}: no attachable console");
        }
    }
    width
}

#[cfg(not(windows))]
fn term_width() -> Option<usize> {
    None
}

fn pct_seg(v: &Value, path: &[&str], label: &str) -> Option<Seg> {
    let pct = get_f64(v, path)?.round() as i64;
    let mut seg = Seg::default();
    seg.push(usage_color(pct), &format!("{label} {pct}%"));
    Some(seg)
}

/// Rate-limit segment: colored percentage plus a dim time-to-reset.
/// The 5h window (and a weekly reset landing today) shows hours/minutes;
/// a weekly reset on another day shows the weekday.
fn limit_seg(v: &Value, bucket: &str, label: &str, weekly: bool) -> Option<Seg> {
    let mut seg = pct_seg(v, &["rate_limits", bucket, "used_percentage"], label)?;
    let reset = get_f64(v, &["rate_limits", bucket, "resets_at"]);
    if let Some(ann) = reset.and_then(|r| reset_annotation(r as i64, weekly)) {
        seg.push("", " ");
        seg.push(DIM, &format!("({ann})"));
    }
    Some(seg)
}

fn reset_annotation(resets_at: i64, weekly: bool) -> Option<String> {
    use chrono::{DateTime, Local};
    let reset: DateTime<Local> = DateTime::from_timestamp(resets_at, 0)?.with_timezone(&Local);
    let now = Local::now();
    let mins = (reset - now).num_minutes();
    if mins <= 0 {
        return None;
    }
    if weekly && reset.date_naive() != now.date_naive() {
        return Some(reset.format("%a").to_string());
    }
    Some(if mins >= 60 {
        format!("{}h", (mins + 59) / 60)
    } else {
        format!("{}m", mins.max(1))
    })
}

fn main() {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).ok();
    let v: Value = serde_json::from_str(&input).unwrap_or(Value::Null);

    let display_name = get_str(&v, &["model", "display_name"]).unwrap_or("Claude");
    let model_id = get_str(&v, &["model", "id"]).unwrap_or("");
    let cwd: PathBuf = get_str(&v, &["workspace", "current_dir"])
        .or_else(|| get_str(&v, &["cwd"]))
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default();
    let project_dir = get_str(&v, &["workspace", "project_dir"]).unwrap_or("");

    // Model: full variant carries the id, short variant is the trim fallback.
    let mut model_full = Seg::default();
    let mut model_short = Seg::default();
    model_short.push(ORANGE, display_name);
    if model_id.is_empty() {
        model_full.push(ORANGE, display_name);
    } else {
        model_full.push(ORANGE, &format!("{display_name} [{model_id}]"));
    }

    // Location: project name + relative path when inside a project subdir.
    let mut loc = Seg::default();
    let pd = Path::new(project_dir);
    if !project_dir.is_empty() && cwd != pd && cwd.starts_with(pd) {
        let name = pd
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let rel = cwd
            .strip_prefix(pd)
            .map(|r| r.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        loc.push(DIM_BLUE, &name);
        loc.push("", "/");
        loc.push(DIM_CYAN, &rel);
    } else {
        let base = cwd
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| cwd.to_string_lossy().into_owned());
        loc.push(DIM_CYAN, &base);
    }

    let git_seg = find_repo(&cwd).map(|repo| {
        // Run both git calls concurrently; the diffstat is simply discarded
        // when the tree turns out to be clean.
        let diff_cwd = cwd.clone();
        let diff_handle = std::thread::spawn(move || diffstat(&diff_cwd));
        let status = git_status(&cwd);
        let diff = diff_handle.join().unwrap_or(None);

        let mut g = Seg::default();
        let branch = branch_name(&repo.gitdir).unwrap_or_else(|| "?".into());
        match status {
            Some(st) => {
                let dirty = st.tracked_dirty > 0;
                let branch_color = if dirty { YELLOW } else { GREEN };
                g.push(branch_color, &branch);
                g.push("", " ");
                g.push(branch_color, if dirty { "●" } else { "✓" });
                if !dirty && st.untracked > 0 {
                    g.push("", " ");
                    g.push(DIM, &format!("?{}", st.untracked));
                }
                if st.ahead > 0 {
                    g.push("", " ");
                    g.push(CYAN, &format!("↑{}", st.ahead));
                }
                if st.behind > 0 {
                    g.push("", " ");
                    g.push(MAGENTA, &format!("↓{}", st.behind));
                }
                if dirty {
                    if let Some((add, del)) = diff {
                        g.push("", " ");
                        g.push(GREEN, &format!("+{add}"));
                        g.push("", " ");
                        g.push(RED, &format!("-{del}"));
                    }
                }
            }
            // git errored or isn't on PATH: state is unknown, don't claim clean.
            None => {
                g.push(DIM, &branch);
                g.push("", " ");
                g.push(DIM, "?");
            }
        }
        let stashes = stash_count(&repo.commondir);
        if stashes > 0 {
            g.push("", " ");
            g.push(DIM, &format!("⚑{stashes}"));
        }
        g
    });

    // Fields available but unused so far: `pr` {number, url, review_state},
    // `workspace.git_worktree`, `session_name`, `vim.mode`. A PR segment
    // would be the most useful next addition, though it overlaps with the
    // branch/worktree info already shown.
    let ctx_seg = pct_seg(&v, &["context_window", "used_percentage"], "Context");
    let five_seg = limit_seg(&v, "five_hour", "5h", false);
    let week_seg = limit_seg(&v, "seven_day", "W", true);

    let cost_seg = get_f64(&v, &["cost", "total_cost_usd"]).map(|c| {
        let mut seg = Seg::default();
        seg.push(DIM, &format!("${c:.2}"));
        seg
    });

    let session_seg = get_str(&v, &["session_id"]).map(|id| {
        let mut seg = Seg::default();
        seg.push(DIM, id);
        seg
    });

    // Assemble; when the console is narrow, drop the model id first, then cost.
    // The session UUID is always kept.
    let budget = term_width().unwrap_or(usize::MAX).saturating_sub(3);
    let mut full_model = true;
    let mut with_cost = true;
    let line = loop {
        let mut segs: Vec<&Seg> = Vec::new();
        segs.push(if full_model { &model_full } else { &model_short });
        segs.push(&loc);
        for opt in [&git_seg, &ctx_seg, &five_seg, &week_seg] {
            if let Some(s) = opt {
                segs.push(s);
            }
        }
        if with_cost {
            if let Some(s) = &cost_seg {
                segs.push(s);
            }
        }
        if let Some(s) = &session_seg {
            segs.push(s);
        }

        let width: usize = segs.iter().map(|s| s.width).sum::<usize>() + 3 * (segs.len() - 1);
        if width <= budget || (!full_model && !with_cost) {
            let sep = format!(" {DIM}\u{2022}{RESET} ");
            break segs
                .iter()
                .map(|s| s.text.as_str())
                .collect::<Vec<_>>()
                .join(&sep);
        }
        if full_model {
            full_model = false;
        } else {
            with_cost = false;
        }
    };

    println!("{line}");
}
