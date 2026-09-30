//! The object every frontend receives.
//!
//! `SystemContext` is the only thing the CLI and the TUI are handed. Neither of
//! them knows which OS it runs on, because the context is built once by the
//! platform layer and contains only `x-core` traits.

use crate::disk::DiskManager;
use crate::error::{Error, Result};
use crate::network::NetworkManager;
use crate::port::PortManager;
use crate::process::ProcessManager;
use crate::service::ServiceManager;
use crate::system::SystemManager;
use std::sync::Arc;

/// Capabilities shared by every frontend.
#[derive(Clone)]
pub struct SystemContext {
    /// System facts and live CPU/memory counters.
    pub system: Arc<dyn SystemManager>,
    /// Process listing, lookup and termination.
    pub process: Arc<dyn ProcessManager>,
    /// Socket enumeration, attribution and port reclaiming.
    pub port: Arc<dyn PortManager>,
    /// Interfaces, addresses, routes and DNS.
    pub network: Arc<dyn NetworkManager>,
    /// Services, across SCM / systemd / OpenRC / launchd.
    pub service: Arc<dyn ServiceManager>,
    /// Mounted filesystems.
    pub disk: Arc<dyn DiskManager>,
}

impl std::fmt::Debug for SystemContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SystemContext")
            .field("os", &self.os())
            .finish_non_exhaustive()
    }
}

impl SystemContext {
    /// Start building a context.
    pub fn builder() -> SystemContextBuilder {
        SystemContextBuilder::default()
    }

    /// Name of the current user, when the platform exposes it.
    pub fn current_user(&self) -> Option<String> {
        self.system.current_user()
    }

    /// OS family the context was built for.
    pub fn os(&self) -> crate::system::OsFamily {
        self.system
            .info()
            .map(|i| i.os)
            .unwrap_or(crate::system::OsFamily::Other)
    }
}

/// Builder for [`SystemContext`]. Only the platform layer calls [`Self::build`].
#[derive(Default)]
pub struct SystemContextBuilder {
    system: Option<Arc<dyn SystemManager>>,
    process: Option<Arc<dyn ProcessManager>>,
    port: Option<Arc<dyn PortManager>>,
    network: Option<Arc<dyn NetworkManager>>,
    service: Option<Arc<dyn ServiceManager>>,
    disk: Option<Arc<dyn DiskManager>>,
}

impl SystemContextBuilder {
    /// Provide the system capability.
    pub fn system(mut self, mgr: Arc<dyn SystemManager>) -> Self {
        self.system = Some(mgr);
        self
    }

    /// Provide the process capability.
    pub fn process(mut self, mgr: Arc<dyn ProcessManager>) -> Self {
        self.process = Some(mgr);
        self
    }

    /// Provide the port capability.
    pub fn port(mut self, mgr: Arc<dyn PortManager>) -> Self {
        self.port = Some(mgr);
        self
    }

    /// Provide the network capability.
    pub fn network(mut self, mgr: Arc<dyn NetworkManager>) -> Self {
        self.network = Some(mgr);
        self
    }

    /// Provide the service capability.
    pub fn service(mut self, mgr: Arc<dyn ServiceManager>) -> Self {
        self.service = Some(mgr);
        self
    }

    /// Provide the disk capability.
    pub fn disk(mut self, mgr: Arc<dyn DiskManager>) -> Self {
        self.disk = Some(mgr);
        self
    }

    /// Finish, failing if a mandatory capability is missing.
    pub fn build(self) -> Result<SystemContext> {
        Ok(SystemContext {
            system: self
                .system
                .ok_or_else(|| Error::unsupported("system capability not registered"))?,
            process: self
                .process
                .ok_or_else(|| Error::unsupported("process capability not registered"))?,
            port: self
                .port
                .ok_or_else(|| Error::unsupported("port capability not registered"))?,
            network: self
                .network
                .ok_or_else(|| Error::unsupported("network capability not registered"))?,
            service: self
                .service
                .ok_or_else(|| Error::unsupported("service capability not registered"))?,
            disk: self
                .disk
                .ok_or_else(|| Error::unsupported("disk capability not registered"))?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Result;

    struct Dummy;
    impl SystemManager for Dummy {
        fn info(&self) -> Result<crate::system::SystemInfo> {
            unimplemented!()
        }
        fn cpu_usage(&self) -> Result<crate::system::CpuUsage> {
            unimplemented!()
        }
    }

    #[test]
    fn builder_reports_missing_capability() {
        let err = match SystemContext::builder().build() {
            Err(err) => err,
            Ok(_) => panic!("builder must reject an incomplete context"),
        };
        assert_eq!(err.message(), "system capability not registered");
    }

    #[test]
    fn context_is_cloneable() {
        let ctx = SystemContext::builder()
            .system(Arc::new(Dummy))
            .process(Arc::new(crate::testing::NoopProcess))
            .port(Arc::new(crate::testing::NoopPort))
            .network(Arc::new(crate::testing::NoopNetwork))
            .service(Arc::new(crate::testing::NoopService))
            .disk(Arc::new(crate::testing::NoopDisk))
            .build()
            .unwrap();
        let cloned = ctx.clone();
        assert!(Arc::ptr_eq(&ctx.process, &cloned.process));
    }
}
