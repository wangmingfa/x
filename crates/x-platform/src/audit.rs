//! The local audit log for destructive operations.
//!
//! `x` kills processes, reclaims ports and drives services, so it keeps a
//! trail the operator can read back: one JSON object per line, appended to a
//! plain file. This module owns everything OS-specific about that trail:
//!
//! * **where** the file lives per platform, with `X_AUDIT_PATH` as an
//!   explicit override and `X_AUDIT=off` as the kill switch,
//! * **who** writes it: the [`attach`] decorators wrap the destructive
//!   capabilities of a real context, so both the CLI and the TUI are audited
//!   through the same code path.
//!
//! Auditing must never break the operation it documents, so every write error
//! is swallowed after the attempt. Records are written *after* the call, with
//! the outcome, because a refused operation is as interesting as a successful
//! one.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use x_core::audit::AuditEntry;
use x_core::bluetooth::{BluetoothAdapter, BluetoothDevice, BluetoothManager};
use x_core::context::SystemContext;
use x_core::error::Result;
use x_core::firewall::{FirewallManager, FirewallRule, FirewallStack};
use x_core::port::{KillPlan, PortInfo, PortListOptions, PortManager, PortQuery};
use x_core::process::{KillSignal, ProcessInfo, ProcessListOptions, ProcessManager};
use x_core::service::{
    NativeOutput, ServiceAction, ServiceInfo, ServiceListOptions, ServiceLogPage, ServiceManager,
    ServiceManagerType,
};

/// Set to `off` to stop writing records entirely.
const ENABLE_VAR: &str = "X_AUDIT";
/// Full path of the audit log, overriding the platform default.
const PATH_VAR: &str = "X_AUDIT_PATH";

/// Where records go, or `None` when auditing is switched off.
pub fn log_path() -> Option<PathBuf> {
    if std::env::var(ENABLE_VAR).as_deref() == Ok("off") {
        return None;
    }
    match std::env::var_os(PATH_VAR) {
        Some(path) if !path.is_empty() => Some(PathBuf::from(path)),
        _ => Some(default_path()),
    }
}

fn default_path() -> PathBuf {
    #[cfg(windows)]
    let base = first_non_empty_env(&["LOCALAPPDATA"]).or_else(|| {
        std::env::var_os("USERPROFILE")
            .map(|profile| PathBuf::from(profile).join("AppData").join("Local"))
    });

    #[cfg(target_os = "linux")]
    let base = first_non_empty_env(&["XDG_STATE_HOME"])
        .or_else(|| first_non_empty_env(&["HOME"]).map(|home| home.join(".local").join("state")));

    #[cfg(target_os = "macos")]
    let base =
        first_non_empty_env(&["HOME"]).map(|home| home.join("Library").join("Application Support"));

    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    let base: Option<PathBuf> = None;

    let base = base.unwrap_or_else(std::env::temp_dir);
    base.join("x").join("audit.log")
}

fn first_non_empty_env(keys: &[&str]) -> Option<PathBuf> {
    keys.iter()
        .filter_map(std::env::var_os)
        .find(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Append one record, creating the parent directory on demand.
pub fn record(entry: &AuditEntry) -> std::io::Result<()> {
    let Some(path) = log_path() else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(entry.json_line().as_bytes())
}

/// [`record`] for code paths where a failure must not leak out.
fn record_quiet(entry: &AuditEntry) {
    let _ = record(entry);
}

/// The shared state of the three decorators.
struct Recorder {
    user: Option<String>,
}

impl Recorder {
    fn note(&self, action: &str, target: String, outcome: String) {
        record_quiet(&AuditEntry {
            time: x_core::audit::now_utc_rfc3339(),
            user: self.user.clone(),
            action: action.to_string(),
            target,
            outcome,
        });
    }

    /// Record a finished call: `ok`, or the error's message.
    fn note_result<T>(&self, action: &str, target: String, result: &Result<T>) {
        let outcome = match result {
            Ok(_) => "ok".to_string(),
            Err(error) => format!("error: {}", error.message()),
        };
        self.note(action, target, outcome);
    }
}

/// Name a [`KillSignal`] the way the audit reader expects to grep it.
fn signal_name(signal: KillSignal) -> &'static str {
    match signal {
        KillSignal::Terminate => "terminate",
        KillSignal::Interrupt => "interrupt",
        KillSignal::Kill => "kill",
        KillSignal::Quit => "quit",
        KillSignal::Hangup => "hangup",
    }
}

/// Name a [`ServiceAction`] as one word.
fn action_verb(action: ServiceAction) -> &'static str {
    match action {
        ServiceAction::Start => "start",
        ServiceAction::Stop => "stop",
        ServiceAction::Restart => "restart",
        ServiceAction::Reload => "reload",
        ServiceAction::Enable => "enable",
        ServiceAction::Disable => "disable",
    }
}

