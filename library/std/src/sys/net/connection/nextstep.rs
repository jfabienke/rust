//! BSD socket networking implementation for NeXTSTEP (m68k).
//!
//! IPv4 only — NeXTSTEP has no IPv6 support.
//! Uses 4.3BSD socket API directly via nextstep_sys FFI bindings.

use crate::ffi::c_int;
use crate::fmt;
use crate::io::{self, BorrowedCursor, ErrorKind, IoSlice, IoSliceMut};
use crate::net::{Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, SocketAddrV4};
use crate::time::Duration;

// Re-use nextstep_sys types/functions.
// nextstep_sys is linked via the sysroot rlib (extern crate in the PAL module).
use nextstep_sys as sys;
use core::ffi::c_void;
use core::mem;
use core::ptr;

// =========================================================================
// Helpers
// =========================================================================

fn cvt(ret: c_int) -> io::Result<c_int> {
    if ret < 0 {
        Err(io::Error::from_raw_os_error(sys::get_errno()))
    } else {
        Ok(ret)
    }
}

fn cvt_ssize(ret: isize) -> io::Result<usize> {
    if ret < 0 {
        Err(io::Error::from_raw_os_error(sys::get_errno()))
    } else {
        Ok(ret as usize)
    }
}

fn unsupported_v6() -> io::Error {
    io::Error::new(ErrorKind::Unsupported, "IPv6 not available on NeXTSTEP")
}

fn require_v4(addr: &SocketAddr) -> io::Result<&SocketAddrV4> {
    match addr {
        SocketAddr::V4(a) => Ok(a),
        SocketAddr::V6(_) => Err(unsupported_v6()),
    }
}

fn std_to_sockaddr(addr: &SocketAddrV4) -> sys::sockaddr_in {
    sys::sockaddr_in {
        sin_len: mem::size_of::<sys::sockaddr_in>() as u8,
        sin_family: sys::AF_INET as sys::sa_family_t,
        sin_port: addr.port().to_be(),
        sin_addr: sys::in_addr {
            s_addr: u32::from_ne_bytes(addr.ip().octets()),
        },
        sin_zero: [0u8; 8],
    }
}

fn sockaddr_to_std(addr: &sys::sockaddr_in) -> SocketAddr {
    let ip = Ipv4Addr::from(addr.sin_addr.s_addr.to_ne_bytes());
    let port = u16::from_be(addr.sin_port);
    SocketAddr::V4(SocketAddrV4::new(ip, port))
}

fn socket_addr_from_raw(
    storage: *const sys::sockaddr_storage,
    _len: sys::socklen_t,
) -> io::Result<SocketAddr> {
    unsafe {
        if (*storage).ss_family as c_int == sys::AF_INET {
            let sa_in = &*(storage as *const sys::sockaddr_in);
            Ok(sockaddr_to_std(sa_in))
        } else {
            Err(io::Error::new(ErrorKind::InvalidInput, "unsupported address family"))
        }
    }
}

fn setsockopt_raw(fd: c_int, level: c_int, name: c_int, val: *const c_void, len: sys::socklen_t) -> io::Result<()> {
    cvt(unsafe { sys::setsockopt(fd, level, name, val, len) })?;
    Ok(())
}

fn setsockopt_int(fd: c_int, level: c_int, name: c_int, val: c_int) -> io::Result<()> {
    setsockopt_raw(fd, level, name, &val as *const c_int as *const c_void, mem::size_of::<c_int>() as sys::socklen_t)
}

fn getsockopt_int(fd: c_int, level: c_int, name: c_int) -> io::Result<c_int> {
    let mut val: c_int = 0;
    let mut len: sys::socklen_t = mem::size_of::<c_int>() as sys::socklen_t;
    cvt(unsafe {
        sys::getsockopt(fd, level, name, &mut val as *mut c_int as *mut c_void, &mut len)
    })?;
    Ok(val)
}

fn sockname_fn<F: FnOnce(*mut sys::sockaddr, *mut sys::socklen_t) -> c_int>(f: F) -> io::Result<SocketAddr> {
    unsafe {
        let mut storage: sys::sockaddr_storage = mem::zeroed();
        let mut len = mem::size_of::<sys::sockaddr_storage>() as sys::socklen_t;
        cvt(f(&mut storage as *mut _ as *mut sys::sockaddr, &mut len))?;
        socket_addr_from_raw(&storage, len)
    }
}

