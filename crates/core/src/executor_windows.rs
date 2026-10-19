use super::*;
use crate::security::windows::{sid_for_token, wide};
use std::{ffi::OsStr, fs::File, os::windows::io::FromRawHandle, ptr};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, SetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE,
    },
    Security::{
        GetTokenInformation, TokenElevation, TokenLinkedToken, TokenSessionId, TokenStatistics,
        SECURITY_ATTRIBUTES, TOKEN_ELEVATION, TOKEN_LINKED_TOKEN, TOKEN_STATISTICS,
    },
    Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING},
    System::{
        Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock},
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        },
        Pipes::CreatePipe,
        RemoteDesktop::{
            WTSEnumerateSessionsW, WTSFreeMemory, WTSQueryUserToken, WTS_SESSION_INFOW,
        },
        Threading::*,
    },
    UI::Shell::{GetUserProfileDirectoryW, LoadUserProfileW, UnloadUserProfile, PROFILEINFOW},
};
struct Handle(HANDLE);
unsafe impl Send for Handle {}
impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}
fn checked(handle: HANDLE) -> Result<Handle> {
    ensure!(
        !handle.is_null() && handle != INVALID_HANDLE_VALUE,
        "E_EXEC: {}",
        std::io::Error::last_os_error()
    );
    Ok(Handle(handle))
}
fn info<T>(token: HANDLE, class: i32) -> Result<T> {
    let mut value: T = unsafe { std::mem::zeroed() };
    let mut size = 0;
    ensure!(
        unsafe {
            GetTokenInformation(
                token,
                class,
                (&mut value as *mut T).cast(),
                std::mem::size_of::<T>() as u32,
                &mut size,
            )
        } != 0,
        "E_TOKEN: {}",
        std::io::Error::last_os_error()
    );
    Ok(value)
}
pub fn elevated(token: HANDLE) -> Result<bool> {
    Ok(info::<TOKEN_ELEVATION>(token, TokenElevation)?.TokenIsElevated != 0)
}
fn owner_token(id: u32, owner: &str) -> Result<Handle> {
    let mut raw = ptr::null_mut();
    ensure!(
        unsafe { WTSQueryUserToken(id, &mut raw) } != 0,
        "E_TOKEN: WTSQueryUserToken requires SYSTEM/SeTcbPrivilege: {}",
        std::io::Error::last_os_error()
    );
    let mut token = checked(raw)?;
    ensure!(
        sid_for_token(token.0)? == owner,
        "E_IDENTITY: session SID differs from Owner"
    );
    if !elevated(token.0)? {
        if let Ok(linked) = info::<TOKEN_LINKED_TOKEN>(token.0, TokenLinkedToken) {
            token = checked(linked.LinkedToken)?;
        }
    }
    ensure!(
        sid_for_token(token.0)? == owner,
        "E_IDENTITY: linked token SID differs from Owner"
    );
    ensure!(
        info::<u32>(token.0, TokenSessionId)? == id,
        "E_IDENTITY: linked token session differs"
    );
    Ok(token)
}
fn account(token: HANDLE) -> Result<(String, String)> {
    use windows_sys::Win32::Security::{LookupAccountSidW, TokenUser, TOKEN_USER};
    let mut bytes = vec![0u8; 4096];
    let mut needed = 0;
    ensure!(
        unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                bytes.as_mut_ptr().cast(),
                bytes.len() as u32,
                &mut needed,
            )
        } != 0,
        "E_TOKEN: cannot read user SID"
    );
    let user = unsafe { &*bytes.as_ptr().cast::<TOKEN_USER>() };
    let mut name = vec![0u16; 512];
    let mut domain = vec![0u16; 512];
    let mut n = name.len() as u32;
    let mut d = domain.len() as u32;
    let mut kind = 0;
    ensure!(
        unsafe {
            LookupAccountSidW(
                ptr::null(),
                user.User.Sid,
                name.as_mut_ptr(),
                &mut n,
                domain.as_mut_ptr(),
                &mut d,
                &mut kind,
            )
        } != 0,
        "E_OWNER: cannot resolve Owner name"
    );
    let account = String::from_utf16_lossy(&name[..n as usize]);
    let mut home = vec![0u16; 32768];
    let mut len = home.len() as u32;
    ensure!(
        unsafe { GetUserProfileDirectoryW(token, home.as_mut_ptr(), &mut len) } != 0,
        "E_PROFILE: cannot resolve Owner profile"
    );
    let home = String::from_utf16_lossy(&home[..home.iter().position(|c| *c == 0).unwrap_or(0)]);
    Ok((account, home))
}
pub fn sessions(owner: &str) -> Result<Vec<Session>> {
    let mut array: *mut WTS_SESSION_INFOW = ptr::null_mut();
    let mut count = 0;
    ensure!(
        unsafe { WTSEnumerateSessionsW(ptr::null_mut(), 0, 1, &mut array, &mut count) } != 0,
        "E_SESSION: {}",
        std::io::Error::last_os_error()
    );
    let mut result = Vec::new();
    for item in unsafe { std::slice::from_raw_parts(array, count as usize) } {
        if item.SessionId == 0 {
            continue;
        }
        // Locked and disconnected RDP sessions still have a logon token.
        if let Ok(token) = owner_token(item.SessionId, owner) {
            if let (Ok((name, home)), Ok(stat)) = (
                account(token.0),
                info::<TOKEN_STATISTICS>(token.0, TokenStatistics),
            ) {
                result.push(Session {
                    id: item.SessionId.to_string(),
                    generation: format!(
                        "{}:{}",
                        stat.AuthenticationId.HighPart, stat.AuthenticationId.LowPart
                    ),
                    owner: owner.into(),
                    account: name,
                    state: format!("{}", item.State),
                    home,
                    shell: "powershell".into(),
                    elevated: elevated(token.0)?,
                });
            }
        }
    }
    unsafe {
        WTSFreeMemory(array.cast());
    }
    Ok(result)
}
pub struct Process {
    process: Handle,
    job: Handle,
    token: Handle,
    profile: HANDLE,
}
// All handles are owned and accessed through Worker/Mutex; Win32 permits cross-thread use.
unsafe impl Send for Process {}
impl Process {
    pub fn terminate(&mut self) {
        unsafe {
            TerminateJobObject(self.job.0, 1);
        }
    }
    pub fn wait(&mut self) -> Result<()> {
        unsafe {
            WaitForSingleObject(self.process.0, INFINITE);
        }
        Ok(())
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        self.terminate();
        let _ = self.wait();
        unsafe {
            UnloadUserProfile(self.token.0, self.profile);
        }
    }
}
struct Environment(*mut std::ffi::c_void);
impl Drop for Environment {
    fn drop(&mut self) {
        unsafe {
            DestroyEnvironmentBlock(self.0);
        }
    }
}
struct Profile {
    token: Handle,
    handle: HANDLE,
}
impl Drop for Profile {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                UnloadUserProfile(self.token.0, self.handle);
            }
        }
    }
}
fn pipe() -> Result<(Handle, Handle)> {
    let attrs = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: ptr::null_mut(),
        bInheritHandle: 1,
    };
    let mut read = ptr::null_mut();
    let mut write = ptr::null_mut();
    ensure!(
        unsafe { CreatePipe(&mut read, &mut write, &attrs, 65536) } != 0,
        "E_PIPE: {}",
        std::io::Error::last_os_error()
    );
    Ok((checked(read)?, checked(write)?))
}
pub fn spawn(session: &Session) -> Result<Worker> {
    static LAUNCH: Mutex<()> = Mutex::new(());
    let _launch = LAUNCH.lock().unwrap();
    let token = owner_token(session.id.parse()?, &session.owner)?;
    ensure!(
        elevated(token.0)?,
        "E_OWNER_NOT_ADMIN: elevated token is unavailable"
    );
    let stat = info::<TOKEN_STATISTICS>(token.0, TokenStatistics)?;
    ensure!(
        format!(
            "{}:{}",
            stat.AuthenticationId.HighPart, stat.AuthenticationId.LowPart
        ) == session.generation,
        "E_OWNER_NOT_LOGGED_IN: logon session was replaced"
    );
    let mut name = wide(OsStr::new(&session.account));
    let mut profile: PROFILEINFOW = unsafe { std::mem::zeroed() };
    profile.dwSize = std::mem::size_of_val(&profile) as u32;
    profile.lpUserName = name.as_mut_ptr();
    profile.dwFlags = 1;
    ensure!(
        unsafe { LoadUserProfileW(token.0, &mut profile) } != 0,
        "E_PROFILE: {}",
        std::io::Error::last_os_error()
    );
    let mut profile = Profile {
        token,
        handle: profile.hProfile,
    };
    let mut environment = ptr::null_mut();
    ensure!(
        unsafe { CreateEnvironmentBlock(&mut environment, profile.token.0, 0) } != 0,
        "E_ENVIRONMENT: {}",
        std::io::Error::last_os_error()
    );
    let environment = Environment(environment);
    let (input_read, input_write) = pipe()?;
    let (output_read, output_write) = pipe()?;
    ensure!(
        unsafe { SetHandleInformation(input_write.0, HANDLE_FLAG_INHERIT, 0) } != 0
            && unsafe { SetHandleInformation(output_read.0, HANDLE_FLAG_INHERIT, 0) } != 0,
        "E_PIPE: cannot restrict inheritance"
    );
    let attrs = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: ptr::null_mut(),
        bInheritHandle: 1,
    };
    let nul = wide(OsStr::new("NUL"));
    let error = checked(unsafe {
        CreateFileW(
            nul.as_ptr(),
            windows_sys::Win32::Foundation::GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            &attrs,
            OPEN_EXISTING,
            0,
            ptr::null_mut(),
        )
    })?;
    let mut attr_size = 0;
    unsafe {
        InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &mut attr_size);
    }
    let mut attributes = vec![0usize; attr_size.div_ceil(std::mem::size_of::<usize>())];
    let list = attributes.as_mut_ptr().cast();
    ensure!(
        unsafe { InitializeProcThreadAttributeList(list, 1, 0, &mut attr_size) } != 0,
        "E_EXEC: initialize inheritance list"
    );
    struct AttributeList(LPPROC_THREAD_ATTRIBUTE_LIST);
    impl Drop for AttributeList {
        fn drop(&mut self) {
            unsafe {
                DeleteProcThreadAttributeList(self.0);
            }
        }
    }
    let list = AttributeList(list);
    let mut inherited = [input_read.0, output_write.0, error.0];
    ensure!(
        unsafe {
            UpdateProcThreadAttribute(
                list.0,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                inherited.as_mut_ptr().cast(),
                std::mem::size_of_val(&inherited),
                ptr::null_mut(),
                ptr::null_mut(),
            )
        } != 0,
        "E_EXEC: restrict inherited handles"
    );
    let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    startup.StartupInfo.cb = std::mem::size_of_val(&startup) as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = input_read.0;
    startup.StartupInfo.hStdOutput = output_write.0;
    startup.StartupInfo.hStdError = error.0;
    startup.lpAttributeList = list.0;
    let exe = std::env::current_exe()?;
    let app = wide(exe.as_os_str());
    let mut command = wide(OsStr::new(&format!("\"{}\" worker", exe.display())));
    let cwd = wide(OsStr::new(&session.home));
    let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    ensure!(
        unsafe {
            CreateProcessAsUserW(
                profile.token.0,
                app.as_ptr(),
                command.as_mut_ptr(),
                ptr::null(),
                ptr::null(),
                1,
                CREATE_SUSPENDED
                    | CREATE_UNICODE_ENVIRONMENT
                    | CREATE_NO_WINDOW
                    | EXTENDED_STARTUPINFO_PRESENT,
                environment.0,
                cwd.as_ptr(),
                &startup.StartupInfo,
                &mut pi,
            )
        } != 0,
        "E_EXEC: CreateProcessAsUser: {}",
        std::io::Error::last_os_error()
    );
    let process = checked(pi.hProcess)?;
    let thread = checked(pi.hThread)?;
    let containment = (|| -> Result<Handle> {
        let job = checked(unsafe { CreateJobObjectW(ptr::null(), ptr::null()) })?;
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        ensure!(
            unsafe {
                SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    std::mem::size_of_val(&limits) as u32,
                )
            } != 0
                && unsafe { AssignProcessToJobObject(job.0, process.0) } != 0,
            "E_EXEC: contain suspended worker before execution"
        );
        Ok(job)
    })();
    let job = match containment {
        Ok(job) => job,
        Err(e) => {
            unsafe {
                TerminateProcess(process.0, 1);
            }
            return Err(e);
        }
    };
    ensure!(
        unsafe { ResumeThread(thread.0) } != u32::MAX,
        "E_EXEC: resume worker"
    );
    let profile_handle = std::mem::replace(&mut profile.handle, ptr::null_mut());
    let token = std::mem::replace(&mut profile.token, Handle(ptr::null_mut()));
    let input = unsafe { File::from_raw_handle(input_write.0) };
    std::mem::forget(input_write);
    let output = unsafe { File::from_raw_handle(output_read.0) };
    std::mem::forget(output_read);
    Ok(Worker {
        input: Arc::new(Mutex::new(Box::new(input))),
        output: BufReader::new(Box::new(output)),
        process: Process {
            process,
            job,
            token,
            profile: profile_handle,
        },
        session: session.clone(),
    })
}
pub fn enter(session: &Session, _admin: bool) -> Result<()> {
    use crate::security;
    ensure!(
        security::current_principal()? == session.owner,
        "E_IDENTITY: worker SID mismatch"
    );
    let mut token = ptr::null_mut();
    ensure!(
        unsafe {
            OpenProcessToken(
                GetCurrentProcess(),
                windows_sys::Win32::Security::TOKEN_QUERY,
                &mut token,
            )
        } != 0,
        "E_TOKEN: query worker token"
    );
    let token = checked(token)?;
    ensure!(
        elevated(token.0)?,
        "E_OWNER_NOT_ADMIN: worker is not elevated"
    );
    ensure!(
        info::<u32>(token.0, TokenSessionId)?.to_string() == session.id,
        "E_IDENTITY: worker session mismatch"
    );
    Ok(())
}
