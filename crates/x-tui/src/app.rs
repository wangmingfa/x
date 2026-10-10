//! Interface state: what is shown, what is selected, and what is pending.
//!
//! All logic here is pure with respect to the terminal, which is what makes the
//! key handling testable without a tty.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use x_core::disk::{DirUsage, DiskInfo};
use x_core::error::Result;
use x_core::net_top::{diff, NetSnapshot, NetTopReport, NetTopSampler};
use x_core::network::{AddressInfo, DnsConfig, InterfaceInfo, RouteInfo};
use x_core::port::{ConnectionState, KillPlan, PortInfo, PortListOptions, PortQuery};
use x_core::process::{ProcessInfo, ProcessListOptions, ProcessNode, ProcessSort, ProcessTree};
use x_core::service::{ServiceInfo, ServiceListOptions, ServiceState};
use x_core::system::{MemoryUsage, SystemInfo};
use x_core::{format_bytes, KillSignal, SystemContext};

use crate::palette::{self, Command, CommandId};

/// Rows assumed before the first draw, when the real height is still unknown.
const DEFAULT_VISIBLE_ROWS: usize = 20;

/// The order `s` cycles process sorting through.
const SORT_CYCLE: [ProcessSort; 5] = [
    ProcessSort::Cpu,
    ProcessSort::Memory,
    ProcessSort::Pid,
    ProcessSort::Name,
    ProcessSort::StartTime,
];

/// How many search hits each family contributes at most.
const SEARCH_HITS_PER_FAMILY: usize = 8;

/// The pages of the interface, in sidebar order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// Overview of the machine: CPU, memory, disks, sockets.
    Dashboard,
    /// Sockets and their owners.
    Ports,
    /// Processes.
    Processes,
    /// Network configuration and live connections.
    Network,
    /// Services.
    Services,
    /// System health.
    System,
    /// Mounted filesystems and a usage tree of the launch directory.
    Disks,
    /// Snapshots from another machine that runs x, over SSH.
    Remote,
    /// Live per-process network usage (`x net top`).
    NetTop,
}

impl View {
    /// Every view, for tab cycling.
    pub const ALL: [View; 9] = [
        View::Dashboard,
        View::Ports,
        View::Processes,
        View::Network,
        View::Services,
        View::System,
        View::Disks,
        View::Remote,
        View::NetTop,
    ];

    /// Sidebar label.
    pub fn title(self) -> &'static str {
        match self {
            Self::Dashboard => "dashboard",
            Self::Ports => "ports",
            Self::Processes => "processes",
            Self::Network => "network",
            Self::Services => "services",
            Self::System => "system",
            Self::Disks => "disks",
            Self::Remote => "remote",
            Self::NetTop => "net top",
        }
    }

    /// The next view, wrapping around.
    pub fn next(self) -> Self {
        let index = Self::ALL.iter().position(|v| *v == self).unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }

    /// The previous view, wrapping around.
    pub fn previous(self) -> Self {
        let index = Self::ALL.iter().position(|v| *v == self).unwrap_or(0);
        Self::ALL[(index + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

/// What a confirmation will act on.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// Free a port by running exactly this plan.
    Sockets(KillPlan),
    /// Terminate one process.
    Process {
        /// Process id.
        pid: u32,
        /// Name, for the dialog title.
        name: String,
    },
}

/// What the interface is currently asking the user.
#[derive(Debug, Clone, PartialEq)]
pub enum Modal {
    /// No dialog.
    None,
    /// Waiting for a filter string.
    Prompt {
        /// What is being asked.
        label: &'static str,
        /// Text typed so far.
        input: String,
    },
    /// Confirming a destructive action.
    Confirm {
        /// Description of the target.
        title: String,
        /// What will run on confirmation.
        target: Target,
        /// Signal that will be delivered.
        signal: KillSignal,
    },
    /// The Ctrl+P command palette.
    Palette {
        /// Text typed so far.
        input: String,
        /// Selected row of the filtered command list.
        selected: usize,
    },
    /// The global search overlay.
    Search {
        /// Text typed so far.
        input: String,
        /// Selected row of the hit list.
        selected: usize,
    },
    /// A read-only snapshot of one entity, with an optional kill target.
    Detail {
        /// Dialog title.
        title: String,
        /// `(field, value)` pairs, already rendered to text.
        rows: Vec<(String, String)>,
        /// What `k` would confirm, when the entity can be acted on.
        target: Option<Target>,
    },
}

/// One row of the global search overlay.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    /// Which family the hit belongs to (`process`, `port`, …).
    pub family: &'static str,
    /// What is shown bold.
    pub label: String,
    /// The supporting line under the label.
    pub detail: String,
    action: SearchAction,
}

/// What happens when a search hit is chosen.
#[derive(Debug, Clone, PartialEq)]
enum SearchAction {
    /// Switch to this page; `query` becomes its filter when present.
    Filter { view: View, query: Option<String> },
    /// Reveal this directory in the disk usage tree.
    Usage(PathBuf),
}

/// Socket counts shown on the dashboard.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PortSummary {
    /// Sockets in `listen`.
    pub listening: usize,
    /// Sockets in `established`.
    pub established: usize,
    /// Every socket the platform reported.
    pub total: usize,
}

impl PortSummary {
    fn from_rows(rows: &[PortInfo]) -> Self {
        let mut summary = Self {
            total: rows.len(),
            ..Default::default()
        };
        for row in rows {
            match row.state {
                ConnectionState::Listen => summary.listening += 1,
                ConnectionState::Established => summary.established += 1,
                _ => {}
            }
        }
        summary
    }
}

/// Human label of a socket state, shared with the renderer.
pub fn state_label(state: ConnectionState) -> &'static str {
    match state {
        ConnectionState::Listen => "listen",
        ConnectionState::Established => "established",
        ConnectionState::SynReceived => "syn-received",
        ConnectionState::SynSent => "syn-sent",
        ConnectionState::FinWait1 => "fin-wait-1",
        ConnectionState::FinWait2 => "fin-wait-2",
        ConnectionState::CloseWait => "close-wait",
        ConnectionState::Closing => "closing",
        ConnectionState::LastAck => "last-ack",
        ConnectionState::TimeWait => "time-wait",
        ConnectionState::Closed => "closed",
        ConnectionState::Bound => "bound",
        ConnectionState::Unknown => "unknown",
    }
}

/// Human label of a service state, shared with the renderer.
pub fn service_state_label(state: ServiceState) -> &'static str {
    match state {
        ServiceState::Running => "running",
        ServiceState::Stopped => "stopped",
        ServiceState::Starting => "starting",
        ServiceState::Stopping => "stopping",
        ServiceState::Failed => "failed",
        ServiceState::Disabled => "disabled",
        ServiceState::Enabled => "enabled",
        ServiceState::ActiveEnabled => "active_enabled",
        ServiceState::NotFound => "not_found",
        ServiceState::Unknown => "unknown",
    }
}

/// Short name of a process sort key, for the table title.
pub fn sort_label(sort: ProcessSort) -> &'static str {
    match sort {
        ProcessSort::Cpu => "cpu",
        ProcessSort::Memory => "memory",
        ProcessSort::Pid => "pid",
        ProcessSort::Name => "name",
        ProcessSort::StartTime => "start",
    }
}

