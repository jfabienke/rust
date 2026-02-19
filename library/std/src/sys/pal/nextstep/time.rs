//! Time support for NeXTSTEP.
//!
//! Both Instant and SystemTime use gettimeofday(). Instant is NOT truly
//! monotonic since NeXTSTEP lacks CLOCK_MONOTONIC. This is a documented
//! limitation acceptable for a uniprocessor m68k target.

use crate::time::Duration;

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct Instant(Duration);

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct SystemTime(Duration);

pub const UNIX_EPOCH: SystemTime = SystemTime(Duration::from_secs(0));

fn gettimeofday_duration() -> Duration {
    let mut tv = nextstep_sys::timeval { tv_sec: 0, tv_usec: 0 };
    let ret = unsafe {
        nextstep_sys::gettimeofday(&mut tv, core::ptr::null_mut())
    };
    debug_assert!(ret == 0, "gettimeofday failed");
    Duration::new(tv.tv_sec as u64, (tv.tv_usec as u32) * 1000)
}

impl Instant {
    pub fn now() -> Instant {
        Instant(gettimeofday_duration())
    }

    pub fn checked_sub_instant(&self, other: &Instant) -> Option<Duration> {
        self.0.checked_sub(other.0)
    }

    pub fn checked_add_duration(&self, other: &Duration) -> Option<Instant> {
        self.0.checked_add(*other).map(Instant)
    }

    pub fn checked_sub_duration(&self, other: &Duration) -> Option<Instant> {
        self.0.checked_sub(*other).map(Instant)
    }
}

impl SystemTime {
    pub fn now() -> SystemTime {
        SystemTime(gettimeofday_duration())
    }

    pub fn sub_time(&self, other: &SystemTime) -> Result<Duration, Duration> {
        self.0.checked_sub(other.0).ok_or_else(|| other.0 - self.0)
    }

    pub fn checked_add_duration(&self, other: &Duration) -> Option<SystemTime> {
        self.0.checked_add(*other).map(SystemTime)
    }

    pub fn checked_sub_duration(&self, other: &Duration) -> Option<SystemTime> {
        self.0.checked_sub(*other).map(SystemTime)
    }
}
