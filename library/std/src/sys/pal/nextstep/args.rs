//! Command-line argument access for NeXTSTEP.
//!
//! Reads argc/argv from statics saved during init().

use crate::ffi::{CStr, OsString};
use crate::fmt;
use crate::os::raw::c_char;
use crate::vec;

pub struct Args {
    iter: vec::IntoIter<OsString>,
}

pub fn args() -> Args {
    let argc = super::common::argc();
    let argv = super::common::argv();
    let mut v = Vec::new();
    if !argv.is_null() && argc > 0 {
        for i in 0..argc {
            let ptr = unsafe { *argv.offset(i) };
            if ptr.is_null() {
                break;
            }
            let cstr = unsafe { CStr::from_ptr(ptr as *const c_char) };
            // SAFETY: NeXTSTEP argv entries are byte strings; treat as OsString.
            let os = OsString::from_encoded_bytes_unchecked(cstr.to_bytes().to_vec());
            v.push(os);
        }
    }
    Args { iter: v.into_iter() }
}

impl fmt::Debug for Args {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter.as_slice()).finish()
    }
}

impl Iterator for Args {
    type Item = OsString;
    fn next(&mut self) -> Option<OsString> {
        self.iter.next()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl ExactSizeIterator for Args {
    fn len(&self) -> usize {
        self.iter.len()
    }
}

impl DoubleEndedIterator for Args {
    fn next_back(&mut self) -> Option<OsString> {
        self.iter.next_back()
    }
}