/// The whole interface state.
pub struct App {
    context: SystemContext,
    view: View,
    quit: bool,
    modal: Modal,
    status: String,

    filter: String,
    selected: usize,
    scroll: usize,

    ports: Vec<PortInfo>,
    port_summary: Option<PortSummary>,
    processes: Vec<ProcessInfo>,
    process_sort: ProcessSort,
    tree_mode: bool,
    process_tree: Option<ProcessTree>,
    /// Pids whose subtree is hidden in tree mode.
    folded: BTreeSet<u32>,
    services: Vec<ServiceInfo>,
    interfaces: Vec<InterfaceInfo>,
    addresses: Vec<AddressInfo>,
    routes: Vec<RouteInfo>,
    dns: DnsConfig,
    /// Established and other non-listening sockets, for the connections table.
    connections: Vec<PortInfo>,
    system: Option<SystemInfo>,
    memory: Option<MemoryUsage>,
    disks: Vec<DiskInfo>,

    /// Root of the directory-usage scan, fixed at launch like `ncdu`.
    usage_root: PathBuf,
    /// Completed scan, empty until the walker thread reports.
    usage: Vec<DirUsage>,
    /// Directories whose subtree is currently hidden.
    collapsed: BTreeSet<PathBuf>,
    /// Mailbox of the in-flight scan; `None` once collected.
    scan: Option<Arc<Mutex<Option<Vec<DirUsage>>>>>,

    /// Global search cache, recomputed when the query changes.
    search_hits: Vec<SearchHit>,
    search_notes: Vec<String>,

    last_refresh: std::time::Instant,
    cpu: f32,
    /// How many rows the table body currently fits, updated while drawing.
    visible_rows: usize,

    /// SSH hosts the remote page can jump to (from `~/.ssh/config`).
    remote_hosts: Vec<String>,
    /// Live per-process network sampler, wired in by the composition root.
    /// `None` leaves the net top page showing a hint instead of a table.
    net_top: Option<std::sync::Arc<dyn NetTopSampler>>,
    /// Latest diffed report shown on the net top page.
    net_top_report: Option<NetTopReport>,
    /// Previous snapshot, for diffing the next round against.
    net_top_previous: Option<NetSnapshot>,
    /// In-flight sampling round (mailbox slot, like the disk walker).
    net_top_slot: Option<std::sync::Arc<Mutex<Option<NetSnapshot>>>>,
    /// Effective key bindings (defaults overridden by config.toml).
    keys: crate::keys::Keys,
    /// Host whose snapshot is currently displayed, if any.
    remote_host: Option<String>,
    /// Snapshot rows, already rendered to text.
    remote_rows: Vec<String>,
    /// Error from the last remote fetch, shown in place of data.
    remote_error: Option<String>,
    /// Mailbox of the in-flight remote fetch; `None` once collected.
    remote_fetch: Option<Arc<Mutex<Option<RemoteFetch>>>>,
}

impl App {
    /// Build the interface and take the first snapshot.
    ///
    /// The snapshot happens here so the first drawn frame is never empty, and so
    /// tests can assert on state without calling anything.
    pub fn new(context: SystemContext) -> Self {
        Self::with_keys(context, crate::keys::Keys::default())
    }

    /// Build the interface with explicit key bindings (from config.toml).
    pub fn with_keys(context: SystemContext, keys: crate::keys::Keys) -> Self {
        let mut app = Self {
            context,
            view: View::Dashboard,
            quit: false,
            modal: Modal::None,
            status: String::new(),
            filter: String::new(),
            selected: 0,
            scroll: 0,
            ports: Vec::new(),
            port_summary: None,
            processes: Vec::new(),
            process_sort: ProcessSort::Cpu,
            tree_mode: false,
            process_tree: None,
            folded: BTreeSet::new(),
            services: Vec::new(),
            interfaces: Vec::new(),
            addresses: Vec::new(),
            routes: Vec::new(),
            dns: DnsConfig::default(),
            connections: Vec::new(),
            system: None,
            memory: None,
            disks: Vec::new(),
            usage_root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            usage: Vec::new(),
            collapsed: BTreeSet::new(),
            scan: None,
            search_hits: Vec::new(),
            search_notes: Vec::new(),
            last_refresh: std::time::Instant::now(),
            cpu: 0.0,
            visible_rows: DEFAULT_VISIBLE_ROWS,
            remote_hosts: Vec::new(),
            remote_host: None,
            remote_rows: Vec::new(),
            remote_error: None,
            remote_fetch: None,
            net_top: None,
            net_top_report: None,
            net_top_previous: None,
            net_top_slot: None,
            keys,
        };
        app.load_remote_hosts();
        app.refresh();
        app
    }

    /// Build the interface with a live net-top sampler. Called by the
    /// composition root, the one place allowed to know the platform.
    pub fn with_net_top(
        context: SystemContext,
        keys: crate::keys::Keys,
        net_top: std::sync::Arc<dyn NetTopSampler>,
    ) -> Self {
        let mut app = Self::with_keys(context, keys);
        app.net_top = Some(net_top);
        app
    }

    /// Show a message in the status line (config warnings, …).
    pub fn set_status(&mut self, status: impl Into<String>) {
        self.status = status.into();
    }

    /// SSH hosts the remote page offers, from `~/.ssh/config`.
    fn load_remote_hosts(&mut self) {
        let home = ["HOME", "USERPROFILE"]
            .iter()
            .find_map(|name| std::env::var(name).ok())
            .unwrap_or_default();
        let overview = x_core::sshcfg::overview(std::path::Path::new(&home));
        self.remote_hosts = overview
            .hosts
            .iter()
            .map(|host| host.name.clone())
            .collect();
    }

    /// Hosts shown on the remote page.
    pub fn remote_hosts(&self) -> &[String] {
        &self.remote_hosts
    }

    /// Host whose snapshot is currently displayed.
    pub fn remote_host(&self) -> Option<&str> {
        self.remote_host.as_deref()
    }

    /// Snapshot rows, already rendered to text.
    pub fn remote_rows(&self) -> &[String] {
        &self.remote_rows
    }

    /// Error from the last remote fetch, if any.
    pub fn remote_error(&self) -> Option<&str> {
        self.remote_error.as_deref()
    }

    /// Whether a snapshot fetch is in flight.
    pub fn remote_fetching(&self) -> bool {
        self.remote_fetch.is_some()
    }

    /// Borrow the context, e.g. for tests.
    pub fn context(&self) -> &SystemContext {
        &self.context
    }

    /// The current page.
    pub fn view(&self) -> View {
        self.view
    }

    /// The pending dialog, if any.
    pub fn modal(&self) -> &Modal {
        &self.modal
    }

    /// The current filter text.
    pub fn filter(&self) -> &str {
        &self.filter
    }

    /// The selected row index of the current page.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Sockets in the current snapshot.
    pub fn ports(&self) -> &[PortInfo] {
        &self.ports
    }

    /// Socket counts of the last dashboard refresh.
    pub fn port_summary(&self) -> Option<&PortSummary> {
        self.port_summary.as_ref()
    }

    /// Processes in the current snapshot.
    pub fn processes(&self) -> &[ProcessInfo] {
        &self.processes
    }

    /// The process sort key in effect.
    pub fn process_sort(&self) -> ProcessSort {
        self.process_sort
    }

