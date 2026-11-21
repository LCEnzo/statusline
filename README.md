# statusline

A program to render Claude Code's status line. Used to be in Linux, now on 
Windows. 

Build with `cargo build --release`; test with `cargo test` (it builds the fake
navigator in `examples/` that the integration tests use).

Point `statusLine.command` in `~/.claude/settings.json` at
`target/release/statusline.exe --navigator` to append navigator's segment, or
drop the flag for the base line alone. The flag runs `navigator statusline`
directly (`NAVIGATOR_BIN`, else `navigator` on `PATH`), which replaces
navigator's `contrib/nav-wrapper.sh` on Windows: Git Bash starts native
children suspended, and a cancelled refresh could strand them that way.

## Usage cache

Claude Code's stdin carries the 5-hour and weekly limits but not the per-model
weekly ones, such as Fable. Those come from the endpoint behind `/usage`
(`GET https://api.anthropic.com/api/oauth/usage?skip_spend=1`), called with the
access token in `~/.claude/.credentials.json`, which this program never
refreshes. All running status lines together fetch at most once per five
minutes, and write the answer atomically to `cache/usage.json` (gitignored;
`STATUSLINE_CACHE_DIR` moves it) for hooks to read:

```json
{"version": 1, "fetched_at": "2026-11-21T23:21:36Z", "stale": false, "error": null,
 "windows": [
  {"name": "weekly", "used_percent": 41.0, "resets_at": "2026-11-27T18:00:00Z", "window_seconds": 604800},
  {"name": "Fable", "used_percent": 32.0, "resets_at": "2026-11-27T17:59:59Z", "window_seconds": 604800}]}
```

`name` is `5-hour`, `weekly` or a model's name; a window the plan lacks is
absent. The window began at `resets_at` minus `window_seconds`; `resets_at` is
null when the endpoint gave none. `stale: true` means the last refresh failed
(`error` says why, for people), so the numbers are as old as `fetched_at`,
which is null before the first success. A change of shape gets a new `version`.
