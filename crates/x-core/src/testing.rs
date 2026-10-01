//! Stub capabilities so frontends and tests can run without a platform.
//!
//! These implement every `x-core` trait with predictable, empty data. They are
//! used by x-cli's unit tests and available to downstream crates that need to
//! render a frontend without touching the host.

use crate::disk::{DiskInfo, DiskManager};
use crate::error::Result;
use crate::network::{AddressInfo, DnsConfig, InterfaceInfo, NetworkManager, RouteInfo};
use crate::port::{ConnectionState, PortInfo, PortListOptions, PortManager, Protocol};
use crate::process::{KillSignal, ProcessInfo, ProcessListOptions, ProcessManager, ProcessState};
use crate::service::{
    NativeOutput, ServiceAction, ServiceInfo, ServiceListOptions, ServiceLogPage, ServiceManager,
    ServiceManagerType,
};
use crate::system::{CpuUsage, MemoryUsage, SystemInfo, SystemManager};
use std::net::{IpAddr, Ipv4Addr};

/// Empty system capability.
pub struct NoopSystem;

impl SystemManager for NoopSystem {
    fn info(&self) -> Result<SystemInfo> {
        Ok(SystemInfo {
            os: crate::system::OsFamily::Other,
            os_name: "unknown".into(),
            os_version: "0".into(),
            arch: std::env::consts::ARCH.into(),
            hostname: "localhost".into(),
            ..SystemInfo::default()
        })
    }

    fn cpu_usage(&self) -> Result<CpuUsage> {
        Ok(CpuUsage::default())
    }

    fn memory_usage(&self) -> Result<MemoryUsage> {
        Ok(MemoryUsage {
            total_bytes: 0,
            used_bytes: 0,
            available_bytes: 0,
            percent: 0.0,
            swap_total_bytes: 0,
            swap_used_bytes: 0,
            pressure: None,
        })
    }
}

/// Empty process capability.
pub struct NoopProcess;

impl ProcessManager for NoopProcess {
    fn list(&self, _options: &ProcessListOptions) -> Result<Vec<ProcessInfo>> {
        Ok(Vec::new())
    }

    fn kill(&self, _pid: u32, _signal: KillSignal) -> Result<()> {
        Ok(())
    }
}

/// Empty port capability.
pub struct NoopPort;

impl PortManager for NoopPort {
    fn list(&self, _options: &PortListOptions) -> Result<Vec<PortInfo>> {
        Ok(Vec::new())
    }
}

/// Empty network capability.
pub struct NoopNetwork;

impl NetworkManager for NoopNetwork {
    fn interfaces(&self) -> Result<Vec<InterfaceInfo>> {
        Ok(Vec::new())
    }

    fn addresses(&self) -> Result<Vec<AddressInfo>> {
        Ok(Vec::new())
    }

    fn routes(&self) -> Result<Vec<RouteInfo>> {
        Ok(Vec::new())
    }

    fn dns(&self) -> Result<DnsConfig> {
        Ok(DnsConfig::default())
    }
}

/// Service capability that reports `Unknown`.
pub struct NoopService;

impl ServiceManager for NoopService {
    fn manager_type(&self) -> ServiceManagerType {
        ServiceManagerType::Unknown
    }

    fn list(&self, _options: &ServiceListOptions) -> Result<Vec<ServiceInfo>> {
        Ok(Vec::new())
    }

    fn action(&self, name: &str, _action: ServiceAction) -> Result<()> {
        Err(crate::error::Error::unsupported(format!(
            "no service manager available, cannot act on `{name}`"
        )))
    }
}

/// Empty disk capability.
pub struct NoopDisk;

impl DiskManager for NoopDisk {
    fn list(&self) -> Result<Vec<DiskInfo>> {
        Ok(Vec::new())
    }
}

