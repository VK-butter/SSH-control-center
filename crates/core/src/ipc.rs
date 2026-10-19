use crate::{config, security};
use anyhow::{bail, Context, Result};
use std::io::Write;
#[cfg(unix)]
pub type Connection = std::os::unix::net::UnixStream;
#[cfg(windows)]
pub struct Connection(std::fs::File);
#[cfg(windows)]
impl std::os::windows::io::AsRawHandle for Connection {
    fn as_raw_handle(&self) -> std::os::windows::io::RawHandle {
        std::os::windows::io::AsRawHandle::as_raw_handle(&self.0)
    }
}
#[cfg(windows)]
fn overlapped() -> std::io::Result<(std::fs::File, windows_sys::Win32::System::IO::OVERLAPPED)> {
    use std::{os::windows::io::FromRawHandle, ptr};
    let handle = unsafe {
        windows_sys::Win32::System::Threading::CreateEventW(ptr::null(), 1, 0, ptr::null())
    };
    if handle.is_null() {
        return Err(std::io::Error::last_os_error());
    }
    let event = unsafe { std::fs::File::from_raw_handle(handle) };
    let mut operation: windows_sys::Win32::System::IO::OVERLAPPED = unsafe { std::mem::zeroed() };
    operation.hEvent = handle;
    Ok((event, operation))
}
#[cfg(windows)]
impl Connection {
    pub fn try_clone(&self) -> std::io::Result<Self> {
        Ok(Self(self.0.try_clone()?))
    }
    fn io(&self, buffer: *mut u8, length: usize, write: bool) -> std::io::Result<usize> {
        if length == 0 {
            return Ok(0);
        }
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::{
            Foundation::{ERROR_BROKEN_PIPE, ERROR_IO_PENDING},
            Storage::FileSystem::{ReadFile, WriteFile},
            System::IO::GetOverlappedResult,
        };
        let (_event, mut operation) = overlapped()?;
        let mut count = 0;
        let handle = self.as_raw_handle();
        let success = unsafe {
            if write {
                WriteFile(
                    handle,
                    buffer,
                    length.min(u32::MAX as usize) as u32,
                    &mut count,
                    &mut operation,
                )
            } else {
                ReadFile(
                    handle,
                    buffer,
                    length.min(u32::MAX as usize) as u32,
                    &mut count,
                    &mut operation,
                )
            }
        };
        if success == 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
                if !write && error.raw_os_error() == Some(ERROR_BROKEN_PIPE as i32) {
                    return Ok(0);
                }
                return Err(error);
            }
            if unsafe { GetOverlappedResult(handle, &operation, &mut count, 1) } == 0 {
                let error = std::io::Error::last_os_error();
                if !write && error.raw_os_error() == Some(ERROR_BROKEN_PIPE as i32) {
                    return Ok(0);
                }
                return Err(error);
            }
        }
        Ok(count as usize)
    }
}
#[cfg(windows)]
impl std::io::Read for Connection {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.io(bytes.as_mut_ptr(), bytes.len(), false)
    }
}
#[cfg(windows)]
impl Write for Connection {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.io(bytes.as_ptr().cast_mut(), bytes.len(), true)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(unix)]
pub struct Listener {
    inner: std::os::unix::net::UnixListener,
    path: std::path::PathBuf,
}
#[cfg(unix)]
impl Listener {
    pub fn bind(_cfg: &config::Config) -> Result<Self> {
        use std::os::unix::fs::PermissionsExt;
        let path = config::endpoint();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if path.exists() {
            if std::os::unix::net::UnixStream::connect(&path).is_ok() {
                bail!("E_SERVICE_RUNNING: socket is in use");
            }
            let meta = std::fs::symlink_metadata(&path)?;
            use std::os::unix::fs::{FileTypeExt, MetadataExt};
            if !meta.file_type().is_socket() || meta.uid() != unsafe { libc::geteuid() } {
                bail!("E_PERMISSION: refusing to replace unowned socket");
            }
            std::fs::remove_file(&path)?;
        }
        let inner = std::os::unix::net::UnixListener::bind(&path)?;
        // The kernel's SO_PEERCRED authenticates every caller before dispatch.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666))?;
        Ok(Self { inner, path })
    }
    pub fn accept(&mut self) -> Result<Connection> {
        Ok(self.inner.accept()?.0)
    }
}
#[cfg(unix)]
impl Drop for Listener {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
#[cfg(unix)]
pub fn peer(connection: &Connection) -> Result<String> {
    use std::os::fd::AsRawFd;
    let mut credential = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut size = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            connection.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credential as *mut libc::ucred).cast(),
            &mut size,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(credential.uid.to_string())
}
#[cfg(unix)]
pub fn connect() -> Result<Connection> {
    let stream = Connection::connect(config::endpoint())
        .context("E_SERVICE_UNAVAILABLE: cannot connect to local service")?;
    let server = peer(&stream)?;
    if !security::is_system(&server) && server != security::current_principal()? {
        bail!("E_PERMISSION: untrusted socket server");
    }
    Ok(stream)
}

