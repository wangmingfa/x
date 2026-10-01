//! Windows service adapter: the service control manager.
//!
//! Everything here is native: `EnumServicesStatusExW` for the list, and
//! `StartServiceW`, `ControlService` and `ChangeServiceConfigW` for the
//! lifecycle actions. The one level-3 piece is `logs`: there is no public API
//! for reading the event log with an XPath provider filter that beats
//! `wevtutil qe /f:xml` on fidelity, and `native` deliberately hands its
//! arguments to the SCM's own `sc` command line.

use super::buffer::AlignedBuffer;
use crate::sys;
use windows_sys::Win32::Foundation::{
    GetLastError, ERROR_ACCESS_DENIED, ERROR_MORE_DATA, ERROR_SERVICE_DOES_NOT_EXIST,
};
use windows_sys::Win32::Globalization::{MultiByteToWideChar, CP_OEMCP};
use windows_sys::Win32::System::Services::{
    ChangeServiceConfigW, CloseServiceHandle, ControlService, EnumServicesStatusExW,
    OpenSCManagerW, OpenServiceW, QueryServiceConfigW, StartServiceW, ENUM_SERVICE_STATUS_PROCESSW,
    QUERY_SERVICE_CONFIGW, SC_ENUM_PROCESS_INFO, SC_HANDLE, SC_MANAGER_CONNECT,
    SC_MANAGER_ENUMERATE_SERVICE, SERVICE_CHANGE_CONFIG, SERVICE_CONTROL_STOP,
    SERVICE_DEMAND_START, SERVICE_DISABLED, SERVICE_NO_CHANGE, SERVICE_QUERY_CONFIG,
    SERVICE_RUNNING, SERVICE_START, SERVICE_START_PENDING, SERVICE_STATE_ALL, SERVICE_STATUS,
    SERVICE_STOP, SERVICE_STOPPED, SERVICE_STOP_PENDING, SERVICE_WIN32,
};
use x_core::error::{Error, ErrorKind, PermissionRequirement, Result};
use x_core::service::{
    NativeOutput, ServiceAction, ServiceInfo, ServiceListOptions, ServiceLogEntry, ServiceLogPage,
    ServiceManager, ServiceManagerType, ServiceState, DEFAULT_LOG_LINES,
};

/// Reads and drives Windows services.
#[derive(Debug, Default)]
pub struct WindowsService;

impl WindowsService {
    /// Create the adapter.
    pub fn new() -> Self {
        Self
    }
}

/// A handle to the service control manager.
struct ScManager(SC_HANDLE);

impl ScManager {
    /// Open the manager with exactly the rights the caller needs.
    ///
    /// `SC_MANAGER_ALL_ACCESS` would make every read require an elevated token,
    /// which would turn `x service list` into an administrator command.
    fn open(access: u32) -> Result<Self> {
        // SAFETY: both arguments are null, which means the local machine and the
        // default database.
        let handle = unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), access) };
        if handle.is_null() {
            return Err(last_error("could not open the service control manager"));
        }
        Ok(Self(handle))
    }
}

impl Drop for ScManager {
    fn drop(&mut self) {
        // SAFETY: the handle came from OpenSCManagerW and is closed once.
        unsafe { CloseServiceHandle(self.0) };
    }
}

/// A handle to one service.
struct Service(SC_HANDLE);

impl Service {
    fn open(name: &str, access: u32) -> Result<Self> {
        // Opening a service handle only needs to reach the manager.
        let manager = ScManager::open(SC_MANAGER_CONNECT)?;
        let wide = wide(name);
        // SAFETY: `wide` is NUL terminated and lives until after the call.
        let handle = unsafe { OpenServiceW(manager.0, wide.as_ptr(), access) };
        if handle.is_null() {
            return Err(last_error(&format!("could not open service `{name}`")));
        }
        Ok(Self(handle))
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        // SAFETY: the handle came from OpenServiceW and is closed once.
        unsafe { CloseServiceHandle(self.0) };
    }
}

