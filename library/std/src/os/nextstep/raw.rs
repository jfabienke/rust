//! NeXTSTEP-specific raw type definitions.

#![stable(feature = "rust1", since = "1.0.0")]

/// Raw file descriptors.
#[stable(feature = "rust1", since = "1.0.0")]
pub type RawFd = core::ffi::c_int;
