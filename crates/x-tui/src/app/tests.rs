//! Key-handling tests: pure state transitions, no terminal involved.

use super::*;
use crossterm::event::KeyEventState;
use std::net::{IpAddr, Ipv4Addr};
use x_core::error::{ErrorKind, PermissionRequirement};
use x_core::port::Protocol;
use x_core::system::{CpuUsage, OsFamily};
use x_core::testing::{stub_process, stub_service, stub_socket, StubFailure, Stubs};

use crate::palette::CommandId;

/// One key press, with the modifiers the interface looks at.
fn key(code: KeyCode) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: KeyModifiers::NONE,
        kind: crossterm::event::KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent {
        code: KeyCode::Char(c),
        modifiers: KeyModifiers::CONTROL,
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

fn app_at(stubs: &Stubs, view: View) -> App {
    let mut app = App::new(stubs.context());
    app.goto_view(view);
    app
}

/// Search tests must not spawn a walk over the real working directory.
fn quiet_search(mut app: App) -> App {
    app.usage_root = std::env::temp_dir();
    app.usage = sample_usage();
    app
}

fn established_socket(port: u16, remote_port: u16) -> PortInfo {
    PortInfo {
        protocol: Protocol::Tcp,
        local_address: IpAddr::V4(Ipv4Addr::LOCALHOST),
        local_port: port,
        remote_address: Some(IpAddr::V4(Ipv4Addr::LOCALHOST)),
        remote_port: Some(remote_port),
        state: ConnectionState::Established,
        pid: Some(42),
        process_name: Some("node".into()),
        user: None,
        path: None,
        send_queue_bytes: None,
        recv_queue_bytes: None,
    }
}

fn disk_row(mount: &str, percent: f32) -> DiskInfo {
    DiskInfo {
        mount_point: mount.to_string(),
        name: None,
        file_system: Some("ntfs".into()),
        read_only: None,
        label: None,
        volume_uuid: None,
        partition_uuid: None,
        device_model: None,
        device_serial: None,
        media_type: None,
        total_bytes: 1000,
        available_bytes: 400,
        percent,
    }
}

fn small_system() -> (SystemInfo, CpuUsage, MemoryUsage) {
    let info = SystemInfo {
        os: OsFamily::Windows,
        os_name: "TestOS".into(),
        os_version: "1.0".into(),
        arch: "x86_64".into(),
        hostname: "test-host".into(),
        cpu_count: 8,
        total_memory_bytes: 16_000,
        available_memory_bytes: 8_000,
        uptime_seconds: 3600,
        ..SystemInfo::default()
    };
    let cpu = CpuUsage {
        total_percent: 12.5,
        ..CpuUsage::default()
    };
    let memory = MemoryUsage {
        total_bytes: 16_000,
        used_bytes: 9_000,
        available_bytes: 7_000,
        percent: 56.25,
        swap_total_bytes: 0,
        swap_used_bytes: 0,
        pressure: None,
    };
    (info, cpu, memory)
}

#[test]
fn view_cycles_and_wraps_in_both_directions() {
    let (_stubs, mut app) = app_with_sockets(1);

    assert_eq!(app.view(), View::Dashboard, "the dashboard is home");
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.view(), View::Ports);
    app.on_key(key(KeyCode::BackTab));
    assert_eq!(app.view(), View::Dashboard);

    app.on_key(key(KeyCode::Left));
    assert_eq!(app.view(), View::NetTop, "left from the first page wraps");
    app.on_key(key(KeyCode::Right));
    assert_eq!(app.view(), View::Dashboard, "right from the last wraps");
}

#[test]
fn digits_jump_straight_to_a_page() {
    let (_stubs, mut app) = app_with_sockets(1);

    app.on_key(key(KeyCode::Char('5')));
    assert_eq!(app.view(), View::Services);
    app.on_key(key(KeyCode::Char('7')));
    assert_eq!(app.view(), View::Disks);
    app.on_key(key(KeyCode::Char('1')));
    assert_eq!(app.view(), View::Dashboard);
    assert_eq!(app.status(), "", "jumping is not an error");
}

