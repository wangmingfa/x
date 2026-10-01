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
        .file(Arc::new(NoopFile))
        .clipboard(Arc::new(NoopClipboard))
        .user(Arc::new(NoopUser))
        .shell(Arc::new(NoopShell))
        .proxy(Arc::new(NoopProxy))
        .power(Arc::new(NoopPower))
        .mount(Arc::new(NoopMount))
        .build()
        .expect("stub capabilities are always complete")
}

/// File capability stub: inspection works on the real filesystem, the
/// destructive open/reveal/trash family does nothing.
pub struct NoopFile;

impl crate::file::FileManager for NoopFile {
    fn open(&self, _path: &std::path::Path) -> crate::error::Result<()> {
        Ok(())
    }
    fn reveal(&self, _path: &std::path::Path) -> crate::error::Result<()> {
        Ok(())
    }
    fn trash(&self, _path: &std::path::Path) -> crate::error::Result<()> {
        Ok(())
    }
}

/// Clipboard stub that keeps the last written text in memory.
pub struct NoopClipboard;

impl crate::clipboard::ClipboardManager for NoopClipboard {
    fn get(&self) -> crate::error::Result<String> {
        Ok(String::new())
    }
    fn set(&self, _text: &str) -> crate::error::Result<()> {
        Ok(())
    }
    fn clear(&self) -> crate::error::Result<()> {
        Ok(())
    }
}

/// User capability stub with one fake current user.
pub struct NoopUser;

impl crate::user::UserManager for NoopUser {
    fn current(&self) -> crate::error::Result<crate::user::UserInfo> {
        Ok(crate::user::UserInfo {
            name: "stub".into(),
            ..crate::user::UserInfo::default()
        })
    }
    fn list(&self) -> crate::error::Result<Vec<crate::user::UserInfo>> {
        Ok(vec![self.current()?])
    }
    fn groups(&self) -> crate::error::Result<Vec<crate::user::GroupInfo>> {
        Ok(vec![crate::user::GroupInfo {
            name: "stub".into(),
            ..crate::user::GroupInfo::default()
        }])
    }
}

/// Shell capability stub.
pub struct NoopShell;

impl crate::shell::ShellManager for NoopShell {
    fn current(&self) -> crate::error::Result<crate::shell::ShellInfo> {
        Ok(crate::shell::ShellInfo {
            name: "stub".into(),
            ..crate::shell::ShellInfo::default()
        })
    }
    fn list(&self) -> crate::error::Result<Vec<crate::shell::ShellInfo>> {
        Ok(vec![self.current()?])
    }
    fn default(&self) -> crate::error::Result<crate::shell::ShellInfo> {
        self.current()
    }
}

/// Proxy stub: env layer is real, system layer absent.
pub struct NoopProxy;

impl crate::proxy::ProxyManager for NoopProxy {}

/// Power stub: battery absent, verbs succeed without side effects.
pub struct NoopPower;

impl crate::power::PowerManager for NoopPower {
    fn sleep(&self) -> crate::error::Result<()> {
        Ok(())
    }
    fn shutdown(&self, _delay_seconds: u32) -> crate::error::Result<()> {
        Ok(())
    }
    fn reboot(&self, _delay_seconds: u32) -> crate::error::Result<()> {
        Ok(())
    }
}

/// Mount stub: nothing mounted, verbs succeed without side effects.
pub struct NoopMount;

impl crate::mount::MountManager for NoopMount {
    fn list(&self) -> crate::error::Result<Vec<crate::mount::MountInfo>> {
        Ok(Vec::new())
    }
    fn mount(&self, _source: &str, _target: &str) -> crate::error::Result<()> {
        Ok(())
    }
    fn unmount(&self, _target: &str) -> crate::error::Result<()> {
        Ok(())
    }
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