/// Build a context wired to the stubs above.
pub fn stub_context() -> crate::context::SystemContext {
    use std::sync::Arc;
    crate::context::SystemContext::builder()
        .system(Arc::new(NoopSystem))
        .process(Arc::new(NoopProcess))
        .port(Arc::new(NoopPort))
        .network(Arc::new(NoopNetwork))
        .service(Arc::new(NoopService))
        .disk(Arc::new(NoopDisk))
        .build()
        .expect("stub capabilities are always complete")
}

/// A listening TCP socket on `0.0.0.0:port` owned by `pid`.
pub fn stub_socket(port: u16, pid: u32, process: &str) -> PortInfo {
    PortInfo {
        protocol: Protocol::Tcp,
        local_address: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        local_port: port,
        remote_address: None,
        remote_port: None,
        state: ConnectionState::Listen,
        pid: Some(pid),
        process_name: Some(process.to_string()),
        user: None,
        path: None,
    }
}

/// A minimal running process.
pub fn stub_process(pid: u32, parent: Option<u32>, name: &str) -> ProcessInfo {
    ProcessInfo {
        pid,
        parent_pid: parent,
        name: name.to_string(),
        executable: None,
        command_line: None,
        user: None,
        cpu_usage: Some(0.0),
        memory_bytes: Some(0),
        virtual_memory_bytes: None,
        threads: Some(1),
        start_time: None,
        state: ProcessState::Running,
        cwd: None,
        open_files: None,
        connections: None,
        environment: None,
    }
}

// ---------------------------------------------------------------------------
// Configurable stubs
// ---------------------------------------------------------------------------
//
// The `Noop*` types above answer "nothing". Frontends also need to be tested
// *with* data: a table with rows, a kill that records what it was asked to do,
// an error that a test can trigger. The types below provide that without a
// platform, which is the whole point of keeping the traits in `x-core`.

use crate::port::{KillPlan, PortOwner};
use crate::process::ProcessTree;
use std::sync::Mutex;

/// Port capability backed by a fixed list, recording every kill.
#[derive(Debug, Default)]
pub struct StubPort {
    rows: Mutex<Vec<PortInfo>>,
    killed: Mutex<Vec<u32>>,
    failure: Mutex<Option<StubFailure>>,
}

/// What a stub capability should fail with.
///
/// Errors are not `Clone`, so a failure is stored as its parts and rebuilt on
/// every call. That also lets a test attach the privilege guidance a real
/// adapter would attach.
#[derive(Debug, Clone, Default)]
pub struct StubFailure {
    kind: crate::error::ErrorKind,
    message: String,
    permission: Option<crate::error::PermissionRequirement>,
}

impl StubFailure {
    /// Fail with `kind` and `message`.
    pub fn new(kind: crate::error::ErrorKind, message: &str) -> Self {
        Self {
            kind,
            message: message.to_string(),
            permission: None,
        }
    }

    /// A privilege failure, with the guidance the frontends print.
    pub fn denied(requirement: crate::error::PermissionRequirement, message: &str) -> Self {
        Self {
            kind: crate::error::ErrorKind::PermissionDenied,
            message: message.to_string(),
            permission: Some(requirement),
        }
    }

    /// Rebuild the error.
    pub fn error(&self) -> crate::error::Error {
        let mut error = crate::error::Error::new(self.kind, self.message.clone());
        if let Some(requirement) = &self.permission {
            error = error.with_permission(*requirement);
        }
        error
    }
}

impl StubPort {
    /// Rows this capability returns.
    pub fn new(rows: Vec<PortInfo>) -> Self {
        Self {
            rows: Mutex::new(rows),
            ..Default::default()
        }
    }

    /// Make every call fail, to exercise the error paths of a frontend.
    pub fn fail_with(&self, failure: StubFailure) {
        *self.failure.lock().expect("stub mutex") = Some(failure);
    }

    /// Pids the frontend asked to kill, in order.
    pub fn killed(&self) -> Vec<u32> {
        self.killed.lock().expect("stub mutex").clone()
    }
}

