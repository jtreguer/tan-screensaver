//! SIGINT, SIGTERM and SIGHUP end the screensaver cleanly (SPEC §2, Exiting).
//!
//! A signal handler may only call async-signal-safe functions, so it writes the signal
//! number to a socket; a thread blocked on the other end passes it to the event loop.
//! The handful of libc calls are declared here rather than pulling in a crate for them.

use std::io::Read;
use std::os::fd::IntoRawFd;
use std::os::raw::{c_int, c_void};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration;

const SIGHUP: c_int = 1;
const SIGINT: c_int = 2;
const SIGTERM: c_int = 15;
const SIG_ERR: usize = usize::MAX;
/// After a signal the event loop gets this long to end cleanly. A main thread stuck in
/// a present (a stalled compositor, say) must not make the screensaver unkillable short
/// of SIGKILL.
const GRACE: Duration = Duration::from_secs(2);

extern "C" {
    fn signal(signum: c_int, handler: extern "C" fn(c_int)) -> usize;
    fn write(fd: c_int, buf: *const c_void, count: usize) -> isize;
    fn __errno_location() -> *mut c_int;
}

static WRITE_FD: AtomicI32 = AtomicI32::new(-1);

extern "C" fn on_signal(signum: c_int) {
    let byte = signum as u8;
    // SAFETY: write(2) is async-signal-safe; the fd stays open for the life of the process.
    // errno is restored, since the handler may run between a failed call and its check.
    unsafe {
        let errno = *__errno_location();
        write(
            WRITE_FD.load(Ordering::Relaxed),
            (&byte as *const u8).cast(),
            1,
        );
        *__errno_location() = errno;
    }
}

/// Calls `on_signal_number` from a background thread with the number of the first signal
/// received, then exits the process with status 0 on a second signal or after `GRACE`.
pub fn install(on_signal_number: impl Fn(i32) + Send + 'static) -> Result<(), String> {
    let (mut reader, writer) =
        UnixStream::pair().map_err(|e| format!("cannot watch signals: {e}"))?;
    WRITE_FD.store(writer.into_raw_fd(), Ordering::Relaxed);
    std::thread::Builder::new()
        .name("signals".into())
        .spawn(move || {
            let mut byte = [0u8; 1];
            if reader.read_exact(&mut byte).is_err() {
                return;
            }
            on_signal_number(byte[0] as i32);
            // A timeout or a second signal both end the wait.
            let _ = reader.set_read_timeout(Some(GRACE));
            let _ = reader.read_exact(&mut byte);
            std::process::exit(0);
        })
        .map_err(|e| format!("cannot watch signals: {e}"))?;
    for signum in [SIGHUP, SIGINT, SIGTERM] {
        // SAFETY: glibc's signal() installs a persistent handler with SA_RESTART, and
        // on_signal only makes async-signal-safe calls.
        if unsafe { signal(signum, on_signal) } == SIG_ERR {
            return Err(format!("cannot handle signal {signum}"));
        }
    }
    Ok(())
}
