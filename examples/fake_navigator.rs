// Stand-in for `navigator` in tests/navigator.rs. Records its argv and stdin under
// $FAKE_NAV_RECORD and prints $FAKE_NAV_OUT (default "[nav]") with a CRLF, as Python does.

use std::io::Read;
use std::path::Path;

fn main() {
    let mut stdin = Vec::new();
    std::io::stdin().read_to_end(&mut stdin).unwrap();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(dir) = std::env::var_os("FAKE_NAV_RECORD") {
        let dir = Path::new(&dir);
        std::fs::write(dir.join("args.txt"), args.join("\n")).unwrap();
        std::fs::write(dir.join("stdin.bin"), &stdin).unwrap();
    }
    let out = std::env::var("FAKE_NAV_OUT").unwrap_or_else(|_| "[nav]".into());
    if !out.is_empty() {
        print!("{out}\r\n");
    }
}
