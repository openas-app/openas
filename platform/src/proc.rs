//! Child processes: started without a console window flashing up (Windows),
//! and, for programs an app runs, stoppable together with everything they
//! started.

use std::process::{Child, Command};

/// A background program (the Helper, a build): no console window on Windows.
pub fn background(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// A program an app runs: its own process group on a Mac (and the usual
/// PATH, which apps opened from the Finder lack), no console on Windows.
/// After spawning, `Group::of` makes it stoppable as a whole.
pub fn grouped(cmd: &mut Command) -> &mut Command {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.env(
            "PATH",
            "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        )
        .process_group(0);
    }
    background(cmd)
}

/// A started program and everything it starts.
pub struct Group {
    #[cfg(unix)]
    pid: i32,
    #[cfg(windows)]
    job: Option<isize>,
}

impl Group {
    pub fn of(child: &Child) -> Group {
        #[cfg(unix)]
        {
            Group {
                pid: child.id() as i32,
            }
        }
        #[cfg(windows)]
        {
            Group {
                job: win::job_for(child),
            }
        }
    }

    /// Stops the program and everything it started.
    pub fn kill(&self) {
        #[cfg(unix)]
        // SAFETY: kill(2) takes no pointers. The child was started as the
        // leader of its own group (pgid = pid), so the negative pid reaches
        // it and everything it started, and nothing else.
        unsafe {
            libc::kill(-self.pid, libc::SIGKILL);
        }
        #[cfg(windows)]
        if let Some(j) = self.job {
            win::terminate(j);
        }
    }
}

#[cfg(windows)]
impl Drop for Group {
    fn drop(&mut self) {
        if let Some(j) = self.job {
            win::close(j);
        }
    }
}

#[cfg(windows)]
mod win {
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;

    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, TerminateJobObject,
    };

    /// A Job Object holding the child; whatever it starts joins the job too.
    /// (A program that starts something in the moment before it joins can
    /// leave that one behind.)
    pub fn job_for(child: &Child) -> Option<isize> {
        // SAFETY: the job handle is ours; the child's handle is live while
        // `child` is.
        unsafe {
            let job = CreateJobObjectW(None, None).ok()?;
            if AssignProcessToJobObject(job, HANDLE(child.as_raw_handle() as *mut _)).is_err() {
                let _ = CloseHandle(job);
                return None;
            }
            Some(job.0 as isize)
        }
    }

    pub fn terminate(job: isize) {
        // SAFETY: a job handle we own.
        unsafe {
            let _ = TerminateJobObject(HANDLE(job as *mut _), 1);
        }
    }

    pub fn close(job: isize) {
        // SAFETY: a job handle we own, closed once (Drop).
        unsafe {
            let _ = CloseHandle(HANDLE(job as *mut _));
        }
    }
}
