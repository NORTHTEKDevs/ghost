//! Launching applications.
//!
//! The Windows engine needs a registry "App Paths" fallback because no browser
//! is on `PATH` there. Linux is the opposite: applications are normally on
//! `PATH`, so plain resolution works. The gaps worth covering are desktop-entry
//! names (`org.gnome.Nautilus`) and URLs, both handled below.
//!
//! Children are spawned fully detached with stdio nulled. A GUI app inheriting
//! the MCP server's stdout would corrupt the JSON-RPC stream on stdout -- which
//! is the transport Ghost speaks to Claude over.

use std::process::{Command, Stdio};

use crate::error::{CoreError, Result};

/// Common aliases so an agent can say "chrome" on any distro.
const ALIASES: &[(&str, &[&str])] = &[
    ("chrome", &["google-chrome", "google-chrome-stable", "chromium", "chromium-browser"]),
    ("chromium", &["chromium", "chromium-browser"]),
    ("firefox", &["firefox"]),
    ("edge", &["microsoft-edge", "microsoft-edge-stable"]),
    ("terminal", &["gnome-terminal", "konsole", "xfce4-terminal", "kitty", "alacritty"]),
    ("files", &["nautilus", "dolphin", "thunar"]),
    ("editor", &["gedit", "gnome-text-editor", "kate"]),
];

fn on_path(cmd: &str) -> bool {
    let Ok(path) = std::env::var("PATH") else { return false };
    std::env::split_paths(&path).any(|dir| {
        let p = dir.join(cmd);
        p.is_file()
    })
}

/// Resolve a user-supplied program name to something executable.
pub fn resolve(cmd: &str) -> Option<String> {
    if cmd.contains('/') && std::path::Path::new(cmd).is_file() {
        return Some(cmd.to_string());
    }
    if on_path(cmd) {
        return Some(cmd.to_string());
    }
    let lc = cmd.to_ascii_lowercase();
    for (alias, candidates) in ALIASES {
        if lc == *alias {
            for c in *candidates {
                if on_path(c) {
                    return Some((*c).to_string());
                }
            }
        }
    }
    None
}

/// Launch a program (or open a URL), returning its pid.
///
/// Signature matches `ghost_core::process::manager::launch`.
pub fn launch(exe: &str) -> Result<u32> {
    launch_with_args(exe, &[])
}

/// Launch with explicit arguments.
pub fn launch_with_args(cmd: &str, args: &[String]) -> Result<u32> {
    // A URL is opened with the desktop's default handler rather than treated as
    // a binary name.
    if cmd.starts_with("http://") || cmd.starts_with("https://") {
        return spawn("xdg-open", &[cmd.to_string()]);
    }

    if let Some(program) = resolve(cmd) {
        return spawn(&program, args);
    }

    // Desktop-entry id, e.g. "org.gnome.Nautilus" or "firefox.desktop".
    if cmd.contains('.') && on_path("gtk-launch") {
        let entry = cmd.trim_end_matches(".desktop").to_string();
        if let Ok(pid) = spawn("gtk-launch", &[entry]) {
            return Ok(pid);
        }
    }

    Err(CoreError::NotFound(format!(
        "cannot launch {cmd:?}: not on PATH and no desktop entry matched. \
         Try the full path, or the real binary name (e.g. 'google-chrome' rather than 'chrome')"
    )))
}

fn spawn(program: &str, args: &[String]) -> Result<u32> {
    Command::new(program)
        .args(args)
        // Never let a child write to the MCP server's stdout: that stream is
        // the JSON-RPC transport.
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|c| c.id())
        .map_err(|e| CoreError::platform(format!("failed to launch {program}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_paths_to_real_files_resolve() {
        assert_eq!(resolve("/bin/sh"), Some("/bin/sh".to_string()));
    }

    #[test]
    fn nonexistent_programs_do_not_resolve() {
        assert!(resolve("definitely-not-a-real-program-xyzzy").is_none());
    }

    #[test]
    fn launch_failure_names_the_program_and_suggests_a_fix() {
        let err = launch("definitely-not-a-real-program-xyzzy").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("definitely-not-a-real-program-xyzzy"));
        assert!(msg.contains("PATH"));
    }

    #[test]
    fn aliases_are_lowercase_so_lookup_matches() {
        for (alias, _) in ALIASES {
            assert_eq!(*alias, alias.to_ascii_lowercase());
        }
    }
}