fn duration_to_timeval(dur: Duration) -> sys::timeval {
    sys::timeval {
        tv_sec: dur.as_secs() as sys::time_t,
        tv_usec: dur.subsec_micros() as sys::c_long,
    }
}

fn timeval_to_duration(tv: &sys::timeval) -> Option<Duration> {
    if tv.tv_sec == 0 && tv.tv_usec == 0 {
        None
    } else {
        Some(Duration::new(tv.tv_sec as u64, (tv.tv_usec as u32) * 1000))
    }
}

fn set_timeout(fd: c_int, dur: Option<Duration>, opt: c_int) -> io::Result<()> {
    let tv = match dur {
        Some(d) => {
            if d.is_zero() {
                return Err(io::Error::new(ErrorKind::InvalidInput, "cannot set a 0 duration timeout"));
            }
            duration_to_timeval(d)
        }
        None => sys::timeval { tv_sec: 0, tv_usec: 0 },
    };
    setsockopt_raw(
        fd,
        sys::SOL_SOCKET,
        opt,
        &tv as *const sys::timeval as *const c_void,
        mem::size_of::<sys::timeval>() as sys::socklen_t,
    )
}

fn get_timeout(fd: c_int, opt: c_int) -> io::Result<Option<Duration>> {
    let mut tv: sys::timeval = unsafe { mem::zeroed() };
    let mut len = mem::size_of::<sys::timeval>() as sys::socklen_t;
    cvt(unsafe {
        sys::getsockopt(fd, sys::SOL_SOCKET, opt, &mut tv as *mut _ as *mut c_void, &mut len)
    })?;
    Ok(timeval_to_duration(&tv))
}

fn set_nonblocking(fd: c_int, nonblocking: bool) -> io::Result<()> {
    let flags = cvt(unsafe { sys::fcntl(fd, sys::F_GETFL, 0 as c_int) })?;
    let flags = if nonblocking {
        flags | sys::O_NONBLOCK
    } else {
        flags & !sys::O_NONBLOCK
    };
    cvt(unsafe { sys::fcntl(fd, sys::F_SETFL, flags) })?;
    Ok(())
}

fn socket_new(sock_type: c_int) -> io::Result<c_int> {
    let fd = cvt(unsafe { sys::socket(sys::AF_INET, sock_type, 0) })?;
    Ok(fd)
}

fn socket_close(fd: c_int) {
    unsafe { sys::close(fd); }
}

fn socket_dup(fd: c_int) -> io::Result<c_int> {
    cvt(unsafe { sys::dup(fd) })
}

fn take_error(fd: c_int) -> io::Result<Option<io::Error>> {
    let errno = getsockopt_int(fd, sys::SOL_SOCKET, sys::SO_ERROR)?;
    if errno == 0 {
        Ok(None)
    } else {
        Ok(Some(io::Error::from_raw_os_error(errno)))
    }
}

// =========================================================================
// TcpStream
// =========================================================================

pub struct TcpStream {
    fd: c_int,
}

impl TcpStream {
    pub fn connect(addr: io::Result<&SocketAddr>) -> io::Result<TcpStream> {
        let addr = addr?;
        let v4 = require_v4(addr)?;
        let fd = socket_new(sys::SOCK_STREAM)?;
        let sa = std_to_sockaddr(v4);
        let ret = cvt(unsafe {
            sys::connect(fd, &sa as *const _ as *const sys::sockaddr, mem::size_of::<sys::sockaddr_in>() as sys::socklen_t)
        });
        if let Err(e) = ret {
            socket_close(fd);
            return Err(e);
        }
        Ok(TcpStream { fd })
    }

