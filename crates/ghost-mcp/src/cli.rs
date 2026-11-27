//! Command-line flags. ghost-mcp is started by an MCP host and talks over
//! stdio; `--version` and `--help` answer and exit before anything starts
//! (input hooks, the audit, the orphaned-browser sweep). Other arguments are
//! ignored, as before, so a host that passes extra ones keeps working.

const HELP: &str = "\
ghost-mcp - the Ghost MCP server (desktop and browser automation)

Usage: ghost-mcp [--version | --help]

Started by an MCP host (Claude Desktop, Claude Code, Cursor...), it speaks
JSON-RPC over stdio: point the host's MCP config at this program.
Settings come from the environment, e.g. GHOST_FOCUS_POLICY, GHOST_SHELL=off,
GHOST_LAUNCH_ALLOW; see the README. `ghost verify` checks this machine.

Options:
  -V, --version  Print the version and exit
  -h, --help     Print this help and exit";

/// What to print instead of serving, when the arguments ask for it.
pub fn cli_answer(args: &[String]) -> Option<String> {
    args.iter().find_map(|a| match a.as_str() {
        "--version" | "-V" => Some(format!("ghost-mcp {}", env!("CARGO_PKG_VERSION"))),
        "--help" | "-h" => Some(HELP.to_string()),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn version_and_help_answer_and_everything_else_serves() {
        for v in ["--version", "-V"] {
            let out = cli_answer(&args(&[v])).expect(v);
            assert_eq!(out, format!("ghost-mcp {}", env!("CARGO_PKG_VERSION")));
        }
        for h in ["--help", "-h"] {
            let out = cli_answer(&args(&[h])).expect(h);
            assert!(out.contains("Usage: ghost-mcp"), "{out}");
            assert!(out.contains("stdio"), "{out}");
            assert!(out.contains("--version"), "{out}");
        }
        assert_eq!(cli_answer(&args(&[])), None);
        assert_eq!(cli_answer(&args(&["--some-host-flag"])), None);
        // The first recognised flag wins, wherever it is.
        assert!(cli_answer(&args(&["--some-host-flag", "--version"])).is_some());
    }
}
