# statusline

Claude Code's status line, in Rust, for Windows. Claude Code pipes the session
JSON to it on stdin, and it prints one ANSI-coloured line: model, location in
the project, git branch and state (clean or dirty, ahead/behind, diffstat,
stashes), context use, the 5-hour and weekly rate limits with time to reset,
session cost, and session ID.

A refresh costs at most two git subprocesses; the rest is read straight from
`.git`. When the console is too narrow it drops the model ID first, then the
cost; the session ID always stays. The width comes from the terminal's console,
found by walking up the process tree, so trimming only happens on Windows.

Build with `cargo build --release`. Point `statusLine.command` in
`~/.claude/settings.json` at `target/release/statusline.exe`, or at navigator's
`contrib/nav-wrapper.sh`, which runs this build as the base half and appends
navigator's segment.
