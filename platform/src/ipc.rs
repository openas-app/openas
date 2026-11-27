//! The Helper's line to its callers: a Unix socket on a Mac, a named pipe on
//! Windows. One request per connection; the server names the caller by the
//! program file of the process on the other end.
//!
//! Windows: the pipe only admits the current user (its own DACL), refuses
//! remote clients, and the first instance is created with
//! FILE_FLAG_FIRST_PIPE_INSTANCE, so a second server can't take the name.
//! One instance is always waiting; a caller that finds them all busy waits.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

/// An FNV-1a hash of a path, to name a setup's socket or pipe.
fn fnv(p: &Path) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in p.to_string_lossy().bytes() {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    h
}

/// Where the Helper of the setup at `root` listens: `<root>/helper.sock`
/// (or a short path in the temp folder when that is too long for a Unix
/// socket, about 104 bytes on a Mac); on Windows `\\.\pipe\openas-<hash>`.
pub fn address(root: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(format!(r"\\.\pipe\openas-{:016x}", fnv(root)))
    }
    #[cfg(not(windows))]
    {
        let p = root.join("helper.sock");
        if p.as_os_str().len() < 100 {
            return p;
        }
        std::env::temp_dir().join(format!("openas-{:016x}.sock", fnv(root)))
    }
}

/// One connection, from either end.
pub struct Conn(os::Stream);

impl Conn {
    pub fn try_clone(&self) -> io::Result<Conn> {
        self.0.try_clone().map(Conn)
    }

    /// The program on the other end (server side).
    pub fn peer_exe(&self) -> Option<PathBuf> {
        os::peer_exe(&self.0)
    }

    /// Something another thread can use to cut this connection, so a reader
    /// blocked on it returns and the other end sees it gone.
    pub fn stopper(&self) -> Option<Stopper> {
        os::stopper(&self.0).map(Stopper)
    }
}

impl Read for Conn {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl Write for Conn {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl Read for &Conn {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (&self.0).read(buf)
    }
}

impl Write for &Conn {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        (&self.0).write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        (&self.0).flush()
    }
}

pub struct Stopper(os::Stopper);

impl Stopper {
    pub fn stop(&self) {
        os::stop(&self.0)
    }
}

/// Calls the Helper at `root`. `ErrorKind::NotFound` / `ConnectionRefused`
/// mean none is running.
pub fn connect(root: &Path) -> io::Result<Conn> {
    os::connect(&address(root)).map(Conn)
}

pub struct Listener(os::Listener);

impl Listener {
    /// Starts listening; fails with `AddrInUse` when a Helper already runs.
    pub fn bind(root: &Path) -> io::Result<Listener> {
        os::Listener::bind(&address(root)).map(Listener)
    }

    pub fn accept(&self) -> io::Result<Conn> {
        self.0.accept().map(Conn)
    }

    /// Before the server exits (removes the socket file on a Mac).
    pub fn close(&self) {
        self.0.close()
    }
}