/// Describe what a kill plan was about.
fn plan_target(plan: &KillPlan) -> String {
    match &plan.query {
        PortQuery::Port(port) => format!("port {port}"),
        PortQuery::Process(name) => format!("process {name}"),
    }
}

/// Process capability that records every termination.
pub struct AuditedProcess {
    inner: Arc<dyn ProcessManager>,
    recorder: Recorder,
}

impl ProcessManager for AuditedProcess {
    fn list(&self, options: &ProcessListOptions) -> Result<Vec<ProcessInfo>> {
        self.inner.list(options)
    }

    fn get(&self, pid: u32) -> Result<ProcessInfo> {
        self.inner.get(pid)
    }

    fn kill(&self, pid: u32, signal: KillSignal) -> Result<()> {
        let result = self.inner.kill(pid, signal);
        self.recorder.note_result(
            "process.kill",
            format!("pid {pid} ({})", signal_name(signal)),
            &result,
        );
        result
    }

    fn kill_many(&self, pids: &[u32], signal: KillSignal) -> Result<Vec<x_core::Error>> {
        // The batch is one intent, so it gets one record even though the
        // inner default implementation would loop per pid.
        let failures = self.inner.kill_many(pids, signal)?;
        let shown: Vec<String> = pids.iter().take(16).map(|pid| pid.to_string()).collect();
        let more = if pids.len() > 16 {
            format!(" +{} more", pids.len() - 16)
        } else {
            String::new()
        };
        let outcome = if failures.is_empty() {
            format!("ok, {} killed", pids.len())
        } else {
            format!("partial, {} failed", failures.len())
        };
        self.recorder.note(
            "process.kill_many",
            format!("[{}{more}] ({})", shown.join(", "), signal_name(signal)),
            outcome,
        );
        Ok(failures)
    }
}

/// Port capability that records every reclaiming of a port.
pub struct AuditedPort {
    inner: Arc<dyn PortManager>,
    recorder: Recorder,
}

impl PortManager for AuditedPort {
    fn list(&self, options: &PortListOptions) -> Result<Vec<PortInfo>> {
        self.inner.list(options)
    }

    fn required_permission(&self, rows: &[PortInfo]) -> x_core::PermissionRequirement {
        self.inner.required_permission(rows)
    }

    fn kill_plan(&self, plan: &KillPlan, signal: KillSignal) -> Result<usize> {
        let result = self.inner.kill_plan(plan, signal);
        let outcome = match &result {
            Ok(killed) => format!("ok, {killed} process(es) killed"),
            Err(error) => format!("error: {}", error.message()),
        };
        self.recorder.note(
            "port.kill_plan",
            format!("{} ({})", plan_target(plan), signal_name(signal)),
            outcome,
        );
        result
    }
}

/// Service capability that records lifecycle actions and native passthroughs.
pub struct AuditedService {
    inner: Arc<dyn ServiceManager>,
    recorder: Recorder,
}

impl ServiceManager for AuditedService {
    fn manager_type(&self) -> ServiceManagerType {
        self.inner.manager_type()
    }

    fn list(&self, options: &ServiceListOptions) -> Result<Vec<ServiceInfo>> {
        self.inner.list(options)
    }

    fn status(&self, name: &str) -> Result<ServiceInfo> {
        self.inner.status(name)
    }

    fn logs(&self, name: &str, limit: Option<usize>) -> Result<ServiceLogPage> {
        self.inner.logs(name, limit)
    }

    fn action(&self, name: &str, action: ServiceAction) -> Result<()> {
        let result = self.inner.action(name, action);
        self.recorder.note_result(
            "service.action",
            format!("`{name}` ({})", action_verb(action)),
            &result,
        );
        result
    }