    /// `true` while the processes page shows a tree.
    pub fn tree_mode(&self) -> bool {
        self.tree_mode
    }

    /// The visible process rows: the flat list, or the tree flattened with
    /// folded subtrees omitted. Each row carries its depth.
    pub fn process_rows(&self) -> Vec<(usize, &ProcessInfo)> {
        if !self.tree_mode {
            return self.processes.iter().map(|process| (0, process)).collect();
        }
        let Some(tree) = &self.process_tree else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for root in &tree.roots {
            flatten_folded(root, 0, &self.folded, &mut out);
        }
        out
    }

    /// First visible row, kept in sync with the selection.
    pub fn scroll(&self) -> usize {
        self.scroll
    }

    /// `true` once the user asked to leave.
    pub fn should_quit(&self) -> bool {
        self.quit
    }

    /// The pending message, if the last action reported one.
    pub fn status(&self) -> &str {
        &self.status
    }

    /// Aggregate CPU utilization of the last refresh.
    pub fn cpu(&self) -> f32 {
        self.cpu
    }

    /// Memory utilization of the last system or dashboard refresh.
    pub fn memory_usage(&self) -> Option<&MemoryUsage> {
        self.memory.as_ref()
    }

    /// Services of the last services refresh.
    pub fn services(&self) -> &[ServiceInfo] {
        &self.services
    }

    /// Interfaces of the last network refresh.
    pub fn interfaces(&self) -> &[InterfaceInfo] {
        &self.interfaces
    }

    /// Addresses of the last network refresh.
    pub fn addresses(&self) -> &[AddressInfo] {
        &self.addresses
    }

    /// DNS configuration of the last network refresh.
    pub fn dns(&self) -> &DnsConfig {
        &self.dns
    }

    /// Routes of the last network refresh.
    pub fn routes(&self) -> &[RouteInfo] {
        &self.routes
    }

    /// Non-listening sockets of the last network refresh.
    pub fn connections(&self) -> &[PortInfo] {
        &self.connections
    }

    /// Static system facts of the last system refresh.
    pub fn system(&self) -> Option<&SystemInfo> {
        self.system.as_ref()
    }

    /// Cached global search hits.
    pub fn search_hits(&self) -> &[SearchHit] {
        &self.search_hits
    }

    /// Notes about families the search could not read.
    pub fn search_notes(&self) -> &[String] {
        &self.search_notes
    }

    /// Palette commands matching the text typed so far.
    pub fn palette_matches(&self) -> Vec<Command> {
        match &self.modal {
            Modal::Palette { input, .. } => palette::matching(input),
            _ => palette::matching(""),
        }
    }

    /// Re-read the snapshot for the visible page.
    pub fn refresh(&mut self) {
        let result = match self.view {
            View::Dashboard => self.refresh_dashboard(),
            View::Ports => self.refresh_ports(),
            View::Processes => self.refresh_processes(),
            View::Network => self.refresh_network(),
            View::Services => self.refresh_services(),
            View::System => self.refresh_system(),
            View::Disks => self.refresh_disks(),
            View::Remote => self.refresh_remote(),
            View::NetTop => self.refresh_net_top(),
        };
        if let Err(error) = result {
            self.status = error.message().to_string();
        }
        self.last_refresh = std::time::Instant::now();
    }

    /// The dashboard is the one page that summarizes several capabilities at
    /// once, so a failing one is reported but never blanks the rest.
    fn refresh_dashboard(&mut self) -> Result<()> {
        let mut errors: Vec<String> = Vec::new();
        match self.context.system.info() {
            Ok(info) => self.system = Some(info),
            Err(error) => errors.push(error.message().to_string()),
        }
        match self.context.system.cpu_usage() {
            Ok(usage) => self.cpu = usage.total_percent,
            Err(error) => errors.push(error.message().to_string()),
        }
        match self.context.system.memory_usage() {
            Ok(usage) => self.memory = Some(usage),
            Err(error) => errors.push(error.message().to_string()),
        }
        match self.context.disk.list() {
            Ok(rows) => self.disks = rows,
            Err(error) => errors.push(error.message().to_string()),
        }
        match self.context.port.list(&PortListOptions::default()) {
            Ok(rows) => self.port_summary = Some(PortSummary::from_rows(&rows)),
            Err(error) => errors.push(error.message().to_string()),
        }
        if !errors.is_empty() {
            self.status = errors.join("; ");
        }
        Ok(())
    }

    fn refresh_ports(&mut self) -> Result<()> {
        // `?` keeps the previous rows on failure: a transient error should
        // show a message, not blank the screen.
        self.ports = self.context.port.list(&PortListOptions {
            listening_only: false,
            search: self.search(),
            ..Default::default()
        })?;
        Ok(())
    }

    fn refresh_processes(&mut self) -> Result<()> {
        // Only the visible page is refreshed: sampling CPU on every process is
        // the most expensive call in the workspace.
        let options = ProcessListOptions {
            search: self.search(),
            sort: self.process_sort,
            with_usage: true,
            limit: Some(200),
            user: None,
        };
        if self.tree_mode {
            self.process_tree = Some(self.context.process.tree(&options)?);
        } else {
            self.processes = self.context.process.list(&options)?;
        }
        Ok(())
    }

    fn refresh_network(&mut self) -> Result<()> {
        self.interfaces = self.context.network.interfaces()?;
        self.addresses = self.context.network.addresses()?;
        self.routes = self.context.network.routes()?;
        self.dns = self.context.network.dns()?;
        self.connections = self.context.port.list(&PortListOptions {
            search: self.search(),
            ..Default::default()
        })?;
        // Listeners are the ports page's job; this table is about connections.
        self.connections
            .retain(|row| row.state != ConnectionState::Listen);
        Ok(())
    }

    fn refresh_services(&mut self) -> Result<()> {
        self.services = self.context.service.list(&ServiceListOptions {
            search: self.search(),
            running_only: false,
            ..Default::default()
        })?;
        Ok(())
    }

    fn refresh_system(&mut self) -> Result<()> {
        self.system = Some(self.context.system.info()?);
        self.cpu = self.context.system.cpu_usage()?.total_percent;
        self.memory = Some(self.context.system.memory_usage()?);
        Ok(())
    }

    fn refresh_disks(&mut self) -> Result<()> {
        self.disks = self.context.disk.list()?;
        self.start_scan();
        Ok(())
    }

    /// Collect a finished remote fetch, if any. The fetch itself runs on a
    /// helper thread (ssh blocks), so this only harvests the shared slot.
    fn refresh_remote(&mut self) -> Result<()> {
        if let Some(fetch) = &self.remote_fetch {
            let finished = fetch.lock().ok().and_then(|mut slot| slot.take());
            if let Some(finished) = finished {
                self.remote_fetch = None;
                match finished.outcome {
                    Ok(rows) => {
                        self.remote_rows = rows;
                        self.remote_error = None;
                        self.remote_host = Some(finished.host);
                        self.selected = 0;
                        self.scroll = 0;
                    }
                    Err(error) => {
                        self.remote_error = Some(error);
                        self.remote_rows.clear();
                    }
                }
            }
        }
        Ok(())
    }