impl PortManager for StubPort {
    fn list(&self, options: &PortListOptions) -> Result<Vec<PortInfo>> {
        if let Some(failure) = self.failure.lock().expect("stub mutex").clone() {
            return Err(failure.error());
        }
        Ok(options.apply(self.rows.lock().expect("stub mutex").clone()))
    }

    fn kill_plan(&self, plan: &KillPlan, _signal: KillSignal) -> Result<usize> {
        let pids = plan.target_pids();
        self.killed.lock().expect("stub mutex").extend(pids.clone());
        Ok(pids.len())
    }

    fn owners(&self, options: &PortListOptions) -> Result<Vec<PortOwner>> {
        Ok(crate::port::manager::group_by_owner(self.list(options)?))
    }
}

/// A running service owned by `pid`.
pub fn stub_service(name: &str, pid: u32) -> crate::service::ServiceInfo {
    crate::service::ServiceInfo {
        name: name.to_string(),
        display_name: None,
        description: None,
        state: crate::service::ServiceState::Running,
        pid: Some(pid),
        enabled: Some(true),
        manager: ServiceManagerType::Unknown,
    }
}

/// Process capability backed by a fixed list, recording every kill.
#[derive(Debug, Default)]
pub struct StubProcess {
    rows: Mutex<Vec<ProcessInfo>>,
    killed: Mutex<Vec<u32>>,
}

impl StubProcess {
    /// Rows this capability returns.
    pub fn new(rows: Vec<ProcessInfo>) -> Self {
        Self {
            rows: Mutex::new(rows),
            killed: Mutex::new(Vec::new()),
        }
    }

    /// Pids the frontend asked to kill, in order.
    pub fn killed(&self) -> Vec<u32> {
        self.killed.lock().expect("stub mutex").clone()
    }
}

impl ProcessManager for StubProcess {
    fn list(&self, options: &ProcessListOptions) -> Result<Vec<ProcessInfo>> {
        let mut rows = self.rows.lock().expect("stub mutex").clone();
        if let Some(term) = options.search.as_deref() {
            let term = term.to_ascii_lowercase();
            rows.retain(|row| row.name.to_ascii_lowercase().contains(&term));
        }
        if let Some(user) = options.user.as_deref() {
            rows.retain(|row| row.user.as_deref() == Some(user));
        }
        rows.truncate(options.limit.unwrap_or(usize::MAX));
        Ok(rows)
    }

    fn get(&self, pid: u32) -> Result<ProcessInfo> {
        self.rows
            .lock()
            .expect("stub mutex")
            .iter()
            .find(|row| row.pid == pid)
            .cloned()
            .ok_or_else(|| crate::error::Error::not_found(format!("process {pid} not found")))
    }

    fn tree(&self, options: &ProcessListOptions) -> Result<ProcessTree> {
        Ok(crate::process::model::build_tree(self.list(options)?))
    }

    fn kill(&self, pid: u32, _signal: KillSignal) -> Result<()> {
        self.killed.lock().expect("stub mutex").push(pid);
        Ok(())
    }
}

/// Network capability backed by fixed data.
#[derive(Debug, Default)]
pub struct StubNetwork {
    interfaces: Vec<InterfaceInfo>,
    addresses: Vec<AddressInfo>,
    routes: Vec<RouteInfo>,
    dns: DnsConfig,
}

impl StubNetwork {
    /// Fixed data.
    pub fn new(
        interfaces: Vec<InterfaceInfo>,
        addresses: Vec<AddressInfo>,
        routes: Vec<RouteInfo>,
        dns: DnsConfig,
    ) -> Self {
        Self {
            interfaces,
            addresses,
            routes,
            dns,
        }
    }
}

impl NetworkManager for StubNetwork {
    fn interfaces(&self) -> Result<Vec<InterfaceInfo>> {
        Ok(self.interfaces.clone())
    }

    fn addresses(&self) -> Result<Vec<AddressInfo>> {
        Ok(self.addresses.clone())
    }

    fn routes(&self) -> Result<Vec<RouteInfo>> {
        Ok(self.routes.clone())
    }

