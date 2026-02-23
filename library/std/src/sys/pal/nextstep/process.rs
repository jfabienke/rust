//! Process support for NeXTSTEP.
//!
//! Command::spawn() via fork()/execve(). Process::wait() via wait4().
//! Uses nextstep_sys FFI directly.

use crate::ffi::{OsStr, OsString};
use crate::fmt;
use crate::io;
use crate::num::NonZero;
use crate::path::Path;
use crate::sys::pal::nextstep::pipe::AnonPipe;
use crate::sys::pal::nextstep::fs::File;
use crate::sys_common::process::CommandEnv;

use nextstep_sys as sys;

pub use crate::ffi::OsString as EnvKey;

// =========================================================================
// Stdio
// =========================================================================

#[derive(Debug)]
pub enum Stdio {
    Inherit,
    Null,
    MakePipe,
    ParentStdout,
    ParentStderr,
    InheritFile(File),
}

impl From<AnonPipe> for Stdio {
    fn from(_pipe: AnonPipe) -> Stdio {
        Stdio::MakePipe
    }
}

impl From<io::Stdout> for Stdio {
    fn from(_: io::Stdout) -> Stdio {
        Stdio::ParentStdout
    }
}

impl From<io::Stderr> for Stdio {
    fn from(_: io::Stderr) -> Stdio {
        Stdio::ParentStderr
    }
}

impl From<File> for Stdio {
    fn from(file: File) -> Stdio {
        Stdio::InheritFile(file)
    }
}

// =========================================================================
// StdioPipes
// =========================================================================

pub struct StdioPipes {
    pub stdin: Option<AnonPipe>,
    pub stdout: Option<AnonPipe>,
    pub stderr: Option<AnonPipe>,
}

// =========================================================================
// Command
// =========================================================================

pub struct Command {
    program: OsString,
    args: Vec<OsString>,
    env: CommandEnv,
    cwd: Option<OsString>,
    stdin: Option<Stdio>,
    stdout: Option<Stdio>,
    stderr: Option<Stdio>,
}

impl Command {
    pub fn new(program: &OsStr) -> Command {
        Command {
            program: program.to_os_string(),
            args: vec![program.to_os_string()],
            env: Default::default(),
            cwd: None,
            stdin: None,
            stdout: None,
            stderr: None,
        }
    }

    pub fn arg(&mut self, arg: &OsStr) {
        self.args.push(arg.to_os_string());
    }

    pub fn env_mut(&mut self) -> &mut CommandEnv {
        &mut self.env
    }

    pub fn cwd(&mut self, dir: &OsStr) {
        self.cwd = Some(dir.to_os_string());
    }

    pub fn stdin(&mut self, stdin: Stdio) {
        self.stdin = Some(stdin);
    }

    pub fn stdout(&mut self, stdout: Stdio) {
        self.stdout = Some(stdout);
    }

    pub fn stderr(&mut self, stderr: Stdio) {
        self.stderr = Some(stderr);
    }

    pub fn get_program(&self) -> &OsStr {
        &self.program
    }

