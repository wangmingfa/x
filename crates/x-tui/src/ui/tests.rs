//! Render tests: pages and dialogs drawn into an in-memory buffer, so the
//! drawing code is exercised for real without a terminal.

use super::*;
use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyEventState, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use x_core::disk::{DirUsage, DiskInfo};
use x_core::port::{ConnectionState, PortInfo, Protocol};
use x_core::system::{CpuUsage, MemoryUsage, OsFamily, SystemInfo};
use x_core::testing::{stub_process, stub_service, stub_socket, Stubs};

use crate::app::{App, View};

/// Draw one frame at `width`x`height` and return the screen, trailing
/// whitespace trimmed per line so assertions can span row boundaries.
fn render(app: &mut App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test backend");
    terminal.draw(|frame| draw(frame, app)).expect("draw frame");
    let buffer = terminal.backend().buffer();
    let mut screen = String::new();
    for y in 0..buffer.area.height {
        let mut line = String::new();
        for x in 0..buffer.area.width {
            line.push_str(buffer.cell((x, y)).map(|cell| cell.symbol()).unwrap_or(" "));
        }
        screen.push_str(line.trim_end());
        screen.push('\n');
    }
    screen
}

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

fn press(app: &mut App, code: KeyCode) {
    app.on_key(key(code));
}

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
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
    vec![
        dir_usage("r", 0, 100),
        dir_usage("r/big", 1, 60),
        dir_usage("r/small", 1, 30),
    ]
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

