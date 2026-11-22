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

## Usage windows

Claude Code's stdin carries the 5-hour and weekly limits but not the per-model
weekly ones, such as Fable. The `usage-relay` mod fetches those in the background
and writes `~/.claude/usage-relay/usage.json` (`STATUSLINE_USAGE_FILE` overrides
the path). The status line only reads that file and fetches nothing. It draws
`F 32%`, with a dim `?` when the file says `stale: true`, and hides a window
whose `resets_at` has passed. Stdin wins for `5h` and `W`; the file fills one
that stdin lacks. A missing or unreadable file means no extra segments.

The file is version 1: `error`, `fetched_at`, `stale`, `version`, and
`windows[]` of `name` (`5-hour`, `weekly` or a model's name), `used_percent`,
`resets_at` and `window_seconds`.