    pub fn get_args(&self) -> CommandArgs<'_> {
        CommandArgs { iter: self.args[1..].iter() }
    }

    pub fn get_envs(&self) -> CommandEnvs<'_> {
        self.env.iter()
    }

    pub fn get_current_dir(&self) -> Option<&Path> {
        self.cwd.as_ref().map(|s| Path::new(s))
    }

    pub fn spawn(
        &mut self,
        default: Stdio,
        _needs_stdin: bool,
    ) -> io::Result<(Process, StdioPipes)> {
        // Build null-terminated C strings for argv
        let mut c_args: Vec<alloc::ffi::CString> = Vec::new();
        for arg in &self.args {
            let c = alloc::ffi::CString::new(arg.as_encoded_bytes())
                .map_err(|_| io::Error::from_raw_os_error(sys::EINVAL))?;
            c_args.push(c);
        }
        let mut argv_ptrs: Vec<*const u8> = c_args.iter().map(|c| c.as_ptr() as *const u8).collect();
        argv_ptrs.push(core::ptr::null());

        let program = alloc::ffi::CString::new(self.program.as_encoded_bytes())
            .map_err(|_| io::Error::from_raw_os_error(sys::EINVAL))?;

        // Pre-open /dev/null if any stdio is Null
        let devnull_path = b"/dev/null\0";

        // --- Create pipes for MakePipe stdio ---
        // Each pipe: [0] = read end, [1] = write end
        let mut stdin_pipe: [i32; 2] = [-1, -1];
        let mut stdout_pipe: [i32; 2] = [-1, -1];
        let mut stderr_pipe: [i32; 2] = [-1, -1];

        let stdin_cfg = self.stdin.as_ref().unwrap_or(&default);
        let stdout_cfg = self.stdout.as_ref().unwrap_or(&default);
        let stderr_cfg = self.stderr.as_ref().unwrap_or(&default);

        if matches!(stdin_cfg, Stdio::MakePipe) {
            if unsafe { sys::pipe(stdin_pipe.as_mut_ptr()) } < 0 {
                return Err(io::Error::from_raw_os_error(super::common::errno()));
            }
        }
        if matches!(stdout_cfg, Stdio::MakePipe) {
            if unsafe { sys::pipe(stdout_pipe.as_mut_ptr()) } < 0 {
                close_if_valid(stdin_pipe[0]); close_if_valid(stdin_pipe[1]);
                return Err(io::Error::from_raw_os_error(super::common::errno()));
            }
        }
        if matches!(stderr_cfg, Stdio::MakePipe) {
            if unsafe { sys::pipe(stderr_pipe.as_mut_ptr()) } < 0 {
                close_if_valid(stdin_pipe[0]); close_if_valid(stdin_pipe[1]);
                close_if_valid(stdout_pipe[0]); close_if_valid(stdout_pipe[1]);
                return Err(io::Error::from_raw_os_error(super::common::errno()));
            }
        }

        // --- Build envp ---
        // If the user modified the environment, build a custom envp array.
        // Otherwise, inherit the parent's environ.
        // We hold the CStrings and pointer vec in `_env_storage` to keep them alive.
        let _env_storage: Option<(Vec<alloc::ffi::CString>, Vec<*const u8>)>;
        let envp: *const *const u8;

        if self.env.is_unchanged() {
            _env_storage = None;
            envp = unsafe { sys::environ as *const *const u8 };
        } else {
            let cstrings: Vec<alloc::ffi::CString> = self.env.capture()
                .into_iter()
                .map(|(k, v)| {
                    let mut entry = k.as_encoded_bytes().to_vec();
                    entry.push(b'=');
                    entry.extend_from_slice(v.as_encoded_bytes());
                    alloc::ffi::CString::new(entry).unwrap()
                })
                .collect();
            let mut ptrs: Vec<*const u8> = cstrings.iter().map(|c| c.as_ptr() as *const u8).collect();
            ptrs.push(core::ptr::null());
            envp = ptrs.as_ptr();
            _env_storage = Some((cstrings, ptrs));
        }

        let pid = unsafe { sys::fork() };
        if pid < 0 {
            // Fork failed — clean up pipes
            close_if_valid(stdin_pipe[0]); close_if_valid(stdin_pipe[1]);
            close_if_valid(stdout_pipe[0]); close_if_valid(stdout_pipe[1]);
            close_if_valid(stderr_pipe[0]); close_if_valid(stderr_pipe[1]);
            return Err(io::Error::from_raw_os_error(super::common::errno()));
        }

        if pid == 0 {
            // ===== Child process =====

            // Set up stdin
            match self.stdin.as_ref().unwrap_or(&default) {
                Stdio::MakePipe => {
                    unsafe { sys::dup2(stdin_pipe[0], 0); sys::close(stdin_pipe[0]); sys::close(stdin_pipe[1]); }
                }
                Stdio::Null => {
                    let fd = unsafe { sys::open(devnull_path.as_ptr(), sys::O_RDONLY, 0u16) };
                    if fd >= 0 { unsafe { sys::dup2(fd, 0); sys::close(fd); } }
                }
                Stdio::InheritFile(ref f) => {
                    unsafe { sys::dup2(f.raw_fd(), 0); }
                }
                Stdio::Inherit | Stdio::ParentStdout | Stdio::ParentStderr => { /* keep fd 0 */ }
            }

            // Set up stdout
            match self.stdout.as_ref().unwrap_or(&default) {
                Stdio::MakePipe => {
                    unsafe { sys::dup2(stdout_pipe[1], 1); sys::close(stdout_pipe[0]); sys::close(stdout_pipe[1]); }
                }
                Stdio::Null => {
                    let fd = unsafe { sys::open(devnull_path.as_ptr(), sys::O_WRONLY, 0u16) };
                    if fd >= 0 { unsafe { sys::dup2(fd, 1); sys::close(fd); } }
                }
                Stdio::ParentStdout => { /* already fd 1 */ }
                Stdio::ParentStderr => {
                    unsafe { sys::dup2(2, 1); }
                }
                Stdio::InheritFile(ref f) => {
                    unsafe { sys::dup2(f.raw_fd(), 1); }
                }
                Stdio::Inherit => { /* keep fd 1 */ }
            }

            // Set up stderr
            match self.stderr.as_ref().unwrap_or(&default) {
                Stdio::MakePipe => {
                    unsafe { sys::dup2(stderr_pipe[1], 2); sys::close(stderr_pipe[0]); sys::close(stderr_pipe[1]); }
                }
                Stdio::Null => {
                    let fd = unsafe { sys::open(devnull_path.as_ptr(), sys::O_WRONLY, 0u16) };
                    if fd >= 0 { unsafe { sys::dup2(fd, 2); sys::close(fd); } }
                }
                Stdio::ParentStdout => {
                    unsafe { sys::dup2(1, 2); }
                }
                Stdio::ParentStderr => { /* already fd 2 */ }
                Stdio::InheritFile(ref f) => {
                    unsafe { sys::dup2(f.raw_fd(), 2); }
                }
                Stdio::Inherit => { /* keep fd 2 */ }
            }

            // Change directory if requested — exit on failure
            if let Some(ref dir) = self.cwd {
                let cdir = alloc::ffi::CString::new(dir.as_encoded_bytes()).unwrap();
                if unsafe { sys::chdir(cdir.as_ptr() as *const u8) } != 0 {
                    unsafe { sys::_exit(127) };
                }
            }

            unsafe {
                sys::execve(
                    program.as_ptr() as *const u8,
                    argv_ptrs.as_ptr(),
                    envp,
                );
            }
            // execve only returns on error
            unsafe { sys::_exit(127) };
        }

        // ===== Parent process =====

        // Close child-side pipe ends, keep parent-side ends
        let mut pipes = StdioPipes { stdin: None, stdout: None, stderr: None };

        if stdin_pipe[0] >= 0 {
            unsafe { sys::close(stdin_pipe[0]); }  // close read end (child's)
            pipes.stdin = Some(AnonPipe::from_raw_fd(stdin_pipe[1]));  // parent writes
        }
        if stdout_pipe[1] >= 0 {
            unsafe { sys::close(stdout_pipe[1]); }  // close write end (child's)
            pipes.stdout = Some(AnonPipe::from_raw_fd(stdout_pipe[0]));  // parent reads
        }
        if stderr_pipe[1] >= 0 {
            unsafe { sys::close(stderr_pipe[1]); }  // close write end (child's)
            pipes.stderr = Some(AnonPipe::from_raw_fd(stderr_pipe[0]));  // parent reads
        }

        Ok((Process { pid }, pipes))
    }

    pub fn output(&mut self) -> io::Result<(ExitStatus, Vec<u8>, Vec<u8>)> {
        self.stdout(Stdio::MakePipe);
        self.stderr(Stdio::MakePipe);
        let (proc, pipes) = self.spawn(Stdio::MakePipe, false)?;

        let mut stdout_data = Vec::new();
        let mut stderr_data = Vec::new();

        // Read stdout and stderr simultaneously using read2 if both are present
        match (pipes.stdout, pipes.stderr) {
            (Some(out_pipe), Some(err_pipe)) => {
                super::pipe::read2(out_pipe, &mut stdout_data, err_pipe, &mut stderr_data)?;
            }
            (Some(out_pipe), None) => {
                out_pipe.read_to_end(&mut stdout_data)?;
            }
            (None, Some(err_pipe)) => {
                err_pipe.read_to_end(&mut stderr_data)?;
            }
            (None, None) => {}
        }

        let status = proc.wait_internal()?;
        Ok((status, stdout_data, stderr_data))
    }
}