#[test]
fn selection_is_clamped_to_the_last_row() {
    let (stubs, mut app) = app_with_sockets(3);
    app.goto_view(View::Ports);
    let _ = &stubs;
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
    let (stubs, mut app) = app_with_sockets(10);
    app.goto_view(View::Ports);
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
    let _ = &stubs;
}

#[test]
fn filter_prompt_filters_the_visible_page() {
    let stubs = Stubs::new()
        .with_ports(vec![
            stub_socket(8080, 42, "node"),
            stub_socket(9090, 43, "python"),
        ])
        .with_processes(vec![stub_process(1, None, "launchd")]);
    let mut app = app_at(&stubs, View::Ports);
    app.note_viewport(10);

    app.on_key(key(KeyCode::Char('f')));
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
    let (stubs, mut app) = app_with_sockets(3);
    app.goto_view(View::Ports);
    app.note_viewport(10);

    app.on_key(key(KeyCode::Char('f')));
    for event in text("80") {
        app.on_key(event);
    }
    app.on_key(key(KeyCode::Esc));

    assert!(matches!(app.modal(), Modal::None));
    assert_eq!(app.filter(), "");
    assert_eq!(app.ports().len(), 3);
    let _ = &stubs;
}

#[test]
fn killing_a_socket_needs_confirmation_and_then_runs_the_plan() {
    let (stubs, mut app) = app_with_sockets(1);
    app.goto_view(View::Ports);

    app.on_key(key(KeyCode::Char('c')));
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
    app.goto_view(View::Ports);

    app.on_key(key(KeyCode::Char('c')));
    app.on_key(key(KeyCode::Char('n')));

    assert!(stubs.port.killed().is_empty());
    assert_eq!(app.status(), "aborted: free port 8000");
    assert!(matches!(app.modal(), Modal::None));
}

#[test]
fn a_dialog_swallows_page_keys() {
    let (stubs, mut app) = app_with_sockets(2);
    app.goto_view(View::Ports);

    app.on_key(key(KeyCode::Char('c')));
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Char('j')));

    assert_eq!(app.view(), View::Ports, "tab and j went to the dialog");
    assert!(stubs.port.killed().is_empty());
    assert!(!app.should_quit(), "q inside a dialog must not quit");
}

#[test]
fn killing_a_process_confirms_the_pid_and_name() {
    let stubs = Stubs::new().with_processes(vec![stub_process(4242, Some(1), "bluecode")]);
    let mut app = app_at(&stubs, View::Processes);

    app.on_key(key(KeyCode::Char('c')));
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
    app.goto_view(View::System);

    app.on_key(key(KeyCode::Char('c')));
    assert_eq!(app.status(), "nothing to kill on this page");
    assert!(matches!(app.modal(), Modal::None));
}

