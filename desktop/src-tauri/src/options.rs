pub fn benchmark_options(args: &[String]) -> Result<Option<(u8, std::path::PathBuf)>, String> {
    let mut cycles = None;
    let mut report = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--benchmark-cycles" => {
                let value = iter.next().ok_or("--benchmark-cycles needs an integer")?;
                let value: u8 = value
                    .parse()
                    .map_err(|_| "benchmark cycles must be 1..10")?;
                if !(1..=10).contains(&value) {
                    return Err("benchmark cycles must be 1..10".into());
                }
                cycles = Some(value);
            }
            "--benchmark-report" => {
                report = Some(std::path::PathBuf::from(
                    iter.next().ok_or("--benchmark-report needs a path")?,
                ));
            }
            _ => {}
        }
    }
    match (cycles, report) {
        (None, None) => Ok(None),
        (Some(cycles), Some(report)) => Ok(Some((cycles, report))),
        _ => Err("benchmark mode requires both --benchmark-cycles and --benchmark-report".into()),
    }
}

//

/// Words of the tray menu and of the one message shown before any window exists. The tray is its own
/// process, starts before any window and cannot see the window's EN | ไทย choice, so it follows the OS
/// language. The product name stays English in both.
#[allow(dead_code)] // each binary shows only some of these
pub struct TrayText {
    pub open: &'static str,
    pub quit: &'static str,
    pub cannot_open: &'static str,
}

pub fn tray_text(thai: bool) -> TrayText {
    if thai {
        TrayText {
            open: "เปิด SSH Control Center",
            quit: "ออกจากไอคอน tray (บริการยังทำงานต่อ)",
            cannot_open: "เปิดหน้าต่างจัดการไม่ได้",
        }
    } else {
        TrayText {
            open: "Open SSH Control Center",
            quit: "Quit tray icon (service keeps running)",
            cannot_open: "Cannot open the management window",
        }
    }
}

/// True for a Thai locale name: `th`, `th-TH`, `th_TH.UTF-8` and the like.
pub fn is_thai(locale: &str) -> bool {
    let locale = locale.trim().to_ascii_lowercase();
    locale == "th" || ["th-", "th_", "th."].iter().any(|prefix| locale.starts_with(prefix))
}

/// The OS language, read once when the tray starts: the user's UI language on Windows, the gettext
/// variables (LANGUAGE, LC_ALL, LC_MESSAGES, LANG, first one set) elsewhere.
#[cfg(windows)]
pub fn os_is_thai() -> bool {
    // LANG_THAI is primary language 0x1E.
    let language = unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() };
    language & 0x3ff == 0x1e
}

#[cfg(not(windows))]
pub fn os_is_thai() -> bool {
    ["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|value| !value.is_empty())
        .is_some_and(|value| is_thai(value.split(':').next().unwrap_or("")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thai_locales() {
        for locale in ["th", "th-TH", "th_TH.UTF-8", "TH_th", " th "] {
            assert!(is_thai(locale), "{locale}");
        }
        for locale in ["", "en", "en_US.UTF-8", "C", "thx", "ta_IN"] {
            assert!(!is_thai(locale), "{locale}");
        }
    }

    #[test]
    fn tray_words() {
        assert_eq!(tray_text(false).open, "Open SSH Control Center");
        assert!(tray_text(true).open.contains("SSH Control Center"));
        assert_ne!(tray_text(true).quit, tray_text(false).quit);
    }
}