    /// Start a snapshot fetch for the selected SSH host on a helper thread.
    ///
    /// Read-only by construction: the remote command is `x port list --json`
    /// plus `x ps list --json` — nothing on the wire can mutate either end.
    fn start_remote_fetch(&mut self) {
        if self.remote_fetch.is_some() {
            return; // one fetch at a time
        }
        let Some(host) = self
            .remote_hosts
            .get(self.selected.min(self.remote_hosts.len().saturating_sub(1)))
            .cloned()
        else {
            self.remote_error = Some("no SSH hosts found in ~/.ssh/config".to_string());
            return;
        };
        self.remote_error = None;
        self.remote_rows.clear();
        self.status = format!("fetching snapshot from {host}…");
        let slot: Arc<Mutex<Option<RemoteFetch>>> = Arc::new(Mutex::new(None));
        self.remote_fetch = Some(slot.clone());
        std::thread::spawn(move || {
            let outcome = fetch_remote_snapshot(&host);
            if let Ok(mut guard) = slot.lock() {
                *guard = Some(RemoteFetch { host, outcome });
            }
        });
    }

    /// Kick off one net-top sampling round on a helper thread.
    ///
    /// The capture window inside `sample` blocks for the whole interval, so
    /// like the disk walker it reports through a mailbox slot instead of
    /// stalling the draw loop. Sampling starts on the first visit to the
    /// page and keeps one round in flight from then on.
    fn refresh_net_top(&mut self) -> Result<()> {
        let Some(sampler) = self.net_top.as_ref() else {
            return Ok(());
        };
        if self.net_top_slot.is_some() {
            return Ok(());
        }
        let sampler = sampler.clone();
        let context = self.context.clone();
        let window = crate::refresh_interval();
        let slot: Arc<Mutex<Option<NetSnapshot>>> = Arc::new(Mutex::new(None));
        self.net_top_slot = Some(slot.clone());
        std::thread::spawn(move || {
            let snapshot = sampler.sample(&context, window);
            // A dropped mailbox only means the round is never shown; the
            // sampling thread itself cannot fail.
            if let Ok(mut guard) = slot.lock() {
                *guard = Some(snapshot);
            }
        });
        Ok(())
    }

    /// Pick up a finished sampling round and diff it against the previous
    /// one. Returns `true` when a new report is on screen.
    fn collect_net_top(&mut self) -> bool {
        let snapshot = self
            .net_top_slot
            .as_ref()
            .and_then(|slot| slot.lock().ok())
            .and_then(|mut guard| guard.take());
        if snapshot.is_some() {
            self.net_top_slot = None;
        }
        let Some(current) = snapshot else {
            return false;
        };
        let window = crate::refresh_interval();
        let previous = self.net_top_previous.take().unwrap_or_default();
        self.net_top_report = Some(diff(&previous, &current, window));
        self.net_top_previous = Some(current);
        true
    }

    /// The diffed net-top report, when a round has completed.
    pub fn net_top_report(&self) -> Option<&NetTopReport> {
        self.net_top_report.as_ref()
    }

    /// Process name for `pid` from the sampler's most recent socket read.
    pub fn net_top_process_name(&self, pid: i32) -> Option<String> {
        self.net_top.as_ref().and_then(|s| s.process_name(pid))
    }

    /// Whether a net-top sampler is wired into this session.
    pub fn net_top_available(&self) -> bool {
        self.net_top.is_some()
    }

    /// Walk the usage root once per session on a helper thread.
    ///
    /// The event loop has no async runtime by design, and a full subtree scan
    /// of a real working directory can take seconds, so the walker reports
    /// through a shared slot instead of blocking the draw.
    fn start_scan(&mut self) {
        if self.scan.is_some() || !self.usage.is_empty() {
            return;
        }
        let root = self.usage_root.clone();
        let slot = Arc::new(Mutex::new(None));
        self.scan = Some(slot.clone());
        std::thread::spawn(move || {
            let rows = x_core::walk_directory(&root, None);
            // A poisoned or dropped mailbox only means the result is never
            // collected; the walker thread itself cannot fail.
            if let Ok(mut guard) = slot.lock() {
                *guard = Some(rows);
            }
        });
    }

    /// Pick up a finished background scan, if one is waiting. Returns `true`
    /// when new rows arrived.
    fn collect_scan(&mut self) -> bool {
        let rows = self
            .scan
            .as_ref()
            .and_then(|slot| slot.lock().ok())
            .and_then(|mut guard| guard.take());
        if let Some(rows) = rows {
            self.status = format!("usage: {} directories", rows.len().saturating_sub(1));
            self.usage = rows;
            self.scan = None;
            return true;
        }
        false
    }

    /// Mounted filesystems of the last disk refresh.
    pub fn disks(&self) -> &[DiskInfo] {
        &self.disks
    }

    /// The directory that the usage scan is rooted at.
    pub fn usage_root(&self) -> &std::path::Path {
        &self.usage_root
    }

    /// `true` while the background walker is still running.
    pub fn usage_scanning(&self) -> bool {
        self.scan.is_some()
    }

    /// Point the usage tree at a fixture so tests never walk the real tree.
    #[cfg(test)]
    pub(crate) fn set_usage_fixture(&mut self, root: std::path::PathBuf, rows: Vec<DirUsage>) {
        self.usage_root = root;
        self.usage = rows;
    }

    /// The visible usage rows in tree order: the scan regrouped by parent,
    /// each level sorted by total size, subtrees of collapsed directories
    /// omitted.
    pub fn usage_tree(&self) -> Vec<&DirUsage> {
        let mut children: BTreeMap<Option<&std::path::Path>, Vec<&DirUsage>> = BTreeMap::new();
        for row in &self.usage {
            let key = if row.depth == 0 {
                None
            } else {
                row.path.parent()
            };
            children.entry(key).or_default().push(row);
        }
        for bucket in children.values_mut() {
            bucket.sort_by(|a, b| {
                b.total_bytes
                    .cmp(&a.total_bytes)
                    .then_with(|| a.path.cmp(&b.path))
            });
        }
        let mut out = Vec::new();
        let mut pending: Vec<&DirUsage> = children.get(&None).cloned().unwrap_or_default();
        while let Some(node) = pending.pop() {
            out.push(node);
            if self.collapsed.contains(&node.path) {
                continue;
            }
            if let Some(kids) = children.get(&Some(node.path.as_path())) {
                pending.extend(kids.iter().rev().copied());
            }
        }
        out
    }

    /// Show or hide the subtree under the selected usage row.
    fn toggle_collapse(&mut self) {
        let Some(path) = self
            .usage_tree()
            .get(self.selected)
            .map(|row| row.path.clone())
        else {
            return;
        };
        if !self.collapsed.remove(&path) {
            self.collapsed.insert(path);
        }
        self.selected = self.selected.min(self.row_count().saturating_sub(1));
        self.clamp_scroll();
    }

    /// Show or hide the subtree under the selected process row.
    fn toggle_process_fold(&mut self) {
        let Some(pid) = self
            .process_rows()
            .get(self.selected)
            .map(|(_, row)| row.pid)
        else {
            return;
        };
        if !self.folded.remove(&pid) {
            self.folded.insert(pid);
        }
        self.selected = self.selected.min(self.row_count().saturating_sub(1));
        self.clamp_scroll();
    }

