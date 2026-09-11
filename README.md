# statusline

A program to render Claude Code's status line. Used to be in Linux, now on 
Windows. 

Build with `cargo build --release`. 

Point `statusLine.command` in `~/.claude/settings.json` at 
`target/release/statusline.exe`, or at navigator's `contrib/nav-wrapper.sh`, 
which runs this build as the base half and appends navigator's segment.
