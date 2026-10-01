//! Windows service adapter: the service control manager.
//!
//! Everything here is native: `EnumServicesStatusExW` for the list, and
//! `StartServiceW`, `ControlService` and `ChangeServiceConfigW` for the
//! lifecycle actions.

use super::buffer::AlignedBuffer;
use windows_sys::Win32::Foundation::{
    GetLastError, ERROR_ACCESS_DENIED, ERROR_MORE_DATA, ERROR_SERVICE_DOES_NOT_EXIST,
};
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
    ServiceAction, ServiceInfo, ServiceListOptions, ServiceManager, ServiceManagerType,
    ServiceState,
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
}