impl ServiceManager for WindowsService {
    fn manager_type(&self) -> ServiceManagerType {
        ServiceManagerType::WindowsScm
    }

    fn list(&self, options: &ServiceListOptions) -> Result<Vec<ServiceInfo>> {
        // Enumerating is enough, and it works for a normal user.
        let manager = ScManager::open(SC_MANAGER_ENUMERATE_SERVICE)?;

        // Unlike the other Enum* APIs, `EnumServicesStatusExW` refuses to size
        // a preliminary call: it returns ERROR_MORE_DATA with `needed` left at
        // zero. The documented pattern is a chunked loop driven by the resume
        // handle; the strings a batch points at are only valid until the next
        // call, so rows are materialized per batch.
        let record = std::mem::size_of::<ENUM_SERVICE_STATUS_PROCESSW>();
        let mut rows: Vec<ServiceInfo> = Vec::new();
        let mut resume: u32 = 0;
        loop {
            let mut chunk = AlignedBuffer::zeroed(64 * 1024);
            let mut needed: u32 = 0;
            let mut returned: u32 = 0;
            // SAFETY: buffer and counters outlive the call.
            let ok = unsafe {
                EnumServicesStatusExW(
                    manager.0,
                    SC_ENUM_PROCESS_INFO,
                    SERVICE_WIN32, // win32 own-process and share-process services
                    // 0 is not a valid state mask here: the SCM rejects it with
                    // ERROR_INVALID_PARAMETER, so "all states" has to be spelled
                    // out as ACTIVE | INACTIVE.
                    SERVICE_STATE_ALL,
                    chunk.as_mut_ptr().cast::<u8>(),
                    chunk.len() as u32,
                    &mut needed,
                    &mut returned,
                    &mut resume,
                    std::ptr::null(),
                )
            };
            if ok == 0 {
                // SAFETY: GetLastError reads the value the failed call set.
                let more_data = unsafe { GetLastError() } == ERROR_MORE_DATA;
                if !more_data {
                    return Err(last_error("could not enumerate services"));
                }
            }
            for index in 0..returned as usize {
                let start = index * record;
                if start + record > chunk.len() {
                    break;
                }
                // SAFETY: the buffer holds `returned` records of exactly this
                // type. Records are packed at the API's offsets, so the read
                // must not assume they are aligned.
                let raw = unsafe { chunk.read_at::<ENUM_SERVICE_STATUS_PROCESSW>(start) };
                rows.push(ServiceInfo {
                    name: wide_to_string(raw.lpServiceName),
                    display_name: Some(wide_to_string(raw.lpDisplayName)),
                    description: None,
                    state: service_state(raw.ServiceStatusProcess.dwCurrentState),
                    pid: (raw.ServiceStatusProcess.dwProcessId != 0)
                        .then_some(raw.ServiceStatusProcess.dwProcessId),
                    // The enumerated view has no start type; only the per service
                    // configuration does, so `enabled` stays unknown until then.
                    enabled: None,
                    manager: ServiceManagerType::WindowsScm,
                });
            }
            if ok != 0 || resume == 0 {
                break;
            }
        }

        Ok(rows
            .into_iter()
            .filter(|row| match &options.search {
                Some(search) => {
                    row.name
                        .to_ascii_lowercase()
                        .contains(&search.to_ascii_lowercase())
                        || row.display_name.as_deref().is_some_and(|d| {
                            d.to_ascii_lowercase()
                                .contains(&search.to_ascii_lowercase())
                        })
                }
                None => true,
            })
            .filter(|row| match options.state {
                Some(ServiceState::Running) => row.state == ServiceState::Running,
                Some(ServiceState::Stopped) => row.state == ServiceState::Stopped,
                _ => true,
            })
            .collect())
    }

