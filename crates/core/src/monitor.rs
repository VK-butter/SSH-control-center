//! Machine utilization for the Monitoring tab. One call returns raw counters (CPU time, network
//! bytes) plus current gauges; the window turns two consecutive samples into rates, so the Target
//! never sleeps, samples in the background or keeps state between calls.
use anyhow::{bail, Result};
use serde_json::{json, Value};

pub fn handle(method: &str, _p: &Value) -> Result<Value> {
    match method {
        "sys.stats" => snapshot(),
        _ => bail!("E_ARGUMENT: unknown monitoring method"),
    }
}

fn now_ms() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

/// `cpu` line of /proc/stat: (total, idle) jiffies, idle including iowait; guest time is already in user.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_stat(text: &str) -> Option<(u64, u64, usize)> {
    let mut lines = text.lines();
    let fields: Vec<u64> = lines.next()?.strip_prefix("cpu ")?.split_whitespace().filter_map(|v| v.parse().ok()).collect();
    if fields.len() < 4 {
        return None;
    }
    let total = fields.iter().take(8).sum();
    let idle = fields[3] + fields.get(4).copied().unwrap_or(0);
    let count = lines.filter(|line| line.starts_with("cpu") && line.as_bytes().get(3).is_some_and(u8::is_ascii_digit)).count();
    Some((total, idle, count.max(1)))
}

/// Value in bytes of a /proc/meminfo field reported in kB.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn meminfo(text: &str, key: &str) -> Option<u64> {
    text.lines().find_map(|line| line.strip_prefix(key)?.strip_prefix(':')?.split_whitespace().next()?.parse::<u64>().ok()).map(|kb| kb * 1024)
}

/// Received and transmitted bytes over all interfaces except loopback.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_netdev(text: &str) -> (u64, u64) {
    text.lines().skip(2).filter_map(|line| line.split_once(':')).filter(|(name, _)| name.trim() != "lo").fold((0, 0), |(rx, tx), (_, values)| {
        let fields: Vec<u64> = values.split_whitespace().filter_map(|v| v.parse().ok()).collect();
        (rx + fields.first().copied().unwrap_or(0), tx + fields.get(8).copied().unwrap_or(0))
    })
}

/// Filesystems on real disks; everything else in /proc/self/mounts (tmpfs, overlay, squashfs snaps, proc, sysfs,
/// cgroup…) is not a disk a person can fill.
const REAL_FS: &[&str] = &["ext2", "ext3", "ext4", "xfs", "btrfs", "zfs", "f2fs", "jfs", "reiserfs", "vfat", "exfat", "ntfs", "ntfs3", "fuseblk", "bcachefs"];

/// The `df`-style partitions in /proc/self/mounts: real filesystems mounted on a directory (containers bind single
/// files such as /etc/hostname from the host's disk), one mount per device (the first, so a bind mount of the same
/// disk isn't counted twice). Each is (mount point, filesystem).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn partitions(mounts: &str, is_dir: impl Fn(&str) -> bool) -> Vec<(String, String)> {
    let unescape = |text: &str| text.replace("\\040", " ").replace("\\011", "\t").replace("\\134", "\\");
    let mut devices = std::collections::BTreeSet::new();
    mounts.lines().filter_map(|line| {
        let mut fields = line.split_whitespace();
        let (device, mount, fs) = (fields.next()?, fields.next()?, fields.next()?);
        let mount = unescape(mount);
        (REAL_FS.contains(&fs) && is_dir(&mount) && devices.insert(device.to_owned())).then(|| (mount, fs.to_owned()))
    }).collect()
}

/// Fixed drives from a `GetLogicalDrives` mask: `kind(letter)` is the drive type, 3 = fixed (removable, network,
/// CD-ROM and RAM disks are left out).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn fixed_drives(mask: u32, kind: impl Fn(char) -> u32) -> Vec<String> {
    (0..26u8).filter(|bit| mask & (1 << bit) != 0).map(|bit| (b'A' + bit) as char).filter(|letter| kind(*letter) == 3)
        .map(|letter| format!("{letter}:\\")).collect()
}