    pub fn connect_timeout(addr: &SocketAddr, timeout: Duration) -> io::Result<TcpStream> {
        let v4 = require_v4(addr)?;
        let fd = socket_new(sys::SOCK_STREAM)?;

        // Set non-blocking for the connect
        if let Err(e) = set_nonblocking(fd, true) {
            socket_close(fd);
            return Err(e);
        }

        let sa = std_to_sockaddr(v4);
        let ret = unsafe {
            sys::connect(fd, &sa as *const _ as *const sys::sockaddr, mem::size_of::<sys::sockaddr_in>() as sys::socklen_t)
        };

        if ret < 0 {
            let errno = sys::get_errno();
            if errno != sys::EINPROGRESS {
                socket_close(fd);
                return Err(io::Error::from_raw_os_error(errno));
            }

            // Wait for connect with select()
            let mut tv = duration_to_timeval(timeout);
            let mut wfds = sys::fd_set::new_empty();
            wfds.set(fd);
            let sel = cvt(unsafe {
                sys::select(
                    fd + 1,
                    ptr::null_mut(),
                    &mut wfds as *mut _ as *mut c_void,
                    ptr::null_mut(),
                    &mut tv,
                )
            });
            match sel {
                Err(e) => {
                    socket_close(fd);
                    return Err(e);
                }
                Ok(0) => {
                    socket_close(fd);
                    return Err(io::Error::new(ErrorKind::TimedOut, "connection timed out"));
                }
                Ok(_) => {
                    // Check SO_ERROR
                    let err = getsockopt_int(fd, sys::SOL_SOCKET, sys::SO_ERROR);
                    match err {
                        Ok(0) => {}
                        Ok(e) => {
                            socket_close(fd);
                            return Err(io::Error::from_raw_os_error(e));
                        }
                        Err(e) => {
                            socket_close(fd);
                            return Err(e);
                        }
                    }
                }
            }
        }

        // Restore blocking mode
        if let Err(e) = set_nonblocking(fd, false) {
            socket_close(fd);
            return Err(e);
        }

        Ok(TcpStream { fd })
    }

    pub fn set_read_timeout(&self, dur: Option<Duration>) -> io::Result<()> {
        set_timeout(self.fd, dur, sys::SO_RCVTIMEO)
    }

    pub fn set_write_timeout(&self, dur: Option<Duration>) -> io::Result<()> {
        set_timeout(self.fd, dur, sys::SO_SNDTIMEO)
    }

    pub fn read_timeout(&self) -> io::Result<Option<Duration>> {
        get_timeout(self.fd, sys::SO_RCVTIMEO)
    }

    pub fn write_timeout(&self) -> io::Result<Option<Duration>> {
        get_timeout(self.fd, sys::SO_SNDTIMEO)
    }

    pub fn peek(&self, buf: &mut [u8]) -> io::Result<usize> {
        cvt_ssize(unsafe {
            sys::recv(self.fd, buf.as_mut_ptr() as *mut c_void, buf.len(), sys::MSG_PEEK)
        })
    }

    pub fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        cvt_ssize(unsafe {
            sys::recv(self.fd, buf.as_mut_ptr() as *mut c_void, buf.len(), 0)
        })
    }

    pub fn read_buf(&self, mut cursor: BorrowedCursor<'_>) -> io::Result<()> {
        let buf = cursor.ensure_init().init_mut();
        let n = self.read(buf)?;
        unsafe { cursor.advance_unchecked(n); }
        Ok(())
    }

    pub fn read_vectored(&self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        cvt_ssize(unsafe {
            sys::readv(self.fd, bufs.as_ptr() as *const c_void, bufs.len() as c_int)
        }.try_into().unwrap_or(-1))
    }

    pub fn is_read_vectored(&self) -> bool {
        true
    }

    pub fn write(&self, buf: &[u8]) -> io::Result<usize> {
        cvt_ssize(unsafe {
            sys::send(self.fd, buf.as_ptr() as *const c_void, buf.len(), 0)
        })
    }

    pub fn write_vectored(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        cvt_ssize(unsafe {
            sys::writev(self.fd, bufs.as_ptr() as *const c_void, bufs.len() as c_int)
        }.try_into().unwrap_or(-1))
    }

    pub fn is_write_vectored(&self) -> bool {
        true
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        sockname_fn(|buf, len| unsafe { sys::getpeername(self.fd, buf, len) })
    }

    pub fn socket_addr(&self) -> io::Result<SocketAddr> {
        sockname_fn(|buf, len| unsafe { sys::getsockname(self.fd, buf, len) })
    }

    pub fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        let how = match how {
            Shutdown::Read => sys::SHUT_RD,
            Shutdown::Write => sys::SHUT_WR,
            Shutdown::Both => sys::SHUT_RDWR,
        };
        cvt(unsafe { sys::shutdown(self.fd, how) })?;
        Ok(())
    }

    pub fn duplicate(&self) -> io::Result<TcpStream> {
        socket_dup(self.fd).map(|fd| TcpStream { fd })
    }

    pub fn set_linger(&self, linger: Option<Duration>) -> io::Result<()> {
        let val = sys::linger {
            l_onoff: linger.is_some() as c_int,
            l_linger: linger.map_or(0, |d| d.as_secs() as c_int),
        };
        setsockopt_raw(
            self.fd,
            sys::SOL_SOCKET,
            sys::SO_LINGER,
            &val as *const _ as *const c_void,
            mem::size_of::<sys::linger>() as sys::socklen_t,
        )
    }

    pub fn linger(&self) -> io::Result<Option<Duration>> {
        let mut val: sys::linger = unsafe { mem::zeroed() };
        let mut len = mem::size_of::<sys::linger>() as sys::socklen_t;
        cvt(unsafe {
            sys::getsockopt(self.fd, sys::SOL_SOCKET, sys::SO_LINGER, &mut val as *mut _ as *mut c_void, &mut len)
        })?;
        if val.l_onoff == 0 {
            Ok(None)
        } else {
            Ok(Some(Duration::from_secs(val.l_linger as u64)))
        }
    }

    pub fn set_nodelay(&self, nodelay: bool) -> io::Result<()> {
        setsockopt_int(self.fd, sys::IPPROTO_TCP, sys::TCP_NODELAY, nodelay as c_int)
    }

    pub fn nodelay(&self) -> io::Result<bool> {
        getsockopt_int(self.fd, sys::IPPROTO_TCP, sys::TCP_NODELAY).map(|v| v != 0)
    }

    pub fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        setsockopt_int(self.fd, sys::IPPROTO_IP, sys::IP_TTL, ttl as c_int)
    }

    pub fn ttl(&self) -> io::Result<u32> {
        getsockopt_int(self.fd, sys::IPPROTO_IP, sys::IP_TTL).map(|v| v as u32)
    }

    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        take_error(self.fd)
    }

    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        set_nonblocking(self.fd, nonblocking)
    }
}