#[test]
fn a_failed_refresh_reports_the_error_and_keeps_the_rows() {
    let stubs = Stubs::new().with_ports(vec![stub_socket(8080, 42, "node")]);
    let mut app = app_at(&stubs, View::Ports);
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
fn every_page_refreshes_its_own_capability() {
    let stubs = Stubs::new()
        .with_ports(vec![stub_socket(8080, 42, "node")])
        .with_processes(vec![stub_process(7, None, "node")])
        .with_services(vec![stub_service("sshd", 12)]);
    let mut app = App::new(stubs.context());

    for view in View::ALL {
        app.goto_view(view);
        app.note_viewport(10);
        let expected = match view {
            View::Dashboard => 0,
            View::Ports => 1,
            View::Processes => 1,
            // The stub's only socket listens, and listeners are filtered
            // out of the connections table.
            View::Network => 0,
            View::Services => 1,
            View::System => 0,
            // The walker thread has not reported inside the test.
            View::Disks => 0,
            // The remote page lists whatever hosts ~/.ssh/config exposes; the
            // count is environment-dependent but must match what was loaded.
            View::Remote => app.remote_hosts().len(),
            // No net-top sampler is wired into stub contexts, so the page
            // stays empty instead of sampling.
            View::NetTop => 0,
        };
        assert_eq!(app.rows(), expected, "{view:?}");
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
    app.on_key(ctrl('c'));
    assert!(app.should_quit());

    let (stubs, mut app) = app_with_sockets(1);
    app.goto_view(View::Ports);
    app.on_key(key(KeyCode::Char('c')));
    assert!(!app.should_quit(), "a bare c kills the selection instead");
    assert!(matches!(app.modal(), Modal::Confirm { .. }));
    assert!(stubs.port.killed().is_empty());
}

#[test]
fn default_keys_unchanged() {
    let (_stubs, mut app) = app_with_sockets(1);
    app.on_key(key(KeyCode::Char('r')));
    assert!(
        app.status.to_lowercase().contains("refresh")
            || app.last_refresh.elapsed() < std::time::Duration::from_secs(1)
    );

    let (_stubs, mut app) = app_with_sockets(1);
    app.on_key(key(KeyCode::Char('/')));
    assert!(matches!(app.modal(), Modal::Search { .. }));

    let (_stubs, mut app) = app_with_sockets(1);
    app.on_key(key(KeyCode::Char('f')));
    assert!(matches!(app.modal(), Modal::Prompt { .. }));
}

#[test]
fn custom_keys_take_effect_and_defaults_stop_working() {
    let keys = crate::keys::Keys::from_config(&x_core::config::Config {
        keys: vec![("key_search".to_string(), "S".to_string())],
        ..Default::default()
    })
    .0;
    let stubs = Stubs::new();
    let mut app = App::with_keys(stubs.context(), keys);
    app.usage_root = std::env::temp_dir();

    app.on_key(key(KeyCode::Char('/')));
    assert!(
        !matches!(app.modal(), Modal::Search { .. }),
        "the old key must stop working"
    );

    app.on_key(key(KeyCode::Char('S')));
    assert!(
        matches!(app.modal(), Modal::Search { .. }),
        "{}",
        app.status
    );
}

#[test]
fn the_dashboard_summarizes_cpu_memory_disks_and_sockets() {
    let (info, cpu, memory) = small_system();
    let stubs = Stubs::new()
        .with_system(info, cpu, memory)
        .with_disks(vec![disk_row("C:\\", 60.0)])
        .with_ports(vec![
            stub_socket(8080, 42, "node"),
            established_socket(51000, 443),
        ]);

    let app = App::new(stubs.context());

    assert_eq!(app.view(), View::Dashboard);
    assert_eq!(app.cpu(), 12.5);
    assert_eq!(app.memory_usage().unwrap().percent, 56.25);
    assert_eq!(app.disks().len(), 1);
    let summary = app.port_summary().expect("summary");
    assert_eq!(summary.listening, 1);
    assert_eq!(summary.established, 1);
    assert_eq!(summary.total, 2);
    assert_eq!(app.system().unwrap().hostname, "test-host");
}

#[test]
fn a_failing_dashboard_card_does_not_blank_the_others() {
    let (info, cpu, memory) = small_system();
    let stubs = Stubs::new().with_system(info, cpu, memory);
    stubs
        .port
        .fail_with(StubFailure::new(ErrorKind::System, "socket table exploded"));

    let app = App::new(stubs.context());

    assert!(app.system().is_some(), "the facts card survives");
    assert!(app.port_summary().is_none());
    assert!(
        app.status().contains("socket table exploded"),
        "status: {}",
        app.status()
    );
}

#[test]
fn the_palette_opens_filters_and_runs_a_command() {
    let stubs = Stubs::new().with_services(vec![stub_service("sshd", 12)]);
    let mut app = app_at(&stubs, View::Dashboard);

    app.on_key(ctrl('p'));
    assert!(matches!(app.modal(), Modal::Palette { .. }));

    for event in text("serv") {
        app.on_key(event);
    }
    let matches = app.palette_matches();
    assert_eq!(matches.len(), 1, "{matches:?}");
    assert_eq!(matches[0].id, CommandId::Goto(View::Services));

    app.on_key(key(KeyCode::Enter));
    assert!(matches!(app.modal(), Modal::None));
    assert_eq!(app.view(), View::Services);
    assert_eq!(app.services().len(), 1, "the command refreshed the page");
}

#[test]
fn the_palette_closes_cleanly_without_a_match() {
    let (_stubs, mut app) = app_with_sockets(1);

    app.on_key(ctrl('p'));
    for event in text("zzz") {
        app.on_key(event);
    }
    app.on_key(key(KeyCode::Enter));
    assert!(matches!(app.modal(), Modal::None));
    assert_eq!(app.status(), "no matching command");

    app.on_key(ctrl('p'));
    app.on_key(key(KeyCode::Esc));
    assert!(matches!(app.modal(), Modal::None));
}

#[test]
fn the_global_search_finds_processes_and_jumps_with_the_filter() {
    let stubs = Stubs::new()
        .with_ports(vec![
            stub_socket(8080, 42, "node"),
            stub_socket(9090, 43, "python"),
        ])
        .with_processes(vec![stub_process(42, None, "node")]);
    let mut app = quiet_search(app_at(&stubs, View::Dashboard));

    app.on_key(key(KeyCode::Char('/')));
    assert!(matches!(app.modal(), Modal::Search { .. }));
    for event in text("node") {
        app.on_key(event);
    }
    let families: Vec<&str> = app.search_hits().iter().map(|hit| hit.family).collect();
    assert!(families.contains(&"process"), "{families:?}");
    assert!(families.contains(&"port"), "{families:?}");

    app.on_key(key(KeyCode::Enter));
    assert!(matches!(app.modal(), Modal::None));
    assert_eq!(app.view(), View::Processes);
    assert_eq!(app.filter(), "node");
    assert_eq!(app.processes().len(), 1);
}

#[test]
fn a_port_hit_lands_on_the_ports_page_with_the_query_kept() {
    let stubs = Stubs::new()
        .with_ports(vec![stub_socket(8080, 42, "node")])
        .with_processes(vec![stub_process(42, None, "node")]);
    let mut app = quiet_search(app_at(&stubs, View::Dashboard));

    app.on_key(key(KeyCode::Char('/')));
    for event in text("8080") {
        app.on_key(event);
    }
    // Only the socket matches "8080"; the process name does not.
    assert_eq!(app.search_hits().len(), 1);
    assert_eq!(app.search_hits()[0].family, "port");

    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.view(), View::Ports);
    assert_eq!(app.filter(), "8080");
    assert_eq!(app.ports().len(), 1);
}

#[test]
fn the_search_notes_families_it_cannot_read() {
    let stubs = Stubs::new().with_processes(vec![stub_process(42, None, "node")]);
    stubs
        .port
        .fail_with(StubFailure::new(ErrorKind::System, "socket table exploded"));
    let mut app = quiet_search(app_at(&stubs, View::Dashboard));

    app.on_key(key(KeyCode::Char('/')));
    for event in text("node") {
        app.on_key(event);
    }
    assert!(
        app.search_notes().iter().any(|note| note.contains("ports")),
        "notes: {:?}",
        app.search_notes()
    );
    assert_eq!(app.search_hits()[0].family, "process");

    // Escape closes and clears the cache.
    app.on_key(key(KeyCode::Esc));
    assert!(matches!(app.modal(), Modal::None));
    assert!(app.search_hits().is_empty());
}

#[test]
fn a_file_hit_selects_the_usage_row() {
    let (_stubs, mut app) = app_with_sockets(1);
    app.goto_view(View::Disks);
    app.usage = sample_usage();

    app.on_key(key(KeyCode::Char('/')));
    for event in text("big") {
        app.on_key(event);
    }
    let file_hits: Vec<&SearchHit> = app
        .search_hits()
        .iter()
        .filter(|hit| hit.family == "file")
        .collect();
    assert!(!file_hits.is_empty(), "the usage tree is searchable");

    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.view(), View::Disks);
    let tree = app.usage_tree();
    assert_eq!(
        tree[app.selected()].path,
        PathBuf::from("r").join("big"),
        "the search landed on the matched directory"
    );
}

