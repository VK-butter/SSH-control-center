//! A window runs in a private Windows JobObject. Assign the suspended process
//! before it can initialize WebView2, and terminate only its own descendants.
use std::{
    ffi::{OsStr, OsString},
    io,
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0},
    System::{
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicAccountingInformation,
            JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
            TerminateJobObject, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_BREAKAWAY_OK,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        },
        Threading::{
            CreateProcessW, GetExitCodeProcess, ResumeThread, TerminateProcess,
            WaitForSingleObject, CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_CONSOLE,
            CREATE_NO_WINDOW, CREATE_SUSPENDED, INFINITE, PROCESS_INFORMATION,
            STARTUPINFOW,
        },
    },
};

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

// CreateProcessW receives an explicit executable path, never a shell. Quote each
// argument using the Windows command-line parsing rules, including trailing '\\'.
fn quoted(argument: &OsStr) -> io::Result<Vec<u16>> {
    let mut result = vec![b'"' as u16];
    let mut slashes = 0;
    for ch in argument.encode_wide() {
        if ch == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "argument contains NUL",
            ));
        }
        if ch == b'\\' as u16 {
            slashes += 1;
            continue;
        }
        if ch == b'"' as u16 {
            result.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2 + 1));
        } else {
            result.extend(std::iter::repeat_n(b'\\' as u16, slashes));
        }
        slashes = 0;
        result.push(ch);
    }
    result.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2));
    result.push(b'"' as u16);
    Ok(result)
}

pub fn spawn_shell(executable: &Path, arguments: &[OsString]) -> io::Result<u32> {
    let mut executable_wide: Vec<u16> = executable.as_os_str().encode_wide().collect();
    executable_wide.push(0);
    let mut command_line = quoted(executable.as_os_str())?;
    for argument in arguments {
        command_line.push(b' ' as u16);
        command_line.extend(quoted(argument)?);
    }
    command_line.push(0);
    // Give the new console its own input/output handles, even when the GUI has
    // no console or the parent was launched with redirected diagnostic output.
    let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
    startup.cb = std::mem::size_of_val(&startup) as u32;
    let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe {
        CreateProcessW(executable_wide.as_ptr(), command_line.as_mut_ptr(),
            std::ptr::null(), std::ptr::null(), 0,
            CREATE_NEW_CONSOLE | CREATE_BREAKAWAY_FROM_JOB, std::ptr::null(),
            std::ptr::null(), &startup, &mut info)
    } == 0 {
        return Err(io::Error::last_os_error());
    }
    drop(Handle(info.hThread));
    let pid = info.dwProcessId;
    // Windows needs no child reaping; closing this handle leaves the process alive.
    drop(Handle(info.hProcess));
    Ok(pid)
}

