use super::Mutex;
use crate::cell::UnsafeCell;
use crate::pin::Pin;
use crate::time::Duration;
use core::ffi::c_void;

pub struct Condvar {
    inner: UnsafeCell<*mut c_void>,
}

impl Condvar {
    /// C-threads have no timed condition_wait, so timeouts are imprecise.
    /// The sys/sync/condvar/pthread.rs wrapper does its own elapsed-time check
    /// when this is false.
    pub const PRECISE_TIMEOUT: bool = false;

    pub fn new() -> Condvar {
        Condvar { inner: UnsafeCell::new(core::ptr::null_mut()) }
    }

    #[inline]
    fn raw(&self) -> *mut c_void {
        unsafe { *self.inner.get() }
    }

    /// # Safety
    /// May only be called once per instance of `Self`.
    pub unsafe fn init(self: Pin<&mut Self>) {
        let ptr = unsafe { *self.inner.get() };
        if ptr.is_null() {
            unsafe { *self.inner.get() = nextstep_sys::condition_alloc(); }
        }
    }

    /// # Safety
    /// `init` must have been called on this instance.
    #[inline]
    pub unsafe fn notify_one(self: Pin<&Self>) {
        unsafe { nextstep_sys::condition_signal(self.raw()); }
    }

    /// # Safety
    /// `init` must have been called on this instance.
    #[inline]
    pub unsafe fn notify_all(self: Pin<&Self>) {
        unsafe { nextstep_sys::condition_broadcast(self.raw()); }
    }

    /// # Safety
    /// * `init` must have been called on this instance.
    /// * `mutex` must be locked by the current thread.
    /// * This condition variable may only be used with the same mutex.
    #[inline]
    pub unsafe fn wait(self: Pin<&Self>, mutex: Pin<&Mutex>) {
        // condition_wait atomically unlocks mutex, blocks, re-locks on wakeup.
        unsafe { nextstep_sys::condition_wait(self.raw(), mutex.raw()); }
    }

    /// # Safety
    /// * `init` must have been called on this instance.
    /// * `mutex` must be locked by the current thread.
    /// * This condition variable may only be used with the same mutex.
    ///
    /// C-threads have no timed condition_wait variant.
    /// We unlock the mutex, sleep for the duration, then re-lock.
    /// This means we won't wake early on a signal during the sleep window,
    /// but with PRECISE_TIMEOUT=false the caller handles timing.
    ///
    /// TODO(Build69): Replace sleep-based timeout with Mach msg_receive timeout
    /// for more precise wakeup behavior.
    pub unsafe fn wait_timeout(&self, mutex: Pin<&Mutex>, dur: Duration) -> bool {
        unsafe { mutex.unlock(); }
        crate::thread::sleep(dur);
        unsafe { mutex.lock(); }
        // Always report "timed out" since we can't distinguish signal vs timeout.
        false
    }
}

impl !Unpin for Condvar {}

unsafe impl Sync for Condvar {}
unsafe impl Send for Condvar {}

impl Drop for Condvar {
    fn drop(&mut self) {
        let ptr = unsafe { *self.inner.get() };
        if !ptr.is_null() {
            unsafe { nextstep_sys::condition_free(ptr); }
        }
    }
}
