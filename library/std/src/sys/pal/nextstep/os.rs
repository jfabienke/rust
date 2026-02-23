//! OS-level operations for NeXTSTEP.
//!
//! Provides getcwd, chdir, environment variable access, exit, getpid, etc.
//! Uses nextstep_sys FFI directly since cfg(unix) is false for NeXTSTEP.

use crate::error::Error as StdError;
use crate::ffi::{CStr, OsStr, OsString};
use crate::fmt;
use crate::io;
use crate::os::raw::c_char;
use crate::path::{self, PathBuf};
use crate::vec;

const PATH_MAX: usize = 1024;

/// Get the current working directory.
pub fn getcwd() -> io::Result<PathBuf> {
    let mut buf = vec![0u8; PATH_MAX];
    let ptr = unsafe { nextstep_sys::getcwd(buf.as_mut_ptr(), buf.len()) };
    if ptr.is_null() {
        Err(io::Error::from_raw_os_error(super::common::errno()))
    } else {
        let len = unsafe { CStr::from_ptr(ptr as *const c_char) }.to_bytes().len();
        buf.truncate(len);
        Ok(PathBuf::from(unsafe { OsString::from_encoded_bytes_unchecked(buf) }))
    }
}

/// Change the current working directory.
pub fn chdir(p: &path::Path) -> io::Result<()> {
    let p = cstr_from_path(p)?;
    super::common::cvt(unsafe { nextstep_sys::chdir(p.as_ptr() as *const u8) })?;
    Ok(())
}

/// Return the path to the current executable via argv[0].
///
/// Best-effort: if argv[0] is relative, prepends the cwd. May not reflect
/// the true path if the binary was invoked via a bare PATH lookup.
pub fn current_exe() -> io::Result<PathBuf> {
    let argv = super::common::argv();
    if argv.is_null() || super::common::argc() < 1 {
        return super::common::unsupported();
    }
    let arg0_ptr = unsafe { *argv };
    if arg0_ptr.is_null() {
        return super::common::unsupported();
    }
    let arg0 = unsafe { CStr::from_ptr(arg0_ptr as *const c_char) };
    let path = PathBuf::from(unsafe {
        OsString::from_encoded_bytes_unchecked(arg0.to_bytes().to_vec())
    });
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(getcwd()?.join(path))
    }
}

/// Get the errno value.
pub fn errno() -> i32 {
    super::common::errno()
}

/// Convert an errno value to a human-readable string.
pub fn error_string(errno: i32) -> String {
    let ptr = unsafe { nextstep_sys::strerror(errno) };
    if ptr.is_null() {
        return format!("Unknown error {}", errno);
    }
    let cstr = unsafe { CStr::from_ptr(ptr as *const c_char) };
    cstr.to_string_lossy().into_owned()
}

// --- Environment variable access ---

/// Iterator over environment variables.
pub struct Env {
    iter: vec::IntoIter<(OsString, OsString)>,
}

impl Env {
    pub fn str_debug(&self) -> impl fmt::Debug + '_ {
        &self.iter
    }
}

impl fmt::Debug for Env {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter.as_slice()).finish()
    }
}