#[test]
fn the_process_tree_folds_and_unfolds() {
    let stubs = Stubs::new().with_processes(vec![
        stub_process(1, None, "init"),
        stub_process(2, Some(1), "worker"),
        stub_process(3, Some(2), "child"),
    ]);
    let mut app = app_at(&stubs, View::Processes);

    app.on_key(key(KeyCode::Char('t')));
    assert!(app.tree_mode());
    let depths: Vec<usize> = app.process_rows().iter().map(|(depth, _)| *depth).collect();
    assert_eq!(depths, [0, 1, 2]);
    assert_eq!(app.rows(), 3);

    // Fold the worker: the child disappears.
    app.selected = 1;
    app.on_key(key(KeyCode::Char(' ')));
    assert_eq!(app.rows(), 2);
    let names: Vec<&str> = app
        .process_rows()
        .iter()
        .map(|(_, row)| row.name.as_str())
        .collect();
    assert_eq!(names, ["init", "worker"]);

    app.on_key(key(KeyCode::Char(' ')));
    assert_eq!(app.rows(), 3, "unfolding brings the child back");

    app.on_key(key(KeyCode::Char('t')));
    assert!(!app.tree_mode(), "t toggles the tree off again");
}

#[test]
fn the_sort_key_cycles_through_every_order() {
    let stubs = Stubs::new().with_processes(vec![stub_process(7, None, "node")]);
    let mut app = app_at(&stubs, View::Processes);

    assert_eq!(app.process_sort(), ProcessSort::Cpu);
    app.on_key(key(KeyCode::Char('s')));
    assert_eq!(app.process_sort(), ProcessSort::Memory);
    assert_eq!(app.status(), "sort: memory");
    for _ in 0..4 {
        app.on_key(key(KeyCode::Char('s')));
    }
    assert_eq!(app.process_sort(), ProcessSort::Cpu, "the cycle wraps");
}

