//! Panics from reading untrusted files are caught where the file is read
//! (a damaged file is an error, not a crash); any other panic is a bug and
//! ends the app as before (release used to be `panic = "abort"`).
//!
//! `catch` marks the thread while it runs; the app's panic hook calls
//! `catching()` and aborts when the panic happened outside one. Without the
//! hook (tests), panics unwind as usual.

use std::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind};

thread_local! {
    static DEPTH: Cell<u32> = const { Cell::new(0) };
}

/// Runs `f`, which reads data from a file; a panic in it comes back as Err.
pub fn catch<R>(f: impl FnOnce() -> R) -> std::thread::Result<R> {
    DEPTH.with(|d| d.set(d.get() + 1));
    let r = catch_unwind(AssertUnwindSafe(f));
    DEPTH.with(|d| d.set(d.get() - 1));
    r
}

/// This thread is inside `catch`: a panic now is caught there.
pub fn catching() -> bool {
    DEPTH.with(|d| d.get() > 0)
}

/// The panic's message, for an error text.
pub fn message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_the_thread_while_it_runs() {
        assert!(!catching());
        assert!(catch(catching).unwrap());
        assert!(!catching());
        let r = catch(|| -> u32 { panic!("damaged") });
        assert_eq!(message(r.unwrap_err().as_ref()), "damaged");
        assert!(!catching(), "back to 0 after a panic");
    }
}