    fn native(&self, args: &[String]) -> Result<NativeOutput> {
        // The empty argument list is how `x capability` probes whether this
        // adapter has a native command at all; validating refused it, so it
        // is a feature test, not an operator action, and stays out of the log.
        if args.is_empty() {
            return self.inner.native(args);
        }
        let result = self.inner.native(args);
        let outcome = match &result {
            Ok(output) if output.exit_code == 0 => "ok".to_string(),
            Ok(output) => format!("exit {}", output.exit_code),
            Err(error) => format!("error: {}", error.message()),
        };
        self.recorder
            .note("service.native", args.join(" "), outcome);
        result
    }
}

/// Firewall capability that records rule changes.
pub struct AuditedFirewall {
    inner: Arc<dyn FirewallManager>,
    recorder: Recorder,
}

/// `port 8080/tcp as \`web\`` — what a firewall change was about.
fn rule_target(port: u16, protocol: Option<&str>, name: Option<&str>) -> String {
    match name {
        Some(name) => format!("port {port}/{} as `{name}`", protocol.unwrap_or("tcp")),
        None => format!("port {port}/{}", protocol.unwrap_or("tcp")),
    }
}

impl FirewallManager for AuditedFirewall {
    fn stack(&self) -> FirewallStack {
        self.inner.stack()
    }

    fn enabled(&self) -> Result<bool> {
        self.inner.enabled()
    }

    fn list(&self) -> Result<Vec<FirewallRule>> {
        self.inner.list()
    }

    fn allow(&self, port: u16, protocol: Option<&str>, name: Option<&str>) -> Result<()> {
        let result = self.inner.allow(port, protocol, name);
        self.recorder
            .note_result("firewall.allow", rule_target(port, protocol, name), &result);
        result
    }

    fn deny(&self, port: u16, protocol: Option<&str>, name: Option<&str>) -> Result<()> {
        let result = self.inner.deny(port, protocol, name);
        self.recorder
            .note_result("firewall.deny", rule_target(port, protocol, name), &result);
        result
    }
}

/// Bluetooth capability that records the link-changing verbs.
///
/// Reads (adapters, devices, scan) pass through: the doctrine records
/// changes, not queries. Only a backend with real verbs (Linux) ever
/// reaches the log; the others answer Unsupported, which is recorded too —
/// a refused attempt is a fact worth keeping.
pub struct AuditedBluetooth {
    inner: Arc<dyn BluetoothManager>,
    recorder: Recorder,
}

impl BluetoothManager for AuditedBluetooth {
    fn adapters(&self) -> Result<Vec<BluetoothAdapter>> {
        self.inner.adapters()
    }

    fn devices(&self) -> Result<Vec<BluetoothDevice>> {
        self.inner.devices()
    }

    fn scan(&self, timeout_ms: u64) -> Result<Vec<BluetoothDevice>> {
        self.inner.scan(timeout_ms)
    }

    fn connect(&self, address: &str) -> Result<()> {
        let result = self.inner.connect(address);
        self.recorder
            .note_result("bluetooth.connect", address.to_string(), &result);
        result
    }

    fn disconnect(&self, address: &str) -> Result<()> {
        let result = self.inner.disconnect(address);
        self.recorder
            .note_result("bluetooth.disconnect", address.to_string(), &result);
        result
    }
}