#[test]
fn the_socket_detail_shows_the_owner_and_keeps_the_kill_target() {
    let stubs = Stubs::new()
        .with_ports(vec![stub_socket(8080, 42, "node")])
        .with_processes(vec![stub_process(42, None, "node")]);
    let mut app = app_at(&stubs, View::Ports);

    app.on_key(key(KeyCode::Enter));
    let Modal::Detail {
        title,
        rows,
        target,
    } = app.modal()
    else {
        panic!("expected the detail dialog, got {:?}", app.modal());
    };
    assert!(title.contains("8080"), "{title}");
    assert!(
        rows.iter()
            .any(|(key, value)| key == "command" && value == "-"),
        "the stub process has no command line: {rows:?}"
    );
    assert!(target.is_some(), "the socket can be freed");

    app.on_key(key(KeyCode::Char('c')));
    assert!(matches!(app.modal(), Modal::Confirm { .. }));
    app.on_key(key(KeyCode::Char('y')));
    assert_eq!(stubs.port.killed(), vec![42]);
}

#[test]
fn the_process_detail_kill_flow_reaches_the_same_target() {
    let stubs = Stubs::new().with_processes(vec![stub_process(4242, Some(1), "bluecode")]);
    let mut app = app_at(&stubs, View::Processes);

    app.on_key(key(KeyCode::Enter));
    let Modal::Detail { target, .. } = app.modal() else {
        panic!("expected the detail dialog");
    };
    assert_eq!(
        *target,
        Some(Target::Process {
            pid: 4242,
            name: "bluecode".into()
        })
    );

    app.on_key(key(KeyCode::Char('c')));
    app.on_key(key(KeyCode::Char('y')));
    assert_eq!(stubs.process.killed(), vec![4242]);
}

