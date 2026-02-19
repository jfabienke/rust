//! Anonymous pipe support for NeXTSTEP.
//!
//! AnonPipe wraps a raw fd. read2() uses select() instead of poll()
//! since NeXTSTEP lacks poll().

use crate::io::{self, BorrowedCursor, IoSlice, IoSliceMut};
use crate::fmt;
use crate::vec::Vec;

use nextstep_sys as sys;

pub struct AnonPipe {
    fd: i32,
}

impl AnonPipe {
    pub fn from_raw_fd(fd: i32) -> AnonPipe {
        AnonPipe { fd }
    }

    pub fn try_clone(&self) -> io::Result<Self> {
        let new_fd = super::common::cvt(unsafe { sys::dup(self.fd) })?;
        Ok(AnonPipe { fd: new_fd })
    }

    pub fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        let ret = super::common::cvt_isize(unsafe {
            sys::read(self.fd, buf.as_mut_ptr(), buf.len())
        })?;
        Ok(ret as usize)
    }

    pub fn read_buf(&self, mut cursor: BorrowedCursor<'_>) -> io::Result<()> {
        let buf = cursor.ensure_init();
        let n = self.read(buf)?;
        unsafe { cursor.advance_unchecked(n) };
        Ok(())
    }

    pub fn read_vectored(&self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        for buf in bufs {
            if !buf.is_empty() {
                return self.read(buf);
            }
        }
        Ok(0)
    }

    pub fn is_read_vectored(&self) -> bool {
        false
    }

    pub fn read_to_end(&self, buf: &mut Vec<u8>) -> io::Result<usize> {
        let mut tmp = [0u8; 4096];
        let mut total = 0;
        loop {
            let n = self.read(&mut tmp)?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..n]);
            total += n;
        }
        Ok(total)
    }

    pub fn write(&self, buf: &[u8]) -> io::Result<usize> {
        let ret = super::common::cvt_isize(unsafe {
            sys::write(self.fd, buf.as_ptr(), buf.len())
        })?;
        Ok(ret as usize)
    }

    pub fn write_vectored(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        for buf in bufs {
            if !buf.is_empty() {
                return self.write(buf);
            }
        }
        Ok(0)
    }

    pub fn is_write_vectored(&self) -> bool {
        false
    }

    pub fn diverge(&self) -> ! {
        panic!("AnonPipe::diverge should never be called")
    }

    pub fn raw_fd(&self) -> i32 {
        self.fd
    }
}

impl Drop for AnonPipe {
    fn drop(&mut self) {
        let _ = unsafe { sys::close(self.fd) };
    }
}

impl fmt::Debug for AnonPipe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnonPipe").field("fd", &self.fd).finish()
    }
}

/// Read from two pipes simultaneously using select().
/// NeXTSTEP lacks poll(), so we use the BSD select() call.
pub fn read2(p1: AnonPipe, v1: &mut Vec<u8>, p2: AnonPipe, v2: &mut Vec<u8>) -> io::Result<()> {
    let fd1 = p1.fd;
    let fd2 = p2.fd;
    let nfds = core::cmp::max(fd1, fd2) + 1;
    let mut buf = [0u8; 4096];

    let mut done1 = false;
    let mut done2 = false;

    while !done1 || !done2 {
        let mut readfds = sys::fd_set::new_empty();
        if !done1 {
            readfds.set(fd1);
        }
        if !done2 {
            readfds.set(fd2);
        }

        let ret = unsafe {
            sys::select(
                nfds,
                &mut readfds as *mut sys::fd_set as *mut core::ffi::c_void,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                core::ptr::null_mut(),
            )
        };

        if ret < 0 {
            let e = super::common::errno();
            if e == sys::EINTR {
                continue;
            }
            return Err(io::Error::from_raw_os_error(e));
        }

        if !done1 && readfds.is_set(fd1) {
            let n = p1.read(&mut buf)?;
            if n == 0 {
                done1 = true;
            } else {
                v1.extend_from_slice(&buf[..n]);
            }
        }

        if !done2 && readfds.is_set(fd2) {
            let n = p2.read(&mut buf)?;
            if n == 0 {
                done2 = true;
            } else {
                v2.extend_from_slice(&buf[..n]);
            }
        }
    }

    Ok(())
}