/// Wrap the destructive capabilities of `context` so real runs are audited.
///
/// This is called from the composition root only: stub-driven CLI and TUI
/// tests build their own contexts and never touch the audit file.
pub fn attach(context: SystemContext) -> SystemContext {
    let user = crate::sys::current_user_name();
    let recorder = || Recorder { user: user.clone() };
    SystemContext {
        system: context.system,
        network: context.network,
        disk: context.disk,
        process: Arc::new(AuditedProcess {
            inner: context.process,
            recorder: recorder(),
        }),
        port: Arc::new(AuditedPort {
            inner: context.port,
            recorder: recorder(),
        }),
        service: Arc::new(AuditedService {
            inner: context.service,
            recorder: recorder(),
        }),
        file: context.file,
        clipboard: context.clipboard,
        user: context.user,
        shell: context.shell,
        proxy: context.proxy,
        power: context.power,
        mount: context.mount,
        startup: context.startup,
        schedule: context.schedule,
        firewall: context.firewall.map(|inner| {
            Arc::new(AuditedFirewall {
                inner,
                recorder: recorder(),
            }) as Arc<dyn FirewallManager>
        }),
        // Log reads are never audited: the doctrine records changes, not reads.
        logs: context.logs,
        // Device enumeration is a read too; it passes straight through.
        device: context.device,
        bluetooth: context.bluetooth.map(|inner| {
            Arc::new(AuditedBluetooth {
                inner,
                recorder: recorder(),
            }) as Arc<dyn BluetoothManager>
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use x_core::testing::{
        stub_socket, NoopProcess, NoopService, StubBluetooth, StubFirewall, StubPort, StubProcess,
        StubService,
    };

    /// Every test in here manipulates the process-wide audit environment
    /// variables, so they run one at a time.
    static ENV: Mutex<()> = Mutex::new(());

    fn scratch(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("x-audit-{}-{tag}.log", std::process::id()))
    }

    /// Point the log at a private file (or switch it off) for one test.
    fn use_log(path: Option<PathBuf>) {
        std::env::remove_var(ENABLE_VAR);
        match path {
            Some(path) => std::env::set_var(PATH_VAR, path),
            None => std::env::remove_var(PATH_VAR),
        }
    }

    fn read_lines(path: &PathBuf) -> Vec<String> {
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn record_appends_one_json_line_per_entry() {
        let _guard = ENV.lock().expect("env mutex");
        let path = scratch("append");
        use_log(Some(path.clone()));

        let entry = |action: &str| AuditEntry {
            time: "2026-10-01T00:00:00Z".into(),
            user: Some("alice".into()),
            action: action.into(),
            target: "pid 42 (kill)".into(),
            outcome: "ok".into(),
        };
        record(&entry("process.kill")).expect("record");
        record(&entry("service.action")).expect("record");

        let lines = read_lines(&path);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].contains("\"action\":\"process.kill\""));
        assert!(lines[1].contains("\"action\":\"service.action\""));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn the_off_switch_prevents_any_write() {
        let _guard = ENV.lock().expect("env mutex");
        let path = scratch("off");
        use_log(Some(path.clone()));
        std::env::set_var(ENABLE_VAR, "off");

        let entry = AuditEntry {
            time: "2026-10-01T00:00:00Z".into(),
            user: None,
            action: "process.kill".into(),
            target: "pid 1 (kill)".into(),
            outcome: "ok".into(),
        };
        assert!(record(&entry).is_ok());
        assert!(!path.exists());
        std::env::remove_var(ENABLE_VAR);
    }

    #[test]
    fn the_default_path_is_named_after_the_trail_it_holds() {
        let _guard = ENV.lock().expect("env mutex");
        use_log(None);
        let path = log_path().expect("on by default");
        assert_eq!(path.file_name().expect("name"), "audit.log");
        assert_eq!(
            path.parent().and_then(|p| p.file_name()),
            Some("x".as_ref())
        );
    }

    #[test]
    fn a_kill_through_the_decorator_leaves_a_record() {
        let _guard = ENV.lock().expect("env mutex");
        let path = scratch("kill");
        use_log(Some(path.clone()));

        let inner = Arc::new(StubProcess::new(Vec::new()));
        let process = AuditedProcess {
            inner,
            recorder: Recorder { user: None },
        };
        process.kill(4242, KillSignal::Kill).expect("stub kill");

        let lines = read_lines(&path);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains("\"action\":\"process.kill\""));
        assert!(lines[0].contains("pid 4242 (kill)"));
        assert!(lines[0].contains("\"outcome\":\"ok\""));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_refused_kill_is_recorded_with_its_error() {
        let _guard = ENV.lock().expect("env mutex");
        let path = scratch("refused");
        use_log(Some(path.clone()));

        let process = AuditedProcess {
            inner: Arc::new(NoopProcess),
            recorder: Recorder { user: None },
        };
        // Noop kills succeed; the service decorator is the one with a
        // guaranteed refusal, so exercise the error shape there.
        let service = AuditedService {
            inner: Arc::new(NoopService),
            recorder: Recorder { user: None },
        };
        assert!(service.action("ghost", ServiceAction::Stop).is_err());
        process.kill(7, KillSignal::Terminate).expect("noop kill");

        let lines = read_lines(&path);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].contains("error: no service manager available"));
        assert!(lines[1].contains("pid 7 (terminate)"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_batch_kill_is_recorded_once() {
        let _guard = ENV.lock().expect("env mutex");
        let path = scratch("batch");
        use_log(Some(path.clone()));

        let stub = Arc::new(StubProcess::new(Vec::new()));
        let inner: Arc<dyn ProcessManager> = stub.clone();
        let process = AuditedProcess {
            inner,
            recorder: Recorder { user: None },
        };
        let failures = process
            .kill_many(&[3, 4, 5], KillSignal::Terminate)
            .expect("stub batch kill");
        assert!(failures.is_empty());
        assert_eq!(stub.killed(), vec![3, 4, 5]);

        let lines = read_lines(&path);
        assert_eq!(lines.len(), 1, "one intent, one record: {lines:?}");
        assert!(lines[0].contains("\"action\":\"process.kill_many\""));
        assert!(lines[0].contains("[3, 4, 5]"));
        assert!(lines[0].contains("3 killed"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn port_and_service_actions_are_attributed_to_their_target() {
        let _guard = ENV.lock().expect("env mutex");
        let path = scratch("targets");
        use_log(Some(path.clone()));

        let port = AuditedPort {
            inner: Arc::new(StubPort::new(vec![stub_socket(8080, 99, "node")])),
            recorder: Recorder { user: None },
        };
        let plan = port
            .plan(&PortQuery::Port(8080), x_core::PortSort::Port)
            .expect("stub plan");
        assert_eq!(port.kill_plan(&plan, KillSignal::Kill).expect("stub"), 1);

        let service = AuditedService {
            inner: Arc::new(StubService::new(vec![x_core::testing::stub_service(
                "sshd", 42,
            )])),
            recorder: Recorder { user: None },
        };
        service
            .action("sshd", ServiceAction::Stop)
            .expect("stub action");
        service
            .native(&["query".to_string(), "sshd".to_string()])
            .expect("stub native");

        let lines = read_lines(&path);
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert!(lines[0].contains("\"action\":\"port.kill_plan\""));
        assert!(lines[0].contains("port 8080 (kill)"));
        assert!(lines[0].contains("1 process(es) killed"));
        assert!(lines[1].contains("`sshd` (stop)"));
        assert!(lines[2].contains("\"action\":\"service.native\""));
        assert!(lines[2].contains("\"target\":\"query sshd\""));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn the_capability_probe_stays_out_of_the_log() {
        let _guard = ENV.lock().expect("env mutex");
        let path = scratch("probe");
        use_log(Some(path.clone()));

        let service = AuditedService {
            inner: Arc::new(StubService::new(Vec::new())),
            recorder: Recorder { user: None },
        };
        // The probe shape: empty args, outcome irrelevant.
        let _ = service.native(&[]);
        assert!(read_lines(&path).is_empty(), "probe wrote a record");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn firewall_changes_are_attributed_to_their_rule_target() {
        let _guard = ENV.lock().expect("env mutex");
        let path = scratch("firewall");
        use_log(Some(path.clone()));

        let firewall = AuditedFirewall {
            inner: Arc::new(StubFirewall::new(true, Vec::new())),
            recorder: Recorder { user: None },
        };
        firewall.allow(8080, Some("tcp"), None).expect("stub allow");
        firewall
            .deny(53, Some("udp"), Some("dns"))
            .expect("stub deny");

        let lines = read_lines(&path);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].contains("\"action\":\"firewall.allow\""));
        assert!(lines[0].contains("\"target\":\"port 8080/tcp\""));
        assert!(lines[0].contains("\"outcome\":\"ok\""));
        assert!(lines[1].contains("\"action\":\"firewall.deny\""));
        assert!(lines[1].contains("port 53/udp as `dns`"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn bluetooth_verbs_are_recorded_but_reads_are_not() {
        let _guard = ENV.lock().expect("env mutex");
        let path = scratch("bluetooth");
        use_log(Some(path.clone()));

        let bluetooth = AuditedBluetooth {
            inner: Arc::new(StubBluetooth::new(Vec::new(), Vec::new())),
            recorder: Recorder { user: None },
        };
        bluetooth.adapters().expect("stub adapters");
        bluetooth
            .connect("AA:BB:CC:DD:EE:FF")
            .expect("stub connect");
        bluetooth
            .disconnect("AA:BB:CC:DD:EE:FF")
            .expect("stub disconnect");

        let lines = read_lines(&path);
        assert_eq!(lines.len(), 2, "reads stay out of the log: {lines:?}");
        assert!(lines[0].contains("\"action\":\"bluetooth.connect\""));
        assert!(lines[0].contains("\"target\":\"AA:BB:CC:DD:EE:FF\""));
        assert!(lines[1].contains("\"action\":\"bluetooth.disconnect\""));
        std::fs::remove_file(&path).ok();
    }
}
