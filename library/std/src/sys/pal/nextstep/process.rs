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
        _default: Stdio,
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

        // Build envp from current environment (we don't modify env for now)
        let envp = unsafe { sys::environ as *const *const u8 };

        let program = alloc::ffi::CString::new(self.program.as_encoded_bytes())
            .map_err(|_| io::Error::from_raw_os_error(sys::EINVAL))?;

        let pid = unsafe { sys::fork() };
        if pid < 0 {
            return Err(io::Error::from_raw_os_error(super::common::errno()));
        }

        if pid == 0 {
            // Child process
            if let Some(ref dir) = self.cwd {
                let cdir = alloc::ffi::CString::new(dir.as_encoded_bytes()).unwrap();
                unsafe { sys::chdir(cdir.as_ptr() as *const u8) };
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

        // Parent process
        Ok((
            Process { pid },
            StdioPipes {
                stdin: None,
                stdout: None,
                stderr: None,
            },
        ))
    }

    pub fn output(&mut self) -> io::Result<(ExitStatus, Vec<u8>, Vec<u8>)> {
        let (proc, _pipes) = self.spawn(Stdio::MakePipe, false)?;
        let status = proc.wait_internal()?;
        Ok((status, Vec::new(), Vec::new()))
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

pub type CommandEnvs<'a> = crate::sys_common::process::CommandEnvs<'a>;
