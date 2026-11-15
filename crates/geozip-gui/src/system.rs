use std::io;
use std::path::Path;
use std::process::Command;

#[cfg(target_os = "windows")]
const OPENER: &str = "explorer";
#[cfg(target_os = "macos")]
const OPENER: &str = "open";
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const OPENER: &str = "xdg-open";

pub fn open_with_default_program(path: &Path) -> io::Result<()> {
    Command::new(OPENER).arg(path).spawn().map(drop)
}
