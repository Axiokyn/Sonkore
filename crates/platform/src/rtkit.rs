//! Real-time scheduling for the audio thread, through
//! rtkit: the D-Bus service PipeWire and PulseAudio use; in a Flatpak, the
//! Realtime portal in front of it. An ordinary user may not set SCHED_FIFO
//! directly, rtkit grants it within safe limits.

use std::path::Path;
use std::time::Duration;

use crate::bus::{Bus, get_property, value_to_i64};

/// Where rtkit is reached: rtkit itself on the system bus, or inside a
/// Flatpak sandbox (which cannot reach it) the Realtime portal on the
/// session bus. Same methods and properties, and the portal maps the
/// sandbox's PIDs to the host's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Service {
    pub name: &'static str,
    pub path: &'static str,
    pub interface: &'static str,
    pub system_bus: bool,
}

pub const RTKIT: Service = Service {
    name: "org.freedesktop.RealtimeKit1",
    path: "/org/freedesktop/RealtimeKit1",
    interface: "org.freedesktop.RealtimeKit1",
    system_bus: true,
};

pub const PORTAL: Service = Service {
    name: "org.freedesktop.portal.Desktop",
    path: "/org/freedesktop/portal/desktop",
    interface: "org.freedesktop.portal.Realtime",
    system_bus: false,
};

/// The service for this process: the portal inside a Flatpak.
pub fn service() -> Service {
    if Path::new("/.flatpak-info").exists() { PORTAL } else { RTKIT }
}

/// The calling thread's kernel id (what `tid` means below).
pub fn current_tid() -> i64 {
    // SAFETY: gettid has no preconditions.
    i64::from(unsafe { libc::gettid() })
}

/// Priority of `tid` (a thread of this process) when it already runs under
/// SCHED_FIFO / SCHED_RR, otherwise 0.
pub fn current_priority(tid: i64) -> i32 {
    let Ok(tid) = libc::pid_t::try_from(tid) else {
        return 0;
    };
    // SAFETY: plain syscalls on a thread id; `param` is a valid out pointer.
    unsafe {
        // rtkit sets SCHED_RR with SCHED_RESET_ON_FORK, which comes back or'ed in.
        let policy = libc::sched_getscheduler(tid);
        if policy < 0 {
            return 0;
        }
        let policy = policy & !libc::SCHED_RESET_ON_FORK;
        if policy != libc::SCHED_FIFO && policy != libc::SCHED_RR {
            return 0;
        }
        let mut param = libc::sched_param { sched_priority: 0 };
        if libc::sched_getparam(tid, &mut param) == 0 { param.sched_priority } else { 0 }
    }
}

/// Asks rtkit for real-time scheduling (SCHED_RR) of `tid` at the highest
/// priority it allows. Returns the priority granted, or 0 when rtkit is
/// missing or refuses. Blocks for the D-Bus round trips: call it from the
/// thread being set up (or any worker), not the UI thread.
pub fn make_realtime(tid: i64) -> i32 {
    let rt = service();
    let bus = if rt.system_bus { Bus::System } else { Bus::Session };
    make_realtime_on(&bus, rt, tid)
}

/// [`make_realtime`] through the given bus and service.
pub fn make_realtime_on(bus: &Bus, rt: Service, tid: i64) -> i32 {
    let Ok(conn) = bus.connect(Duration::from_secs(5)) else {
        return 0;
    };
    let property =
        |name: &str| get_property(&conn, rt.name, rt.path, rt.interface, name).ok().and_then(|v| value_to_i64(&v));
    let priority = property("MaxRealtimePriority").unwrap_or(0);
    let rt_time = property("RTTimeUSecMax").unwrap_or(0);
    if priority <= 0 || rt_time <= 0 {
        return 0;
    }
    // rtkit only serves processes whose real-time CPU time is capped: a
    // runaway real-time thread then gets SIGXCPU instead of freezing the system.
    let limit = libc::rlimit { rlim_cur: rt_time as libc::rlim_t, rlim_max: rt_time as libc::rlim_t };
    // SAFETY: `limit` is a valid rlimit.
    if unsafe { libc::setrlimit(libc::RLIMIT_RTTIME, &limit) } != 0 {
        return 0;
    }
    let pid = u64::from(std::process::id());
    let Ok(tid_u) = u64::try_from(tid) else {
        return 0;
    };
    let call = conn.call_method(
        Some(rt.name),
        rt.path,
        Some(rt.interface),
        "MakeThreadRealtimeWithPID",
        &(pid, tid_u, priority as u32),
    );
    if call.is_err() {
        return 0;
    }
    current_priority(tid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ordinary_thread_has_no_rt_priority() {
        assert_eq!(current_priority(current_tid()), 0);
        assert_eq!(current_priority(-1), 0);
    }

    #[test]
    fn picks_the_service() {
        let s = service();
        assert!(s == RTKIT || s == PORTAL);
        assert_eq!(s == PORTAL, Path::new("/.flatpak-info").exists());
    }

    /// Asks the real rtkit for a thread that then ends. Run by hand:
    /// `cargo test -p sonkore-platform -- --ignored rtkit`.
    #[test]
    #[ignore]
    fn rtkit_grants_a_priority() {
        let granted = std::thread::spawn(|| make_realtime(current_tid())).join().unwrap();
        eprintln!("rtkit granted priority {granted}");
    }
}
