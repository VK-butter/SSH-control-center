use super::*;
use std::{
    ffi::{CStr, CString},
    fs,
    path::PathBuf,
    process::{Child, Command, Stdio},
};
#[link(name = "systemd")]
unsafe extern "C" {
    fn sd_uid_get_sessions(uid: u32, active: i32, sessions: *mut *mut *mut libc::c_char) -> i32;
    fn sd_session_get_state(id: *const libc::c_char, state: *mut *mut libc::c_char) -> i32;
    fn sd_session_get_class(id: *const libc::c_char, class: *mut *mut libc::c_char) -> i32;
    fn sd_session_get_leader(id: *const libc::c_char, leader: *mut libc::pid_t) -> i32;
}
pub fn account(uid: u32) -> Result<(String, String, String, u32)> {
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut out = std::ptr::null_mut();
    let mut buf = vec![0u8; 65536];
    let rc =
        unsafe { libc::getpwuid_r(uid, &mut pwd, buf.as_mut_ptr().cast(), buf.len(), &mut out) };
    ensure!(
        rc == 0 && !out.is_null(),
        "E_OWNER: Owner UID no longer exists"
    );
    let text = |p| unsafe { CStr::from_ptr(p).to_string_lossy().into_owned() };
    Ok((
        text(pwd.pw_name),
        text(pwd.pw_dir),
        text(pwd.pw_shell),
        pwd.pw_gid,
    ))
}
pub fn sessions(owner: &str) -> Result<Vec<Session>> {
    let uid: u32 = owner.parse()?;
    let (name, home, shell, _) = account(uid)?;
    let mut array = std::ptr::null_mut();
    let count = unsafe { sd_uid_get_sessions(uid, 0, &mut array) };
    ensure!(
        count >= 0,
        "E_LOGIND: logind session enumeration failed ({count})"
    );
    let mut result = Vec::new();
    for i in 0..count as usize {
        let id = unsafe { *array.add(i) };
        let mut state = std::ptr::null_mut();
        let mut class = std::ptr::null_mut();
        let mut leader = 0;
        if unsafe { sd_session_get_state(id, &mut state) } >= 0
            && unsafe { sd_session_get_class(id, &mut class) } >= 0
            && unsafe { sd_session_get_leader(id, &mut leader) } >= 0
        {
            let s = unsafe { CStr::from_ptr(state).to_string_lossy().into_owned() };
            let c = unsafe { CStr::from_ptr(class).to_string_lossy() };
            if matches!(s.as_str(), "active" | "online")
                && matches!(c.as_ref(), "user" | "user-early")
            {
                // A login leader's kernel start time distinguishes a replaced session.
                if let Ok(stat) = fs::read_to_string(format!("/proc/{leader}/stat")) {
                    let start = stat
                        .rsplit_once(')')
                        .and_then(|(_, tail)| tail.split_whitespace().nth(19))
                        .unwrap_or("");
                    result.push(Session {
                        id: unsafe { CStr::from_ptr(id).to_string_lossy().into_owned() },
                        generation: format!("{leader}:{start}"),
                        owner: owner.into(),
                        account: name.clone(),
                        state: s,
                        home: home.clone(),
                        shell: shell.clone(),
                        elevated: uid == 0,
                    });
                }
            }
        }
        unsafe {
            libc::free(state.cast());
            libc::free(class.cast());
            libc::free(id.cast());
        }
    }
    unsafe {
        libc::free(array.cast());
    }
    Ok(result)
}
pub struct Process {
    child: Child,
    cgroup: PathBuf,
}
impl Process {
    pub fn terminate(&mut self) {
        let _ = fs::write(self.cgroup.join("cgroup.kill"), "1");
        let _ = self.child.kill();
    }
    pub fn wait(&mut self) -> Result<()> {
        self.child.wait()?;
        Ok(())
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        self.terminate();
        let _ = self.child.wait();
        let _ = fs::remove_dir(&self.cgroup);
    }
}
pub fn spawn(session: &Session) -> Result<Worker> {
    use std::os::unix::process::CommandExt;
    let membership = fs::read_to_string("/proc/self/cgroup")?;
    let parent = membership
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .context("E_CGROUP: cgroup v2 is required")?;
    let cgroup = PathBuf::from("/sys/fs/cgroup")
        .join(parent.trim_start_matches('/'))
        .join(format!("sshcc-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&cgroup)
        .context("E_CGROUP: service requires Delegate=yes on a writable cgroup v2 hierarchy")?;
    let mut child = match Command::new(std::env::current_exe()?)
        .arg("worker")
        .env_clear()
        .env(
            "PATH",
            "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        )
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            let _ = fs::remove_dir(&cgroup);
            return Err(e.into());
        }
    };
    if let Err(e) = fs::write(cgroup.join("cgroup.procs"), child.id().to_string()) {
        let _ = child.kill();
        let _ = child.wait();
        let _ = fs::remove_dir(&cgroup);
        return Err(e).context("E_CGROUP: attach worker before execution");
    }
    let input = child.stdin.take().unwrap();
    let output = child.stdout.take().unwrap();
    Ok(Worker {
        input: Arc::new(Mutex::new(Box::new(input))),
        output: BufReader::new(Box::new(output)),
        process: Process { child, cgroup },
        session: session.clone(),
    })
}
pub fn enter(session: &Session, admin: bool) -> Result<()> {
    let uid: u32 = session.owner.parse()?;
    let (name, home, shell, gid) = account(uid)?;
    ensure!(
        name == session.account && home == session.home,
        "E_OWNER: Owner account changed"
    );
    ensure!(
        unsafe { libc::geteuid() } == 0,
        "E_PERMISSION: worker must start as root"
    );
    if !admin {
        let name = CString::new(name.as_str())?;
        ensure!(
            unsafe { libc::initgroups(name.as_ptr(), gid) } == 0
                && unsafe { libc::setgid(gid) } == 0
                && unsafe { libc::setuid(uid) } == 0,
            "E_PERMISSION: cannot enter Owner UID/GID"
        );
    }
    // The helper is single threaded until all credentials have been set.
    for (key, value) in [
        ("HOME", home),
        ("USER", name.clone()),
        ("LOGNAME", name),
        ("SHELL", shell),
        ("LANG", "C.UTF-8".into()),
        ("XDG_RUNTIME_DIR", format!("/run/user/{uid}")),
    ] {
        std::env::set_var(key, value);
    }
    Ok(())
}
