use crate::alloc::{GlobalAlloc, Layout, System};
use nextstep_sys::{sys_vm_allocate, sys_vm_deallocate};

// Page size on NeXTSTEP m68k
const PAGE_SIZE: usize = 4096;

fn round_up_to_page(size: usize) -> usize {
    (size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
}

unsafe impl GlobalAlloc for System {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let size = round_up_to_page(layout.size());
        match sys_vm_allocate(size, true) {
            Ok(ptr) => ptr as *mut u8,
            Err(_) => core::ptr::null_mut(),
        }
    }

    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let size = round_up_to_page(layout.size());
        let _ = sys_vm_deallocate(ptr as *mut core::ffi::c_void, size);
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // vm_allocate returns zeroed memory
        self.alloc(layout)
    }
}