#[test]
fn the_service_detail_is_read_only() {
    let stubs = Stubs::new().with_services(vec![stub_service("sshd", 12)]);
    let mut app = app_at(&stubs, View::Services);

    app.on_key(key(KeyCode::Enter));
    let Modal::Detail { title, target, .. } = app.modal() else {
        panic!("expected the detail dialog");
    };
    assert_eq!(title, "service sshd");
    assert!(target.is_none());

    app.on_key(key(KeyCode::Char('c')));
    assert_eq!(app.status(), "nothing to kill here");
    assert!(matches!(app.modal(), Modal::None));
}

#[test]
fn the_network_page_shows_connections_but_not_listeners() {
    let stubs = Stubs::new().with_ports(vec![
        stub_socket(8080, 42, "node"),
        established_socket(51000, 443),
    ]);
    let mut app = app_at(&stubs, View::Network);

    assert_eq!(app.connections().len(), 1);
    assert_eq!(app.connections()[0].local_port, 51000);

    app.on_key(key(KeyCode::Enter));
    assert!(matches!(app.modal(), Modal::Detail { .. }));
    app.on_key(key(KeyCode::Esc));
    assert!(matches!(app.modal(), Modal::None));
}

#[test]
fn selecting_a_process_can_jump_to_its_ports() {
    let stubs = Stubs::new()
        .with_ports(vec![
            stub_socket(8080, 42, "node"),
            stub_socket(9090, 43, "python"),
        ])
        .with_processes(vec![stub_process(42, None, "node")]);
    let mut app = app_at(&stubs, View::Processes);

    app.on_key(key(KeyCode::Char('p')));
    assert_eq!(app.view(), View::Ports);
    assert_eq!(app.filter(), "node");
    assert_eq!(app.ports().len(), 1);
    assert_eq!(app.ports()[0].local_port, 8080);
    assert_eq!(app.status(), "ports of node");
}

fn dir_usage(path: &str, depth: usize, total_bytes: u64) -> DirUsage {
    DirUsage {
        path: PathBuf::from(path),
        depth,
        total_bytes,
        files: 1,
        dirs: 0,
        unreadable: 0,
    }
}

fn sample_usage() -> Vec<DirUsage> {
    let root = PathBuf::from("r");
    vec![
        dir_usage(".", 0, 100),
        dir_usage("big", 1, 60),
        dir_usage("small", 1, 30),
        dir_usage("big/inner", 2, 50),
    ]
    .into_iter()
    .map(|mut row| {
        row.path = if row.depth == 0 {
            root.clone()
        } else {
            root.join(row.path)
        };
        row
    })
    .collect()
}

#[test]
fn the_usage_tree_orders_children_by_size_then_collapses() {
    let (_stubs, mut app) = app_with_sockets(1);
    app.view = View::Disks;
    app.usage = sample_usage();

    let names: Vec<String> = app
        .usage_tree()
        .iter()
        .map(|row| row.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["r", "big", "inner", "small"]);

    // Enter on `big` hides its subtree; Enter again brings it back.
    app.selected = 1;
    app.on_key(key(KeyCode::Enter));
    let names: Vec<String> = app
        .usage_tree()
        .iter()
        .map(|row| row.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["r", "big", "small"]);

    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.usage_tree().len(), 4);
}

#[test]
fn a_finished_background_scan_is_collected_on_the_next_tick() {
    let (_stubs, mut app) = app_with_sockets(1);
    app.view = View::Disks;
    let rows = sample_usage();
    app.scan = Some(Arc::new(Mutex::new(Some(rows))));

    app.on_tick();

    assert_eq!(app.usage.len(), 4);
    assert!(app.scan.is_none());
    assert!(app.status().starts_with("usage:"), "{}", app.status());
    assert!(!app.usage_scanning());
}

#[test]
fn collapsing_a_parent_of_the_selection_keeps_the_selection_in_range() {
    let (_stubs, mut app) = app_with_sockets(1);
    app.view = View::Disks;
    app.usage = sample_usage();

    app.selected = 1;
    app.on_key(key(KeyCode::Enter)); // collapse `big`, hiding `inner`
    assert_eq!(app.rows(), 3);
    assert!(app.selected < app.rows(), "selection stays inside the tree");
}