    /// Replace the rows after the fact — for watch-style tests that must
    /// change the world between two polls.
    pub fn set_rows(&self, rows: Vec<ProcessInfo>) {
        *self.rows.lock().expect("stub mutex") = rows;
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
    resolved: Option<Vec<std::net::IpAddr>>,
    tls: Option<crate::netdiag::TlsInfo>,
    response: Option<crate::netdiag::HttpResponse>,
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
            resolved: None,
            tls: None,
            response: None,
        }
    }

    /// Canned answers for the resolve / TLS / HTTP probes.
    pub fn with_probes(
        mut self,
        resolved: Option<Vec<std::net::IpAddr>>,
        tls: Option<crate::netdiag::TlsInfo>,
        response: Option<crate::netdiag::HttpResponse>,
    ) -> Self {
        self.resolved = resolved;
        self.tls = tls;
        self.response = response;
        self
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

    fn resolve(&self, _host: &str) -> Result<Vec<std::net::IpAddr>> {
        match &self.resolved {
            Some(rows) => Ok(rows.clone()),
            None => Err(crate::Error::unsupported("stub cannot resolve")),
        }
    }

    fn tls_info(&self, host: &str, port: u16, _timeout_ms: u64) -> Result<crate::netdiag::TlsInfo> {
        match &self.tls {
            Some(info) => {
                let mut info = info.clone();
                info.host = host.to_string();
                info.port = port;
                Ok(info)
            }
            None => Err(crate::Error::unsupported("stub cannot probe TLS")),
        }
    }

    fn http_probe(
        &self,
        url: &str,
        _method: &str,
        _timeout_ms: u64,
    ) -> Result<crate::netdiag::HttpResponse> {
        match &self.response {
            Some(response) => {
                let mut response = response.clone();
                response.url = url.to_string();
                Ok(response)
            }
            None => Err(crate::Error::unsupported("stub cannot probe HTTP")),
        }
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

/// Firewall capability backed by a fixed rule list, recording every change.
#[derive(Debug, Default)]
pub struct StubFirewall {
    enabled: bool,
    rules: Mutex<Vec<crate::firewall::FirewallRule>>,
    changes: Mutex<Vec<(String, u16, String)>>,
}

impl StubFirewall {
    /// A firewall that answers with `rules` and reports `enabled`.
    pub fn new(enabled: bool, rules: Vec<crate::firewall::FirewallRule>) -> Self {
        Self {
            enabled,
            rules: Mutex::new(rules),
            changes: Mutex::new(Vec::new()),
        }
    }

    /// Changes the frontend asked for, in order: `(verb, port, protocol)`.
    pub fn changes(&self) -> Vec<(String, u16, String)> {
        self.changes.lock().expect("stub mutex").clone()
    }
}

impl crate::firewall::FirewallManager for StubFirewall {
    fn stack(&self) -> crate::firewall::FirewallStack {
        crate::firewall::FirewallStack::Unknown
    }

    fn enabled(&self) -> Result<bool> {
        Ok(self.enabled)
    }

    fn list(&self) -> Result<Vec<crate::firewall::FirewallRule>> {
        Ok(self.rules.lock().expect("stub mutex").clone())
    }

    fn allow(&self, port: u16, protocol: Option<&str>, _name: Option<&str>) -> Result<()> {
        self.changes.lock().expect("stub mutex").push((
            "allow".to_string(),
            port,
            protocol.unwrap_or("tcp").to_string(),
        ));
        Ok(())
    }

    fn deny(&self, port: u16, protocol: Option<&str>, _name: Option<&str>) -> Result<()> {
        self.changes.lock().expect("stub mutex").push((
            "deny".to_string(),
            port,
            protocol.unwrap_or("tcp").to_string(),
        ));
        Ok(())
    }
}

/// Log reader stub that answers with a fixed page and records the scope asked
/// for, so tests can assert what the frontend requested.
#[derive(Debug)]
pub struct StubLogs {
    source: String,
    entries: Vec<crate::logs::LogEntry>,
    reads: std::sync::Mutex<Vec<String>>,
}

impl Default for StubLogs {
    fn default() -> Self {
        Self {
            source: "stub".to_string(),
            entries: Vec::new(),
            reads: std::sync::Mutex::new(Vec::new()),
        }
    }
}

impl StubLogs {
    /// A reader serving `entries` from `source`.
    pub fn new(source: &str, entries: Vec<crate::logs::LogEntry>) -> Self {
        Self {
            source: source.to_string(),
            entries,
            reads: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Scopes read so far, in order.
    pub fn reads(&self) -> Vec<String> {
        self.reads.lock().expect("stub mutex").clone()
    }
}

impl crate::logs::LogReader for StubLogs {
    fn read(
        &self,
        scope: &crate::logs::LogScope,
        limit: usize,
    ) -> crate::error::Result<crate::logs::LogPage> {
        self.reads.lock().expect("stub mutex").push(scope.label());
        let mut entries = self.entries.clone();
        entries.truncate(limit);
        Ok(crate::logs::LogPage {
            scope: scope.label(),
            source: self.source.clone(),
            entries,
        })
    }
}

/// Device inventory stub serving a fixed list, or an honest "cannot list"
/// when no list was configured.
#[derive(Debug, Default)]
pub struct StubDevices {
    devices: Option<Vec<crate::device::DeviceInfo>>,
}

impl StubDevices {
    /// Fixed inventory.
    pub fn new(devices: Vec<crate::device::DeviceInfo>) -> Self {
        Self {
            devices: Some(devices),
        }
    }
}

impl crate::device::DeviceManager for StubDevices {
    fn devices(&self) -> crate::error::Result<Vec<crate::device::DeviceInfo>> {
        match &self.devices {
            Some(rows) => Ok(rows.clone()),
            None => Err(crate::Error::unsupported("stub cannot list devices")),
        }
    }
}

/// Bluetooth stub serving fixed adapter/device lists and recording the verbs
/// it was asked to run. Unconfigured reads stay honest: they fail unsupported.
#[derive(Debug, Default)]
pub struct StubBluetooth {
    adapters: Option<Vec<crate::bluetooth::BluetoothAdapter>>,
    devices: Option<Vec<crate::bluetooth::BluetoothDevice>>,
    verbs: Mutex<Vec<(String, String)>>,
}

impl StubBluetooth {
    /// Fixed inventory.
    pub fn new(
        adapters: Vec<crate::bluetooth::BluetoothAdapter>,
        devices: Vec<crate::bluetooth::BluetoothDevice>,
    ) -> Self {
        Self {
            adapters: Some(adapters),
            devices: Some(devices),
            verbs: Mutex::new(Vec::new()),
        }
    }

    /// Verbs requested so far, in order: `("connect", address)`, …
    pub fn verbs(&self) -> Vec<(String, String)> {
        self.verbs.lock().expect("stub mutex").clone()
    }

    fn verb(&self, name: &str, address: &str) -> crate::error::Result<()> {
        if self.adapters.is_none() {
            return Err(crate::Error::unsupported("stub bluetooth not configured"));
        }
        self.verbs
            .lock()
            .expect("stub mutex")
            .push((name.to_string(), address.to_string()));
        Ok(())
    }
}

impl crate::bluetooth::BluetoothManager for StubBluetooth {
    fn adapters(&self) -> crate::error::Result<Vec<crate::bluetooth::BluetoothAdapter>> {
        match &self.adapters {
            Some(rows) => Ok(rows.clone()),
            None => Err(crate::Error::unsupported(
                "stub cannot list bluetooth adapters",
            )),
        }
    }

    fn devices(&self) -> crate::error::Result<Vec<crate::bluetooth::BluetoothDevice>> {
        match &self.devices {
            Some(rows) => Ok(rows.clone()),
            None => Err(crate::Error::unsupported(
                "stub cannot list bluetooth devices",
            )),
        }
    }

    fn connect(&self, address: &str) -> crate::error::Result<()> {
        self.verb("connect", address)
    }

    fn disconnect(&self, address: &str) -> crate::error::Result<()> {
        self.verb("disconnect", address)
    }
}

/// Display topology stub serving a fixed list, or an honest "cannot list"
/// when no list was configured.
#[derive(Debug, Default)]
pub struct StubDisplay {
    displays: Option<Vec<crate::display::DisplayInfo>>,
}

impl StubDisplay {
    /// Fixed topology.
    pub fn new(displays: Vec<crate::display::DisplayInfo>) -> Self {
        Self {
            displays: Some(displays),
        }
    }
}

impl crate::display::DisplayManager for StubDisplay {
    fn displays(&self) -> crate::error::Result<Vec<crate::display::DisplayInfo>> {
        match &self.displays {
            Some(rows) => Ok(rows.clone()),
            None => Err(crate::Error::unsupported("stub cannot list displays")),
        }
    }
}

/// Window stub serving a fixed list, marking the row whose `active` is
/// `Some(true)` as the focused one, and recording every verb it was asked to
/// run. Unconfigured reads stay honest: they fail unsupported.
#[derive(Debug, Default)]
pub struct StubWindows {
    rows: Option<Vec<crate::window::WindowInfo>>,
    verbs: Mutex<Vec<(String, String)>>,
}

impl StubWindows {
    /// Fixed inventory.
    pub fn new(rows: Vec<crate::window::WindowInfo>) -> Self {
        Self {
            rows: Some(rows),
            verbs: Mutex::new(Vec::new()),
        }
    }

    /// Verbs requested so far, in order: `("focus", "`Title`")`, …
    pub fn verbs(&self) -> Vec<(String, String)> {
        self.verbs.lock().expect("stub mutex").clone()
    }

    fn verb(&self, name: &str, window: &crate::window::WindowInfo) -> crate::error::Result<()> {
        if self.rows.is_none() {
            return Err(crate::Error::unsupported("stub windows not configured"));
        }
        self.verbs
            .lock()
            .expect("stub mutex")
            .push((name.to_string(), window.label()));
        Ok(())
    }
}

impl crate::window::WindowManager for StubWindows {
    fn windows(&self) -> crate::error::Result<Vec<crate::window::WindowInfo>> {
        match &self.rows {
            Some(rows) => Ok(rows.clone()),
            None => Err(crate::Error::unsupported("stub cannot list windows")),
        }
    }

    fn active(&self) -> crate::error::Result<Option<crate::window::WindowInfo>> {
        match &self.rows {
            Some(rows) => Ok(rows.iter().find(|row| row.active == Some(true)).cloned()),
            None => Err(crate::Error::unsupported("stub cannot list windows")),
        }
    }

    fn focus(&self, window: &crate::window::WindowInfo) -> crate::error::Result<()> {
        self.verb("focus", window)
    }

    fn minimize(&self, window: &crate::window::WindowInfo) -> crate::error::Result<()> {
        self.verb("minimize", window)
    }

    fn maximize(&self, window: &crate::window::WindowInfo) -> crate::error::Result<()> {
        self.verb("maximize", window)
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
    /// Firewall capability.
    pub firewall: std::sync::Arc<StubFirewall>,
    /// Log reader capability.
    pub logs: std::sync::Arc<StubLogs>,
    /// Device inventory capability.
    pub device: std::sync::Arc<StubDevices>,
    /// Bluetooth capability.
    pub bluetooth: std::sync::Arc<StubBluetooth>,
    /// Display capability.
    pub display: std::sync::Arc<StubDisplay>,
    /// Window capability.
    pub window: std::sync::Arc<StubWindows>,
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
            firewall: std::sync::Arc::new(StubFirewall::default()),
            logs: std::sync::Arc::new(StubLogs::default()),
            device: std::sync::Arc::new(StubDevices::default()),
            bluetooth: std::sync::Arc::new(StubBluetooth::default()),
            display: std::sync::Arc::new(StubDisplay::default()),
            window: std::sync::Arc::new(StubWindows::default()),
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

    /// Use `rules` as the firewall rule list with the given global state.
    pub fn with_firewall(self, enabled: bool, rules: Vec<crate::firewall::FirewallRule>) -> Self {
        Self {
            firewall: std::sync::Arc::new(StubFirewall::new(enabled, rules)),
            ..self
        }
    }

    /// Serve `entries` from a log reader named `source`.
    pub fn with_logs(self, source: &str, entries: Vec<crate::logs::LogEntry>) -> Self {
        Self {
            logs: std::sync::Arc::new(StubLogs::new(source, entries)),
            ..self
        }
    }

    /// Serve `rows` as the device inventory.
    pub fn with_devices(self, rows: Vec<crate::device::DeviceInfo>) -> Self {
        Self {
            device: std::sync::Arc::new(StubDevices::new(rows)),
            ..self
        }
    }

    /// Serve fixed Bluetooth adapters and devices.
    pub fn with_bluetooth(
        self,
        adapters: Vec<crate::bluetooth::BluetoothAdapter>,
        devices: Vec<crate::bluetooth::BluetoothDevice>,
    ) -> Self {
        Self {
            bluetooth: std::sync::Arc::new(StubBluetooth::new(adapters, devices)),
            ..self
        }
    }

    /// Serve `rows` as the display topology.
    pub fn with_displays(self, rows: Vec<crate::display::DisplayInfo>) -> Self {
        Self {
            display: std::sync::Arc::new(StubDisplay::new(rows)),
            ..self
        }
    }

    /// Serve `rows` as the window list.
    pub fn with_windows(self, rows: Vec<crate::window::WindowInfo>) -> Self {
        Self {
            window: std::sync::Arc::new(StubWindows::new(rows)),
            ..self
        }
    }

    /// Canned DNS / TLS / HTTP probe answers on the network capability.
    pub fn with_net_probes(
        self,
        resolved: Option<Vec<std::net::IpAddr>>,
        tls: Option<crate::netdiag::TlsInfo>,
        response: Option<crate::netdiag::HttpResponse>,
    ) -> Self {
        Self {
            network: std::sync::Arc::new(
                StubNetwork::default().with_probes(resolved, tls, response),
            ),
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
            .firewall(Arc::clone(&self.firewall) as Arc<dyn crate::firewall::FirewallManager>)
            .logs(Arc::clone(&self.logs) as Arc<dyn crate::logs::LogReader>)
            .device(Arc::clone(&self.device) as Arc<dyn crate::device::DeviceManager>)
            .bluetooth(Arc::clone(&self.bluetooth) as Arc<dyn crate::bluetooth::BluetoothManager>)
            .display(Arc::clone(&self.display) as Arc<dyn crate::display::DisplayManager>)
            .window(Arc::clone(&self.window) as Arc<dyn crate::window::WindowManager>)
            .build()
            .expect("stub capabilities are always complete")
    }
}
