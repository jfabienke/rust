//! Filesystem operations for NeXTSTEP.
//!
//! File wraps a raw i32 fd with manual Drop. OpenOptions translates to
//! nextstep_sys open() flags. FileAttr wraps nextstep_sys::stat.
//! ReadDir uses BSD getdirentries().

use crate::ffi::{CStr, OsStr, OsString};
use crate::fmt;
use crate::io::{self, BorrowedCursor, IoSlice, IoSliceMut, SeekFrom};
use crate::os::raw::c_char;
use crate::path::{Path, PathBuf};
use crate::sync::Arc;

use nextstep_sys as sys;

fn cstr_from_path(p: &Path) -> io::Result<alloc::ffi::CString> {
    alloc::ffi::CString::new(p.as_os_str().as_encoded_bytes())
        .map_err(|_| io::Error::from_raw_os_error(sys::EINVAL))
}

// =========================================================================
// File
// =========================================================================

pub struct File {
    fd: i32,
}

impl File {
    pub fn open(path: &Path, opts: &OpenOptions) -> io::Result<File> {
        let cpath = cstr_from_path(path)?;
        let flags = opts.flags();
        let fd = super::common::cvt(unsafe {
            sys::open(cpath.as_ptr() as *const u8, flags, opts.mode)
        })?;
        Ok(File { fd })
    }

    fn raw_fd(&self) -> i32 {
        self.fd
    }

    pub fn file_attr(&self) -> io::Result<FileAttr> {
        let mut st = unsafe { core::mem::zeroed::<sys::stat>() };
        super::common::cvt(unsafe { sys::fstat(self.fd, &mut st) })?;
        Ok(FileAttr(st))
    }

    pub fn fsync(&self) -> io::Result<()> {
        super::common::cvt(unsafe { sys::fsync(self.fd) })?;
        Ok(())
    }

    pub fn datasync(&self) -> io::Result<()> {
        // NeXTSTEP has no fdatasync; use fsync
        self.fsync()
    }

    pub fn lock(&self) -> io::Result<()> {
        super::common::cvt(unsafe { sys::flock(self.fd, sys::LOCK_EX) })?;
        Ok(())
    }

    pub fn lock_shared(&self) -> io::Result<()> {
        super::common::cvt(unsafe { sys::flock(self.fd, sys::LOCK_SH) })?;
        Ok(())
    }

    pub fn try_lock(&self) -> io::Result<bool> {
        let ret = unsafe { sys::flock(self.fd, sys::LOCK_EX | sys::LOCK_NB) };
        if ret == 0 {
            Ok(true)
        } else {
            let errno = super::common::errno();
            if errno == sys::EWOULDBLOCK {
                Ok(false)
            } else {
                Err(io::Error::from_raw_os_error(errno))
            }
        }
    }

    pub fn try_lock_shared(&self) -> io::Result<bool> {
        let ret = unsafe { sys::flock(self.fd, sys::LOCK_SH | sys::LOCK_NB) };
        if ret == 0 {
            Ok(true)
        } else {
            let errno = super::common::errno();
            if errno == sys::EWOULDBLOCK {
                Ok(false)
            } else {
                Err(io::Error::from_raw_os_error(errno))
            }
        }
    }

    pub fn unlock(&self) -> io::Result<()> {
        super::common::cvt(unsafe { sys::flock(self.fd, sys::LOCK_UN) })?;
        Ok(())
    }

    pub fn truncate(&self, size: u64) -> io::Result<()> {
        super::common::cvt(unsafe { sys::ftruncate(self.fd, size as sys::off_t) })?;
        Ok(())
    }

