//! Common helpers for the NeXTSTEP PAL.
//!
//! Provides errno access, error conversion, and process initialization.

use crate::io;

// Statics for argc/argv saved during init()
static mut ARGC: isize = 0;
static mut ARGV: *const *const u8 = core::ptr::null();

/// SAFETY: must be called only once during runtime initialization.
pub unsafe fn init(argc: isize, argv: *const *const u8, _sigpipe: u8) {
    unsafe {
        ARGC = argc;
        ARGV = argv;
    }
}

/// SAFETY: must be called only once during runtime cleanup.
pub unsafe fn cleanup() {}

/// Read the argc value saved during init.
pub fn argc() -> isize {
    unsafe { ARGC }
}

/// Read the argv value saved during init.
pub fn argv() -> *const *const u8 {
    unsafe { ARGV }
}

/// Read the NeXTSTEP errno global.
pub fn errno() -> i32 {
    nextstep_sys::get_errno()
}

/// Set the NeXTSTEP errno global.
pub fn set_errno(e: i32) {
    nextstep_sys::set_errno(e);
}

/// Convert a libc return value (-1 = error) to io::Result.
pub fn cvt(ret: i32) -> io::Result<i32> {
    if ret == -1 {
        Err(io::Error::from_raw_os_error(errno()))
    } else {
        Ok(ret)
    }
}

/// Convert an isize libc return value (-1 = error) to io::Result.
pub fn cvt_isize(ret: isize) -> io::Result<isize> {
    if ret == -1 {
        Err(io::Error::from_raw_os_error(errno()))
    } else {
        Ok(ret)
    }
}

/// Check if an errno value indicates EINTR.
pub fn is_interrupted(code: i32) -> bool {
    code == nextstep_sys::EINTR
}

/// Map a NeXTSTEP errno to an io::ErrorKind.
pub fn decode_error_kind(errno: i32) -> io::ErrorKind {
    use io::ErrorKind;
    use nextstep_sys::*;
    match errno {
        EACCES | EPERM => ErrorKind::PermissionDenied,
        ENOENT => ErrorKind::NotFound,
        EINTR => ErrorKind::Interrupted,
        EAGAIN => ErrorKind::WouldBlock,
        EEXIST => ErrorKind::AlreadyExists,
        EINVAL => ErrorKind::InvalidInput,
        EPIPE => ErrorKind::BrokenPipe,
        ENOTDIR => ErrorKind::NotADirectory,
        EISDIR => ErrorKind::IsADirectory,
        ENOSYS => ErrorKind::Unsupported,
        ENOMEM => ErrorKind::OutOfMemory,
        EADDRINUSE => ErrorKind::AddrInUse,
        EADDRNOTAVAIL => ErrorKind::AddrNotAvailable,
        ECONNREFUSED => ErrorKind::ConnectionRefused,
        ECONNRESET => ErrorKind::ConnectionReset,
        ECONNABORTED => ErrorKind::ConnectionAborted,
        ENOTCONN => ErrorKind::NotConnected,
        ETIMEDOUT => ErrorKind::TimedOut,
        EALREADY => ErrorKind::AlreadyExists,
        _ => ErrorKind::Uncategorized,
    }
}

pub fn unsupported<T>() -> io::Result<T> {
    Err(unsupported_err())
}

pub fn unsupported_err() -> io::Error {
    io::Error::UNSUPPORTED_PLATFORM
}

pub fn abort_internal() -> ! {
    core::intrinsics::abort();
}
