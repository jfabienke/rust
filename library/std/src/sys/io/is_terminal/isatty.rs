use crate::os::fd::{AsFd, AsRawFd};

pub fn is_terminal(fd: &impl AsFd) -> bool {
    let fd = fd.as_fd();
    #[cfg(not(target_os = "nextstep"))]
    {
        unsafe { libc::isatty(fd.as_raw_fd()) != 0 }
    }
    #[cfg(target_os = "nextstep")]
    {
        unsafe { nextstep_sys::isatty(fd.as_raw_fd()) != 0 }
    }
}
