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
    /// File open/reveal/trash, when the platform adapter is compiled in.
    pub file: Option<Arc<dyn crate::file::FileManager>>,
    /// Clipboard text, when the platform adapter is compiled in.
    pub clipboard: Option<Arc<dyn crate::clipboard::ClipboardManager>>,
    /// Users and groups, when the platform adapter is compiled in.
    pub user: Option<Arc<dyn crate::user::UserManager>>,
    /// Login shells, when the platform adapter is compiled in.
    pub shell: Option<Arc<dyn crate::shell::ShellManager>>,
    /// Proxy inspection, when the platform adapter is compiled in.
    pub proxy: Option<Arc<dyn crate::proxy::ProxyManager>>,
    /// Power verbs and battery facts, when the platform adapter is compiled in.
    pub power: Option<Arc<dyn crate::power::PowerManager>>,
    /// Mounted filesystems and mount/unmount verbs.
    pub mount: Option<Arc<dyn crate::mount::MountManager>>,
    /// Login startup items.
    pub startup: Option<Arc<dyn crate::startup::StartupManager>>,
    /// Scheduled tasks / jobs.
    pub schedule: Option<Arc<dyn crate::schedule::ScheduleManager>>,
    /// Firewall inspection and rule changes.
    pub firewall: Option<Arc<dyn crate::firewall::FirewallManager>>,
    /// System log reads.
    pub logs: Option<Arc<dyn crate::logs::LogReader>>,
    /// Hardware device inventory.
    pub device: Option<Arc<dyn crate::device::DeviceManager>>,
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
    file: Option<Arc<dyn crate::file::FileManager>>,
    clipboard: Option<Arc<dyn crate::clipboard::ClipboardManager>>,
    user: Option<Arc<dyn crate::user::UserManager>>,
    shell: Option<Arc<dyn crate::shell::ShellManager>>,
    proxy: Option<Arc<dyn crate::proxy::ProxyManager>>,
    power: Option<Arc<dyn crate::power::PowerManager>>,
    mount: Option<Arc<dyn crate::mount::MountManager>>,
    startup: Option<Arc<dyn crate::startup::StartupManager>>,
    schedule: Option<Arc<dyn crate::schedule::ScheduleManager>>,
    firewall: Option<Arc<dyn crate::firewall::FirewallManager>>,
    logs: Option<Arc<dyn crate::logs::LogReader>>,
    device: Option<Arc<dyn crate::device::DeviceManager>>,
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

    /// Provide the file capability (optional).
    pub fn file(mut self, mgr: Arc<dyn crate::file::FileManager>) -> Self {
        self.file = Some(mgr);
        self
    }

    /// Provide the clipboard capability (optional).
    pub fn clipboard(mut self, mgr: Arc<dyn crate::clipboard::ClipboardManager>) -> Self {
        self.clipboard = Some(mgr);
        self
    }

    /// Provide the user capability (optional).
    pub fn user(mut self, mgr: Arc<dyn crate::user::UserManager>) -> Self {
        self.user = Some(mgr);
        self
    }

    /// Provide the shell capability (optional).
    pub fn shell(mut self, mgr: Arc<dyn crate::shell::ShellManager>) -> Self {
        self.shell = Some(mgr);
        self
    }

    /// Provide the proxy capability (optional).
    pub fn proxy(mut self, mgr: Arc<dyn crate::proxy::ProxyManager>) -> Self {
        self.proxy = Some(mgr);
        self
    }

    /// Provide the power capability (optional).
    pub fn power(mut self, mgr: Arc<dyn crate::power::PowerManager>) -> Self {
        self.power = Some(mgr);
        self
    }

    /// Provide the mount capability (optional).
    pub fn mount(mut self, mgr: Arc<dyn crate::mount::MountManager>) -> Self {
        self.mount = Some(mgr);
        self
    }

    /// Provide the startup capability (optional).
    pub fn startup(mut self, mgr: Arc<dyn crate::startup::StartupManager>) -> Self {
        self.startup = Some(mgr);
        self
    }

    /// Provide the schedule capability (optional).
    pub fn schedule(mut self, mgr: Arc<dyn crate::schedule::ScheduleManager>) -> Self {
        self.schedule = Some(mgr);
        self
    }

    /// Provide the firewall capability (optional).
    pub fn firewall(mut self, mgr: Arc<dyn crate::firewall::FirewallManager>) -> Self {
        self.firewall = Some(mgr);
        self
    }

    /// Provide the log reader capability (optional).
    pub fn logs(mut self, mgr: Arc<dyn crate::logs::LogReader>) -> Self {
        self.logs = Some(mgr);
        self
    }

    /// Provide the device capability (optional).
    pub fn device(mut self, mgr: Arc<dyn crate::device::DeviceManager>) -> Self {
        self.device = Some(mgr);
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
            file: self.file,
            clipboard: self.clipboard,
            user: self.user,
            shell: self.shell,
            proxy: self.proxy,
            power: self.power,
            mount: self.mount,
            startup: self.startup,
            schedule: self.schedule,
            firewall: self.firewall,
            logs: self.logs,
            device: self.device,
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