impl fmt::Debug for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.program)?;
        for arg in &self.args[1..] {
            write!(f, " {:?}", arg)?;
        }
        Ok(())
    }
}

// =========================================================================
// Process
// =========================================================================

pub struct Process {
    pid: sys::pid_t,
}

impl Process {
    pub fn id(&self) -> u32 {
        self.pid as u32
    }

    pub fn kill(&mut self) -> io::Result<()> {
        super::common::cvt(unsafe { sys::kill(self.pid, sys::SIGKILL) })?;
        Ok(())
    }

    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        self.wait_internal()
    }

    fn wait_internal(&self) -> io::Result<ExitStatus> {
        let mut status: sys::c_int = 0;
        loop {
            let ret = unsafe {
                sys::wait4(self.pid, &mut status, 0, core::ptr::null_mut())
            };
            if ret < 0 {
                let e = super::common::errno();
                if e == sys::EINTR {
                    continue;
                }
                return Err(io::Error::from_raw_os_error(e));
            }
            break;
        }
        Ok(ExitStatus(status))
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        let mut status: sys::c_int = 0;
        let ret = unsafe {
            sys::wait4(self.pid, &mut status, sys::WNOHANG, core::ptr::null_mut())
        };
        if ret < 0 {
            return Err(io::Error::from_raw_os_error(super::common::errno()));
        }
        if ret == 0 {
            Ok(None)
        } else {
            Ok(Some(ExitStatus(status)))
        }
    }
}