    pub fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        let ret = super::common::cvt_isize(unsafe {
            sys::read(self.fd, buf.as_mut_ptr(), buf.len())
        })?;
        Ok(ret as usize)
    }

    pub fn read_vectored(&self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        // Fall back to reading into the first non-empty buffer
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

    pub fn read_buf(&self, mut cursor: BorrowedCursor<'_>) -> io::Result<()> {
        let buf = cursor.ensure_init();
        let n = self.read(buf)?;
        unsafe { cursor.advance_unchecked(n) };
        Ok(())
    }

    pub fn write(&self, buf: &[u8]) -> io::Result<usize> {
        let ret = super::common::cvt_isize(unsafe {
            sys::write(self.fd, buf.as_ptr(), buf.len())
        })?;
        Ok(ret as usize)
    }

    pub fn write_vectored(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        // Fall back to writing the first non-empty buffer
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

    pub fn flush(&self) -> io::Result<()> {
        Ok(())
    }

    pub fn seek(&self, pos: SeekFrom) -> io::Result<u64> {
        let (offset, whence) = match pos {
            SeekFrom::Start(n) => (n as sys::off_t, sys::SEEK_SET),
            SeekFrom::End(n) => (n as sys::off_t, sys::SEEK_END),
            SeekFrom::Current(n) => (n as sys::off_t, sys::SEEK_CUR),
        };
        let ret = super::common::cvt(unsafe { sys::lseek(self.fd, offset, whence) })?;
        Ok(ret as u64)
    }

    pub fn duplicate(&self) -> io::Result<File> {
        let new_fd = super::common::cvt(unsafe { sys::dup(self.fd) })?;
        Ok(File { fd: new_fd })
    }

    pub fn set_permissions(&self, perm: FilePermissions) -> io::Result<()> {
        super::common::cvt(unsafe { sys::fchmod(self.fd, perm.mode) })?;
        Ok(())
    }

    pub fn set_times(&self, _times: FileTimes) -> io::Result<()> {
        // utimes requires a path; fchmod-style time setting not available
        super::common::unsupported()
    }
}

impl Drop for File {
    fn drop(&mut self) {
        let _ = unsafe { sys::close(self.fd) };
    }
}

impl fmt::Debug for File {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("File").field("fd", &self.fd).finish()
    }
}

// =========================================================================
// FileAttr
// =========================================================================

pub struct FileAttr(sys::stat);

impl FileAttr {
    pub fn size(&self) -> u64 {
        self.0.st_size as u64
    }

    pub fn perm(&self) -> FilePermissions {
        FilePermissions { mode: self.0.st_mode & 0o7777 }
    }

    pub fn file_type(&self) -> FileType {
        FileType { mode: self.0.st_mode }
    }

    pub fn modified(&self) -> io::Result<crate::time::SystemTime> {
        Ok(crate::time::SystemTime::UNIX_EPOCH
            + crate::time::Duration::from_secs(self.0.st_mtime as u64))
    }

    pub fn accessed(&self) -> io::Result<crate::time::SystemTime> {
        Ok(crate::time::SystemTime::UNIX_EPOCH
            + crate::time::Duration::from_secs(self.0.st_atime as u64))
    }

    pub fn created(&self) -> io::Result<crate::time::SystemTime> {
        // NeXTSTEP stat doesn't have a birth time field
        super::common::unsupported()
    }
}

impl Clone for FileAttr {
    fn clone(&self) -> FileAttr {
        // SAFETY: stat is a plain C struct, safe to copy
        FileAttr(unsafe { core::ptr::read(&self.0) })
    }
}

// =========================================================================
// FilePermissions
// =========================================================================

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FilePermissions {
    mode: sys::mode_t,
}

impl FilePermissions {
    pub fn readonly(&self) -> bool {
        self.mode & 0o222 == 0
    }

    pub fn set_readonly(&mut self, readonly: bool) {
        if readonly {
            self.mode &= !0o222;
        } else {
            self.mode |= 0o222;
        }
    }

    pub fn mode(&self) -> u32 {
        self.mode as u32
    }
}

// =========================================================================
// FileType
// =========================================================================

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct FileType {
    mode: sys::mode_t,
}

impl FileType {
    pub fn is_dir(&self) -> bool {
        (self.mode & sys::S_IFMT) == sys::S_IFDIR
    }

    pub fn is_file(&self) -> bool {
        (self.mode & sys::S_IFMT) == sys::S_IFREG
    }

    pub fn is_symlink(&self) -> bool {
        (self.mode & sys::S_IFMT) == sys::S_IFLNK
    }
}

// =========================================================================
// OpenOptions
// =========================================================================

#[derive(Clone, Debug)]
pub struct OpenOptions {
    read: bool,
    write: bool,
    append: bool,
    truncate: bool,
    create: bool,
    create_new: bool,
    mode: sys::mode_t,
}

impl OpenOptions {
    pub fn new() -> OpenOptions {
        OpenOptions {
            read: false,
            write: false,
            append: false,
            truncate: false,
            create: false,
            create_new: false,
            mode: 0o666,
        }
    }

    pub fn read(&mut self, read: bool) {
        self.read = read;
    }
    pub fn write(&mut self, write: bool) {
        self.write = write;
    }
    pub fn append(&mut self, append: bool) {
        self.append = append;
    }
    pub fn truncate(&mut self, truncate: bool) {
        self.truncate = truncate;
    }
    pub fn create(&mut self, create: bool) {
        self.create = create;
    }
    pub fn create_new(&mut self, create_new: bool) {
        self.create_new = create_new;
    }
    pub fn mode(&mut self, mode: u32) {
        self.mode = mode as sys::mode_t;
    }