impl Iterator for Env {
    type Item = (OsString, OsString);
    fn next(&mut self) -> Option<(OsString, OsString)> {
        self.iter.next()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

/// Return an iterator over the environment variables.
pub fn env() -> Env {
    let mut entries = Vec::new();
    unsafe {
        let mut ptr = nextstep_sys::environ;
        if !ptr.is_null() {
            while !(*ptr).is_null() {
                let entry = CStr::from_ptr(*ptr as *const c_char);
                let bytes = entry.to_bytes();
                if let Some(eq_pos) = bytes.iter().position(|&b| b == b'=') {
                    let key = OsString::from_encoded_bytes_unchecked(
                        bytes[..eq_pos].to_vec(),
                    );
                    let val = OsString::from_encoded_bytes_unchecked(
                        bytes[eq_pos + 1..].to_vec(),
                    );
                    entries.push((key, val));
                }
                ptr = ptr.add(1);
            }
        }
    }
    Env { iter: entries.into_iter() }
}

/// Get an environment variable by name.
pub fn getenv(key: &OsStr) -> Option<OsString> {
    let key = cstr_from_osstr(key).ok()?;
    let ptr = unsafe { nextstep_sys::getenv(key.as_ptr() as *const u8) };
    if ptr.is_null() {
        None
    } else {
        let cstr = unsafe { CStr::from_ptr(ptr as *const c_char) };
        Some(unsafe { OsString::from_encoded_bytes_unchecked(cstr.to_bytes().to_vec()) })
    }
}

/// Set an environment variable.
///
/// # Safety
/// Must not be called concurrently with other env operations.
pub unsafe fn setenv(key: &OsStr, val: &OsStr) -> io::Result<()> {
    let key = cstr_from_osstr(key)?;
    let val = cstr_from_osstr(val)?;
    let ret = unsafe {
        nextstep_sys::setenv(key.as_ptr() as *const u8, val.as_ptr() as *const u8, 1)
    };
    if ret != 0 {
        Err(io::Error::from_raw_os_error(super::common::errno()))
    } else {
        Ok(())
    }
}

/// Unset an environment variable.
///
/// # Safety
/// Must not be called concurrently with other env operations.
pub unsafe fn unsetenv(key: &OsStr) -> io::Result<()> {
    let key = cstr_from_osstr(key)?;
    let ret = unsafe { nextstep_sys::unsetenv(key.as_ptr() as *const u8) };
    if ret != 0 {
        Err(io::Error::from_raw_os_error(super::common::errno()))
    } else {
        Ok(())
    }
}

// --- SplitPaths / JoinPaths ---

pub struct SplitPaths<'a> {
    iter: crate::str::Split<'a, char>,
}

pub fn split_paths(unparsed: &OsStr) -> SplitPaths<'_> {
    // NeXTSTEP uses ':' as the path separator
    let s = unparsed.as_encoded_bytes();
    let s = unsafe { core::str::from_utf8_unchecked(s) };
    SplitPaths { iter: s.split(':') }
}

impl<'a> Iterator for SplitPaths<'a> {
    type Item = PathBuf;
    fn next(&mut self) -> Option<PathBuf> {
        self.iter.next().map(|s| PathBuf::from(unsafe { OsString::from_encoded_bytes_unchecked(s.as_bytes().to_vec()) }))
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

#[derive(Debug)]
pub struct JoinPathsError;

pub fn join_paths<I, T>(paths: I) -> Result<OsString, JoinPathsError>
where
    I: Iterator<Item = T>,
    T: AsRef<OsStr>,
{
    let mut joined = Vec::new();
    for (i, path) in paths.enumerate() {
        let bytes = path.as_ref().as_encoded_bytes();
        if bytes.contains(&b':') {
            return Err(JoinPathsError);
        }
        if i > 0 {
            joined.push(b':');
        }
        joined.extend_from_slice(bytes);
    }
    Ok(unsafe { OsString::from_encoded_bytes_unchecked(joined) })
}

impl fmt::Display for JoinPathsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "path segment contains separator `:`")
    }
}

impl StdError for JoinPathsError {
    fn description(&self) -> &str {
        "failed to join paths"
    }
}

// --- Process operations ---

pub fn exit(code: i32) -> ! {
    unsafe { nextstep_sys::_exit(code) }
}

pub fn getpid() -> u32 {
    unsafe { nextstep_sys::getpid() as u32 }
}

pub fn temp_dir() -> PathBuf {
    PathBuf::from("/tmp")
}

pub fn home_dir() -> Option<PathBuf> {
    getenv(OsStr::new("HOME")).map(PathBuf::from)
}

// --- Helpers ---

/// Convert an OsStr to a C string, returning an error if it contains interior NULs.
fn cstr_from_osstr(s: &OsStr) -> io::Result<alloc::ffi::CString> {
    alloc::ffi::CString::new(s.as_encoded_bytes())
        .map_err(|_| io::Error::from_raw_os_error(nextstep_sys::EINVAL))
}

/// Convert a Path to a C string.
fn cstr_from_path(p: &path::Path) -> io::Result<alloc::ffi::CString> {
    cstr_from_osstr(p.as_os_str())
}
