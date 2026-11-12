/// When a process started, which tells it apart from a later one given the
/// same pid. `None` once it is gone.
pub fn start_time(pid: u32) -> Option<u64> {
    selvedge::proc::start_time(pid)
}

/// `start_time` the way Claude Code writes it as `procStart` in
/// `sessions/<pid>.json`.
pub fn claude_proc_start(start: u64) -> String {
    start.to_string()
}
