#[cfg(not(windows))]
use anyhow::Result;
#[cfg(not(windows))]
pub fn run() -> Result<()> {
    anyhow::bail!("Windows SCM is only available on Windows")
}
#[cfg(windows)]
mod implementation {
    use anyhow::{Context, Result};
    use std::{
        ffi::OsStr,
        ptr,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc, OnceLock,
        },
    };
    use windows_sys::Win32::{
        Foundation::{ERROR_CALL_NOT_IMPLEMENTED, NO_ERROR},
        System::Services::*,
    };
    static STOP: OnceLock<Arc<AtomicBool>> = OnceLock::new();
    static STATUS: OnceLock<usize> = OnceLock::new();
    fn status(state: u32, code: u32) {
        if let Some(handle) = STATUS.get() {
            let status = SERVICE_STATUS {
                dwServiceType: SERVICE_WIN32_OWN_PROCESS,
                dwCurrentState: state,
                dwControlsAccepted: if state == SERVICE_RUNNING {
                    SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN
                } else {
                    0
                },
                dwWin32ExitCode: code,
                dwServiceSpecificExitCode: 0,
                dwCheckPoint: 0,
                dwWaitHint: if state == SERVICE_STOP_PENDING {
                    15000
                } else {
                    0
                },
            };
            unsafe { SetServiceStatus(*handle as SERVICE_STATUS_HANDLE, &status) };
        }
    }
    unsafe extern "system" fn handler(
        control: u32,
        _event: u32,
        _data: *mut std::ffi::c_void,
        _ctx: *mut std::ffi::c_void,
    ) -> u32 {
        match control {
            SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN => {
                status(SERVICE_STOP_PENDING, 0);
                if let Some(stop) = STOP.get() {
                    stop.store(true, Ordering::SeqCst);
                    crate::service::wake();
                }
                NO_ERROR
            }
            SERVICE_CONTROL_INTERROGATE => NO_ERROR,
            _ => ERROR_CALL_NOT_IMPLEMENTED,
        }
    }
    unsafe extern "system" fn main(_argc: u32, _argv: *mut *mut u16) {
        let name = crate::security::windows::wide(OsStr::new("SSHControlCenter"));
        let handle = RegisterServiceCtrlHandlerExW(name.as_ptr(), Some(handler), ptr::null_mut());
        if handle.is_null() {
            return;
        }
        let _ = STATUS.set(handle as usize);
        let stop = STOP
            .get_or_init(|| Arc::new(AtomicBool::new(false)))
            .clone();
        status(SERVICE_START_PENDING, 0);
        status(SERVICE_RUNNING, 0);
        let result = crate::service::run(stop);
        if let Err(error) = &result {
            eprintln!("SSH Control Center service failed: {error:#}");
        }
        status(SERVICE_STOPPED, if result.is_ok() { 0 } else { 1 });
    }
    pub fn run() -> Result<()> {
        let mut name = crate::security::windows::wide(OsStr::new("SSHControlCenter"));
        let table = [
            SERVICE_TABLE_ENTRYW {
                lpServiceName: name.as_mut_ptr(),
                lpServiceProc: Some(main),
            },
            SERVICE_TABLE_ENTRYW {
                lpServiceName: ptr::null_mut(),
                lpServiceProc: None,
            },
        ];
        if unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) } == 0 {
            return Err(std::io::Error::last_os_error())
                .context("SCM dispatch failed; use service run for foreground testing");
        }
        Ok(())
    }
}
#[cfg(windows)]
pub use implementation::run;