/// A stub socket that also reports kernel queue lengths (Linux shape).
fn queue_socket(port: u16, pid: u32, name: &str, send: u64, recv: u64) -> PortInfo {
    let mut row = stub_socket(port, pid, name);
    row.send_queue_bytes = Some(send);
    row.recv_queue_bytes = Some(recv);
    row
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
fn every_page_draws_with_its_own_sidebar_row_highlighted() {
    let stubs = Stubs::new()
        .with_ports(vec![stub_socket(8080, 42, "node")])
        .with_processes(vec![stub_process(10, None, "init")])
        .with_services(vec![stub_service("sshd", 12)]);
    let mut app = App::new(stubs.context());
    app.set_usage_fixture(PathBuf::from("r"), sample_usage());

    for (index, view) in View::ALL.iter().enumerate() {
        let digit = char::from_digit(index as u32 + 1, 10).expect("digit");
        press(&mut app, KeyCode::Char(digit));
        assert_eq!(app.view(), *view);

        let screen = render(&mut app, 100, 30);
        assert!(
            screen.contains(&format!("> {} {}", index + 1, view.title())),
            "sidebar row for {view:?} is not highlighted:\n{screen}"
        );
    }
}

#[test]
fn each_page_draws_its_own_body() {
    let (info, cpu, memory) = small_system();
    let stubs = Stubs::new()
        .with_ports(vec![stub_socket(8080, 42, "node")])
        .with_processes(vec![stub_process(10, None, "init")])
        .with_services(vec![stub_service("sshd", 12)])
        .with_system(info, cpu, memory)
        .with_disks(vec![disk_row("C:\\", 90.0)]);
    let mut app = App::new(stubs.context());
    app.set_usage_fixture(PathBuf::from("r"), sample_usage());
    flush_disks(&mut app);

    press(&mut app, KeyCode::Char('2'));
    let screen = render(&mut app, 100, 30);
    assert!(screen.contains("sockets"), "ports body missing:\n{screen}");

    press(&mut app, KeyCode::Char('3'));
    let screen = render(&mut app, 100, 30);
    assert!(
        screen.contains("processes | sort cpu"),
        "processes body missing:\n{screen}"
    );

    press(&mut app, KeyCode::Char('4'));
    let screen = render(&mut app, 100, 30);
    assert!(
        screen.contains("interfaces"),
        "network body missing:\n{screen}"
    );

    press(&mut app, KeyCode::Char('5'));
    let screen = render(&mut app, 100, 30);
    assert!(
        screen.contains("display name") && screen.contains("sshd"),
        "services body missing:\n{screen}"
    );

    press(&mut app, KeyCode::Char('6'));
    let screen = render(&mut app, 100, 30);
    assert!(
        screen.contains("test-host"),
        "system body missing:\n{screen}"
    );

    press(&mut app, KeyCode::Char('7'));
    let screen = render(&mut app, 100, 30);
    assert!(
        screen.contains("usage r enter toggles") && screen.contains("C:\\"),
        "disks body missing:\n{screen}"
    );
}

#[test]
fn the_dashboard_draws_the_live_cards() {
    let (info, cpu, memory) = small_system();
    let stubs = Stubs::new()
        .with_system(info, cpu, memory)
        .with_ports(vec![
            stub_socket(8080, 42, "node"),
            established_socket(51000, 443),
        ])
        .with_disks(vec![disk_row("C:\\", 90.0)]);
    let mut app = App::new(stubs.context());
    flush_disks(&mut app);

    let screen = render(&mut app, 100, 30);
    assert!(screen.contains("12.5%"), "cpu gauge:\n{screen}");
    assert!(screen.contains("56.2%"), "memory gauge:\n{screen}");
    assert!(
        screen.contains("listening    1"),
        "listening count:\n{screen}"
    );
    assert!(
        screen.contains("established  1"),
        "established count:\n{screen}"
    );
    assert!(screen.contains("all sockets  2"), "socket total:\n{screen}");
    assert!(screen.contains("C:\\"), "filesystem row:\n{screen}");
}

#[test]
fn the_search_dialog_lists_hits_under_family_headers() {
    let stubs = Stubs::new()
        .with_ports(vec![stub_socket(8080, 42, "node")])
        .with_processes(vec![stub_process(42, None, "node")]);
    let mut app = App::new(stubs.context());
    app.set_usage_fixture(PathBuf::from("r"), sample_usage());

    press(&mut app, KeyCode::Char('/'));
    type_text(&mut app, "node");
    app.on_tick(); // flush the debounced search scan
    let screen = render(&mut app, 100, 30);

    assert!(
        screen.contains("│> node_"),
        "search input line missing:\n{screen}"
    );
    assert!(
        screen.contains("│ process"),
        "first family header missing:\n{screen}"
    );
    assert!(
        screen.contains("node (42)"),
        "process hit missing:\n{screen}"
    );
    assert!(
        screen.contains("node (pid 42)"),
        "port owner detail missing:\n{screen}"
    );
}

#[test]
fn the_palette_lists_only_matching_commands() {
    let stubs = Stubs::new();
    let mut app = App::new(stubs.context());

    app.on_key(ctrl('p'));
    type_text(&mut app, "serv");
    let screen = render(&mut app, 100, 30);

    assert!(screen.contains("> serv_"), "palette input line:\n{screen}");
    assert!(
        screen.contains("go to services"),
        "matching command missing:\n{screen}"
    );
    assert!(
        !screen.contains("go to ports"),
        "an unmatched command was drawn:\n{screen}"
    );
}

#[test]
fn the_confirm_dialog_spells_out_the_plan() {
    let stubs = Stubs::new().with_ports(vec![stub_socket(8080, 42, "node")]);
    let mut app = App::new(stubs.context());

    press(&mut app, KeyCode::Char('2'));
    press(&mut app, KeyCode::Char('c'));
    let screen = render(&mut app, 100, 30);

    assert!(screen.contains("free port 8080"), "title:\n{screen}");
    assert!(
        screen.contains("tcp listen  node (42)"),
        "plan row missing:\n{screen}"
    );
    assert!(
        screen.contains("terminate 1 process(es) to free this port"),
        "plan summary missing:\n{screen}"
    );
    assert!(screen.contains("y confirm"), "hint missing:\n{screen}");
}

#[test]
fn the_socket_detail_is_drawn_with_a_kill_hint() {
    let stubs = Stubs::new()
        .with_ports(vec![stub_socket(8080, 42, "node")])
        .with_processes(vec![stub_process(42, None, "node")]);
    let mut app = App::new(stubs.context());

    press(&mut app, KeyCode::Char('2'));
    press(&mut app, KeyCode::Enter);
    let screen = render(&mut app, 100, 30);

    assert!(
        screen.contains("socket tcp 0.0.0.0:8080 listen"),
        "title missing:\n{screen}"
    );
    assert!(
        screen.contains("k kill    any other key closes"),
        "kill hint missing:\n{screen}"
    );
}

#[test]
fn the_socket_detail_shows_reported_queue_lengths() {
    let stubs = Stubs::new()
        .with_ports(vec![queue_socket(8080, 42, "node", 1536, 0)])
        .with_processes(vec![stub_process(42, None, "node")]);
    let mut app = App::new(stubs.context());

    press(&mut app, KeyCode::Char('2'));
    press(&mut app, KeyCode::Enter);
    let screen = render(&mut app, 100, 30);

    assert!(screen.contains("send queue"), "send queue:\n{screen}");
    assert!(screen.contains("1.5 KB"), "send value:\n{screen}");
    assert!(screen.contains("recv queue"), "recv queue:\n{screen}");
    assert!(screen.contains("0 B"), "recv value:\n{screen}");
}

#[test]
fn the_service_detail_is_drawn_read_only() {
    let stubs = Stubs::new().with_services(vec![stub_service("sshd", 12)]);
    let mut app = App::new(stubs.context());

    press(&mut app, KeyCode::Char('5'));
    press(&mut app, KeyCode::Enter);
    let screen = render(&mut app, 100, 30);

    assert!(screen.contains("service sshd"), "title missing:\n{screen}");
    assert!(screen.contains("any key closes"), "hint missing:\n{screen}");
    assert!(
        !screen.contains("k kill"),
        "a read-only dialog offers a kill:\n{screen}"
    );
}

#[test]
fn the_process_tree_folds_end_to_end() {
    let stubs = Stubs::new().with_processes(vec![
        stub_process(1, None, "init"),
        stub_process(2, Some(1), "worker"),
    ]);
    let mut app = App::new(stubs.context());

    press(&mut app, KeyCode::Char('3'));
    press(&mut app, KeyCode::Char('t'));
    let screen = render(&mut app, 100, 30);
    assert!(
        screen.contains("processes | sort cpu | tree"),
        "tree title missing:\n{screen}"
    );
    assert!(
        screen.contains("init") && screen.contains("worker"),
        "tree rows missing:\n{screen}"
    );

    press(&mut app, KeyCode::Char(' '));
    let screen = render(&mut app, 100, 30);
    assert!(screen.contains("init"), "parent row gone:\n{screen}");
    assert!(
        !screen.contains("worker"),
        "the folded child is still drawn:\n{screen}"
    );
}

#[test]
fn the_disks_page_draws_the_usage_tree_and_folds_it() {
    let stubs = Stubs::new().with_disks(vec![disk_row("C:\\", 90.0)]);
    let mut app = App::new(stubs.context());
    app.set_usage_fixture(PathBuf::from("r"), sample_usage());

    press(&mut app, KeyCode::Char('7'));
    let screen = render(&mut app, 100, 30);
    assert!(
        screen.contains("usage r enter toggles"),
        "usage title missing:\n{screen}"
    );
    assert!(screen.contains("big"), "child directory gone:\n{screen}");

    press(&mut app, KeyCode::Enter);
    let screen = render(&mut app, 100, 30);
    assert!(screen.contains("usage r"), "title after fold:\n{screen}");
    assert!(
        !screen.contains("big"),
        "the folded subtree is still drawn:\n{screen}"
    );
}

#[test]
fn the_status_line_prompts_for_the_filter() {
    let stubs = Stubs::new().with_ports(vec![stub_socket(8080, 42, "node")]);
    let mut app = App::new(stubs.context());

    press(&mut app, KeyCode::Char('2'));
    press(&mut app, KeyCode::Char('f'));
    type_text(&mut app, "no");
    let screen = render(&mut app, 100, 30);

    assert!(screen.contains("filter: no_"), "status prompt:\n{screen}");
    assert!(screen.contains(" filter "), "dialog title:\n{screen}");
}

/// After scrolling, the highlighted row must still be the selected row —
/// `k` and Enter act on the selection, so a shifted highlight can make the
/// user confirm an action against the wrong row.
#[test]
fn the_highlighted_row_is_the_selected_row_after_scrolling() {
    let ports: Vec<PortInfo> = (0..40)
        .map(|index| stub_socket(10_000 + index as u16, 42, &format!("proc{index}")))
        .collect();
    let stubs = Stubs::new().with_ports(ports);
    let mut app = App::new(stubs.context());
    press(&mut app, KeyCode::Char('2')); // Ports page

    // A small viewport forces the scroll to follow the selection.
    app.note_viewport(10);
    for _ in 0..35 {
        press(&mut app, KeyCode::Down);
    }
    assert_eq!(app.selected(), 35);
    assert!(
        app.scroll() > 0,
        "the view should have scrolled: {}",
        app.scroll()
    );

    // Find the row drawn with the selection style (the theme's header
    // background) and check it is the row the app would act on. The theme
    // is read from the same source the draw uses, so this holds even when
    // another test has installed a different theme.
    let header_bg = crate::theme::current().header().bg;
    let mut terminal = Terminal::new(TestBackend::new(100, 40)).expect("test backend");
    terminal
        .draw(|frame| draw(frame, &mut app))
        .expect("draw frame");
    let buffer = terminal.backend().buffer();
    let mut highlighted: Vec<String> = Vec::new();
    for y in 0..buffer.area.height {
        let mut line = String::new();
        let mut is_selected = false;
        for x in 0..buffer.area.width {
            let cell = buffer.cell((x, y)).expect("cell");
            if cell.style().bg == header_bg {
                is_selected = true;
            }
            line.push_str(cell.symbol());
        }
        if is_selected {
            highlighted.push(line.trim_end().to_string());
        }
    }
    let data_rows: Vec<String> = highlighted
        .iter()
        .filter(|line| {
            line.contains("proc0")
                || line.contains("proc1")
                || line.contains("proc2")
                || line.contains("proc3")
        })
        .cloned()
        .collect();
    assert_eq!(
        data_rows.len(),
        1,
        "exactly one data row should be highlighted; selected={} scroll={} highlighted: {highlighted:?}",
        app.selected(),
        app.scroll()
    );
    assert!(
        data_rows[0].contains(&format!("proc{}", app.selected())),
        "highlight drifted off the selected row (selected={}, scroll={}): {}",
        app.selected(),
        app.scroll(),
        data_rows[0]
    );
}

/// The window the keyboard scrolls within must match what the table really
/// fits. It spends two lines on borders and one on the header, so a viewport
/// one row too tall leaves the selection on a row the bottom border clips —
/// scrolling to the end makes the highlight vanish.
#[test]
fn the_last_data_row_stays_visible_when_the_selection_reaches_the_end() {
    let ports: Vec<PortInfo> = (0..40)
        .map(|index| stub_socket(10_000 + index as u16, 42, &format!("proc{index}")))
        .collect();
    let stubs = Stubs::new().with_ports(ports);
    let mut app = App::new(stubs.context());
    press(&mut app, KeyCode::Char('2')); // Ports page

    // Draw first so the real viewport is reported, then walk to the end.
    let _ = render(&mut app, 100, 20);
    for _ in 0..39 {
        press(&mut app, KeyCode::Down);
    }
    assert_eq!(app.selected(), 39);
    let _ = render(&mut app, 100, 20);

    let header_bg = crate::theme::current().header().bg;
    let mut terminal = Terminal::new(TestBackend::new(100, 20)).expect("test backend");
    terminal
        .draw(|frame| draw(frame, &mut app))
        .expect("draw frame");
    let buffer = terminal.backend().buffer();
    let mut highlighted: Vec<String> = Vec::new();
    for y in 0..buffer.area.height {
        let mut line = String::new();
        let mut is_selected = false;
        for x in 0..buffer.area.width {
            let cell = buffer.cell((x, y)).expect("cell");
            if cell.style().bg == header_bg {
                is_selected = true;
            }
            line.push_str(cell.symbol());
        }
        if is_selected {
            highlighted.push(line.trim_end().to_string());
        }
    }
    // The header keeps the warning background in some themes; only data rows
    // matter here, and only one of them may carry the selection.
    let selected_lines: Vec<String> = highlighted
        .iter()
        .filter(|line| line.contains("proc"))
        .cloned()
        .collect();
    assert_eq!(
        selected_lines.len(),
        1,
        "the selected row must be drawn exactly once (selected={}, scroll={}): {highlighted:?}",
        app.selected(),
        app.scroll()
    );
    assert!(
        selected_lines[0].contains("proc39"),
        "the last row must be on screen, not clipped by the border: {}",
        selected_lines[0]
    );
}

/// Wait for the background disk listing to land; the pages that need it
/// harvest it through ticks, the same way a real session does.
fn flush_disks(app: &mut App) {
    for _ in 0..400 {
        if app.disks_pending() {
            std::thread::sleep(std::time::Duration::from_millis(5));
            app.on_tick();
        } else {
            break;
        }
    }
    app.on_tick();
}