    /// Called when the refresh interval elapsed.
    pub fn on_tick(&mut self) {
        self.collect_net_top();
        if self.collect_scan() {
            // A scan finishing while the search overlay is open refreshes its
            // file family instead of leaving a stale empty group.
            if let Modal::Search { input, .. } = &self.modal {
                let query = input.clone();
                self.run_search(&query);
            }
        }
        if self.last_refresh.elapsed() >= crate::refresh_interval() {
            self.refresh();
        }
    }

    /// Handle one key press.
    pub fn on_key(&mut self, key: KeyEvent) {
        // Dialogs swallow input while they are open.
        match std::mem::replace(&mut self.modal, Modal::None) {
            Modal::None => self.on_key_page(key),
            Modal::Prompt { label, input } => self.on_key_prompt(key, label, input),
            Modal::Confirm {
                title,
                target,
                signal,
            } => self.on_key_confirm(key, title, target, signal),
            Modal::Palette { input, selected } => self.on_key_palette(key, input, selected),
            Modal::Search { input, selected } => self.on_key_search(key, input, selected),
            Modal::Detail {
                title,
                rows,
                target,
            } => self.on_key_detail(key, title, rows, target),
        }
    }

    fn on_key_page(&mut self, key: KeyEvent) {
        // Ctrl+P works from anywhere.
        if key.code == KeyCode::Char('p') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.modal = Modal::Palette {
                input: String::new(),
                selected: 0,
            };
            return;
        }
        // Digits jump straight to a page; the sidebar shows the mapping.
        if let KeyCode::Char(digit) = key.code {
            if let Some(index) = digit.to_digit(10) {
                let index = index as usize;
                if (1..=View::ALL.len()).contains(&index) {
                    self.goto_view(View::ALL[index - 1]);
                    return;
                }
            }
        }
        match key.code {
            // On the remote page, Esc steps back from a snapshot to the host
            // list before it quits.
            KeyCode::Esc if self.view == View::Remote && self.remote_host.is_some() => {
                self.remote_host = None;
                self.remote_rows.clear();
                self.remote_error = None;
                self.remote_fetch = None;
                self.selected = 0;
                self.scroll = 0;
            }
            KeyCode::Esc => self.quit = true,
            KeyCode::Char(c) if c == self.keys.key(crate::keys::Action::Quit) => self.quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => self.quit = true,
            KeyCode::Char(c) if c == self.keys.key(crate::keys::Action::Kill) => {
                self.confirm_kill_selection()
            }
            KeyCode::Char(c) if c == self.keys.key(crate::keys::Action::Search) => {
                self.open_search()
            }
            KeyCode::Char(c) if c == self.keys.key(crate::keys::Action::Filter) => {
                self.open_filter()
            }
            KeyCode::Char(c) if c == self.keys.key(crate::keys::Action::Refresh) => self.refresh(),
            KeyCode::Tab | KeyCode::Right => self.goto_view(self.view.next()),
            KeyCode::BackTab | KeyCode::Left => self.goto_view(self.view.previous()),
            KeyCode::Char(c) if c == self.keys.key(crate::keys::Action::Tree) => self.toggle_tree(),
            KeyCode::Char(c) if c == self.keys.key(crate::keys::Action::Sort) => self.cycle_sort(),
            KeyCode::Char(c) if c == self.keys.key(crate::keys::Action::Ports) => {
                self.ports_of_selection()
            }
            KeyCode::Char(' ') => {
                if self.view == View::Processes && self.tree_mode {
                    self.toggle_process_fold();
                }
            }
            KeyCode::Char(c) if c == self.keys.key(crate::keys::Action::Top) => {
                self.selected = 0;
                self.scroll = 0;
            }
            KeyCode::Char(c) if c == self.keys.key(crate::keys::Action::Bottom) => {
                self.selected = self.row_count().saturating_sub(1);
                self.clamp_scroll();
            }
            // `j` and the arrows walk the list; killing owns the other key,
            // which is what this tool is mostly for.
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1),
            KeyCode::Up => self.move_by(-1),
            KeyCode::PageDown => self.move_by(10),
            KeyCode::PageUp => self.move_by(-10),
            KeyCode::Enter => self.activate_selection(),
            _ => {}
        }
    }

    fn on_key_prompt(&mut self, key: KeyEvent, label: &'static str, mut input: String) {
        // Enter commits, Escape abandons, everything else keeps editing: the
        // dialog is re-armed explicitly, never left to whatever was in place.
        match key.code {
            KeyCode::Enter => {
                self.filter = input;
                self.modal = Modal::None;
                self.selected = 0;
                self.scroll = 0;
                self.refresh();
            }
            KeyCode::Esc => self.modal = Modal::None,
            KeyCode::Backspace => {
                input.pop();
                self.modal = Modal::Prompt { label, input };
            }
            KeyCode::Char(c) => {
                input.push(c);
                self.modal = Modal::Prompt { label, input };
            }
            _ => self.modal = Modal::Prompt { label, input },
        }
    }

    fn on_key_confirm(&mut self, key: KeyEvent, title: String, target: Target, signal: KillSignal) {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                self.execute(target, signal)
            }
            _ => self.status = format!("aborted: {title}"),
        }
    }

    fn on_key_palette(&mut self, key: KeyEvent, mut input: String, mut selected: usize) {
        match key.code {
            KeyCode::Esc => self.modal = Modal::None,
            KeyCode::Enter => {
                let matches = palette::matching(&input);
                if let Some(command) = matches.get(selected) {
                    let id = command.id;
                    self.modal = Modal::None;
                    self.run_command(id);
                } else {
                    self.status = "no matching command".into();
                    self.modal = Modal::None;
                }
            }
            KeyCode::Down => {
                let len = palette::matching(&input).len();
                selected = (selected + 1).min(len.saturating_sub(1));
                self.modal = Modal::Palette { input, selected };
            }
            KeyCode::Up => {
                selected = selected.saturating_sub(1);
                self.modal = Modal::Palette { input, selected };
            }
            KeyCode::Backspace => {
                input.pop();
                selected = 0;
                self.modal = Modal::Palette { input, selected };
            }
            KeyCode::Char(c) => {
                input.push(c);
                selected = 0;
                self.modal = Modal::Palette { input, selected };
            }
            _ => self.modal = Modal::Palette { input, selected },
        }
    }

    fn on_key_search(&mut self, key: KeyEvent, mut input: String, mut selected: usize) {
        match key.code {
            KeyCode::Esc => {
                self.modal = Modal::None;
                self.search_hits.clear();
                self.search_notes.clear();
            }
            KeyCode::Enter => {
                let hit = self.search_hits.get(selected).cloned();
                self.modal = Modal::None;
                match hit {
                    Some(hit) => self.apply_search_hit(hit),
                    None => self.status = format!("no matches for {}", input.trim()),
                }
            }
            KeyCode::Down => {
                let len = self.search_hits.len();
                selected = (selected + 1).min(len.saturating_sub(1));
                self.modal = Modal::Search { input, selected };
            }
            KeyCode::Up => {
                selected = selected.saturating_sub(1);
                self.modal = Modal::Search { input, selected };
            }
            KeyCode::PageDown => {
                let len = self.search_hits.len();
                selected = (selected + 5).min(len.saturating_sub(1));
                self.modal = Modal::Search { input, selected };
            }
            KeyCode::PageUp => {
                selected = selected.saturating_sub(5);
                self.modal = Modal::Search { input, selected };
            }
            KeyCode::Backspace => {
                input.pop();
                self.run_search(&input);
                selected = selected.min(self.search_hits.len().saturating_sub(1));
                self.modal = Modal::Search { input, selected };
            }
            KeyCode::Char(c) => {
                input.push(c);
                self.run_search(&input);
                selected = selected.min(self.search_hits.len().saturating_sub(1));
                self.modal = Modal::Search { input, selected };
            }
            _ => self.modal = Modal::Search { input, selected },
        }
    }

    fn on_key_detail(
        &mut self,
        key: KeyEvent,
        _title: String,
        _rows: Vec<(String, String)>,
        target: Option<Target>,
    ) {
        match key.code {
            KeyCode::Char(c) if c == self.keys.key(crate::keys::Action::Kill) => match target {
                Some(target) => {
                    let confirm_title = match &target {
                        Target::Sockets(plan) => match &plan.query {
                            PortQuery::Port(port) => format!("free port {port}"),
                            PortQuery::Process(name) => format!("free ports of {name}"),
                        },
                        Target::Process { pid, name } => format!("kill {name} ({pid})"),
                    };
                    self.modal = Modal::Confirm {
                        title: confirm_title,
                        target,
                        signal: KillSignal::Terminate,
                    };
                }
                None => {
                    self.status = "nothing to kill here".into();
                    self.modal = Modal::None;
                }
            },
            // Any other key closes the snapshot.
            _ => self.modal = Modal::None,
        }
    }

    fn open_filter(&mut self) {
        self.modal = Modal::Prompt {
            label: "filter",
            input: self.filter.clone(),
        };
    }

    fn open_search(&mut self) {
        self.modal = Modal::Search {
            input: String::new(),
            selected: 0,
        };
        self.run_search("");
        // The file family searches the usage tree; opening the search makes
        // sure the walk is at least started.
        self.start_scan();
    }

    /// Recompute the search cache for `query`.
    fn run_search(&mut self, query: &str) {
        let (hits, notes) = self.compute_search(query);
        self.search_hits = hits;
        self.search_notes = notes;
    }

    /// Query every family for `query`. Unreadable families are noted instead
    /// of failing the whole search.
    fn compute_search(&self, query: &str) -> (Vec<SearchHit>, Vec<String>) {
        let mut hits = Vec::new();
        let mut notes = Vec::new();
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return (hits, notes);
        }
        match self.context.process.list(&ProcessListOptions {
            search: Some(trimmed.to_string()),
            sort: ProcessSort::Pid,
            with_usage: false,
            limit: Some(SEARCH_HITS_PER_FAMILY),
            user: None,
        }) {
            Ok(rows) => {
                for row in rows {
                    hits.push(SearchHit {
                        family: "process",
                        label: format!("{} ({})", row.name, row.pid),
                        detail: row
                            .command_line
                            .clone()
                            .or_else(|| row.user.clone())
                            .unwrap_or_else(|| format!("pid {}", row.pid)),
                        action: SearchAction::Filter {
                            view: View::Processes,
                            query: Some(trimmed.to_string()),
                        },
                    });
                }
            }
            Err(_) => notes.push("processes unreadable".into()),
        }
        match self.context.port.list(&PortListOptions {
            search: Some(trimmed.to_string()),
            limit: Some(SEARCH_HITS_PER_FAMILY),
            ..Default::default()
        }) {
            Ok(rows) => {
                for row in rows {
                    let owner = match (row.process_name.as_deref(), row.pid) {
                        (Some(name), Some(pid)) => format!("{name} (pid {pid})"),
                        (Some(name), None) => name.to_string(),
                        _ => "no owner".to_string(),
                    };
                    hits.push(SearchHit {
                        family: "port",
                        label: format!(
                            "{} {} {}",
                            row.protocol.name(),
                            row.endpoint(),
                            state_label(row.state)
                        ),
                        detail: owner,
                        action: SearchAction::Filter {
                            view: View::Ports,
                            query: Some(trimmed.to_string()),
                        },
                    });
                }
            }
            Err(_) => notes.push("ports unreadable".into()),
        }
        match self.context.service.list(&ServiceListOptions {
            search: Some(trimmed.to_string()),
            running_only: false,
            limit: Some(SEARCH_HITS_PER_FAMILY),
            ..Default::default()
        }) {
            Ok(rows) => {
                for row in rows {
                    hits.push(SearchHit {
                        family: "service",
                        label: row.name.clone(),
                        detail: match (row.pid, row.state) {
                            (Some(pid), _) => format!("pid {pid}"),
                            (None, state) => service_state_label(state).to_string(),
                        },
                        action: SearchAction::Filter {
                            view: View::Services,
                            query: Some(trimmed.to_string()),
                        },
                    });
                }
            }
            Err(_) => notes.push("services unreadable".into()),
        }
        match self.context.network.interfaces() {
            Ok(rows) => {
                let needle = trimmed.to_ascii_lowercase();
                let mut count = 0;
                for row in rows {
                    let haystack = format!(
                        "{} {} {}",
                        row.name,
                        row.description.as_deref().unwrap_or(""),
                        row.mac_address.as_deref().unwrap_or("")
                    )
                    .to_ascii_lowercase();
                    if !haystack.contains(&needle) {
                        continue;
                    }
                    hits.push(SearchHit {
                        family: "network",
                        label: row.name.clone(),
                        detail: row
                            .mac_address
                            .clone()
                            .or_else(|| row.description.clone())
                            .unwrap_or_default(),
                        action: SearchAction::Filter {
                            view: View::Network,
                            query: None,
                        },
                    });
                    count += 1;
                    if count >= SEARCH_HITS_PER_FAMILY {
                        break;
                    }
                }
            }
            Err(_) => notes.push("network unreadable".into()),
        }
        // The file family covers what this interface actually holds: the
        // usage tree of the launch directory.
        let needle = trimmed.to_ascii_lowercase();
        let mut files = 0;
        for row in &self.usage {
            if !row
                .path
                .to_string_lossy()
                .to_ascii_lowercase()
                .contains(&needle)
            {
                continue;
            }
            hits.push(SearchHit {
                family: "file",
                label: row.path.display().to_string(),
                detail: format!("{} ({} files)", format_bytes(row.total_bytes), row.files),
                action: SearchAction::Usage(row.path.clone()),
            });
            files += 1;
            if files >= SEARCH_HITS_PER_FAMILY {
                break;
            }
        }
        if files == 0 && self.usage_scanning() {
            notes.push("files still scanning".into());
        }
        (hits, notes)
    }

    fn apply_search_hit(&mut self, hit: SearchHit) {
        match hit.action {
            SearchAction::Filter { view, query } => {
                self.filter = query.unwrap_or_default();
                self.goto_view(view);
            }
            SearchAction::Usage(path) => {
                self.filter.clear();
                self.goto_view(View::Disks);
                if let Some(index) = self.usage_tree().iter().position(|row| row.path == path) {
                    self.selected = index;
                    self.clamp_scroll();
                }
                self.status = format!("file {}", path.display());
            }
        }
    }

    /// Switch pages through the same path the tabs use.
    fn goto_view(&mut self, view: View) {
        self.view = view;
        self.selected = 0;
        self.scroll = 0;
        self.refresh();
    }

    fn run_command(&mut self, id: CommandId) {
        match id {
            CommandId::Goto(view) => self.goto_view(view),
            CommandId::Refresh => self.refresh(),
            CommandId::Filter => self.open_filter(),
            CommandId::Search => self.open_search(),
            CommandId::Kill => self.confirm_kill_selection(),
            CommandId::ToggleTree => self.toggle_tree(),
            CommandId::CycleSort => self.cycle_sort(),
            CommandId::PortsOfSelection => self.ports_of_selection(),
            CommandId::Quit => self.quit = true,
        }
    }

    fn toggle_tree(&mut self) {
        self.tree_mode = !self.tree_mode;
        self.status = if self.tree_mode {
            "tree: on (space folds)".to_string()
        } else {
            "tree: off".to_string()
        };
        self.selected = 0;
        self.scroll = 0;
        self.refresh();
    }

    fn cycle_sort(&mut self) {
        let index = SORT_CYCLE
            .iter()
            .position(|sort| *sort == self.process_sort)
            .unwrap_or(0);
        self.process_sort = SORT_CYCLE[(index + 1) % SORT_CYCLE.len()];
        self.status = format!("sort: {}", sort_label(self.process_sort));
        self.refresh();
    }

    /// Jump to the ports held by the selected process, filtered to it.
    fn ports_of_selection(&mut self) {
        if self.view != View::Processes {
            self.status = "select a process first".into();
            return;
        }
        let rows = self.process_rows();
        let Some((_, row)) = rows.get(self.selected) else {
            return;
        };
        let name = row.name.clone();
        self.filter = name.clone();
        self.goto_view(View::Ports);
        self.status = format!("ports of {name}");
    }

    /// The rows of the visible page.
    pub fn rows(&self) -> usize {
        self.row_count()
    }

    fn row_count(&self) -> usize {
        match self.view {
            View::Dashboard => 0,
            View::Ports => self.ports.len(),
            View::Processes => self.process_rows().len(),
            View::Network => self.connections.len(),
            View::Services => self.services.len(),
            View::System => 0,
            View::Disks => self.usage_tree().len(),
            View::Remote => {
                // Host list until a snapshot is shown, then the snapshot rows.
                if self.remote_host.is_some() || self.remote_error.is_some() {
                    self.remote_rows.len()
                } else {
                    self.remote_hosts.len()
                }
            }
            View::NetTop => self.net_top_report.as_ref().map_or(0, |report| {
                report.processes.len()
                    + usize::from(
                        report.unmapped.rx_bps.is_some() || report.unmapped.tx_bps.is_some(),
                    )
            }),
        }
    }

    fn move_by(&mut self, delta: isize) {
        let last = self.row_count().saturating_sub(1);
        let next = self.selected as isize + delta;
        self.selected = next.clamp(0, last as isize) as usize;
        self.clamp_scroll();
    }

    /// Report how many rows the table body actually fits, and re-clamp.
    ///
    /// Drawing knows the terminal size and the keyboard does not, so the two
    /// agree through this call instead of guessing a window height.
    pub fn note_viewport(&mut self, rows: usize) {
        self.visible_rows = rows.max(1);
        self.clamp_scroll();
    }

    fn clamp_scroll(&mut self) {
        let visible = self.visible_rows;
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + visible {
            self.scroll = self.selected + 1 - visible;
        }
        if self.scroll > self.row_count().saturating_sub(1) {
            self.scroll = self.row_count().saturating_sub(1);
        }
    }

    fn search(&self) -> Option<String> {
        let trimmed = self.filter.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    }

    /// The selected socket/connection row of the current page.
    fn selected_socket(&self) -> Option<&PortInfo> {
        match self.view {
            View::Ports => self.ports.get(self.selected),
            View::Network => self.connections.get(self.selected),
            _ => None,
        }
    }

    /// Open the confirmation dialog for the selected row.
    pub fn confirm_kill_selection(&mut self) {
        match self.view {
            View::Ports | View::Network => {
                let Some(row) = self.selected_socket() else {
                    return;
                };
                let port = row.local_port;
                match self
                    .context
                    .port
                    .plan(&PortQuery::Port(port), x_core::port::PortSort::Port)
                {
                    Ok(plan) if !plan.is_empty() => {
                        self.modal = Modal::Confirm {
                            title: format!("free port {port}"),
                            target: Target::Sockets(plan),
                            signal: KillSignal::Terminate,
                        };
                    }
                    Ok(_) => self.status = format!("nothing holds port {port}"),
                    Err(error) => self.status = error.message().to_string(),
                }
            }
            View::Processes => {
                let rows = self.process_rows();
                let Some((_, row)) = rows.get(self.selected) else {
                    return;
                };
                let pid = row.pid;
                let name = row.name.clone();
                self.modal = Modal::Confirm {
                    title: format!("kill {name} ({pid})"),
                    target: Target::Process { pid, name },
                    signal: KillSignal::Terminate,
                };
            }
            _ => self.status = "nothing to kill on this page".into(),
        }
    }

    fn activate_selection(&mut self) {
        match self.view {
            View::Ports | View::Network => self.open_port_detail(),
            View::Processes => self.open_process_detail(),
            View::Services => self.open_service_detail(),
            View::Disks => self.toggle_collapse(),
            View::Remote => self.start_remote_fetch(),
            _ => {}
        }
    }

    /// Snapshot one socket — plus its owning process when the platform can
    /// read it — into the detail dialog.
    fn open_port_detail(&mut self) {
        let Some(row) = self.selected_socket().cloned() else {
            return;
        };
        let mut rows: Vec<(String, String)> = vec![
            ("port".into(), row.local_port.to_string()),
            ("proto".into(), row.protocol.name().to_string()),
            ("state".into(), state_label(row.state).to_string()),
            ("local".into(), row.endpoint()),
            (
                "remote".into(),
                row.remote_socket_addr()
                    .map(|addr| addr.to_string())
                    .unwrap_or_else(|| "-".into()),
            ),
            (
                "pid".into(),
                row.pid
                    .map(|pid| pid.to_string())
                    .unwrap_or_else(|| "-".into()),
            ),
            (
                "process".into(),
                row.process_name.clone().unwrap_or_else(|| "-".into()),
            ),
            (
                "user".into(),
                row.user.clone().unwrap_or_else(|| "-".into()),
            ),
        ];
        if let Some(bytes) = row.send_queue_bytes {
            rows.push(("send queue".into(), format_bytes(bytes)));
        }
        if let Some(bytes) = row.recv_queue_bytes {
            rows.push(("recv queue".into(), format_bytes(bytes)));
        }
        if let Some(pid) = row.pid {
            match self.context.process.get(pid) {
                Ok(info) => append_process_detail(&mut rows, &info),
                Err(error) => rows.push(("process detail".into(), error.message().to_string())),
            }
        }
        let target = match self.context.port.plan(
            &PortQuery::Port(row.local_port),
            x_core::port::PortSort::Port,
        ) {
            Ok(plan) if !plan.is_empty() => Some(Target::Sockets(plan)),
            _ => None,
        };
        self.modal = Modal::Detail {
            title: format!(
                "socket {} {} {}",
                row.protocol.name(),
                row.endpoint(),
                state_label(row.state)
            ),
            rows,
            target,
        };
    }

    fn open_process_detail(&mut self) {
        let rows = self.process_rows();
        let Some((_, row)) = rows.get(self.selected) else {
            return;
        };
        let pid = row.pid;
        let name = row.name.clone();
        match self.context.process.get(pid) {
            Ok(info) => {
                let mut rows: Vec<(String, String)> = Vec::new();
                append_process_detail(&mut rows, &info);
                self.modal = Modal::Detail {
                    title: format!("process {name} ({pid})"),
                    rows,
                    target: Some(Target::Process { pid, name }),
                };
            }
            Err(error) => self.status = error.message().to_string(),
        }
    }

    fn open_service_detail(&mut self) {
        let Some(row) = self.services.get(self.selected).cloned() else {
            return;
        };
        let rows: Vec<(String, String)> = vec![
            (
                "display".into(),
                row.display_name.clone().unwrap_or_else(|| "-".into()),
            ),
            ("state".into(), service_state_label(row.state).to_string()),
            (
                "pid".into(),
                row.pid
                    .map(|pid| pid.to_string())
                    .unwrap_or_else(|| "-".into()),
            ),
            (
                "enabled".into(),
                match row.enabled {
                    Some(true) => "yes".into(),
                    Some(false) => "no".into(),
                    None => "unknown".into(),
                },
            ),
            ("manager".into(), format!("{:?}", row.manager)),
            (
                "description".into(),
                row.description.clone().unwrap_or_else(|| "-".into()),
            ),
        ];
        self.modal = Modal::Detail {
            title: format!("service {}", row.name),
            rows,
            target: None,
        };
    }

    /// Execute exactly what the confirmation dialog showed.
    fn execute(&mut self, target: Target, signal: KillSignal) {
        let result = match target {
            Target::Sockets(plan) => self.context.port.kill_plan(&plan, signal),
            Target::Process { pid, .. } => self.context.process.kill(pid, signal).map(|()| 1),
        };
        self.report(result, "killed");
    }

    fn report(&mut self, result: Result<usize>, verb: &str) {
        self.status = match result {
            Ok(count) => format!("{verb} {count} process(es)"),
            Err(error) => format!("{verb} failed: {}", error.message()),
        };
        self.refresh();
    }
}

