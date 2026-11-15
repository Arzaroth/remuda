/// When a process started, which tells it apart from a later one given the
/// same pid: clock ticks since boot on Linux, microseconds since the epoch on
/// macOS. `None` once it is gone.
#[cfg(not(target_os = "macos"))]
pub fn start_time(pid: u32) -> Option<u64> {
    selvedge::proc::start_time(pid)
}

#[cfg(target_os = "macos")]
pub fn start_time(pid: u32) -> Option<u64> {
    let info = bsd_info(pid)?;
    Some(info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec)
}

#[cfg(target_os = "macos")]
const ZOMBIE: u32 = 5;

/// Whether `pid` runs as this user: a process it may signal that is not
/// known to be a zombie, even when macOS will not describe it.
#[cfg(target_os = "macos")]
pub fn runs_as_me(pid: u32) -> bool {
    // SAFETY: signal 0 only checks that the process exists and may be signalled.
    let signalled = unsafe { libc::kill(pid as libc::pid_t, 0) } == 0;
    live_and_mine(signalled, state(pid))
}

/// The process's `p_stat` from `KERN_PROC_PID`, which also answers for a
/// zombie, where `proc_pidinfo` does not. The `kinfo_proc` it fills starts
/// with `extern_proc`, whose `p_stat` follows two list pointers, two more
/// pointers and the `p_flag` int.
#[cfg(target_os = "macos")]
fn state(pid: u32) -> Option<u32> {
    const P_STAT: usize = 36;
    let mut mib = [
        libc::CTL_KERN,
        libc::KERN_PROC,
        libc::KERN_PROC_PID,
        pid as libc::c_int,
    ];
    let mut info = [0u8; 1024];
    let mut len = info.len();
    // SAFETY: `info` is `len` bytes long, and sysctl writes at most that.
    let rc = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            4,
            info.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    (rc == 0 && len > P_STAT).then(|| u32::from(info[P_STAT]))
}

#[cfg(target_os = "macos")]
fn live_and_mine(signalled: bool, status: Option<u32>) -> bool {
    signalled && status != Some(ZOMBIE)
}

#[cfg(target_os = "macos")]
fn bsd_info(pid: u32) -> Option<libc::proc_bsdinfo> {
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
    Some(unsafe { info.assume_init() })
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

    #[cfg(target_os = "macos")]
    #[test]
    fn a_process_macos_will_not_describe_still_counts_as_mine() {
        assert!(live_and_mine(true, None));
        assert!(live_and_mine(true, Some(2)));
        assert!(!live_and_mine(true, Some(ZOMBIE)));
        assert!(!live_and_mine(false, None));
        assert!(runs_as_me(std::process::id()));
        assert!(!runs_as_me(1));
        let mut child = std::process::Command::new("/usr/bin/true").spawn().unwrap();
        let pid = child.id();
        while state(pid) != Some(ZOMBIE) {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!runs_as_me(pid));
        child.wait().unwrap();
    }

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