#[cfg(target_os = "linux")]
fn snapshot() -> Result<Value> {
    use anyhow::Context;
    use std::fs::read_to_string;
    let (total, idle, count) = parse_stat(&read_to_string("/proc/stat")?).context("E_STATS: unreadable /proc/stat")?;
    let memory = read_to_string("/proc/meminfo")?;
    let load: Vec<f64> = read_to_string("/proc/loadavg")?.split_whitespace().take(3).filter_map(|v| v.parse().ok()).collect();
    let uptime = read_to_string("/proc/uptime")?.split_whitespace().next().and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
    let net = read_to_string("/proc/net/dev").ok().map(|text| parse_netdev(&text));
    let usage = |path: &str| -> Option<(u64, u64)> {
        let c = std::ffi::CString::new(path).ok()?;
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        (unsafe { libc::statvfs(c.as_ptr(), &mut st) } == 0).then(|| {
            let unit = st.f_frsize as u64;
            (st.f_blocks as u64 * unit, st.f_bavail as u64 * unit)
        })
    };
    let disk = usage("/").map(|(total, free)| json!({"path": "/", "total": total, "free": free}));
    // Every real partition (entry 8). A container whose root is an overlay has none: report / alone.
    let is_dir = |path: &str| std::fs::metadata(path).is_ok_and(|m| m.is_dir());
    let mut disks: Vec<Value> = partitions(&read_to_string("/proc/self/mounts").unwrap_or_default(), is_dir).into_iter()
        .filter_map(|(path, fs)| usage(&path).filter(|(total, _)| *total > 0).map(|(total, free)| json!({"path": path, "fs": fs, "total": total, "free": free})))
        .collect();
    if disks.is_empty() {
        disks.extend(disk.clone().map(|mut d| { d["fs"] = Value::Null; d }));
    }
    Ok(json!({
        "os": "linux", "at": now_ms(), "uptimeSeconds": uptime as u64,
        "cpu": {"total": total, "idle": idle, "count": count},
        "memory": {"total": meminfo(&memory, "MemTotal"), "available": meminfo(&memory, "MemAvailable")},
        "swap": {"total": meminfo(&memory, "SwapTotal"), "free": meminfo(&memory, "SwapFree")},
        "load": load, "disk": disk, "disks": disks,
        "net": net.map(|(rx, tx)| json!({"rx": rx, "tx": tx})),
    }))
}

#[cfg(windows)]
fn snapshot() -> Result<Value> {
    use windows_sys::Win32::{
        Foundation::FILETIME,
        Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives},
        System::{
            SystemInformation::{GetSystemInfo, GetTickCount64, GlobalMemoryStatusEx, MEMORYSTATUSEX, SYSTEM_INFO},
            Threading::GetSystemTimes,
        },
    };
    let ticks = |t: FILETIME| ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64;
    let (mut idle, mut kernel, mut user) = unsafe { (std::mem::zeroed(), std::mem::zeroed(), std::mem::zeroed()) };
    if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
        bail!("E_STATS: GetSystemTimes failed");
    }
    let mut memory: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    memory.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    unsafe { GlobalMemoryStatusEx(&mut memory) };
    let mut info: SYSTEM_INFO = unsafe { std::mem::zeroed() };
    unsafe { GetSystemInfo(&mut info) };
    let wide = |path: &str| path.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let usage = |path: &str| {
        let drive = wide(path);
        let (mut available, mut total, mut free) = (0u64, 0u64, 0u64);
        (unsafe { GetDiskFreeSpaceExW(drive.as_ptr(), &mut available, &mut total, &mut free) } != 0).then_some((total, available))
    };
    let disk = usage("C:\\").map(|(total, free)| json!({"path": "C:\\", "total": total, "free": free}));
    // Every fixed drive (entry 8).
    let disks: Vec<Value> = fixed_drives(unsafe { GetLogicalDrives() }, |letter| unsafe { GetDriveTypeW(wide(&format!("{letter}:\\")).as_ptr()) })
        .into_iter().filter_map(|path| usage(&path).filter(|(total, _)| *total > 0).map(|(total, free)| json!({"path": path, "fs": "ntfs", "total": total, "free": free})))
        .collect();
    // Kernel time already includes idle time.
    Ok(json!({
        "os": "windows", "at": now_ms(), "uptimeSeconds": unsafe { GetTickCount64() } / 1000,
        "cpu": {"total": ticks(kernel) + ticks(user), "idle": ticks(idle), "count": info.dwNumberOfProcessors},
        "memory": {"total": memory.ullTotalPhys, "available": memory.ullAvailPhys},
        "swap": {"total": memory.ullTotalPageFile.saturating_sub(memory.ullTotalPhys), "free": memory.ullAvailPageFile.saturating_sub(memory.ullAvailPhys)},
        "load": null, "disk": disk, "disks": disks, "net": null,
    }))
}