impl Drop for TcpStream {
    fn drop(&mut self) {
        socket_close(self.fd);
    }
}

impl fmt::Debug for TcpStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut res = f.debug_struct("TcpStream");
        if let Ok(addr) = self.socket_addr() {
            res.field("addr", &addr);
        }
        if let Ok(peer) = self.peer_addr() {
            res.field("peer", &peer);
        }
        res.field("fd", &self.fd).finish()
    }
}

// =========================================================================
// TcpListener
// =========================================================================

pub struct TcpListener {
    fd: c_int,
}

impl TcpListener {
    pub fn bind(addr: io::Result<&SocketAddr>) -> io::Result<TcpListener> {
        let addr = addr?;
        let v4 = require_v4(addr)?;
        let fd = socket_new(sys::SOCK_STREAM)?;

        // SO_REUSEADDR for quick rebind
        if let Err(e) = setsockopt_int(fd, sys::SOL_SOCKET, sys::SO_REUSEADDR, 1) {
            socket_close(fd);
            return Err(e);
        }

        let sa = std_to_sockaddr(v4);
        let ret = cvt(unsafe {
            sys::bind(fd, &sa as *const _ as *const sys::sockaddr, mem::size_of::<sys::sockaddr_in>() as sys::socklen_t)
        });
        if let Err(e) = ret {
            socket_close(fd);
            return Err(e);
        }

        let ret = cvt(unsafe { sys::listen(fd, 128) });
        if let Err(e) = ret {
            socket_close(fd);
            return Err(e);
        }

        Ok(TcpListener { fd })
    }

    pub fn socket_addr(&self) -> io::Result<SocketAddr> {
        sockname_fn(|buf, len| unsafe { sys::getsockname(self.fd, buf, len) })
    }

    pub fn accept(&self) -> io::Result<(TcpStream, SocketAddr)> {
        let mut storage: sys::sockaddr_storage = unsafe { mem::zeroed() };
        let mut len = mem::size_of::<sys::sockaddr_storage>() as sys::socklen_t;
        let new_fd = cvt(unsafe {
            sys::accept(self.fd, &mut storage as *mut _ as *mut sys::sockaddr, &mut len)
        })?;
        let addr = socket_addr_from_raw(&storage, len)?;
        Ok((TcpStream { fd: new_fd }, addr))
    }