/// Depth-first walk that skips folded subtrees.
fn flatten_folded<'a>(
    node: &'a ProcessNode,
    depth: usize,
    folded: &BTreeSet<u32>,
    out: &mut Vec<(usize, &'a ProcessInfo)>,
) {
    out.push((depth, &node.process));
    if folded.contains(&node.process.pid) {
        return;
    }
    for child in &node.children {
        flatten_folded(child, depth + 1, folded, out);
    }
}

/// The process facts shared by the process and socket detail dialogs.
fn append_process_detail(rows: &mut Vec<(String, String)>, info: &ProcessInfo) {
    let text = |value: &Option<String>| value.clone().unwrap_or_else(|| "-".into());
    rows.push((
        "name".into(),
        if rows.is_empty() {
            info.name.clone()
        } else {
            format!("[owner] {}", info.name)
        },
    ));
    rows.push((
        "ppid".into(),
        info.parent_pid
            .map(|pid| pid.to_string())
            .unwrap_or_else(|| "-".into()),
    ));
    rows.push(("user".into(), text(&info.user)));
    rows.push(("state".into(), info.state.label().to_string()));
    rows.push((
        "cpu".into(),
        info.cpu_usage
            .map(|cpu| format!("{cpu:.1}%"))
            .unwrap_or_else(|| "-".into()),
    ));
    rows.push((
        "mem".into(),
        info.memory_bytes
            .map(format_bytes)
            .unwrap_or_else(|| "-".into()),
    ));
    rows.push((
        "threads".into(),
        info.threads
            .map(|threads| threads.to_string())
            .unwrap_or_else(|| "-".into()),
    ));
    rows.push(("exe".into(), text(&info.executable)));
    rows.push(("cwd".into(), text(&info.cwd)));
    rows.push(("command".into(), text(&info.command_line)));
    if let Some(files) = &info.open_files {
        if !files.is_empty() {
            rows.push(("open files".into(), files.len().to_string()));
            for file in files.iter().take(5) {
                rows.push((String::new(), file.clone()));
            }
        }
    }
    if let Some(connections) = &info.connections {
        if !connections.is_empty() {
            rows.push(("sockets".into(), connections.len().to_string()));
            for connection in connections.iter().take(5) {
                let target = if connection.remote.is_empty() {
                    connection.local.clone()
                } else {
                    format!("{} -> {}", connection.local, connection.remote)
                };
                rows.push((String::new(), format!("{} {target}", connection.protocol)));
            }
        }
    }
}