#[cfg(unix)]
mod os {
    use std::io;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};

    pub type Stream = UnixStream;
    pub type Stopper = UnixStream;

    pub fn connect(addr: &Path) -> io::Result<UnixStream> {
        UnixStream::connect(addr)
    }

    pub fn stopper(s: &UnixStream) -> Option<UnixStream> {
        s.try_clone().ok()
    }

    pub fn stop(s: &UnixStream) {
        let _ = s.shutdown(std::net::Shutdown::Both);
    }

    #[cfg(target_os = "macos")]
    pub fn peer_exe(s: &UnixStream) -> Option<PathBuf> {
        use std::os::fd::AsRawFd;
        let mut pid: libc::pid_t = 0;
        let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
        // SAFETY: `pid` and `len` are live locals of the size we pass; the
        // kernel writes at most `len` bytes into `pid`. LOCAL_PEERPID asks a
        // Unix socket for the process on its other end (sys/un.h).
        let r = unsafe {
            libc::getsockopt(
                s.as_raw_fd(),
                libc::SOL_LOCAL,
                libc::LOCAL_PEERPID,
                &mut pid as *mut _ as *mut libc::c_void,
                &mut len,
            )
        };
        if r != 0 || pid <= 0 {
            return None;
        }
        let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        // SAFETY: `buf` is a live buffer of the length we pass; proc_pidpath
        // writes at most that many bytes and returns how many it wrote.
        let n = unsafe {
            libc::proc_pidpath(pid, buf.as_mut_ptr() as *mut libc::c_void, buf.len() as u32)
        };
        (n > 0).then(|| PathBuf::from(String::from_utf8_lossy(&buf[..n as usize]).into_owned()))
    }

    #[cfg(not(target_os = "macos"))]
    pub fn peer_exe(_s: &UnixStream) -> Option<PathBuf> {
        None
    }

    pub struct Listener {
        inner: UnixListener,
        path: PathBuf,
    }

    impl Listener {
        pub fn bind(path: &Path) -> io::Result<Listener> {
            if path.exists() {
                if UnixStream::connect(path).is_ok() {
                    return Err(io::Error::new(
                        io::ErrorKind::AddrInUse,
                        "a Helper is already running",
                    ));
                }
                std::fs::remove_file(path)?;
            }
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let inner = UnixListener::bind(path)?;
            // Only this user can connect.
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
            Ok(Listener {
                inner,
                path: path.to_path_buf(),
            })
        }

        pub fn accept(&self) -> io::Result<UnixStream> {
            self.inner.accept().map(|(s, _)| s)
        }

        pub fn close(&self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[cfg(windows)]
mod os {
    use std::fs::File;
    use std::io;
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    use windows::core::{HSTRING, PWSTR};
    use windows::Win32::Foundation::{
        CloseHandle, LocalFree, ERROR_ACCESS_DENIED, ERROR_PIPE_CONNECTED, HANDLE, HLOCAL,
    };
    use windows::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        SDDL_REVISION_1,
    };
    use windows::Win32::Security::{
        GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
        TOKEN_USER,
    };
    use windows::Win32::Storage::FileSystem::{
        FILE_FLAGS_AND_ATTRIBUTES, FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX,
    };
    use windows::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientProcessId, PIPE_READMODE_BYTE,
        PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    };
    use windows::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, OpenProcessToken, QueryFullProcessImageNameW,
        PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::System::IO::CancelIoEx;

    pub type Stream = File;
    /// The raw handle of a connection, to cancel its blocked read.
    pub struct Stopper(isize);

    const ERROR_PIPE_BUSY: i32 = 231;

    pub fn connect(addr: &Path) -> io::Result<File> {
        let start = std::time::Instant::now();
        loop {
            match std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(addr)
            {
                // Every instance is taken for a moment: wait, never start a
                // second Helper.
                Err(e)
                    if e.raw_os_error() == Some(ERROR_PIPE_BUSY)
                        && start.elapsed().as_secs() < 10 =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(30));
                }
                r => return r,
            }
        }
    }

    pub fn stopper(s: &File) -> Option<Stopper> {
        Some(Stopper(s.as_raw_handle() as isize))
    }

    pub fn stop(s: &Stopper) {
        // SAFETY: cancelling I/O on a handle that may already be closed only
        // fails; it never touches memory we pass.
        unsafe {
            let _ = CancelIoEx(HANDLE(s.0 as *mut _), None);
        }
    }

    pub fn peer_exe(s: &File) -> Option<PathBuf> {
        let mut pid = 0u32;
        // SAFETY: the handle is this live pipe connection; `pid`, `buf` and
        // `len` are locals of the sizes passed; the process handle is closed.
        unsafe {
            GetNamedPipeClientProcessId(HANDLE(s.as_raw_handle() as *mut _), &mut pid).ok()?;
            let p = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut buf = vec![0u16; 32768];
            let mut len = buf.len() as u32;
            let r = QueryFullProcessImageNameW(
                p,
                PROCESS_NAME_WIN32,
                PWSTR(buf.as_mut_ptr()),
                &mut len,
            );
            let _ = CloseHandle(p);
            r.ok()?;
            Some(PathBuf::from(String::from_utf16_lossy(
                &buf[..len as usize],
            )))
        }
    }

    /// A security descriptor that lets only the current user open the pipe.
    struct OwnerOnly(PSECURITY_DESCRIPTOR);
    unsafe impl Send for OwnerOnly {}
    unsafe impl Sync for OwnerOnly {}

    impl Drop for OwnerOnly {
        fn drop(&mut self) {
            // SAFETY: allocated by ConvertStringSecurityDescriptor… with LocalAlloc.
            unsafe {
                let _ = LocalFree(Some(HLOCAL(self.0 .0)));
            }
        }
    }

    fn owner_only() -> io::Result<OwnerOnly> {
        // SAFETY: the token handle is closed; the TOKEN_USER buffer is sized
        // by the first call and outlives the SID's use; the strings returned
        // by the Convert calls are freed with LocalFree.
        unsafe {
            let mut tok = HANDLE::default();
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok)?;
            let mut len = 0u32;
            let _ = GetTokenInformation(tok, TokenUser, None, 0, &mut len);
            let mut buf = vec![0u8; len as usize];
            let r = GetTokenInformation(
                tok,
                TokenUser,
                Some(buf.as_mut_ptr() as *mut _),
                len,
                &mut len,
            );
            let _ = CloseHandle(tok);
            r?;
            let user = &*(buf.as_ptr() as *const TOKEN_USER);
            let mut sid = PWSTR::null();
            ConvertSidToStringSidW(user.User.Sid, &mut sid)?;
            let sid_s = sid.to_string().unwrap_or_default();
            let _ = LocalFree(Some(HLOCAL(sid.0 as *mut _)));
            let sddl = HSTRING::from(format!("D:P(A;;GA;;;{sid_s})"));
            let mut sd = PSECURITY_DESCRIPTOR::default();
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                &sddl,
                SDDL_REVISION_1,
                &mut sd,
                None,
            )?;
            Ok(OwnerOnly(sd))
        }
    }

    pub struct Listener {
        name: HSTRING,
        sd: OwnerOnly,
        waiting: Mutex<Option<isize>>,
    }

    impl Listener {
        fn instance(&self, first: bool) -> io::Result<isize> {
            let sa = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: self.sd.0 .0,
                bInheritHandle: false.into(),
            };
            let flags = if first {
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE
            } else {
                PIPE_ACCESS_DUPLEX
            };
            // SAFETY: `name` and `sa` (and the descriptor it points to) live
            // for the call.
            let h = unsafe {
                CreateNamedPipeW(
                    &self.name,
                    FILE_FLAGS_AND_ATTRIBUTES(flags.0),
                    PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                    PIPE_UNLIMITED_INSTANCES,
                    65536,
                    65536,
                    0,
                    Some(&sa),
                )
            };
            if h.is_invalid() {
                let e = io::Error::last_os_error();
                if first && e.raw_os_error() == Some(ERROR_ACCESS_DENIED.0 as i32) {
                    return Err(io::Error::new(
                        io::ErrorKind::AddrInUse,
                        "a Helper is already running",
                    ));
                }
                return Err(e);
            }
            Ok(h.0 as isize)
        }

        pub fn bind(addr: &Path) -> io::Result<Listener> {
            let l = Listener {
                name: HSTRING::from(addr.as_os_str()),
                sd: owner_only()?,
                waiting: Mutex::new(None),
            };
            let first = l.instance(true)?;
            *l.waiting.lock().unwrap() = Some(first);
            Ok(l)
        }

        pub fn accept(&self) -> io::Result<File> {
            loop {
                let h = match self.waiting.lock().unwrap().take() {
                    Some(h) => h,
                    None => self.instance(false)?,
                };
                // SAFETY: `h` is a pipe instance we created and still own.
                let r = unsafe { ConnectNamedPipe(HANDLE(h as *mut _), None) };
                let connected = match r {
                    Ok(()) => true,
                    Err(e) => e.code() == ERROR_PIPE_CONNECTED.to_hresult(),
                };
                // The next caller always finds an instance waiting (or the
                // name busy, never gone), so no second Helper starts.
                *self.waiting.lock().unwrap() = self.instance(false).ok();
                if connected {
                    // SAFETY: we own `h`; the File takes it over and closes it.
                    return Ok(unsafe { File::from_raw_handle(h as *mut _) });
                }
                // SAFETY: as above; a failed instance is closed and we go on.
                unsafe {
                    let _ = CloseHandle(HANDLE(h as *mut _));
                }
            }
        }

        pub fn close(&self) {}
    }
}