    pub fn duplicate(&self) -> io::Result<TcpListener> {
        socket_dup(self.fd).map(|fd| TcpListener { fd })
    }

    pub fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        setsockopt_int(self.fd, sys::IPPROTO_IP, sys::IP_TTL, ttl as c_int)
    }

    pub fn ttl(&self) -> io::Result<u32> {
        getsockopt_int(self.fd, sys::IPPROTO_IP, sys::IP_TTL).map(|v| v as u32)
    }

    pub fn set_only_v6(&self, _only_v6: bool) -> io::Result<()> {
        Err(unsupported_v6())
    }

    pub fn only_v6(&self) -> io::Result<bool> {
        Err(unsupported_v6())
    }

    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        take_error(self.fd)
    }

    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        set_nonblocking(self.fd, nonblocking)
    }
}

impl Drop for TcpListener {
    fn drop(&mut self) {
        socket_close(self.fd);
    }
}

impl fmt::Debug for TcpListener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut res = f.debug_struct("TcpListener");
        if let Ok(addr) = self.socket_addr() {
            res.field("addr", &addr);
        }
        res.field("fd", &self.fd).finish()
    }
}

// =========================================================================
// UdpSocket
// =========================================================================

pub struct UdpSocket {
    fd: c_int,
}

impl UdpSocket {
    pub fn bind(addr: io::Result<&SocketAddr>) -> io::Result<UdpSocket> {
        let addr = addr?;
        let v4 = require_v4(addr)?;
        let fd = socket_new(sys::SOCK_DGRAM)?;
        let sa = std_to_sockaddr(v4);
        let ret = cvt(unsafe {
            sys::bind(fd, &sa as *const _ as *const sys::sockaddr, mem::size_of::<sys::sockaddr_in>() as sys::socklen_t)
        });
        if let Err(e) = ret {
            socket_close(fd);
            return Err(e);
        }
        Ok(UdpSocket { fd })
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        sockname_fn(|buf, len| unsafe { sys::getpeername(self.fd, buf, len) })
    }

    pub fn socket_addr(&self) -> io::Result<SocketAddr> {
        sockname_fn(|buf, len| unsafe { sys::getsockname(self.fd, buf, len) })
    }