/// Result of an in-flight remote fetch, parked for `on_tick` to collect.
struct RemoteFetch {
    /// Host the snapshot came from.
    host: String,
    /// Rendered snapshot rows, or why the fetch failed.
    outcome: std::result::Result<Vec<String>, String>,
}

/// Pull a read-only snapshot from a remote host over SSH.
///
/// Runs the remote x in JSON mode and renders the rows locally, so the page
/// needs no protocol beyond ssh itself. Everything here is read-only: two
/// `list` invocations, nothing else.
fn fetch_remote_snapshot(host: &str) -> std::result::Result<Vec<String>, String> {
    let mut rows = vec![format!("host: {host}")];
    rows.extend(remote_list(
        host,
        &["port", "list", "--json", "--limit", "30"],
        "sockets (top 30)",
        |row| {
            let port = row.get("local_port").and_then(|v| v.as_u64()).unwrap_or(0);
            let proto = row
                .get("protocol")
                .and_then(|v| v.as_str())
                .unwrap_or("tcp");
            let state = row
                .get("state")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let process = row
                .get("process_name")
                .and_then(|v| v.as_str())
                .unwrap_or("-");
            format!("  {port:>5} {proto:<4} {state:<12} {process}")
        },
    )?);
    rows.extend(remote_list(
        host,
        &["ps", "list", "--json", "--limit", "15"],
        "processes (top 15 by cpu)",
        |row| {
            let pid = row.get("pid").and_then(|v| v.as_u64()).unwrap_or(0);
            let name = row.get("name").and_then(|v| v.as_str()).unwrap_or("?");
            let cpu = row.get("cpu_usage").and_then(|v| v.as_f64()).unwrap_or(0.0);
            format!("  {pid:>7}  {cpu:>5.1}%  {name}")
        },
    )?);
    Ok(rows)
}

/// Run one read-only `x … --json` on `host` and render each record.
fn remote_list(
    host: &str,
    args: &[&str],
    title: &str,
    render: impl Fn(&serde_json::Value) -> String,
) -> std::result::Result<Vec<String>, String> {
    let output = std::process::Command::new("ssh")
        .arg(host)
        .arg("--")
        .arg("x")
        .args(args)
        .output()
        .map_err(|e| format!("ssh failed: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!(
            "{title}: remote x failed: {}",
            if stderr.is_empty() {
                "non-zero exit"
            } else {
                &stderr
            }
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(text.trim()).map_err(|e| format!("{title}: bad JSON: {e}"))?;
    let empty = Vec::new();
    let records = parsed.as_array().unwrap_or(&empty);
    let mut rows = vec![title.to_string()];
    if records.is_empty() {
        rows.push("  (none)".to_string());
    }
    for record in records {
        rows.push(render(record));
    }
    Ok(rows)
}

#[cfg(test)]
mod tests;
