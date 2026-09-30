//! Identity and locale facts about the running system: time zone, locale, login
//! shell, terminal, and the local/UTC offset.
//!
//! These are the fields where the three OSes disagree the most, so each helper
//! documents the source it reads. Everything here is a *system* fact; the
//! unified model in `x-core` decides how to render it.

/// Time zone name as the platform spells it.
///
/// Unix keeps the IANA id behind the `/etc/localtime` symlink; Windows stores a
/// time zone key (`China Standard Time`) in the registry, and there is no IANA
/// name to read without shipping a mapping table.
pub fn timezone_name() -> Option<String> {
    #[cfg(unix)]
    {
        let target = std::fs::read_link("/etc/localtime").ok()?;
        let text = target.to_string_lossy().into_owned();
        let (_, name) = text.split_once("zoneinfo/")?;
        (!name.is_empty()).then(|| name.to_string())
    }

    #[cfg(windows)]
    {
        registry_string(
            r"SYSTEM\CurrentControlSet\Control\TimeZoneInformation",
            "TimeZoneKeyName",
        )
    }

    #[cfg(not(any(unix, windows)))]
    {
        None
    }
}

/// Locale of the current process, e.g. `zh_CN.UTF-8`.
///
/// On Unix this is the POSIX precedence `LC_ALL` > category > `LANG`. Windows
/// has no locale environment convention, so the user's locale name is read from
/// the OS instead.
pub fn locale_name() -> Option<String> {
    #[cfg(unix)]
    {
        ["LC_ALL", "LC_CTYPE", "LANG"]
            .into_iter()
            .find_map(environment_value)
    }

    #[cfg(windows)]
    {
        user_locale_name()
    }

    #[cfg(not(any(unix, windows)))]
    {
        None
    }
}

/// Shell the current user logs into.
///
/// Unix reads `pw_shell` from the password database, which is the authoritative
/// value even when `x` runs inside another shell. Windows has no password
/// database, so `COMSPEC` is the closest equivalent.
pub fn login_shell() -> Option<String> {
    #[cfg(unix)]
    {
        use std::ffi::CStr;
        // SAFETY: `getuid` always succeeds for a running process, and `getpwuid`
        // returns either a pointer to a static entry or NULL.
        let passwd = unsafe { libc::getpwuid(libc::getuid()) };
        if passwd.is_null() {
            return None;
        }
        // SAFETY: `pw_shell` inside a live `passwd` entry is NUL terminated.
        let shell = unsafe { CStr::from_ptr((*passwd).pw_shell) };
        basename(&shell.to_string_lossy())
    }

    #[cfg(windows)]
    {
        basename(&environment_value("COMSPEC")?)
    }

    #[cfg(not(any(unix, windows)))]
    {
        None
    }
}

/// Terminal emulator hosting `x`, if it identifies itself.
///
/// Emulators set `TERM_PROGRAM` (macOS) or `WT_SESSION` (Windows Terminal);
/// `TERM` is the next best thing, and finally the tty device name says which
/// pseudo-terminal the process is attached to even when no emulator advertised.
pub fn terminal_name() -> Option<String> {
    if let Some(program) = environment_value("TERM_PROGRAM") {
        return Some(program);
    }
    if environment_value("WT_SESSION").is_some() {
        return Some("Windows Terminal".to_string());
    }
    if let Some(term) = environment_value("TERM") {
        return Some(term);
    }

    #[cfg(unix)]
    {
        use std::ffi::CStr;
        // SAFETY: `ttyname` returns a pointer into a static buffer for a terminal
        // descriptor and NULL for anything else.
        let name = unsafe { libc::ttyname(libc::STDIN_FILENO) };
        if name.is_null() {
            return None;
        }
        // SAFETY: the returned buffer is NUL terminated.
        let path = unsafe { CStr::from_ptr(name) }
            .to_string_lossy()
            .into_owned();
        basename(&path)
    }

    #[cfg(windows)]
    {
        Some("console".to_string())
    }

    #[cfg(not(any(unix, windows)))]
    {
        None
    }
}

/// Seconds to add to a UTC timestamp to get local time, honouring DST.
pub fn utc_offset_seconds() -> Option<i64> {
    #[cfg(unix)]
    {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs() as libc::time_t;
        let mut parts = libc::tm {
            tm_sec: 0,
            tm_min: 0,
            tm_hour: 0,
            tm_mday: 1,
            tm_mon: 0,
            tm_year: 0,
            tm_wday: 0,
            tm_yday: 0,
            tm_isdst: 0,
            tm_gmtoff: 0,
            tm_zone: std::ptr::null_mut(),
        };
        // SAFETY: `parts` is writable storage of the exact type the API expects,
        // and `now` is a valid `time_t`.
        if unsafe { libc::localtime_r(&now, &mut parts) }.is_null() {
            return None;
        }
        Some(parts.tm_gmtoff as i64)
    }

    #[cfg(windows)]
    {
        local_utc_difference()
    }

    #[cfg(not(any(unix, windows)))]
    {
        None
    }
}