    fn dns(&self) -> Result<DnsConfig> {
        Ok(self.dns.clone())
    }
}

/// Service capability backed by a fixed list, recording every action.
#[derive(Debug, Default)]
pub struct StubService {
    rows: Mutex<Vec<ServiceInfo>>,
    actions: Mutex<Vec<(String, ServiceAction)>>,
    logs: Mutex<Option<ServiceLogPage>>,
    native_calls: Mutex<Vec<Vec<String>>>,
}

impl StubService {
    /// Rows this capability returns.
    pub fn new(rows: Vec<ServiceInfo>) -> Self {
        Self {
            rows: Mutex::new(rows),
            actions: Mutex::new(Vec::new()),
            ..Default::default()
        }
    }

    /// Actions the frontend requested, in order.
    pub fn actions(&self) -> Vec<(String, ServiceAction)> {
        self.actions.lock().expect("stub mutex").clone()
    }

    /// Answer every `logs` call with this page.
    pub fn set_logs(&self, page: ServiceLogPage) {
        *self.logs.lock().expect("stub mutex") = Some(page);
    }

    /// Argument lists passed to `native`, in order.
    pub fn native_calls(&self) -> Vec<Vec<String>> {
        self.native_calls.lock().expect("stub mutex").clone()
    }
}

impl ServiceManager for StubService {
    fn manager_type(&self) -> ServiceManagerType {
        ServiceManagerType::Unknown
    }

    fn list(&self, options: &ServiceListOptions) -> Result<Vec<ServiceInfo>> {
        let mut rows = self.rows.lock().expect("stub mutex").clone();
        if let Some(term) = options.search.as_deref() {
            let term = term.to_ascii_lowercase();
            rows.retain(|row| row.name.to_ascii_lowercase().contains(&term));
        }
        if options.running_only {
            rows.retain(|row| row.state == crate::service::ServiceState::Running);
        }
        rows.truncate(options.limit.unwrap_or(usize::MAX));
        Ok(rows)
    }

    fn action(&self, name: &str, action: ServiceAction) -> Result<()> {
        self.actions
            .lock()
            .expect("stub mutex")
            .push((name.to_string(), action));
        Ok(())
    }

    fn logs(&self, _name: &str, _limit: Option<usize>) -> Result<ServiceLogPage> {
        self.logs
            .lock()
            .expect("stub mutex")
            .clone()
            .ok_or_else(|| crate::error::Error::unsupported("stub service has no logs configured"))
    }

    fn native(&self, args: &[String]) -> Result<NativeOutput> {
        self.native_calls
            .lock()
            .expect("stub mutex")
            .push(args.to_vec());
        Ok(NativeOutput {
            program: "stub".to_string(),
            args: args.to_vec(),
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        })
    }
}

/// System capability returning fixed facts.
#[derive(Debug, Default)]
pub struct StubSystem {
    info: Mutex<SystemInfo>,
    cpu: CpuUsage,
    memory: MemoryUsage,
}

impl StubSystem {
    /// Fixed facts and counters.
    pub fn new(info: SystemInfo, cpu: CpuUsage, memory: MemoryUsage) -> Self {
        Self {
            info: Mutex::new(info),
            cpu,
            memory,
        }
    }
}

impl SystemManager for StubSystem {
    fn info(&self) -> Result<SystemInfo> {
        Ok(self.info.lock().expect("stub mutex").clone())
    }

    fn cpu_usage(&self) -> Result<CpuUsage> {
        Ok(self.cpu.clone())
    }

    fn memory_usage(&self) -> Result<MemoryUsage> {
        Ok(self.memory.clone())
    }
}

/// Disk capability backed by a fixed list.
#[derive(Debug, Default)]
pub struct StubDisk {
    rows: Mutex<Vec<DiskInfo>>,
}

impl StubDisk {
    /// Rows this capability returns.
    pub fn new(rows: Vec<DiskInfo>) -> Self {
        Self {
            rows: Mutex::new(rows),
        }
    }
}