    fn flags(&self) -> sys::c_int {
        let mut flags = match (self.read, self.write, self.append) {
            (true, false, false) => sys::O_RDONLY,
            (false, true, false) => sys::O_WRONLY,
            (true, true, false) => sys::O_RDWR,
            (false, _, true) => sys::O_WRONLY | sys::O_APPEND,
            (true, _, true) => sys::O_RDWR | sys::O_APPEND,
            (false, false, false) => sys::O_RDONLY,
        };
        if self.create_new {
            flags |= sys::O_CREAT | sys::O_EXCL;
        } else if self.create {
            flags |= sys::O_CREAT;
        }
        if self.truncate {
            flags |= sys::O_TRUNC;
        }
        flags
    }
}

// =========================================================================
// FileTimes
// =========================================================================

#[derive(Debug)]
pub struct FileTimes {}

impl FileTimes {
    pub fn set_accessed(&mut self, _t: crate::time::SystemTime) {}
    pub fn set_modified(&mut self, _t: crate::time::SystemTime) {}
}

// =========================================================================
// ReadDir + DirEntry
// =========================================================================

pub struct ReadDir {
    inner: Arc<ReadDirInner>,
}

struct ReadDirInner {
    fd: i32,
    root: PathBuf,
    buf: Vec<u8>,
    offset: usize,
    len: usize,
    base: sys::c_long,
}

impl Drop for ReadDirInner {
    fn drop(&mut self) {
        let _ = unsafe { sys::close(self.fd) };
    }
}

pub struct DirEntry {
    name: OsString,
    file_type: Option<FileType>,
    root: Arc<ReadDirInner>,
}

impl fmt::Debug for ReadDir {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReadDir").field("root", &self.inner.root).finish()
    }
}

impl Iterator for ReadDir {
    type Item = io::Result<DirEntry>;

    fn next(&mut self) -> Option<io::Result<DirEntry>> {
        let inner = Arc::get_mut(&mut self.inner)?;
        loop {
            if inner.offset >= inner.len {
                // Refill the buffer
                let ret = unsafe {
                    sys::getdirentries(
                        inner.fd,
                        inner.buf.as_mut_ptr(),
                        inner.buf.len() as sys::c_int,
                        &mut inner.base,
                    )
                };
                if ret < 0 {
                    return Some(Err(io::Error::from_raw_os_error(super::common::errno())));
                }
                if ret == 0 {
                    return None; // End of directory
                }
                inner.len = ret as usize;
                inner.offset = 0;
            }

            // Parse the dirent at the current offset
            if inner.offset + 8 > inner.len {
                return None;
            }
            let ptr = &inner.buf[inner.offset] as *const u8 as *const sys::dirent;
            let de = unsafe { &*ptr };
            let reclen = de.d_reclen as usize;
            if reclen == 0 {
                return None;
            }
            inner.offset += reclen;

            let namlen = de.d_namlen as usize;
            let name_bytes = &de.d_name[..namlen];
            // Skip . and ..
            if name_bytes == b"." || name_bytes == b".." {
                continue;
            }

            let name = OsString::from_encoded_bytes_unchecked(name_bytes.to_vec());
            return Some(Ok(DirEntry {
                name,
                file_type: None,
                root: self.inner.clone(),
            }));
        }
    }
}

impl DirEntry {
    pub fn path(&self) -> PathBuf {
        self.root.root.join(&self.name)
    }

    pub fn file_name(&self) -> OsString {
        self.name.clone()
    }

    pub fn metadata(&self) -> io::Result<FileAttr> {
        lstat(&self.path())
    }

    pub fn file_type(&self) -> io::Result<FileType> {
        if let Some(ft) = self.file_type {
            Ok(ft)
        } else {
            Ok(self.metadata()?.file_type())
        }
    }

    pub fn ino(&self) -> u64 {
        0 // Would need to read from dirent, but type changes per entry
    }
}

// =========================================================================
// DirBuilder
// =========================================================================

pub struct DirBuilder {
    mode: sys::mode_t,
}

impl DirBuilder {
    pub fn new() -> DirBuilder {
        DirBuilder { mode: 0o777 }
    }

    pub fn mkdir(&self, p: &Path) -> io::Result<()> {
        let cpath = cstr_from_path(p)?;
        super::common::cvt(unsafe { sys::mkdir(cpath.as_ptr() as *const u8, self.mode) })?;
        Ok(())
    }

    pub fn set_mode(&mut self, mode: u32) {
        self.mode = mode as sys::mode_t;
    }
}

// =========================================================================
// Free functions
// =========================================================================