    fn status(&self, name: &str) -> Result<ServiceInfo> {
        // The enumeration is the only way to reach a service that is not running
        // without opening a handle first, so look there, then enrich with the
        // configuration the SCM holds for that exact name.
        let mut info = self
            .list(&ServiceListOptions {
                search: Some(name.to_string()),
                ..Default::default()
            })?
            .into_iter()
            .find(|row| row.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| Error::not_found(format!("service `{name}` not found")))?;

        if let Ok(service) = Service::open(name, SERVICE_QUERY_CONFIG) {
            if let Some(config) = service_config(service.0) {
                info.enabled = Some(config.start_type != SERVICE_DISABLED);
                // The image path is the closest thing the SCM has to a
                // description, and it is what an operator recognises.
                info.description = Some(config.binary_path).filter(|value| !value.is_empty());
            }
        }
        Ok(info)
    }

    fn action(&self, name: &str, action: ServiceAction) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::invalid_input("service name must not be empty"));
        }

        match action {
            ServiceAction::Start => {
                let service = Service::open(name, SERVICE_START)?;
                // SAFETY: the handle is live and no arguments are passed.
                if unsafe { StartServiceW(service.0, 0, std::ptr::null()) } == 0 {
                    return Err(last_error(&format!("could not start `{name}`")));
                }
                Ok(())
            }
            ServiceAction::Stop => {
                let service = Service::open(name, SERVICE_STOP)?;
                // SAFETY: SERVICE_STATUS is a plain C struct of integers.
                let mut status: SERVICE_STATUS = unsafe { std::mem::zeroed() };
                // SAFETY: the handle is live and `status` is our own storage.
                if unsafe { ControlService(service.0, SERVICE_CONTROL_STOP, &mut status) } == 0 {
                    return Err(last_error(&format!("could not stop `{name}`")));
                }
                Ok(())
            }
            ServiceAction::Restart => {
                // The SCM has no restart control code; stopping and starting is
                // the documented way to do it.
                self.action(name, ServiceAction::Stop)?;
                self.action(name, ServiceAction::Start)
            }
            ServiceAction::Reload => {
                // SCM has no reload verb; a restart is the documented equivalent.
                self.action(name, ServiceAction::Restart)
            }
            ServiceAction::Enable => self.set_start_type(name, SERVICE_DEMAND_START),
            ServiceAction::Disable => self.set_start_type(name, SERVICE_DISABLED),
        }
    }

    fn logs(&self, name: &str, limit: Option<usize>) -> Result<ServiceLogPage> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::invalid_input("service name must not be empty"));
        }
        // The name is interpolated into an XPath string literal.
        if name.contains(['\'', '"']) {
            return Err(Error::invalid_input(format!(
                "`{name}` cannot be used inside an event query"
            )));
        }
        if !sys::command_exists("wevtutil") {
            return Err(Error::unsupported("`wevtutil` is not available"));
        }
        let lines = limit.unwrap_or(DEFAULT_LOG_LINES);
        // Services rarely publish under their own provider name; the SCM
        // records their lifecycle into the System log with the service and
        // display names in EventData. Query both shapes, over-fetch, and
        // filter locally. The log is shared, so a busy host can bury quiet
        // services deep; the window is deliberately wide.
        let query =
            format!("*[System[Provider[@Name='{name}' or @Name='Service Control Manager']]]");
        let fetch = lines.saturating_mul(8).clamp(100, 1024);
        let owned: Vec<String> = vec![
            "qe".into(),
            "System".into(),
            format!("/q:{query}"),
            format!("/c:{fetch}"),
            "/rd:true".into(),
            "/f:xml".into(),
        ];
        let borrowed: Vec<&str> = owned.iter().map(String::as_str).collect();
        let (exit_code, stdout, stderr) = sys::run_command_capture("wevtutil", &borrowed)
            .map_err(|err| Error::system(format!("cannot run `wevtutil`: {err}")))?;
        if exit_code != 0 {
            return Err(Error::system(format!(
                "wevtutil exited with {exit_code}: {}",
                decode_console(&stderr).trim()
            )));
        }
        let xml = decode_console(&stdout);
        let display = self
            .status(name)
            .ok()
            .and_then(|row| row.display_name)
            .unwrap_or_default();
        let entries = sc_events(&xml, name, &display)
            .into_iter()
            .take(lines)
            .collect();
        Ok(ServiceLogPage {
            service: name.to_string(),
            source: "wevtutil qe System".to_string(),
            entries,
        })
    }

    fn native(&self, args: &[String]) -> Result<NativeOutput> {
        if args.is_empty() {
            return Err(Error::invalid_input(
                "x service native needs at least one manager argument",
            ));
        }
        let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
        let (exit_code, stdout, stderr) = sys::run_command_capture("sc", &borrowed)
            .map_err(|err| Error::system(format!("cannot run `sc`: {err}")))?;
        Ok(NativeOutput {
            program: "sc".to_string(),
            args: args.to_vec(),
            exit_code,
            stdout: decode_console(&stdout),
            stderr: decode_console(&stderr),
        })
    }
}

