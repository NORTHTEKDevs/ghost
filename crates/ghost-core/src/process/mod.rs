pub mod manager;
#[cfg(windows)]
pub mod cmdline;
#[cfg(windows)]
pub mod orphans;
pub use manager::{find_pid_by_name, launch, kill};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonexistent_process_returns_none() {
        let pid = find_pid_by_name("definitely_not_real_ghost_test_12345.exe");
        assert!(pid.is_none());
    }

    #[test]
    fn find_pid_case_insensitive() {
        // System Idle Process is always running; test case insensitivity
        // Try both cases - if "System" or "system" is running, both should find it
        let upper = find_pid_by_name("System");
        let lower = find_pid_by_name("system");
        // Both should return the same result (Some or both None, but same)
        assert_eq!(upper.is_some(), lower.is_some());
    }
}