    pub fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        let mut storage: sys::sockaddr_storage = unsafe { mem::zeroed() };
        let mut addrlen = mem::size_of::<sys::sockaddr_storage>() as sys::socklen_t;
        let n = cvt_ssize(unsafe {
            sys::recvfrom(
                self.fd,
                buf.as_mut_ptr() as *mut c_void,
                buf.len(),
                0,
                &mut storage as *mut _ as *mut sys::sockaddr,
                &mut addrlen,
            )
        })?;
        let addr = socket_addr_from_raw(&storage, addrlen)?;
        Ok((n, addr))
    }

    pub fn peek_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        let mut storage: sys::sockaddr_storage = unsafe { mem::zeroed() };
        let mut addrlen = mem::size_of::<sys::sockaddr_storage>() as sys::socklen_t;
        let n = cvt_ssize(unsafe {
            sys::recvfrom(
                self.fd,
                buf.as_mut_ptr() as *mut c_void,
                buf.len(),
                sys::MSG_PEEK,
                &mut storage as *mut _ as *mut sys::sockaddr,
                &mut addrlen,
            )
        })?;
        let addr = socket_addr_from_raw(&storage, addrlen)?;
        Ok((n, addr))
    }

    pub fn send_to(&self, buf: &[u8], dst: &SocketAddr) -> io::Result<usize> {
        let v4 = require_v4(dst)?;
        let sa = std_to_sockaddr(v4);
        cvt_ssize(unsafe {
            sys::sendto(
                self.fd,
                buf.as_ptr() as *const c_void,
                buf.len(),
                0,
                &sa as *const _ as *const sys::sockaddr,
                mem::size_of::<sys::sockaddr_in>() as sys::socklen_t,
            )
        })
    }

    pub fn duplicate(&self) -> io::Result<UdpSocket> {
        socket_dup(self.fd).map(|fd| UdpSocket { fd })
    }

    pub fn set_read_timeout(&self, dur: Option<Duration>) -> io::Result<()> {
        set_timeout(self.fd, dur, sys::SO_RCVTIMEO)
    }

    pub fn set_write_timeout(&self, dur: Option<Duration>) -> io::Result<()> {
        set_timeout(self.fd, dur, sys::SO_SNDTIMEO)
    }

    pub fn read_timeout(&self) -> io::Result<Option<Duration>> {
        get_timeout(self.fd, sys::SO_RCVTIMEO)
    }

    pub fn write_timeout(&self) -> io::Result<Option<Duration>> {
        get_timeout(self.fd, sys::SO_SNDTIMEO)
    }

    pub fn set_broadcast(&self, broadcast: bool) -> io::Result<()> {
        setsockopt_int(self.fd, sys::SOL_SOCKET, sys::SO_BROADCAST, broadcast as c_int)
    }

    pub fn broadcast(&self) -> io::Result<bool> {
        getsockopt_int(self.fd, sys::SOL_SOCKET, sys::SO_BROADCAST).map(|v| v != 0)
    }

    pub fn set_multicast_loop_v4(&self, multicast_loop_v4: bool) -> io::Result<()> {
        setsockopt_int(self.fd, sys::IPPROTO_IP, sys::IP_MULTICAST_LOOP, multicast_loop_v4 as c_int)
    }

    pub fn multicast_loop_v4(&self) -> io::Result<bool> {
        getsockopt_int(self.fd, sys::IPPROTO_IP, sys::IP_MULTICAST_LOOP).map(|v| v != 0)
    }

    pub fn set_multicast_ttl_v4(&self, ttl: u32) -> io::Result<()> {
        setsockopt_int(self.fd, sys::IPPROTO_IP, sys::IP_MULTICAST_TTL, ttl as c_int)
    }

    pub fn multicast_ttl_v4(&self) -> io::Result<u32> {
        getsockopt_int(self.fd, sys::IPPROTO_IP, sys::IP_MULTICAST_TTL).map(|v| v as u32)
    }

    pub fn set_multicast_loop_v6(&self, _multicast_loop_v6: bool) -> io::Result<()> {
        Err(unsupported_v6())
    }

    pub fn multicast_loop_v6(&self) -> io::Result<bool> {
        Err(unsupported_v6())
    }

    pub fn join_multicast_v4(&self, multiaddr: &Ipv4Addr, interface: &Ipv4Addr) -> io::Result<()> {
        let mreq = sys::ip_mreq {
            imr_multiaddr: sys::in_addr { s_addr: u32::from_ne_bytes(multiaddr.octets()) },
            imr_interface: sys::in_addr { s_addr: u32::from_ne_bytes(interface.octets()) },
        };
        setsockopt_raw(
            self.fd,
            sys::IPPROTO_IP,
            sys::IP_ADD_MEMBERSHIP,
            &mreq as *const _ as *const c_void,
            mem::size_of::<sys::ip_mreq>() as sys::socklen_t,
        )
    }

    pub fn join_multicast_v6(&self, _: &Ipv6Addr, _: u32) -> io::Result<()> {
        Err(unsupported_v6())
    }

    pub fn leave_multicast_v4(&self, multiaddr: &Ipv4Addr, interface: &Ipv4Addr) -> io::Result<()> {
        let mreq = sys::ip_mreq {
            imr_multiaddr: sys::in_addr { s_addr: u32::from_ne_bytes(multiaddr.octets()) },
            imr_interface: sys::in_addr { s_addr: u32::from_ne_bytes(interface.octets()) },
        };
        setsockopt_raw(
            self.fd,
            sys::IPPROTO_IP,
            sys::IP_DROP_MEMBERSHIP,
            &mreq as *const _ as *const c_void,
            mem::size_of::<sys::ip_mreq>() as sys::socklen_t,
        )
    }

    pub fn leave_multicast_v6(&self, _: &Ipv6Addr, _: u32) -> io::Result<()> {
        Err(unsupported_v6())
    }

    pub fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        setsockopt_int(self.fd, sys::IPPROTO_IP, sys::IP_TTL, ttl as c_int)
    }

    pub fn ttl(&self) -> io::Result<u32> {
        getsockopt_int(self.fd, sys::IPPROTO_IP, sys::IP_TTL).map(|v| v as u32)
    }

    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        take_error(self.fd)
    }

    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        set_nonblocking(self.fd, nonblocking)
    }

    pub fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        cvt_ssize(unsafe {
            sys::recv(self.fd, buf.as_mut_ptr() as *mut c_void, buf.len(), 0)
        })
    }

    pub fn peek(&self, buf: &mut [u8]) -> io::Result<usize> {
        cvt_ssize(unsafe {
            sys::recv(self.fd, buf.as_mut_ptr() as *mut c_void, buf.len(), sys::MSG_PEEK)
        })
    }

    pub fn send(&self, buf: &[u8]) -> io::Result<usize> {
        cvt_ssize(unsafe {
            sys::send(self.fd, buf.as_ptr() as *const c_void, buf.len(), 0)
        })
    }

    pub fn connect(&self, addr: io::Result<&SocketAddr>) -> io::Result<()> {
        let addr = addr?;
        let v4 = require_v4(addr)?;
        let sa = std_to_sockaddr(v4);
        cvt(unsafe {
            sys::connect(self.fd, &sa as *const _ as *const sys::sockaddr, mem::size_of::<sys::sockaddr_in>() as sys::socklen_t)
        })?;
        Ok(())
    }
}

