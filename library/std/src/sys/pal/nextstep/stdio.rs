//! Standard I/O for NeXTSTEP.
//!
//! Wraps file descriptors 0/1/2 with Read/Write impls using nextstep_sys.

use crate::io;

pub struct Stdin;
pub struct Stdout;
pub struct Stderr;

impl Stdin {
    pub const fn new() -> Stdin {
        Stdin
    }
}

impl io::Read for Stdin {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let ret = unsafe {
            nextstep_sys::read(nextstep_sys::STDIN_FILENO, buf.as_mut_ptr(), buf.len())
        };
        if ret < 0 {
            Err(io::Error::from_raw_os_error(super::common::errno()))
        } else {
            Ok(ret as usize)
        }
    }
}

impl Stdout {
    pub const fn new() -> Stdout {
        Stdout
    }
}

impl io::Write for Stdout {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let ret = unsafe {
            nextstep_sys::write(nextstep_sys::STDOUT_FILENO, buf.as_ptr(), buf.len())
        };
        if ret < 0 {
            Err(io::Error::from_raw_os_error(super::common::errno()))
        } else {
            Ok(ret as usize)
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Stderr {
    pub const fn new() -> Stderr {
        Stderr
    }
}

impl io::Write for Stderr {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let ret = unsafe {
            nextstep_sys::write(nextstep_sys::STDERR_FILENO, buf.as_ptr(), buf.len())
        };
        if ret < 0 {
            Err(io::Error::from_raw_os_error(super::common::errno()))
        } else {
            Ok(ret as usize)
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub const STDIN_BUF_SIZE: usize = 4096;

pub fn is_ebadf(err: &io::Error) -> bool {
    err.raw_os_error() == Some(nextstep_sys::EBADF)
}

pub fn panic_output() -> Option<impl io::Write> {
    Some(Stderr::new())
}
