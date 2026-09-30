//! Interface state: what is shown, what is selected, and what is pending.
//!
//! All logic here is pure with respect to the terminal, which is what makes the
//! key handling testable without a tty.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use x_core::error::Result;
use x_core::network::{DnsConfig, InterfaceInfo, RouteInfo};
use x_core::port::{KillPlan, PortInfo, PortListOptions, PortQuery};
use x_core::process::{ProcessInfo, ProcessListOptions, ProcessSort};
use x_core::service::{ServiceInfo, ServiceListOptions};
use x_core::system::SystemInfo;
use x_core::{KillSignal, SystemContext};

/// Rows assumed before the first draw, when the real height is still unknown.
const DEFAULT_VISIBLE_ROWS: usize = 20;

/// The pages of the interface, in tab order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// Sockets and their owners.
    Ports,
    /// Processes.
    Processes,
    /// Network configuration.
    Network,
    /// System health.
    System,
}

impl View {
    /// Every view, for tab cycling.
    pub const ALL: [View; 4] = [View::Ports, View::Processes, View::Network, View::System];

    /// Tab label.
    pub fn title(self) -> &'static str {
        match self {
            Self::Ports => "ports",
            Self::Processes => "processes",
            Self::Network => "network",
            Self::System => "system",
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
    processes: Vec<ProcessInfo>,
    services: Vec<ServiceInfo>,
    interfaces: Vec<InterfaceInfo>,
    routes: Vec<RouteInfo>,
    dns: DnsConfig,
    system: Option<SystemInfo>,

    last_refresh: std::time::Instant,
    cpu: f32,
    /// How many rows the table body currently fits, updated while drawing.
    visible_rows: usize,
}

impl App {
    /// Build the interface and take the first snapshot.
    ///
    /// The snapshot happens here so the first drawn frame is never empty, and so
    /// tests can assert on state without calling anything.
    pub fn new(context: SystemContext) -> Self {
        let mut app = Self {
            context,
            view: View::Ports,
            quit: false,
            modal: Modal::None,
            status: String::new(),
            filter: String::new(),
            selected: 0,
            scroll: 0,
            ports: Vec::new(),
            processes: Vec::new(),
            services: Vec::new(),
            interfaces: Vec::new(),
            routes: Vec::new(),
            dns: DnsConfig::default(),
            system: None,
            last_refresh: std::time::Instant::now(),
            cpu: 0.0,
            visible_rows: DEFAULT_VISIBLE_ROWS,
        };
        app.refresh();
        app
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

    /// Processes in the current snapshot.
    pub fn processes(&self) -> &[ProcessInfo] {
        &self.processes
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

    /// Aggregate CPU utilization of the last system refresh.
    pub fn cpu(&self) -> f32 {
        self.cpu
    }

    /// Live memory utilization, read on demand because it is cheap.
    pub fn memory_usage(&self) -> Option<x_core::system::MemoryUsage> {
        self.context.system.memory_usage().ok()
    }

    /// Services of the last system refresh.
    pub fn services(&self) -> &[ServiceInfo] {
        &self.services
    }

    /// Interfaces of the last network refresh.
    pub fn interfaces(&self) -> &[x_core::network::InterfaceInfo] {
        &self.interfaces
    }

    /// DNS configuration of the last network refresh.
    pub fn dns(&self) -> &DnsConfig {
        &self.dns
    }

    /// Routes of the last network refresh.
    pub fn routes(&self) -> &[RouteInfo] {
        &self.routes
    }

    /// Static system facts of the last system refresh.
    pub fn system(&self) -> Option<&SystemInfo> {
        self.system.as_ref()
    }

    /// Re-read the snapshot for the visible page.
    pub fn refresh(&mut self) {
        let result = match self.view {
            View::Ports => self.refresh_ports(),
            View::Processes => self.refresh_processes(),
            View::Network => self.refresh_network(),
            View::System => self.refresh_system(),
        };
        if let Err(error) = result {
            self.status = error.message().to_string();
        }
        self.last_refresh = std::time::Instant::now();
    }

    fn refresh_ports(&mut self) -> Result<()> {
        // `?` keeps the previous rows on failure: a transient error should
        // show a message, not blank the screen.
        self.ports = self.context.port.list(&PortListOptions {
            listening_only: true,
            search: self.search(),
            ..Default::default()
        })?;
        Ok(())
    }

    fn refresh_processes(&mut self) -> Result<()> {
        // Only the visible page is refreshed: sampling CPU on every process is
        // the most expensive call in the workspace.
        self.processes = self.context.process.list(&ProcessListOptions {
            search: self.search(),
            sort: ProcessSort::Cpu,
            with_usage: true,
            limit: Some(200),
            user: None,
        })?;
        Ok(())
    }

    fn refresh_network(&mut self) -> Result<()> {
        self.interfaces = self.context.network.interfaces()?;
        self.routes = self.context.network.routes()?;
        self.dns = self.context.network.dns()?;
        self.services.clear();
        Ok(())
    }

    fn refresh_system(&mut self) -> Result<()> {
        self.system = Some(self.context.system.info()?);
        self.cpu = self.context.system.cpu_usage()?.total_percent;
        self.services = self.context.service.list(&ServiceListOptions {
            search: self.search(),
            running_only: false,
            ..Default::default()
        })?;
        Ok(())
    }

    /// Called when the refresh interval elapsed.
    pub fn on_tick(&mut self) {
        if self.last_refresh.elapsed() >= crate::REFRESH {
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
        }
    }

    fn on_key_page(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => self.quit = true,
            KeyCode::Char('c') => self.confirm_kill_selection(),
            KeyCode::Char('k') => self.confirm_kill_selection(),
            KeyCode::Char('/') => {
                self.modal = Modal::Prompt {
                    label: "filter",
                    input: self.filter.clone(),
                }
            }
            KeyCode::Char('r') => self.refresh(),
            KeyCode::Tab | KeyCode::Right => {
                self.view = self.view.next();
                self.selected = 0;
                self.scroll = 0;
                self.refresh();
            }
            KeyCode::BackTab | KeyCode::Left => {
                self.view = self.view.previous();
                self.selected = 0;
                self.scroll = 0;
                self.refresh();
            }
            KeyCode::Char('g') | KeyCode::Home => {
                self.selected = 0;
                self.scroll = 0;
            }
            KeyCode::Char('G') | KeyCode::End => {
                self.selected = self.row_count().saturating_sub(1);
                self.clamp_scroll();
            }
            // `j` and the arrows walk the list; `k` is reserved for killing,
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

    /// The rows of the visible page.
    pub fn rows(&self) -> usize {
        self.row_count()
    }

    fn row_count(&self) -> usize {
        match self.view {
            View::Ports => self.ports.len(),
            View::Processes => self.processes.len(),
            View::Network => self.interfaces.len(),
            View::System => self.services.len(),
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

    /// Open the confirmation dialog for the selected row.
    pub fn confirm_kill_selection(&mut self) {
        match self.view {
            View::Ports => {
                let Some(row) = self.ports.get(self.selected) else {
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
                let Some(row) = self.processes.get(self.selected) else {
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
            View::Ports | View::Processes => self.confirm_kill_selection(),
            _ => {}
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventState;
    use x_core::error::PermissionRequirement;
    use x_core::testing::{stub_process, stub_service, stub_socket, StubFailure, Stubs};

    /// One key press, with the modifiers the interface looks at.
    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: crossterm::event::KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn text(text: &str) -> Vec<KeyEvent> {
        text.chars().map(|c| key(KeyCode::Char(c))).collect()
    }

    fn app_with_sockets(count: u16) -> (Stubs, App) {
        let stubs = Stubs::new().with_ports(
            (0..count)
                .map(|index| stub_socket(8000 + index, 100 + u32::from(index), "node"))
                .collect(),
        );
        let app = App::new(stubs.context());
        (stubs, app)
    }

    #[test]
    fn view_cycles_and_wraps_in_both_directions() {
        let (_stubs, mut app) = app_with_sockets(1);

        assert_eq!(app.view(), View::Ports);
        app.on_key(key(KeyCode::Tab));
        assert_eq!(app.view(), View::Processes);
        app.on_key(key(KeyCode::BackTab));
        assert_eq!(app.view(), View::Ports);

        app.on_key(key(KeyCode::Left));
        assert_eq!(app.view(), View::System, "left from the first tab wraps");
        app.on_key(key(KeyCode::Right));
        assert_eq!(app.view(), View::Ports, "right from the last tab wraps");
    }

    #[test]
    fn selection_is_clamped_to_the_last_row() {
        let (_stubs, mut app) = app_with_sockets(3);
        assert_eq!(app.rows(), 3);

        for _ in 0..10 {
            app.on_key(key(KeyCode::Down));
        }
        assert_eq!(app.selected(), 2);

        for _ in 0..10 {
            app.on_key(key(KeyCode::Up));
        }
        assert_eq!(app.selected(), 0);
    }

    #[test]
    fn scrolling_follows_the_selection_inside_the_reported_viewport() {
        let (_stubs, mut app) = app_with_sockets(10);
        app.note_viewport(3);

        app.on_key(key(KeyCode::Down));
        assert_eq!(app.scroll(), 0, "first move stays inside the window");
        app.on_key(key(KeyCode::Down));
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.selected(), 3);
        assert_eq!(
            app.scroll(),
            1,
            "the window scrolls once to keep row 3 visible"
        );

        app.on_key(key(KeyCode::Up));
        app.on_key(key(KeyCode::Up));
        app.on_key(key(KeyCode::Up));
        assert_eq!(app.scroll(), 0, "scrolling back does not leave a gap above");
    }

    #[test]
    fn filter_prompt_filters_the_visible_page() {
        let stubs = Stubs::new()
            .with_ports(vec![
                stub_socket(8080, 42, "node"),
                stub_socket(9090, 43, "python"),
            ])
            .with_processes(vec![stub_process(1, None, "launchd")]);
        let mut app = App::new(stubs.context());
        app.note_viewport(10);

        app.on_key(key(KeyCode::Char('/')));
        assert!(matches!(app.modal(), Modal::Prompt { .. }));
        for event in text("8080") {
            app.on_key(event);
        }
        app.on_key(key(KeyCode::Enter));

        assert_eq!(app.filter(), "8080");
        assert!(matches!(app.modal(), Modal::None));
        assert_eq!(app.view(), View::Ports);
        assert_eq!(app.ports().len(), 1);
        assert_eq!(app.ports()[0].local_port, 8080);
    }

    #[test]
    fn escape_cancels_the_filter_prompt_without_applying_it() {
        let (_stubs, mut app) = app_with_sockets(3);

        app.on_key(key(KeyCode::Char('/')));
        for event in text("80") {
            app.on_key(event);
        }
        app.on_key(key(KeyCode::Esc));

        assert!(matches!(app.modal(), Modal::None));
        assert_eq!(app.filter(), "");
        assert_eq!(app.ports().len(), 3);
    }

    #[test]
    fn killing_a_socket_needs_confirmation_and_then_runs_the_plan() {
        let (stubs, mut app) = app_with_sockets(1);

        app.on_key(key(KeyCode::Char('k')));
        let modal = app.modal().clone();
        let Modal::Confirm {
            title,
            target,
            signal,
        } = modal
        else {
            panic!("expected a confirmation dialog, got {modal:?}");
        };
        assert_eq!(title, "free port 8000");
        assert_eq!(signal, KillSignal::Terminate);
        assert_eq!(
            target,
            Target::Sockets(KillPlan::new(
                PortQuery::Port(8000),
                vec![stub_socket(8000, 100, "node")]
            ))
        );
        assert!(
            stubs.port.killed().is_empty(),
            "nothing is killed before confirming"
        );

        app.on_key(key(KeyCode::Char('y')));
        assert_eq!(stubs.port.killed(), vec![100]);
        assert!(matches!(app.modal(), Modal::None));
        assert_eq!(app.status(), "killed 1 process(es)");
    }

    #[test]
    fn declining_the_dialog_kills_nothing() {
        let (stubs, mut app) = app_with_sockets(1);

        app.on_key(key(KeyCode::Char('k')));
        app.on_key(key(KeyCode::Char('n')));

        assert!(stubs.port.killed().is_empty());
        assert_eq!(app.status(), "aborted: free port 8000");
        assert!(matches!(app.modal(), Modal::None));
    }

    #[test]
    fn a_dialog_swallows_page_keys() {
        let (stubs, mut app) = app_with_sockets(2);

        app.on_key(key(KeyCode::Char('k')));
        app.on_key(key(KeyCode::Tab));
        app.on_key(key(KeyCode::Char('j')));

        assert_eq!(app.view(), View::Ports, "tab and j went to the dialog");
        assert!(stubs.port.killed().is_empty());
        assert!(!app.should_quit(), "q inside a dialog must not quit");
    }

    #[test]
    fn killing_a_process_confirms_the_pid_and_name() {
        let stubs = Stubs::new().with_processes(vec![stub_process(4242, Some(1), "bluecode")]);
        let mut app = App::new(stubs.context());
        app.on_key(key(KeyCode::Tab));

        app.on_key(key(KeyCode::Enter));
        let Modal::Confirm { target, .. } = app.modal() else {
            panic!("expected a confirmation dialog");
        };
        assert_eq!(
            *target,
            Target::Process {
                pid: 4242,
                name: "bluecode".into()
            }
        );

        app.on_key(key(KeyCode::Enter));
        assert_eq!(stubs.process.killed(), vec![4242]);
    }

    #[test]
    fn pages_without_something_to_kill_say_so() {
        let (_stubs, mut app) = app_with_sockets(1);

        for _ in 0..2 {
            app.on_key(key(KeyCode::Tab));
        }
        assert_eq!(app.view(), View::Network);
        app.on_key(key(KeyCode::Char('k')));
        assert_eq!(app.status(), "nothing to kill on this page");
        assert!(matches!(app.modal(), Modal::None));
    }

    #[test]
    fn a_failed_refresh_reports_the_error_and_keeps_the_rows() {
        let stubs = Stubs::new().with_ports(vec![stub_socket(8080, 42, "node")]);
        let mut app = App::new(stubs.context());
        assert_eq!(app.ports().len(), 1);

        stubs.port.fail_with(StubFailure::denied(
            PermissionRequirement::Root,
            "must be root",
        ));
        app.refresh();

        assert_eq!(app.status(), "must be root");
        assert_eq!(
            app.ports().len(),
            1,
            "the last good snapshot stays on screen"
        );
    }

    #[test]
    fn every_tab_refreshes_its_own_capability() {
        let stubs = Stubs::new()
            .with_ports(vec![stub_socket(8080, 42, "node")])
            .with_processes(vec![stub_process(7, None, "node")])
            .with_services(vec![stub_service("sshd", 12)]);
        let mut app = App::new(stubs.context());

        for (index, _) in View::ALL.iter().enumerate() {
            app.on_key(key(KeyCode::Tab));
            let view = View::ALL[(index + 1) % View::ALL.len()];
            assert_eq!(app.view(), view);
            app.note_viewport(10);
            assert_eq!(
                app.rows(),
                match view {
                    View::Ports => 1,
                    View::Processes => 1,
                    View::Network => 0,
                    View::System => 1,
                }
            );
        }
        assert_eq!(app.services()[0].name, "sshd");
    }

    #[test]
    fn quit_keys_are_q_escape_and_control_c_but_not_a_bare_c() {
        for code in [KeyCode::Char('q'), KeyCode::Esc] {
            let (_stubs, mut app) = app_with_sockets(1);
            app.on_key(key(code));
            assert!(app.should_quit(), "{code:?} should quit");
        }

        let (_stubs, mut app) = app_with_sockets(1);
        app.on_key(KeyEvent {
            code: KeyCode::Char('c'),
            modifiers: KeyModifiers::CONTROL,
            kind: crossterm::event::KeyEventKind::Press,
            state: KeyEventState::NONE,
        });
        assert!(app.should_quit());

        let (stubs, mut app) = app_with_sockets(1);
        app.on_key(key(KeyCode::Char('c')));
        assert!(!app.should_quit(), "a bare c kills the selection instead");
        assert!(matches!(app.modal(), Modal::Confirm { .. }));
        assert!(stubs.port.killed().is_empty());
    }
}