impl Drop for UdpSocket {
    fn drop(&mut self) {
        socket_close(self.fd);
    }
}

impl fmt::Debug for UdpSocket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut res = f.debug_struct("UdpSocket");
        if let Ok(addr) = self.socket_addr() {
            res.field("addr", &addr);
        }
        if let Ok(peer) = self.peer_addr() {
            res.field("peer", &peer);
        }
        res.field("fd", &self.fd).finish()
    }
}

// =========================================================================
// LookupHost — DNS resolution via gethostbyname()
// =========================================================================

pub struct LookupHost {
    addrs: crate::vec::Vec<SocketAddr>,
    pos: usize,
    port: u16,
}

impl LookupHost {
    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Iterator for LookupHost {
    type Item = SocketAddr;
    fn next(&mut self) -> Option<SocketAddr> {
        if self.pos < self.addrs.len() {
            let addr = self.addrs[self.pos];
            self.pos += 1;
            Some(addr)
        } else {
            None
        }
    }
}

unsafe impl Sync for LookupHost {}
unsafe impl Send for LookupHost {}

impl TryFrom<&str> for LookupHost {
    type Error = io::Error;

    fn try_from(s: &str) -> io::Result<LookupHost> {
        // Parse "host:port"
        let (host, port_str) = s.rsplit_once(':').ok_or_else(|| {
            io::Error::new(ErrorKind::InvalidInput, "invalid socket address")
        })?;
        let port: u16 = port_str.parse().map_err(|_| {
            io::Error::new(ErrorKind::InvalidInput, "invalid port value")
        })?;
        (host, port).try_into()
    }
}

impl<'a> TryFrom<(&'a str, u16)> for LookupHost {
    type Error = io::Error;

    fn try_from((host, port): (&'a str, u16)) -> io::Result<LookupHost> {
        // First try parsing as an IPv4 literal
        if let Ok(ip) = host.parse::<Ipv4Addr>() {
            return Ok(LookupHost {
                addrs: crate::vec![SocketAddr::V4(SocketAddrV4::new(ip, port))],
                pos: 0,
                port,
            });
        }

        // Fall back to DNS via gethostbyname
        // We need a null-terminated C string
        let mut buf = crate::vec::Vec::with_capacity(host.len() + 1);
        buf.extend_from_slice(host.as_bytes());
        buf.push(0);

        let he = unsafe { sys::gethostbyname(buf.as_ptr()) };
        if he.is_null() {
            return Err(io::Error::new(ErrorKind::Other, "DNS lookup failed"));
        }

        let mut addrs = crate::vec::Vec::new();
        unsafe {
            let he = &*he;
            if he.h_addrtype != sys::AF_INET || he.h_length != 4 {
                return Err(io::Error::new(ErrorKind::Other, "unexpected address type from DNS"));
            }
            let mut p = he.h_addr_list;
            while !(*p).is_null() {
                let addr_bytes = core::slice::from_raw_parts(*p, 4);
                let ip = Ipv4Addr::new(addr_bytes[0], addr_bytes[1], addr_bytes[2], addr_bytes[3]);
                addrs.push(SocketAddr::V4(SocketAddrV4::new(ip, port)));
                p = p.add(1);
            }
        }

        if addrs.is_empty() {
            return Err(io::Error::new(ErrorKind::Other, "DNS lookup returned no addresses"));
        }

        Ok(LookupHost { addrs, pos: 0, port })
    }
}