impl DiskManager for StubDisk {
    fn list(&self) -> Result<Vec<DiskInfo>> {
        Ok(self.rows.lock().expect("stub mutex").clone())
    }
}

/// A context assembled from configurable stubs.
///
/// ```no_run
/// use x_core::testing::Stubs;
/// use x_core::testing::stub_socket;
///
/// let stubs = Stubs::new().with_ports(vec![stub_socket(8080, 42, "node")]);
/// let context = stubs.context();
/// ```
#[derive(Debug, Clone)]
pub struct Stubs {
    /// Port capability, shared with the built context.
    pub port: std::sync::Arc<StubPort>,
    /// Process capability, shared with the built context.
    pub process: std::sync::Arc<StubProcess>,
    /// Network capability.
    pub network: std::sync::Arc<StubNetwork>,
    /// Service capability, shared with the built context.
    pub service: std::sync::Arc<StubService>,
    /// System capability.
    pub system: std::sync::Arc<StubSystem>,
    /// Disk capability.
    pub disk: std::sync::Arc<StubDisk>,
}

impl Default for Stubs {
    fn default() -> Self {
        Self::new()
    }
}

impl Stubs {
    /// Empty stubs, equivalent to [`stub_context`].
    pub fn new() -> Self {
        Self {
            port: std::sync::Arc::new(StubPort::default()),
            process: std::sync::Arc::new(StubProcess::default()),
            network: std::sync::Arc::new(StubNetwork::default()),
            service: std::sync::Arc::new(StubService::default()),
            system: std::sync::Arc::new(StubSystem::default()),
            disk: std::sync::Arc::new(StubDisk::default()),
        }
    }

    /// Use `rows` as the socket list.
    pub fn with_ports(self, rows: Vec<PortInfo>) -> Self {
        Self {
            port: std::sync::Arc::new(StubPort::new(rows)),
            ..self
        }
    }

    /// Use `rows` as the process list.
    pub fn with_processes(self, rows: Vec<ProcessInfo>) -> Self {
        Self {
            process: std::sync::Arc::new(StubProcess::new(rows)),
            ..self
        }
    }

    /// Use fixed network data.
    pub fn with_network(
        self,
        interfaces: Vec<InterfaceInfo>,
        addresses: Vec<AddressInfo>,
        routes: Vec<RouteInfo>,
        dns: DnsConfig,
    ) -> Self {
        Self {
            network: std::sync::Arc::new(StubNetwork::new(interfaces, addresses, routes, dns)),
            ..self
        }
    }

    /// Use `rows` as the service list.
    pub fn with_services(self, rows: Vec<ServiceInfo>) -> Self {
        Self {
            service: std::sync::Arc::new(StubService::new(rows)),
            ..self
        }
    }

    /// Use fixed system facts and counters.
    pub fn with_system(self, info: SystemInfo, cpu: CpuUsage, memory: MemoryUsage) -> Self {
        Self {
            system: std::sync::Arc::new(StubSystem::new(info, cpu, memory)),
            ..self
        }
    }

    /// Use `rows` as the disk list.
    pub fn with_disks(self, rows: Vec<DiskInfo>) -> Self {
        Self {
            disk: std::sync::Arc::new(StubDisk::new(rows)),
            ..self
        }
    }

    /// Assemble the context.
    pub fn context(&self) -> crate::context::SystemContext {
        use std::sync::Arc;
        crate::context::SystemContext::builder()
            .system(Arc::clone(&self.system) as Arc<dyn SystemManager>)
            .process(Arc::clone(&self.process) as Arc<dyn ProcessManager>)
            .port(Arc::clone(&self.port) as Arc<dyn PortManager>)
            .network(Arc::clone(&self.network) as Arc<dyn NetworkManager>)
            .service(Arc::clone(&self.service) as Arc<dyn ServiceManager>)
            .disk(Arc::clone(&self.disk) as Arc<dyn DiskManager>)
            .build()
            .expect("stub capabilities are always complete")
    }
}
