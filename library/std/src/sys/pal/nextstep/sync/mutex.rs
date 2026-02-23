use crate::cell::UnsafeCell;
use crate::pin::Pin;
use core::ffi::c_void;

pub struct Mutex {
    inner: UnsafeCell<*mut c_void>,
}

impl Mutex {
    pub fn new() -> Mutex {
        Mutex { inner: UnsafeCell::new(core::ptr::null_mut()) }
    }

    /// Returns the raw C-thread mutex pointer, lazily allocating if needed.
    ///
    /// The Parker's `new_in_place` does NOT call `init()` on the mutex (only on
    /// the condvar), so we must handle first-use allocation here. This is safe
    /// because NeXTSTEP's `mutex_alloc()` returns a fully initialized mutex that
    /// is immediately usable.
    pub(super) fn raw(&self) -> *mut c_void {
        let ptr = unsafe { *self.inner.get() };
        if !ptr.is_null() {
            return ptr;
        }
        // First use — allocate the C-thread mutex.
        // This is not racy for our use case: the Parker creates its Mutex
        // in new_in_place and only one thread accesses it before the first lock.
        let m = unsafe { nextstep_sys::mutex_alloc() };
        unsafe { *self.inner.get() = m; }
        m
    }

    /// # Safety
    /// May only be called once per instance of `Self`.
    pub unsafe fn init(self: Pin<&mut Self>) {
        let ptr = unsafe { *self.inner.get() };
        if ptr.is_null() {
            unsafe { *self.inner.get() = nextstep_sys::mutex_alloc(); }
        }
    }

    /// # Safety
    /// * Destroying a locked mutex causes undefined behaviour.
    pub unsafe fn lock(self: Pin<&Self>) {
        unsafe { nextstep_sys::mutex_lock(self.raw()); }
    }

    /// # Safety
    /// * Destroying a locked mutex causes undefined behaviour.
    ///
    /// C-thread: `mutex_try_lock` returns 1 on success, 0 on failure.
    /// This is the **opposite** of `pthread_mutex_trylock` (which returns 0 on success).
    pub unsafe fn try_lock(self: Pin<&Self>) -> bool {
        unsafe { nextstep_sys::mutex_try_lock(self.raw()) != 0 }
    }

    /// # Safety
    /// The mutex must be locked by the current thread.
    pub unsafe fn unlock(self: Pin<&Self>) {
        unsafe { nextstep_sys::mutex_unlock(self.raw()); }
    }
}

impl !Unpin for Mutex {}

unsafe impl Send for Mutex {}
unsafe impl Sync for Mutex {}

impl Drop for Mutex {
    fn drop(&mut self) {
        let ptr = unsafe { *self.inner.get() };
        if !ptr.is_null() {
            unsafe { nextstep_sys::mutex_free(ptr); }
        }
    }
}