#[cfg(windows)]
pub struct Listener {
    sddl: String,
    pending: Option<Connection>,
    keepalive: Option<Connection>,
}
#[cfg(windows)]
impl Listener {
    pub fn bind(cfg: &config::Config) -> Result<Self> {
        let mut sddl = format!("D:P(A;;GA;;;SY)(A;;GA;;;{})", cfg.owner);
        for p in &cfg.allowed_principals {
            security::validate_principal(p)?;
            sddl.push_str(&format!("(A;;GA;;;{p})"));
        }
        let mut listener = Self {
            sddl,
            pending: None,
            keepalive: None,
        };
        listener.pending = Some(listener.create(true)?);
        Ok(listener)
    }
    fn create(&self, first: bool) -> Result<Connection> {
        use std::os::windows::io::FromRawHandle;
        use windows_sys::Win32::{
            Foundation::{LocalFree, INVALID_HANDLE_VALUE},
            Security::SECURITY_ATTRIBUTES,
            Storage::FileSystem::{
                FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX,
            },
            System::Pipes::{
                CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
                PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
            },
        };
        let sd = security::windows::descriptor(&self.sddl)?;
        let attrs = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd,
            bInheritHandle: 0,
        };
        let name = security::windows::wide(config::endpoint().as_os_str());
        let handle = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX
                    | FILE_FLAG_OVERLAPPED
                    | if first {
                        FILE_FLAG_FIRST_PIPE_INSTANCE
                    } else {
                        0
                    },
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                65536,
                65536,
                5000,
                &attrs,
            )
        };
        unsafe { LocalFree(sd) };
        if handle == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(Connection(unsafe {
            std::fs::File::from_raw_handle(handle)
        }))
    }
    pub fn accept(&mut self) -> Result<Connection> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::{
            Foundation::{ERROR_IO_PENDING, ERROR_PIPE_CONNECTED},
            System::{Pipes::ConnectNamedPipe, IO::GetOverlappedResult},
        };
        let pipe = if let Some(pipe) = self.pending.take() {
            pipe
        } else {
            self.create(false)?
        };
        let (_event, mut operation) = overlapped()?;
        if unsafe { ConnectNamedPipe(pipe.as_raw_handle(), &mut operation) } == 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_IO_PENDING as i32) {
                let mut count = 0;
                if unsafe { GetOverlappedResult(pipe.as_raw_handle(), &operation, &mut count, 1) }
                    == 0
                {
                    return Err(std::io::Error::last_os_error().into());
                }
            } else if error.raw_os_error() != Some(ERROR_PIPE_CONNECTED as i32) {
                return Err(error.into());
            }
        }
        // Keep one instance alive across accept calls to reserve the name continuously.
        self.keepalive = Some(pipe.try_clone()?);
        Ok(pipe)
    }
}
#[cfg(windows)]
pub fn peer(connection: &Connection) -> Result<String> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::{Security::RevertToSelf, System::Pipes::ImpersonateNamedPipeClient};
    if unsafe { ImpersonateNamedPipeClient(connection.as_raw_handle()) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let result = security::windows::thread_sid();
    // Never continue a privileged server thread if reverting impersonation fails.
    if unsafe { RevertToSelf() } == 0 {
        std::process::abort();
    }
    result
}
#[cfg(windows)]
pub fn connect() -> Result<Connection> {
    use std::{os::windows::io::FromRawHandle, ptr};
    use windows_sys::Win32::{
        Foundation::{LocalFree, GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE},
        Security::{
            Authorization::{ConvertSidToStringSidW, GetSecurityInfo, SE_KERNEL_OBJECT},
            OWNER_SECURITY_INFORMATION,
        },
        Storage::FileSystem::{
            CreateFileW, FILE_FLAG_OVERLAPPED, OPEN_EXISTING, READ_CONTROL,
            SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT,
        },
        System::Pipes::WaitNamedPipeW,
    };
    let name = security::windows::wide(config::endpoint().as_os_str());
    let mut handle = INVALID_HANDLE_VALUE;
    for _ in 0..20 {
        handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE | READ_CONTROL,
                0,
                ptr::null(),
                OPEN_EXISTING,
                SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION | FILE_FLAG_OVERLAPPED,
                ptr::null_mut(),
            )
        };
        if handle != INVALID_HANDLE_VALUE {
            break;
        }
        unsafe { WaitNamedPipeW(name.as_ptr(), 250) };
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    if handle == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error())
            .context("E_SERVICE_UNAVAILABLE: cannot open local service pipe");
    }
    let file = unsafe { std::fs::File::from_raw_handle(handle) };
    // Verify kernel object ownership without requiring access to a SYSTEM process token.
    let mut owner = ptr::null_mut();
    let mut descriptor = ptr::null_mut();
    let result = unsafe {
        GetSecurityInfo(
            handle,
            SE_KERNEL_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if result != 0 {
        bail!("E_PERMISSION: cannot verify pipe owner ({result})");
    }
    let mut sid_text = ptr::null_mut();
    let ok = unsafe { ConvertSidToStringSidW(owner, &mut sid_text) };
    unsafe { LocalFree(descriptor) };
    if ok == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut len = 0;
    unsafe {
        while *sid_text.add(len) != 0 {
            len += 1;
        }
    }
    let sid = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(sid_text, len) });
    unsafe { LocalFree(sid_text.cast()) };
    if !security::is_system(&sid) && sid != "S-1-5-32-544" && sid != security::current_principal()?
    {
        bail!("E_PERMISSION: untrusted pipe server");
    }
    Ok(Connection(file))
}
pub fn read_request(connection: &mut Connection) -> Result<crate::protocol::Request> {
    let mut reader = std::io::BufReader::new(connection);
    crate::protocol::read_frame(&mut reader)
}
pub fn reply(connection: &mut impl Write, response: &crate::protocol::Response) -> Result<()> {
    crate::protocol::write_frame(response, connection)
}
