//! Proxy adapter: env layer from `x-core` defaults, system layer per OS.

use x_core::proxy::ProxyManager;

/// The platform proxy adapter.
pub struct PlatformProxy;

impl ProxyManager for PlatformProxy {
    fn system(&self) -> Option<x_core::proxy::SystemProxy> {
        super::proxy_os::system_proxy()
    }
}