pub fn readdir(p: &Path) -> io::Result<ReadDir> {
    let cpath = cstr_from_path(p)?;
    let fd = super::common::cvt(unsafe {
        sys::open(cpath.as_ptr() as *const u8, sys::O_RDONLY, 0)
    })?;
    Ok(ReadDir {
        inner: Arc::new(ReadDirInner {
            fd,
            root: p.to_path_buf(),
            buf: vec![0u8; 4096],
            offset: 0,
            len: 0,
            base: 0,
        }),
    })
}

pub fn unlink(p: &Path) -> io::Result<()> {
    let cpath = cstr_from_path(p)?;
    super::common::cvt(unsafe { sys::unlink(cpath.as_ptr() as *const u8) })?;
    Ok(())
}

pub fn rename(old: &Path, new: &Path) -> io::Result<()> {
    let cold = cstr_from_path(old)?;
    let cnew = cstr_from_path(new)?;
    super::common::cvt(unsafe {
        sys::rename(cold.as_ptr() as *const u8, cnew.as_ptr() as *const u8)
    })?;
    Ok(())
}

pub fn set_perm(p: &Path, perm: FilePermissions) -> io::Result<()> {
    let cpath = cstr_from_path(p)?;
    super::common::cvt(unsafe { sys::chmod(cpath.as_ptr() as *const u8, perm.mode) })?;
    Ok(())
}

pub fn rmdir(p: &Path) -> io::Result<()> {
    let cpath = cstr_from_path(p)?;
    super::common::cvt(unsafe { sys::rmdir(cpath.as_ptr() as *const u8) })?;
    Ok(())
}

pub fn remove_dir_all(path: &Path) -> io::Result<()> {
    // Recursive removal: list entries, remove files, recurse into dirs
    for entry in readdir(path)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        let child = entry.path();
        if ft.is_dir() {
            remove_dir_all(&child)?;
        } else {
            unlink(&child)?;
        }
    }
    rmdir(path)
}

pub fn exists(path: &Path) -> io::Result<bool> {
    match stat(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

pub fn readlink(p: &Path) -> io::Result<PathBuf> {
    let cpath = cstr_from_path(p)?;
    let mut buf = vec![0u8; 1024];
    let ret = super::common::cvt_isize(unsafe {
        sys::readlink(cpath.as_ptr() as *const u8, buf.as_mut_ptr(), buf.len())
    })?;
    buf.truncate(ret as usize);
    Ok(PathBuf::from(OsString::from_encoded_bytes_unchecked(buf)))
}

pub fn symlink(original: &Path, link: &Path) -> io::Result<()> {
    let corig = cstr_from_path(original)?;
    let clink = cstr_from_path(link)?;
    super::common::cvt(unsafe {
        sys::symlink(corig.as_ptr() as *const u8, clink.as_ptr() as *const u8)
    })?;
    Ok(())
}

pub fn link(src: &Path, dst: &Path) -> io::Result<()> {
    let csrc = cstr_from_path(src)?;
    let cdst = cstr_from_path(dst)?;
    super::common::cvt(unsafe {
        sys::link(csrc.as_ptr() as *const u8, cdst.as_ptr() as *const u8)
    })?;
    Ok(())
}

pub fn stat(p: &Path) -> io::Result<FileAttr> {
    let cpath = cstr_from_path(p)?;
    let mut st = unsafe { core::mem::zeroed::<sys::stat>() };
    super::common::cvt(unsafe { sys::stat(cpath.as_ptr() as *const u8, &mut st) })?;
    Ok(FileAttr(st))
}

pub fn lstat(p: &Path) -> io::Result<FileAttr> {
    let cpath = cstr_from_path(p)?;
    let mut st = unsafe { core::mem::zeroed::<sys::stat>() };
    super::common::cvt(unsafe { sys::lstat(cpath.as_ptr() as *const u8, &mut st) })?;
    Ok(FileAttr(st))
}

pub fn canonicalize(p: &Path) -> io::Result<PathBuf> {
    // No realpath on NeXTSTEP; do a minimal normalization
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        super::os::getcwd()?.join(p)
    };
    // Verify it exists
    stat(&abs)?;
    Ok(abs)
}

pub fn copy(from: &Path, to: &Path) -> io::Result<u64> {
    let mut read_opts = OpenOptions::new();
    read_opts.read(true);
    let reader = File::open(from, &read_opts)?;

    let mut write_opts = OpenOptions::new();
    write_opts.write(true);
    write_opts.create(true);
    write_opts.truncate(true);
    let writer = File::open(to, &write_opts)?;

    let mut buf = vec![0u8; 4096];
    let mut total = 0u64;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        writer.write(&buf[..n])?;
        total += n as u64;
    }
    Ok(total)
}