// =========================================================================
// ExitStatus
// =========================================================================

#[derive(PartialEq, Eq, Clone, Copy, Debug, Default)]
pub struct ExitStatus(i32);

impl ExitStatus {
    pub fn exit_ok(&self) -> Result<(), ExitStatusError> {
        if self.code() == Some(0) {
            Ok(())
        } else {
            Err(ExitStatusError(self.0))
        }
    }

    pub fn code(&self) -> Option<i32> {
        if sys::WIFEXITED(self.0) {
            Some(sys::WEXITSTATUS(self.0))
        } else {
            None
        }
    }

    pub fn signal(&self) -> Option<i32> {
        if sys::WIFSIGNALED(self.0) {
            Some(sys::WTERMSIG(self.0))
        } else {
            None
        }
    }
}

impl fmt::Display for ExitStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(code) = self.code() {
            write!(f, "exit status: {}", code)
        } else if let Some(sig) = self.signal() {
            write!(f, "signal: {}", sig)
        } else {
            write!(f, "unknown exit status: {}", self.0)
        }
    }
}

// =========================================================================
// ExitStatusError
// =========================================================================

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ExitStatusError(i32);

impl Into<ExitStatus> for ExitStatusError {
    fn into(self) -> ExitStatus {
        ExitStatus(self.0)
    }
}

impl ExitStatusError {
    pub fn code(self) -> Option<NonZero<i32>> {
        let code = if sys::WIFEXITED(self.0) {
            sys::WEXITSTATUS(self.0)
        } else {
            self.0
        };
        NonZero::new(code)
    }
}

// =========================================================================
// ExitCode
// =========================================================================

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub struct ExitCode(u8);

impl ExitCode {
    pub const SUCCESS: ExitCode = ExitCode(0);
    pub const FAILURE: ExitCode = ExitCode(1);

    pub fn as_i32(&self) -> i32 {
        self.0 as i32
    }
}

impl From<u8> for ExitCode {
    fn from(code: u8) -> Self {
        ExitCode(code)
    }
}

// =========================================================================
// CommandArgs
// =========================================================================

pub struct CommandArgs<'a> {
    iter: crate::slice::Iter<'a, OsString>,
}

impl<'a> Iterator for CommandArgs<'a> {
    type Item = &'a OsStr;
    fn next(&mut self) -> Option<&'a OsStr> {
        self.iter.next().map(|s| s.as_ref())
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl<'a> ExactSizeIterator for CommandArgs<'a> {
    fn len(&self) -> usize {
        self.iter.len()
    }
    fn is_empty(&self) -> bool {
        self.iter.is_empty()
    }
}

impl<'a> fmt::Debug for CommandArgs<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter.clone()).finish()
    }
}

/// Close an fd if it is a valid (non-negative) descriptor.
fn close_if_valid(fd: i32) {
    if fd >= 0 {
        unsafe { sys::close(fd); }
    }
}

pub type CommandEnvs<'a> = crate::sys_common::process::CommandEnvs<'a>;