#[cfg(not(any(target_os = "linux", windows)))]
fn snapshot() -> Result<Value> {
    bail!("E_UNSUPPORTED: monitoring is available on Linux and Windows")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partitions_are_real_disks_once_each() {
        let mounts = "sysfs /sys sysfs rw 0 0\nproc /proc proc rw 0 0\n/dev/nvme0n1p2 / ext4 rw 0 0\ntmpfs /run tmpfs rw 0 0\n\
            /dev/nvme0n1p1 /boot/efi vfat rw 0 0\n/dev/loop3 /snap/core22/1380 squashfs ro 0 0\n/dev/sdb1 /var/lib/my\\040data xfs rw 0 0\n\
            /dev/nvme0n1p2 /home/me/bind ext4 rw 0 0\noverlay /var/lib/docker/overlay2/x/merged overlay rw 0 0\ncgroup2 /sys/fs/cgroup cgroup2 rw 0 0\n";
        assert_eq!(partitions(mounts, |_| true), vec![("/".to_owned(), "ext4".to_owned()), ("/boot/efi".into(), "vfat".into()), ("/var/lib/my data".into(), "xfs".into())]);
        // A container: an overlay root and single files bound from the host's disk; the caller falls back to /.
        let container = "overlay / overlay rw 0 0\nproc /proc proc rw 0 0\n/dev/vda1 /etc/hostname ext4 rw 0 0\n/dev/vda1 /etc/hosts ext4 rw 0 0\n";
        assert!(partitions(container, |path| !path.starts_with("/etc/")).is_empty());
    }
    #[test]
    fn only_fixed_drives_count() {
        // C: fixed, D: fixed, E: CD-ROM (5), Z: network (4).
        let kinds = |letter| match letter { 'C' | 'D' => 3, 'E' => 5, 'Z' => 4, _ => 0 };
        assert_eq!(fixed_drives((1 << 2) | (1 << 3) | (1 << 4) | (1 << 25), kinds), vec!["C:\\", "D:\\"]);
    }

    #[test]
    fn proc_stat_counters() {
        let text = "cpu  100 5 50 800 20 3 2 0 7 0\ncpu0 50 2 25 400 10 1 1 0 0 0\ncpu1 50 3 25 400 10 2 1 0 0 0\nintr 1 2 3\n";
        assert_eq!(parse_stat(text), Some((980, 820, 2)));
        assert_eq!(parse_stat("intr 1\n"), None);
    }

    #[test]
    fn meminfo_values_are_bytes() {
        let text = "MemTotal:        8000000 kB\nMemFree:          100000 kB\nMemAvailable:    6000000 kB\nSwapTotal:             0 kB\n";
        assert_eq!(meminfo(text, "MemTotal"), Some(8_192_000_000));
        assert_eq!(meminfo(text, "MemAvailable"), Some(6_144_000_000));
        assert_eq!(meminfo(text, "SwapTotal"), Some(0));
        assert_eq!(meminfo(text, "Mem"), None);
    }

    #[test]
    fn net_dev_skips_loopback() {
        let text = "Inter-|   Receive |  Transmit\n face |bytes packets|bytes\n    lo: 500 5 0 0 0 0 0 0 500 5 0 0 0 0 0 0\n  eth0: 1000 10 0 0 0 0 0 0 2000 20 0 0 0 0 0 0\n  eth1: 30 1 0 0 0 0 0 0 40 1 0 0 0 0 0 0\n";
        assert_eq!(parse_netdev(text), (1030, 2040));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn live_snapshot_has_counters() {
        let value = snapshot().unwrap();
        assert!(value["cpu"]["total"].as_u64().unwrap() >= value["cpu"]["idle"].as_u64().unwrap());
        assert!(value["memory"]["total"].as_u64().unwrap() > 0);
    }
}
