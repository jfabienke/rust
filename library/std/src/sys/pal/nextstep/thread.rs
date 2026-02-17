// library/std/src/sys/pal/nextstep/thread.rs
//
// Rust std::thread implementation for NeXTSTEP 3.3 (m68k).
// Uses Mach 2.5 thread primitives instead of pthreads.

use crate::ffi::CStr;
use crate::io;
use crate::ptr;
use crate::time::Duration;

use nextstep_sys::{
    kern_return_t, mach_port_t, vm_address_t, vm_size_t,
    mach_task_self, mach_thread_self,
    thread_create, thread_resume, thread_suspend, thread_terminate, thread_switch,
    thread_set_state,
    vm_allocate, vm_deallocate,
    port_allocate, port_deallocate,
    msg_send, msg_receive,
    m68k_thread_state_t, M68K_THREAD_STATE, M68K_THREAD_STATE_COUNT,
    msg_header_t, MSG_OPTION_NONE, MSG_TYPE_NORMAL,
    timeval,
    select,
    KERN_SUCCESS,
};

/// Carries the user closure and the reply port to the child thread.
/// Double-boxing the `FnOnce` fat pointer (8 bytes) into a Box gives us
/// a 4-byte thin pointer suitable for the m68k C ABI stack argument.
struct ThreadPayload {
    closure:    Box<dyn FnOnce()>,
    reply_port: mach_port_t,
}

pub struct Thread {
    mach_port:  mach_port_t,
    reply_port: mach_port_t,
    stack_base: u32,
    stack_size: u32,
}

const DEFAULT_STACK_SIZE: usize = 2 * 1024 * 1024; // 2 MB

impl Thread {
    pub unsafe fn new(stack: usize, p: Box<dyn FnOnce()>) -> io::Result<Thread> {
        let stack_size = if stack == 0 { DEFAULT_STACK_SIZE } else { stack };
        let task = mach_task_self();

        // --- Allocate the Mach reply port for join() ---
        let mut reply_port: mach_port_t = 0;
        if port_allocate(task, &mut reply_port) != KERN_SUCCESS {
            return Err(io::Error::last_os_error());
        }

        // --- Create the Mach thread (suspended) ---
        let mut thread_port: mach_port_t = 0;
        if thread_create(task, &mut thread_port) != KERN_SUCCESS {
            port_deallocate(task, reply_port);
            return Err(io::Error::last_os_error());
        }

        // --- Allocate stack via Mach VM (1 = VM_FLAGS_ANYWHERE) ---
        let mut stack_base: vm_address_t = 0;
        if vm_allocate(task, &mut stack_base, stack_size as vm_size_t, 1) != KERN_SUCCESS {
            thread_terminate(thread_port);
            port_deallocate(task, reply_port);
            return Err(io::Error::last_os_error());
        }

        // --- Pack closure + reply port into a thin pointer ---
        let payload = Box::new(ThreadPayload { closure: p, reply_port });
        let p_ptr = Box::into_raw(payload) as u32;

        // --- Set up m68k CPU state ---
        let mut state: m68k_thread_state_t = core::mem::zeroed();

        // m68k stack grows downward; align to 16 bytes
        let mut sp = (stack_base + stack_size as u32) & !15;

        // m68k C ABI: arguments are on the stack, then return address.
        // Push argument (payload thin pointer) then dummy return address.
        sp -= 4;
        ptr::write(sp as *mut u32, p_ptr);       // first argument
        sp -= 4;
        ptr::write(sp as *mut u32, 0u32);         // dummy return address

        state.aregs[7] = sp;                              // A7 = stack pointer
        state.pc       = thread_trampoline as u32;        // entry point

        if thread_set_state(
            thread_port,
            M68K_THREAD_STATE,
            &mut state as *mut _ as *mut i32,
            M68K_THREAD_STATE_COUNT,
        ) != KERN_SUCCESS {
            vm_deallocate(task, stack_base, stack_size as vm_size_t);
            thread_terminate(thread_port);
            port_deallocate(task, reply_port);
            return Err(io::Error::last_os_error());
        }

        // --- Resume the thread to begin execution ---
        if thread_resume(thread_port) != KERN_SUCCESS {
            vm_deallocate(task, stack_base, stack_size as vm_size_t);
            thread_terminate(thread_port);
            port_deallocate(task, reply_port);
            return Err(io::Error::last_os_error());
        }

        Ok(Thread {
            mach_port: thread_port,
            reply_port,
            stack_base: stack_base as u32,
            stack_size: stack_size as u32,
        })
    }

    pub fn yield_now() {
        unsafe {
            // thread_switch(0, 0, 0) yields to the scheduler
            thread_switch(0, 0, 0);
        }
    }

    pub fn set_name(_name: &CStr) {
        // Mach 2.5 has no native thread naming API. No-op.
    }

    pub fn sleep(dur: Duration) {
        // 4.3BSD: select() on empty fd_sets with a timeval is the standard sleep.
        unsafe {
            let mut tv = timeval {
                tv_sec:  dur.as_secs() as i32,
                tv_usec: dur.subsec_micros() as i32,
            };
            select(
                0,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                &mut tv,
            );
        }
    }

    /// Block until the child thread finishes, then reclaim all resources.
    ///
    /// Protocol:
    ///   1. Parent blocks on `msg_receive` on the reply port.
    ///   2. Child sends a completion message and calls `thread_suspend`.
    ///   3. Parent wakes, calls `thread_terminate` (safe: child is suspended),
    ///      then deallocates the port and stack.
    pub fn join(self) {
        unsafe {
            let task     = mach_task_self();
            let hdr_size = core::mem::size_of::<msg_header_t>() as u32;

            let mut msg: msg_header_t = core::mem::zeroed();
            msg.msg_bits       = hdr_size;        // receive buffer: size only
            msg.msg_local_port = self.reply_port;

            // Block until the child sends the completion notification
            msg_receive(&mut msg as *mut _, MSG_OPTION_NONE, 0);

            // Child is now suspended — safe to terminate and reclaim memory
            thread_terminate(self.mach_port);
            port_deallocate(task, self.reply_port);
            vm_deallocate(task, self.stack_base as vm_address_t, self.stack_size as vm_size_t);
        }
    }
}

/// C-ABI entry point for new Mach threads on m68k.
///
/// Receives the ThreadPayload thin pointer as its sole stack argument,
/// runs the user closure, signals the parent via Mach IPC, then suspends
/// itself so the parent can safely reclaim the stack without a
/// use-after-free race.
extern "C" fn thread_trampoline(payload_ptr: u32) -> ! {
    unsafe {
        // Reconstruct ownership of the ThreadPayload Box
        let payload    = Box::from_raw(payload_ptr as *mut ThreadPayload);
        let reply_port = payload.reply_port;

        // Execute the user closure
        (payload.closure)();

        // --- Notify parent that we have finished ---
        let hdr_size = core::mem::size_of::<msg_header_t>() as u32;
        let mut msg: msg_header_t = core::mem::zeroed();
        // bit 31 = msg_simple (1 = no OOL data), bits 0-30 = msg_size
        msg.msg_bits        = (1u32 << 31) | hdr_size;
        msg.msg_type        = MSG_TYPE_NORMAL;
        msg.msg_local_port  = 0;
        msg.msg_remote_port = reply_port;

        msg_send(&mut msg as *mut _, MSG_OPTION_NONE, 0);

        // Suspend self — parent will call thread_terminate to reclaim us
        thread_suspend(mach_thread_self());

        // Never reached; thread_terminate will kill us before we return
        crate::intrinsics::unreachable();
    }
}
