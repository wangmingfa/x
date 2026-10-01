//! Windows adapters: IP helper API for sockets and adapters, the service control
//! manager for services, `sysinfo` for processes, disks and CPU.
//!
//! Layer by layer, as documented at the crate root:
//!
//! 1. `sysinfo` for processes, disks and CPU.
//! 2. `GetExtendedTcpTable`/`GetExtendedUdpTable` for process/socket attribution
//!    and `GetAdaptersAddresses` for interfaces, addresses and DNS servers; the
//!    SCM for services.
//! 3. `GetIpForwardTable2` for the routing table and the `IcmpSendEcho` family
//!    for ping/trace, so no locale-sensitive command output is parsed.

pub(crate) mod buffer;
pub mod disk;
pub mod display;
pub(crate) mod netprobe;
pub mod network;
pub mod port;
pub mod process;
pub mod service;
pub mod system;

use std::ffi::c_void;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Security::{
    GetTokenInformation, LookupAccountSidW, TokenUser, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};

/// An owned handle that closes itself.
pub struct OwnedHandle(HANDLE);

impl OwnedHandle {
    /// Take ownership of a handle, or `None` when the call failed.
    pub fn new(handle: HANDLE) -> Option<Self> {
        (!handle.is_null() && handle != INVALID_HANDLE_VALUE).then_some(Self(handle))
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: the handle came from a Win32 call that transfers ownership.
        unsafe { CloseHandle(self.0) };
    }
}

/// Windows account name (`DOMAIN\user`) that owns a process.
///
/// `sysinfo` reports no uid on Windows, so the owner is resolved from the
/// process token: open the token, read its user SID, then translate the SID.
pub fn account_name(pid: u32) -> Option<String> {
    // SAFETY: every call below checks its return value and closes its handles.
    unsafe {
        let process = OwnedHandle::new(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid))?;

        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(process.0, TOKEN_QUERY, &mut token) == 0 {
            return None;
        }
        let token = OwnedHandle::new(token)?;

        // The first call sizes the buffer, the second one fills it.
        let mut needed: u32 = 0;
        GetTokenInformation(token.0, TokenUser, std::ptr::null_mut(), 0, &mut needed);
        if needed == 0 {
            return None;
        }
        let mut buffer = buffer::AlignedBuffer::zeroed(needed as usize);
        if GetTokenInformation(token.0, TokenUser, buffer.as_mut_ptr(), needed, &mut needed) == 0 {
            return None;
        }

        // TOKEN_USER starts with a SID_AND_ATTRIBUTES, whose first field is the
        // SID pointer. The struct is copied out while the buffer is still alive.
        // SAFETY: the API filled a TOKEN_USER into this buffer.
        let user = buffer.read::<TOKEN_USER>();
        lookup_sid(user.User.Sid)
    }
}

/// Translate a SID into `DOMAIN\user`.
fn lookup_sid(sid: *mut c_void) -> Option<String> {
    // SAFETY: `sid` is a live SID owned by the token buffer of the caller.
    unsafe {
        let mut name_len: u32 = 0;
        let mut domain_len: u32 = 0;
        // The fill path writes through `pe_use` on success, so it must be a
        // real location, not null.
        let mut use_type: windows_sys::Win32::Security::SID_NAME_USE = 0;
        // Sizing call: both buffers are null, the API reports the lengths.
        LookupAccountSidW(
            std::ptr::null(),
            sid,
            std::ptr::null_mut(),
            &mut name_len,
            std::ptr::null_mut(),
            &mut domain_len,
            &mut use_type,
        );
        if name_len == 0 {
            return None;
        }

        let mut name = vec![0u16; name_len as usize];
        let mut domain = vec![0u16; (domain_len as usize).max(1)];
        // Fill call: both buffers are real and sized. Passing a null domain with
        // a non-zero length makes the API reject the call outright.
        if LookupAccountSidW(
            std::ptr::null(),
            sid,
            name.as_mut_ptr(),
            &mut name_len,
            domain.as_mut_ptr(),
            &mut domain_len,
            &mut use_type,
        ) == 0
        {
            return None;
        }

        let account = String::from_utf16_lossy(&name[..name_len as usize]);
        if domain_len == 0 {
            return Some(account);
        }
        Some(format!(
            "{}\\{}",
            String::from_utf16_lossy(&domain[..domain_len as usize]),
            account
        ))
    }
}