/// Difference between the local and UTC system clocks, in seconds.
///
/// Windows exposes neither of them as an offset, so the two wall clock readings
/// are converted to `FILETIME` (100 ns ticks) and subtracted.
#[cfg(windows)]
fn local_utc_difference() -> Option<i64> {
    use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows_sys::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};
    use windows_sys::Win32::System::Time::SystemTimeToFileTime;

    let to_ticks = |value: &SYSTEMTIME| -> Option<u64> {
        let mut file = FILETIME::default();
        // SAFETY: both are plain value types passed by reference.
        if unsafe { SystemTimeToFileTime(value, &mut file) } == 0 {
            return None;
        }
        Some(((file.dwHighDateTime as u64) << 32) | file.dwLowDateTime as u64)
    };

    let mut local = SYSTEMTIME::default();
    let mut utc = SYSTEMTIME::default();
    // SAFETY: the APIs write into storage this function owns.
    unsafe {
        GetLocalTime(&mut local);
        GetSystemTime(&mut utc);
    }
    Some((to_ticks(&local)? as i64 - to_ticks(&utc)? as i64) / 10_000_000)
}

/// `LOCALE_NAME_MAX_LENGTH` is 85 UTF-16 characters including the terminator.
#[cfg(windows)]
fn user_locale_name() -> Option<String> {
    use windows_sys::Win32::Globalization::GetUserDefaultLocaleName;

    let mut buffer = vec![0u16; 85];
    // SAFETY: the buffer is `buffer.len()` characters, which is what we pass.
    let written =
        unsafe { GetUserDefaultLocaleName(buffer.as_mut_ptr().cast(), buffer.len() as i32) };
    if written <= 0 {
        return None;
    }
    from_utf16(&buffer[..written as usize])
}

/// UTF-16 to UTF-8, stopping at the NUL terminator.
#[cfg(windows)]
fn from_utf16(units: &[u16]) -> Option<String> {
    let end = units.iter().position(|unit| *unit == 0).unwrap_or(0);
    let text = String::from_utf16_lossy(&units[..end]);
    (!text.is_empty()).then_some(text)
}

/// Read a string value from `HKLM`.
#[cfg(windows)]
fn registry_string(subkey: &str, value: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};

    let wide = |text: &str| -> Vec<u16> { text.encode_utf16().chain(std::iter::once(0)).collect() };
    let subkey = wide(subkey);
    let value_name = wide(value);

    let mut buffer = vec![0u16; 256];
    let mut bytes = (buffer.len() * 2) as u32;
    // SAFETY: `buffer` is `bytes` bytes wide and both name vectors outlive the
    // call, which only writes into the buffer.
    let found = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            subkey.as_ptr(),
            value_name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if found != 0 {
        return None;
    }
    from_utf16(&buffer[..(bytes as usize / 2).min(buffer.len())])
}

/// Non-empty environment variable.
fn environment_value(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Last path component, accepting both separators; `None` when there is none.
fn basename(path: &str) -> Option<String> {
    path.replace('\\', "/")
        .rsplit('/')
        .find(|part| !part.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basename_handles_both_separators() {
        assert_eq!(basename("/bin/zsh").as_deref(), Some("zsh"));
        assert_eq!(
            basename(r"C:\Windows\system32\cmd.exe").as_deref(),
            Some("cmd.exe")
        );
        assert_eq!(basename("/usr/").as_deref(), Some("usr"));
        assert_eq!(basename(""), None);
    }

    #[test]
    fn blank_environment_values_are_not_reported() {
        std::env::set_var("X_IDENTITY_TEST_BLANK", "   ");
        assert_eq!(environment_value("X_IDENTITY_TEST_BLANK"), None);
        assert_eq!(environment_value("X_IDENTITY_TEST_UNSET"), None);
    }

    #[cfg(unix)]
    #[test]
    fn the_login_shell_is_a_bare_program_name() {
        let shell = login_shell().expect("pw_shell");
        assert!(!shell.contains('/'), "expected a basename, got {shell}");
        assert!(!shell.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn the_utc_offset_matches_the_system_clock() {
        let offset = utc_offset_seconds().expect("offset");
        // Sanity bounds: no civil time zone is more than 15 hours from UTC.
        assert!((-15 * 3_600..=15 * 3_600).contains(&offset), "{offset}");

        let local = std::process::Command::new("date")
            .arg("+%z")
            .output()
            .expect("date");
        let raw = String::from_utf8_lossy(&local.stdout).trim().to_string();
        let sign = if raw.starts_with('-') { -1 } else { 1 };
        let digits = raw.trim_start_matches(['+', '-']);
        let hours: i64 = digits[..2].parse().expect("hours");
        let minutes: i64 = digits[2..].parse().expect("minutes");
        assert_eq!(offset, sign * (hours * 3_600 + minutes * 60));
    }

    #[test]
    fn system_identity_facts_are_readable() {
        // None of these can be guaranteed on a container, but the call itself
        // must never fail and must never return an empty string.
        for value in [timezone_name(), locale_name(), terminal_name()] {
            assert!(value.map(|v| !v.trim().is_empty()).unwrap_or(true));
        }
    }
}
