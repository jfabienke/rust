//! TLS key implementation for NeXTSTEP using cthread_data().
//!
//! NeXTSTEP's C-thread library provides a single per-thread data slot via
//! `cthread_data()`/`cthread_set_data()`. We use this slot to store a pointer
//! to a per-thread key→value table (a heap-allocated `Vec<*mut u8>`).
//!
//! Keys are globally unique integers from an atomic counter. Destructor
//! function pointers are stored in a global array indexed by key.

use crate::sync::atomic::{AtomicUsize, Ordering};
use core::ffi::c_void;

pub type Key = usize;

/// Maximum number of TLS keys. Rust std typically uses ~20 keys.
const MAX_KEYS: usize = 128;

/// Global key counter. Key 0 is the sentinel value (KEY_SENTVAL in racy.rs),
/// so we start at 1 to ensure no valid key equals the sentinel.
static NEXT_KEY: AtomicUsize = AtomicUsize::new(1);

/// Global destructor table indexed by key.
/// Access is safe because each key index is written exactly once (at creation)
/// and only read during thread cleanup.
static mut DTORS: [Option<unsafe extern "C" fn(*mut u8)>; MAX_KEYS] = [None; MAX_KEYS];

#[inline]
pub fn create(dtor: Option<unsafe extern "C" fn(*mut u8)>) -> Key {
    let key = NEXT_KEY.fetch_add(1, Ordering::Relaxed);
    assert!(key < MAX_KEYS, "NeXTSTEP TLS key limit ({MAX_KEYS}) exceeded");
    if let Some(f) = dtor {
        // SAFETY: Each key is unique and only written once.
        unsafe {
            DTORS[key] = Some(f);
        }
    }
    key
}

#[inline]
pub unsafe fn destroy(_key: Key) {
    // Keys are never reused in practice. No-op.
    // The destructor entry remains but will never be called for new values.
}

#[inline]
pub unsafe fn set(key: Key, value: *mut u8) {
    let table = unsafe { get_or_init_table() };
    let table = unsafe { &mut *table };
    if key >= table.len() {
        table.resize(key + 1, core::ptr::null_mut());
    }
    table[key] = value;
}

#[inline]
pub unsafe fn get(key: Key) -> *mut u8 {
    let table_ptr = unsafe { nextstep_sys::cthread_data(nextstep_sys::cthread_self()) };
    if table_ptr.is_null() {
        return core::ptr::null_mut();
    }
    let table = unsafe { &*(table_ptr as *const Vec<*mut u8>) };
    if key < table.len() { table[key] } else { core::ptr::null_mut() }
}

/// Returns a pointer to the per-thread TLS table, creating it on first access.
unsafe fn get_or_init_table() -> *mut Vec<*mut u8> {
    let ptr = unsafe { nextstep_sys::cthread_data(nextstep_sys::cthread_self()) };
    if !ptr.is_null() {
        return ptr as *mut Vec<*mut u8>;
    }
    // First TLS access on this thread — allocate the table.
    let table = Box::into_raw(Box::new(Vec::<*mut u8>::new()));
    unsafe {
        nextstep_sys::cthread_set_data(nextstep_sys::cthread_self(), table as *mut c_void);
    }
    table
}

/// Run TLS destructors for the current thread and free the per-thread table.
///
/// Called from the thread trampoline before the thread exits.
/// Follows the POSIX convention: iterate up to `PTHREAD_DESTRUCTOR_ITERATIONS` (4)
/// rounds, calling destructors for non-null values and clearing them.
pub unsafe fn run_dtors() {
    for _ in 0..4 {
        let table_ptr = unsafe { nextstep_sys::cthread_data(nextstep_sys::cthread_self()) };
        if table_ptr.is_null() {
            return;
        }
        let table = unsafe { &mut *(table_ptr as *mut Vec<*mut u8>) };

        let mut any_called = false;
        for key in 0..table.len() {
            let val = table[key];
            if !val.is_null() {
                table[key] = core::ptr::null_mut();
                if let Some(dtor) = unsafe { DTORS[key] } {
                    any_called = true;
                    unsafe { dtor(val); }
                }
            }
        }
        if !any_called {
            break;
        }
    }

    // Free the per-thread table.
    let table_ptr = unsafe { nextstep_sys::cthread_data(nextstep_sys::cthread_self()) };
    if !table_ptr.is_null() {
        unsafe {
            drop(Box::from_raw(table_ptr as *mut Vec<*mut u8>));
            nextstep_sys::cthread_set_data(
                nextstep_sys::cthread_self(),
                core::ptr::null_mut(),
            );
        }
    }
}
