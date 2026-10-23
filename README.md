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