fn run_window(arguments: &[OsString]) -> io::Result<u32> {
    let executable = std::env::current_exe()?.with_file_name("sshcc-window.exe");
    let mut executable_wide: Vec<u16> = executable.as_os_str().encode_wide().collect();
    executable_wide.push(0);
    let mut command_line = quoted(executable.as_os_str())?;
    for argument in arguments {
        command_line.push(b' ' as u16);
        command_line.extend(quoted(argument)?);
    }
    command_line.push(0);
    let job = Handle(unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) });
    if job.0.is_null() {
        return Err(io::Error::last_os_error());
    }
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    // Explicit shell launches may outlive the window; WebView2 stays in this job.
    limits.BasicLimitInformation.LimitFlags =
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK;
    if unsafe {
        SetInformationJobObject(
            job.0,
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            std::mem::size_of_val(&limits) as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
    startup.cb = std::mem::size_of_val(&startup) as u32;
    let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe {
        CreateProcessW(
            executable_wide.as_ptr(),
            command_line.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            CREATE_SUSPENDED | CREATE_NO_WINDOW,
            std::ptr::null(),
            std::ptr::null(),
            &startup,
            &mut info,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let process = Handle(info.hProcess);
    let thread = Handle(info.hThread);
    if unsafe { AssignProcessToJobObject(job.0, process.0) } == 0 {
        let error = io::Error::last_os_error();
        unsafe {
            TerminateProcess(process.0, 1);
            WaitForSingleObject(process.0, INFINITE);
        }
        return Err(error);
    }
    if unsafe { ResumeThread(thread.0) } == u32::MAX {
        return Err(io::Error::last_os_error()); // Dropping the job kills the suspended child.
    }
    drop(thread);
    if unsafe { WaitForSingleObject(process.0, INFINITE) } != WAIT_OBJECT_0 {
        return Err(io::Error::last_os_error());
    }
    let mut exit_code = 1;
    if unsafe { GetExitCodeProcess(process.0, &mut exit_code) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // The top-level process is gone; WebView2 descendants may still be alive.
    // Terminate this private job and verify it is empty before reporting a cycle.
    if unsafe { TerminateJobObject(job.0, 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let mut accounting: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe {
            QueryInformationJobObject(
                job.0,
                JobObjectBasicAccountingInformation,
                (&mut accounting as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                std::mem::size_of_val(&accounting) as u32,
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        if accounting.ActiveProcesses == 0 {
            break;
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "management-window descendants did not exit within five seconds",
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(exit_code)
}

pub fn open() {
    std::thread::spawn(|| {
        if let Err(error) = run_window(&[OsString::from("--management-window")]) {
            eprintln!("cannot open management window: {error}");
        }
    });
}

pub fn benchmark(cycles: u8, report: PathBuf) {
    std::thread::spawn(move || {
        let mut completed = 0_u8;
        let cycle_report = report.with_extension(format!("cycle-{}.json", std::process::id()));
        let outcome = (|| -> Result<(), String> {
            if cycle_report.exists() {
                return Err(
                    "temporary cycle report already exists; choose a fresh report path".into(),
                );
            }
            for _ in 0..cycles {
                let exit_code = run_window(&[
                    "--management-window".into(),
                    "--benchmark-cycles".into(),
                    "1".into(),
                    "--benchmark-report".into(),
                    cycle_report.as_os_str().to_owned(),
                ])
                .map_err(|error| error.to_string())?;
                let document: serde_json::Value = serde_json::from_slice(
                    &std::fs::read(&cycle_report).map_err(|error| error.to_string())?,
                )
                .map_err(|error| error.to_string())?;
                if exit_code != 0
                    || document["ok"] != true
                    || document["windowAbsent"] != true
                    || document["cyclesCompleted"] != 1
                {
                    return Err(format!(
                        "management child failed lifecycle validation: {document}"
                    ));
                }
                std::fs::remove_file(&cycle_report).map_err(|error| error.to_string())?;
                completed += 1;
            }
            Ok(())
        })();
        let document = serde_json::json!({
            "schemaVersion":1, "pid":std::process::id(), "ok":outcome.is_ok(),
            "cyclesRequested":cycles, "cyclesCompleted":completed,
            "windowAbsent":outcome.is_ok(), "windowProcessIsolation":true,
            "windowsJobContainment":true,
            "completedAtUnixSeconds":SystemTime::now().duration_since(UNIX_EPOCH)
                .map(|time| time.as_secs()).unwrap_or(0),
            "error":outcome.err()
        });
        match serde_json::to_vec_pretty(&document) {
            Ok(bytes) => {
                if let Err(error) = std::fs::write(&report, bytes) {
                    eprintln!("cannot save benchmark report {}: {error}", report.display());
                }
            }
            Err(error) => eprintln!("cannot serialize benchmark report: {error}"),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::quoted;
    use std::ffi::OsStr;
    #[test]
    fn windows_argument_quotes_preserve_spaces_quotes_and_trailing_slashes() {
        let examples = [
            ("", "\"\""),
            (
                "C:\\Program Files\\SSH Control Center\\",
                "\"C:\\Program Files\\SSH Control Center\\\\\"",
            ),
            ("a\\\"b", "\"a\\\\\\\"b\""),
        ];
        for (input, expected) in examples {
            assert_eq!(
                String::from_utf16(&quoted(OsStr::new(input)).unwrap()).unwrap(),
                expected
            );
        }
        assert!(quoted(OsStr::new("bad\0argument")).is_err());
    }
}
