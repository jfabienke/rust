pub type Key = usize;

pub unsafe fn create(_dtor: Option<unsafe extern "C" fn(*mut u8)>) -> Key {
    0
}

pub unsafe fn set(_key: Key, _value: *mut u8) {
    panic!("tls not supported")
}

pub unsafe fn get(_key: Key) -> *mut u8 {
    core::ptr::null_mut()
}

pub unsafe fn destroy(_key: Key) {
    // No-op
}
