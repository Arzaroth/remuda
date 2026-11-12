/// When a process started, which tells it apart from a later one given the
/// same pid: clock ticks since boot on Linux, microseconds since the epoch on
/// macOS. `None` once it is gone.
#[cfg(not(target_os = "macos"))]
pub fn start_time(pid: u32) -> Option<u64> {
    selvedge::proc::start_time(pid)
}

#[cfg(target_os = "macos")]
pub fn start_time(pid: u32) -> Option<u64> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: `info` is as large as the size given, and read only once
    // proc_pidinfo says it filled it whole.
    let read = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if read != size {
        return None;
    }
    // SAFETY: filled above.
    let info = unsafe { info.assume_init() };
    Some(info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec)
}

/// `start_time` the way Claude Code writes it as `procStart` in
/// `sessions/<pid>.json`: the `/proc/<pid>/stat` field on Linux, and
/// `LC_ALL=C TZ=UTC ps -o lstart=` trimmed on macOS.
#[cfg(not(target_os = "macos"))]
pub fn claude_proc_start(start: u64) -> String {
    start.to_string()
}

#[cfg(target_os = "macos")]
pub fn claude_proc_start(start: u64) -> String {
    jiff::Timestamp::from_second((start / 1_000_000) as i64)
        .map(|t| t.strftime("%a %b %e %H:%M:%S %Y").to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_has_a_start_time_and_a_gone_one_none() {
        assert!(start_time(std::process::id()).is_some());
        assert_eq!(start_time(999_999_999), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn claude_s_proc_start_is_what_ps_prints() {
        let pid = std::process::id();
        let out = std::process::Command::new("ps")
            .args(["-o", "lstart=", "-p", &pid.to_string()])
            .env("LC_ALL", "C")
            .env("TZ", "UTC")
            .output()
            .unwrap();
        let printed = String::from_utf8(out.stdout).unwrap();
        assert_eq!(claude_proc_start(start_time(pid).unwrap()), printed.trim());
        assert_eq!(
            claude_proc_start(1_791_420_078_000_000),
            "Thu Oct  8 00:41:18 2026"
        );
    }
}
