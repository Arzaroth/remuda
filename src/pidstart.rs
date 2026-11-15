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

/// Whether `pid` runs as this user, with this user's effective uid, is not a
/// zombie and never changed its credentials (`P_SUGID`, as `login` and an
/// SSH session do, which macOS keeps their arguments from), even when macOS
/// will not describe it otherwise. One `KERN_PROC_PID` will not describe
/// either counts as this user's when it may be signalled.
#[cfg(target_os = "macos")]
pub fn runs_as_me(pid: u32) -> bool {
    // SAFETY: geteuid cannot fail.
    let me = unsafe { libc::geteuid() };
    live_and_mine(me, record(pid), || {
        // SAFETY: signal 0 only checks that the process exists and may be signalled.
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    })
}

/// The process's `p_stat` and effective uid from `KERN_PROC_PID`, which
/// answers for a zombie and a setuid process, where `proc_pidinfo` and
/// `KERN_PROCARGS2` do not. The 64-bit `kinfo_proc` has `p_flag` and
/// `p_stat` after `extern_proc`'s four pointers, and the uid in
/// `kp_eproc`'s `e_ucred`, after two pointers and `e_pcred`.
#[cfg(target_os = "macos")]
fn record(pid: u32) -> Option<Record> {
    const P_FLAG: usize = 32;
    const P_STAT: usize = 36;
    const CR_UID: usize = 420;
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
    let word = |at: usize| u32::from_ne_bytes(info[at..at + 4].try_into().unwrap());
    (rc == 0 && len >= CR_UID + 4).then(|| Record {
        flag: word(P_FLAG),
        stat: u32::from(info[P_STAT]),
        uid: word(CR_UID),
    })
}

#[cfg(target_os = "macos")]
fn live_and_mine(me: u32, record: Option<Record>, signalled: impl Fn() -> bool) -> bool {
    match record {
        Some(r) => r.uid == me && r.stat != ZOMBIE && r.flag & P_SUGID == 0,
        None => signalled(),
    }
}

#[cfg(target_os = "macos")]
const P_SUGID: u32 = 0x100;

#[cfg(target_os = "macos")]
#[derive(Clone, Copy)]
struct Record {
    flag: u32,
    stat: u32,
    uid: u32,
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
    fn a_zombie_another_uid_and_changed_credentials_are_not_mine() {
        let run = |flag, stat, uid| Some(Record { flag, stat, uid });
        let mine = |record| live_and_mine(501, record, || true);
        assert!(mine(run(0x4004, 2, 501)));
        assert!(!mine(run(0x4004, ZOMBIE, 501)));
        assert!(!mine(run(0x4004, 2, 0)));
        assert!(!mine(run(0x104, 2, 501)));
        assert!(mine(None));
        assert!(!live_and_mine(501, None, || false));
        assert!(runs_as_me(std::process::id()));
        assert!(!runs_as_me(1));
        let mut child = std::process::Command::new("/usr/bin/true").spawn().unwrap();
        let pid = child.id();
        while record(pid).map(|r| r.stat) != Some(ZOMBIE) {
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