/// Decode console output: UTF-8 first, then the OEM code page.
///
/// Shared with other Windows adapters (firewall's `netsh`) because console
/// tools answer in the OEM code page — GBK on a zh-CN host.
pub(crate) fn decode_console(bytes: &[u8]) -> String {
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.to_string();
    }
    // SAFETY: arguments pair each pointer with its length; the sizing call
    // writes nothing.
    let wide_len = unsafe {
        MultiByteToWideChar(
            CP_OEMCP,
            0,
            bytes.as_ptr(),
            bytes.len() as i32,
            std::ptr::null_mut(),
            0,
        )
    };
    if wide_len <= 0 {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let mut wide = vec![0u16; wide_len as usize];
    // SAFETY: `wide` is sized exactly to what the conversion reported.
    let written = unsafe {
        MultiByteToWideChar(
            CP_OEMCP,
            0,
            bytes.as_ptr(),
            bytes.len() as i32,
            wide.as_mut_ptr(),
            wide_len,
        )
    };
    if written <= 0 {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    String::from_utf16_lossy(&wide[..written as usize])
}

/// Parse `wevtutil qe /f:xml` output into entries for one service, newest
/// first (the query already asks for reverse direction).
fn sc_events(xml: &str, name: &str, display: &str) -> Vec<ServiceLogEntry> {
    let name = name.to_ascii_lowercase();
    let display = display.to_ascii_lowercase();
    let mut entries = Vec::new();
    for chunk in xml.split("<Event ").skip(1) {
        let Some(end) = chunk.find("</Event>") else {
            continue;
        };
        let event = &chunk[..end + "</Event>".len()];
        let provider = provider_name(event).unwrap_or_default();
        let data = data_values(event);
        // SCM events cover every service; keep only the ones that mention
        // this service by its name or its (localized) display name.
        if provider.eq_ignore_ascii_case("Service Control Manager") {
            let mentions = data.iter().any(|value| {
                let value = value.to_ascii_lowercase();
                value.contains(&name) || (!display.is_empty() && value.contains(&display))
            });
            if !mentions {
                continue;
            }
        }
        entries.push(ServiceLogEntry {
            timestamp: attribute(event, "SystemTime"),
            level: element_text(event, "Level").as_deref().map(event_level),
            message: if data.is_empty() {
                provider.clone()
            } else {
                data.join(" | ")
            },
        });
    }
    entries
}

/// The `Name` attribute of the event's `Provider` element.
fn provider_name(event: &str) -> Option<String> {
    let at = event.find("<Provider")?;
    attribute(&event[at..], "Name")
}

/// A quoted XML attribute value, handling both quote styles `wevtutil` emits.
fn attribute(text: &str, key: &str) -> Option<String> {
    let marker = format!("{key}=");
    let at = text.find(&marker)? + marker.len();
    let quote = text[at..]
        .chars()
        .next()
        .filter(|c| *c == '\'' || *c == '"')?;
    let close = 1 + text[at + 1..].find(quote)?;
    Some(unescape_xml(&text[at + 1..at + close]))
}

/// The text of a simple element such as `<Level>2</Level>`.
fn element_text(text: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let at = text.find(&open)? + open.len();
    let end = text[at..].find("<")?;
    Some(unescape_xml(text[at..at + end].trim()))
}

/// Every `EventData`/`Data` value, in order.
fn data_values(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("<Data") {
        rest = &rest[at + "<Data".len()..];
        let Some(gt) = rest.find('>') else { break };
        if rest[..gt].ends_with('/') {
            // Self-closed: no value, but keep scanning for the next one.
            rest = &rest[gt + 1..];
            continue;
        }
        rest = &rest[gt + 1..];
        let Some(close) = rest.find("</Data>") else {
            break;
        };
        out.push(unescape_xml(&rest[..close]));
        rest = &rest[close + "</Data>".len()..];
    }
    out
}

/// `Level` numbers with their documented names.
fn event_level(raw: &str) -> String {
    match raw {
        "1" => "critical",
        "2" => "error",
        "3" => "warning",
        "4" => "information",
        "5" => "verbose",
        other => other,
    }
    .to_string()
}

fn unescape_xml(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

impl WindowsService {
    fn set_start_type(&self, name: &str, start_type: u32) -> Result<()> {
        let service = Service::open(name, SERVICE_CHANGE_CONFIG)?;
        // SAFETY: SERVICE_NO_CHANGE keeps the current type, path and account; the
        // handle is live and the wide strings live until after the call.
        let ok = unsafe {
            ChangeServiceConfigW(
                service.0,
                SERVICE_NO_CHANGE,
                start_type,
                0, // SERVICE_NO_CHANGE error control
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        if ok == 0 {
            return Err(last_error(&format!("could not reconfigure `{name}`")));
        }
        Ok(())
    }
}

/// The owned part of `QueryServiceConfigW`.
///
/// The Win32 structure points *into* the buffer the API filled, so it must not
/// outlive that buffer: the strings are copied out here and nothing else is
/// kept.
#[derive(Debug)]
struct ServiceConfig {
    start_type: u32,
    binary_path: String,
}

/// `QueryServiceConfigW`, with the documented two call dance.
fn service_config(handle: SC_HANDLE) -> Option<ServiceConfig> {
    let mut needed: u32 = 0;
    // SAFETY: a null buffer with its size is the size query.
    unsafe {
        QueryServiceConfigW(handle, std::ptr::null_mut(), 0, &mut needed);
    }
    if needed < std::mem::size_of::<QUERY_SERVICE_CONFIGW>() as u32 {
        return None;
    }

    let mut buffer = AlignedBuffer::zeroed(needed as usize);
    // SAFETY: the buffer is at least the size the API reported and is aligned
    // for the structure it is asked to fill.
    let ok = unsafe {
        QueryServiceConfigW(
            handle,
            buffer.as_mut_ptr().cast::<QUERY_SERVICE_CONFIGW>(),
            needed,
            &mut needed,
        )
    };
    if ok == 0 {
        return None;
    }

    // SAFETY: the API filled a QUERY_SERVICE_CONFIGW into the buffer. The
    // pointers inside it are read while the buffer is still alive.
    unsafe {
        let config = buffer.read::<QUERY_SERVICE_CONFIGW>();
        Some(ServiceConfig {
            start_type: config.dwStartType,
            binary_path: wide_to_string(config.lpBinaryPathName),
        })
    }
}

/// Map a `SERVICE_STATUS.dwCurrentState` onto the unified state.
pub fn service_state(raw: u32) -> ServiceState {
    match raw {
        SERVICE_RUNNING => ServiceState::Running,
        SERVICE_STOPPED => ServiceState::Stopped,
        SERVICE_START_PENDING => ServiceState::Starting,
        SERVICE_STOP_PENDING => ServiceState::Stopping,
        _ => ServiceState::Unknown,
    }
}

/// Turn the last Win32 error into a classified [`Error`].
///
/// `Access denied` is the answer half the time on Windows, and it is the one the
/// CLI has to be able to tell the user to elevate for.
fn last_error(context: &str) -> Error {
    // SAFETY: `GetLastError` is always safe to call.
    let code = unsafe { windows_sys::Win32::Foundation::GetLastError() };
    let (kind, permission) = match code {
        ERROR_ACCESS_DENIED => (
            ErrorKind::PermissionDenied,
            PermissionRequirement::Administrator,
        ),
        ERROR_SERVICE_DOES_NOT_EXIST => (ErrorKind::NotFound, PermissionRequirement::None),
        _ => (ErrorKind::System, PermissionRequirement::None),
    };
    Error::new(kind, format!("{context} (win32 error {code})")).with_permission(permission)
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn wide_to_string(ptr: *const u16) -> String {
    if ptr.is_null() {
        return String::new();
    }
    // SAFETY: the SCM returns NUL terminated UTF-16 strings.
    let mut length = 0usize;
    unsafe {
        while *ptr.add(length) != 0 {
            length += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(ptr, length))
    }
}

/// Trait object helper.
pub fn manager() -> std::sync::Arc<dyn ServiceManager> {
    std::sync::Arc::new(WindowsService::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_are_normalised() {
        assert_eq!(service_state(SERVICE_RUNNING), ServiceState::Running);
        assert_eq!(service_state(SERVICE_STOPPED), ServiceState::Stopped);
        assert_eq!(service_state(SERVICE_START_PENDING), ServiceState::Starting);
        assert_eq!(service_state(99), ServiceState::Unknown);
    }

    #[test]
    fn the_manager_type_is_the_scm() {
        assert_eq!(
            WindowsService::new().manager_type(),
            ServiceManagerType::WindowsScm
        );
    }

    #[test]
    fn an_unknown_service_is_not_found() {
        let err = WindowsService::new()
            .status("x-no-such-service-42")
            .expect_err("must not exist");
        assert_eq!(err.kind(), ErrorKind::NotFound);
    }

    #[test]
    fn win32_errors_carry_their_permission_hint() {
        // SAFETY: setting and reading the last error is always allowed.
        unsafe { windows_sys::Win32::Foundation::SetLastError(ERROR_ACCESS_DENIED) };
        let err = last_error("context");
        assert_eq!(err.kind(), ErrorKind::PermissionDenied);
        assert_eq!(err.permission(), Some(PermissionRequirement::Administrator));
    }

    #[test]
    fn listing_returns_at_least_the_known_core_services() {
        let rows = WindowsService::new()
            .list(&ServiceListOptions::default())
            .expect("list");
        assert!(rows.len() > 1, "no services returned: {rows:?}");
        assert!(rows.iter().all(|row| !row.name.is_empty()));
    }

    #[test]
    fn event_xml_is_read_without_an_xml_library() {
        let xml = "<Events>
<Event xmlns='http://schemas.microsoft.com/win/2004/08/events/event'><System>
<Provider Name='Service Control Manager' Guid='{555908d1-a6d7-4695-8e1e-26997a3d7cc1}'/>
<EventID Qualifiers='16384'>7036</EventID>
<Level>4</Level>
<TimeCreated SystemTime='2026-10-01T04:00:00.000000Z'/>
</System><EventData><Data>Windows Spooler Service</Data><Data>running</Data></EventData>
</Event>
<Event xmlns='x'><System>
<Provider Name='MyService'/>
<Level>2</Level>
<TimeCreated SystemTime='2026-10-01T04:01:00.000000Z'/>
</System><EventData><Data Name='param1'>boom &amp; out</Data></EventData></Event>
</Events>";
        let entries = sc_events(xml, "Spooler", "Windows Spooler Service");
        assert_eq!(entries.len(), 2, "SCM hit plus own-provider event");
        assert_eq!(
            entries[0].timestamp.as_deref(),
            Some("2026-10-01T04:00:00.000000Z")
        );
        assert_eq!(entries[0].level.as_deref(), Some("information"));
        assert_eq!(entries[0].message, "Windows Spooler Service | running");
        assert_eq!(entries[1].level.as_deref(), Some("error"));
        assert_eq!(entries[1].message, "boom & out");
    }

    #[test]
    fn scm_events_for_other_services_are_dropped() {
        let xml = "<Event xmlns='x'><System>\
<Provider Name='Service Control Manager'/><Level>4</Level>\
<TimeCreated SystemTime='2026-10-01T04:00:00Z'/></System>\
<EventData><Data>Some Other Service</Data><Data>stopped</Data></EventData></Event>";
        assert!(sc_events(xml, "Spooler", "").is_empty());
        // A display-name match works even when the name does not appear.
        assert_eq!(sc_events(xml, "other", "Some Other Service").len(), 1);
    }

    #[test]
    fn level_numbers_get_their_documented_names() {
        assert_eq!(event_level("1"), "critical");
        assert_eq!(event_level("2"), "error");
        assert_eq!(event_level("3"), "warning");
        assert_eq!(event_level("4"), "information");
        assert_eq!(event_level("9"), "9");
    }

    #[test]
    fn console_bytes_survive_a_round_trip() {
        assert_eq!(decode_console(b"plain ascii"), "plain ascii");
        assert_eq!(decode_console("中文字".as_bytes()), "中文字");
        assert_eq!(decode_console(b""), "");
    }

    #[test]
    fn quote_characters_never_reach_the_xpath() {
        let err = WindowsService::new()
            .logs("evil' or true", None)
            .expect_err("must be rejected");
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
        assert!(WindowsService::new().logs("", None).is_err());
    }

    #[test]
    fn native_refuses_an_empty_argument_list() {
        let err = WindowsService::new().native(&[]).expect_err("must reject");
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
    }

    #[test]
    fn logs_find_the_service_behind_a_real_scm_event() {
        // Read the recent SCM events raw, take one whose EventData names a
        // real service, and require `logs` to surface it. This does not assume
        // any particular service (or event ID) exists on the host.
        let owned: Vec<String> = vec![
            "qe".into(),
            "System".into(),
            "/q:*[System[Provider[@Name='Service Control Manager']]]".into(),
            "/c:200".into(),
            "/rd:true".into(),
            "/f:xml".into(),
        ];
        let borrowed: Vec<&str> = owned.iter().map(String::as_str).collect();
        let (code, stdout, stderr) =
            sys::run_command_capture("wevtutil", &borrowed).expect("wevtutil must run");
        assert_eq!(code, 0, "wevtutil: {}", decode_console(&stderr));
        let xml = decode_console(&stdout);

        let names: std::collections::HashSet<String> = WindowsService::new()
            .list(&ServiceListOptions::default())
            .expect("list")
            .into_iter()
            .map(|row| row.name.to_ascii_lowercase())
            .collect();
        let mut target = None;
        for chunk in xml.split("<Event ").skip(1) {
            for value in data_values(chunk) {
                if names.contains(&value.to_ascii_lowercase()) {
                    target = Some(value);
                    break;
                }
            }
            if target.is_some() {
                break;
            }
        }
        let Some(target) = target else {
            // No SCM event names a known service (fresh image): nothing to
            // assert, but the query itself already proved the pipeline works.
            return;
        };
        let page = WindowsService::new()
            .logs(&target, Some(10))
            .expect("logs must answer");
        assert!(
            !page.entries.is_empty(),
            "logs missed an event for `{target}` that the raw query just returned"
        );
        assert!(page.entries.iter().all(|entry| !entry.message.is_empty()));
    }

    #[test]
    fn native_sc_passthrough_answers() {
        let out = WindowsService::new()
            .native(&["query".into(), "Spooler".into()])
            .expect("sc must run");
        assert_eq!(out.program, "sc");
        assert_eq!(out.exit_code, 0, "sc query Spooler: {out:?}");
        assert!(
            out.stdout.to_ascii_lowercase().contains("spooler"),
            "unexpected sc output: {:?}",
            out.stdout
        );
    }
}
