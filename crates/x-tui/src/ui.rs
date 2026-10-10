//! Drawing.
//!
//! Layout is fixed on purpose: a sidebar with the pages, a body that is one
//! dashboard or table per view, a status line and a footer with the keys that
//! currently do something. Anything the user can act on is visible without
//! scrolling back.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, Gauge, List, ListItem, ListState, Paragraph, Row, Scrollbar,
    ScrollbarOrientation, ScrollbarState, Table,
};
use ratatui::Frame;

use x_core::network::is_default_route;
use x_core::process::ProcessInfo;
use x_core::service::ServiceInfo;
use x_core::{format_bytes, format_duration};

use crate::app::{service_state_label, sort_label, state_label, App, Modal, Target, View};

/// Draw the whole interface.
pub fn draw(frame: &mut Frame, app: &mut App) {
    let rows = Layout::vertical([
        Constraint::Min(3),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(frame.area());
    let main = Layout::horizontal([Constraint::Length(16), Constraint::Min(20)]).split(rows[0]);

    sidebar(frame, app, main[0]);
    body(frame, app, main[1]);
    status(frame, app, rows[1]);
    footer(frame, app, rows[2]);

    if !matches!(app.modal(), Modal::None) {
        dialog(frame, app, frame.area());
    }
}

/// The pages as a vertical list; digits and tab cycle through them.
fn sidebar(frame: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = View::ALL
        .iter()
        .enumerate()
        .map(|(index, view)| ListItem::new(format!("{} {}", index + 1, view.title())))
        .collect();
    let selected = View::ALL.iter().position(|view| *view == app.view());
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" x "))
        .highlight_style(crate::theme::current().accent(false).bold())
        .highlight_symbol("> ");
    let mut state = ListState::default().with_selected(selected);
    frame.render_stateful_widget(list, area, &mut state);
}

fn body(frame: &mut Frame, app: &mut App, area: Rect) {
    match app.view() {
        View::Dashboard => dashboard(frame, app, area),
        View::Ports => ports(frame, app, area),
        View::Processes => processes(frame, app, area),
        View::Network => network(frame, app, area),
        View::Services => services(frame, app, area),
        View::System => system(frame, app, area),
        View::Disks => disks(frame, app, area),
        View::Remote => remote(frame, app, area),
        View::NetTop => net_top(frame, app, area),
    }
}

fn table<'a>(headers: &[(&'a str, usize)], title: &str) -> Table<'a> {
    let widths: Vec<Constraint> = headers
        .iter()
        .map(|(_, width)| Constraint::Length(*width as u16))
        .collect();
    let header = Row::new(headers.iter().map(|(label, _)| Cell::from(*label)))
        .style(crate::theme::current().warning());
    Table::new(Vec::<Row<'a>>::new(), widths)
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {title} ")),
        )
        .column_spacing(1)
}

/// Rows the table body fits, and the window the keyboard should scroll within.
fn inner_rows(app: &mut App, area: Rect) -> usize {
    let rows = area.height.saturating_sub(2) as usize;
    app.note_viewport(rows);
    rows.max(1)
}

fn selected_row(index: usize, selected: usize) -> Style {
    if index == selected {
        crate::theme::current().header()
    } else {
        Style::default()
    }
}

/// `12.3M` style link speed label.
fn format_speed(bps: u64) -> String {
    if bps >= 1_000_000_000 {
        format!("{:.1} Gbps", bps as f64 / 1_000_000_000.0)
    } else if bps >= 1_000_000 {
        format!("{:.0} Mbps", bps as f64 / 1_000_000.0)
    } else if bps >= 1_000 {
        format!("{:.0} kbps", bps as f64 / 1_000.0)
    } else {
        format!("{bps} bps")
    }
}

/// Keep the active filter visible in the table title.
fn titled(base: &str, app: &App) -> String {
    let filter = app.filter().trim();
    if filter.is_empty() {
        base.to_string()
    } else {
        format!("{base} | filter {filter:?}")
    }
}

// ---------------------------------------------------------------------------
// Dashboard
// ---------------------------------------------------------------------------

fn dashboard(frame: &mut Frame, app: &mut App, area: Rect) {
    let rows = Layout::vertical([
        Constraint::Length(8),
        Constraint::Length(8),
        Constraint::Min(5),
    ])
    .split(area);
    let top =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(rows[0]);
    let mid =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(rows[1]);

    let mut facts: Vec<Line> = Vec::new();
    match app.system() {
        Some(info) => {
            facts.push(Line::raw(format!(
                "{} {} ({})",
                info.os_name, info.os_version, info.arch
            )));
            facts.push(Line::raw(format!("host  {}", info.hostname)));
            facts.push(Line::raw(format!(
                "cpu   {} x {}",
                info.cpu_count,
                info.cpu_brand.clone().unwrap_or_else(|| "unknown".into())
            )));
            facts.push(Line::raw(format!(
                "up    {}",
                format_duration(info.uptime_seconds)
            )));
        }
        None => facts.push(Line::raw("system facts unreadable")),
    }
    frame.render_widget(
        Paragraph::new(facts).block(Block::default().borders(Borders::ALL).title(" system ")),
        top[0],
    );

    let cpu = app.cpu().clamp(0.0, 100.0);
    frame.render_widget(
        Gauge::default()
            .block(Block::default().borders(Borders::ALL).title(" cpu "))
            .gauge_style(crate::theme::current().accent(false))
            .ratio(f64::from(cpu) / 100.0)
            .label(format!("{cpu:.1}%")),
        top[1],
    );

    let memory = app.memory_usage();
    let label = match memory {
        Some(usage) => format!(
            "{:.1}% | {} of {}",
            usage.percent,
            format_bytes(usage.used_bytes),
            format_bytes(usage.total_bytes)
        ),
        None => "unreadable".into(),
    };
    frame.render_widget(
        Gauge::default()
            .block(Block::default().borders(Borders::ALL).title(" memory "))
            .gauge_style(ratatui::style::Style::default().fg(crate::theme::current().warning))
            .ratio(
                memory
                    .map(|usage| f64::from(usage.percent.clamp(0.0, 100.0)) / 100.0)
                    .unwrap_or(0.0),
            )
            .label(label),
        mid[0],
    );

    let mut ports: Vec<Line> = Vec::new();
    match app.port_summary() {
        Some(summary) => {
            ports.push(Line::raw(format!("listening    {}", summary.listening)));
            ports.push(Line::raw(format!("established  {}", summary.established)));
            ports.push(Line::raw(format!("all sockets  {}", summary.total)));
        }
        None => ports.push(Line::raw("socket table unreadable")),
    }
    frame.render_widget(
        Paragraph::new(ports).block(Block::default().borders(Borders::ALL).title(" ports ")),
        mid[1],
    );

    let mounts: Vec<Row> = app
        .disks()
        .iter()
        .map(|row| {
            Row::new(vec![
                row.mount_point.clone(),
                row.file_system.clone().unwrap_or_default(),
                row.media_type
                    .map(|media| media.to_string())
                    .unwrap_or_default(),
                format_bytes(row.total_bytes),
                format_bytes(row.used_bytes()),
                format!("{:.0}%", row.percent),
            ])
        })
        .collect();
    frame.render_widget(
        table(
            &[
                ("mount", 6),
                ("fs", 8),
                ("media", 9),
                ("size", 10),
                ("used", 10),
                ("use%", 5),
            ],
            "filesystems",
        )
        .rows(mounts),
        rows[2],
    );
}

// ---------------------------------------------------------------------------
// Ports / Processes / Network / Services / System / Disks
// ---------------------------------------------------------------------------

fn ports(frame: &mut Frame, app: &mut App, area: Rect) {
    let inner_height = inner_rows(app, area);
    let start = app.scroll();
    let rows: Vec<Row> = app
        .ports()
        .iter()
        .enumerate()
        .skip(start)
        .take(inner_height.max(1))
        .map(|(index, row)| {
            Row::new(vec![
                row.local_port.to_string(),
                row.protocol.name().to_string(),
                state_label(row.state).to_string(),
                row.pid.map(|p| p.to_string()).unwrap_or_default(),
                row.process_name.clone().unwrap_or_default(),
                row.user.clone().unwrap_or_default(),
                row.remote_socket_addr()
                    .map(|addr| addr.to_string())
                    .unwrap_or_else(|| row.endpoint()),
            ])
            .style(selected_row(start + index, app.selected()))
        })
        .collect();

    let widget = table(
        &[
            ("port", 6),
            ("proto", 5),
            ("state", 12),
            ("pid", 7),
            ("process", 20),
            ("user", 12),
            ("remote", 40),
        ],
        &titled("sockets", app),
    )
    .rows(rows);
    frame.render_widget(widget, area);
    scrollbar(frame, app, area);
}

fn processes(frame: &mut Frame, app: &mut App, area: Rect) {
    let inner_height = inner_rows(app, area);
    let start = app.scroll();
    let rows: Vec<Row> = app
        .process_rows()
        .into_iter()
        .enumerate()
        .skip(start)
        .take(inner_height.max(1))
        .map(|(index, (depth, row))| process_row(start + index, depth, app.selected(), row))
        .collect();

    let mode = if app.tree_mode() { " | tree" } else { "" };
    let title = titled(
        &format!("processes | sort {}{mode}", sort_label(app.process_sort())),
        app,
    );
    let widget = table(
        &[
            ("pid", 7),
            ("user", 12),
            ("cpu%", 7),
            ("mem", 10),
            ("name", 30),
            ("command", 60),
        ],
        &title,
    )
    .rows(rows);
    frame.render_widget(widget, area);
    scrollbar(frame, app, area);
}

fn process_row(index: usize, depth: usize, selected: usize, row: &ProcessInfo) -> Row<'_> {
    let name = if depth == 0 {
        row.name.clone()
    } else {
        format!("{}{}", "  ".repeat(depth), row.name)
    };
    Row::new(vec![
        row.pid.to_string(),
        row.user.clone().unwrap_or_default(),
        format!("{:.1}", row.cpu_usage.unwrap_or_default()),
        row.memory_bytes.map(format_bytes).unwrap_or_default(),
        name,
        row.command_line.clone().unwrap_or_default(),
    ])
    .style(selected_row(index, selected))
}

fn network(frame: &mut Frame, app: &mut App, area: Rect) {
    let interface_rows = (app.interfaces().len() as u16 + 4).clamp(5, 12);
    let panes =
        Layout::vertical([Constraint::Length(interface_rows), Constraint::Min(5)]).split(area);
    let columns = Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(panes[0]);

    let defaults: Vec<&str> = app
        .routes()
        .iter()
        .filter(|route| is_default_route(route))
        .filter_map(|route| route.interface.as_deref())
        .collect();

    let interfaces: Vec<Row> = app
        .interfaces()
        .iter()
        .map(|row| {
            let ips: Vec<String> = app
                .addresses()
                .iter()
                .filter(|address| address.interface == row.name)
                .take(2)
                .map(|address| {
                    address
                        .prefix_len
                        .map(|prefix| format!("{}/{prefix}", address.address))
                        .unwrap_or_else(|| address.address.to_string())
                })
                .collect();
            Row::new(vec![
                row.name.clone(),
                format!("{:?}", row.state).to_lowercase(),
                ips.join(","),
                row.mac_address.clone().unwrap_or_default(),
                row.link_speed_bps.map(format_speed).unwrap_or_default(),
                if defaults.contains(&row.name.as_str()) {
                    "default"
                } else {
                    ""
                }
                .to_string(),
            ])
        })
        .collect();
    frame.render_widget(
        table(
            &[
                ("interface", 14),
                ("state", 8),
                ("address", 30),
                ("mac", 18),
                ("speed", 10),
                ("route", 8),
            ],
            "interfaces",
        )
        .rows(interfaces),
        columns[0],
    );

    let mut text: Vec<Line> = Vec::new();
    text.push(Line::styled(
        "default routes",
        crate::theme::current().warning(),
    ));
    for route in app.routes().iter().filter(|route| is_default_route(route)) {
        text.push(Line::from(vec![Span::raw(format!(
            "  via {} on {}",
            route
                .gateway
                .map(|ip| ip.to_string())
                .unwrap_or_else(|| "-".into()),
            route.interface.clone().unwrap_or_else(|| "-".into())
        ))]));
    }
    text.push(Line::from(""));
    text.push(Line::styled("resolvers", crate::theme::current().warning()));
    for server in &app.dns().servers {
        text.push(Line::raw(format!("  {}", server.address)));
    }
    if !app.dns().search_domains.is_empty() {
        text.push(Line::raw(format!(
            "  search {}",
            app.dns().search_domains.join(" ")
        )));
    }
    text.push(Line::from(""));
    text.push(Line::styled("addresses", crate::theme::current().warning()));
    for address in app.addresses().iter().take(8) {
        text.push(Line::raw(format!(
            "  {:<12} {}/{}",
            address.interface,
            address.address,
            address
                .prefix_len
                .map(|p| p.to_string())
                .unwrap_or_default()
        )));
    }
    frame.render_widget(
        Paragraph::new(text).block(Block::default().borders(Borders::ALL).title(" routes ")),
        columns[1],
    );

    let inner_height = inner_rows(app, panes[1]);
    let start = app.scroll();
    let connections: Vec<Row> = app
        .connections()
        .iter()
        .enumerate()
        .skip(start)
        .take(inner_height.max(1))
        .map(|(index, row)| {
            Row::new(vec![
                row.protocol.name().to_string(),
                row.endpoint(),
                row.remote_socket_addr()
                    .map(|addr| addr.to_string())
                    .unwrap_or_else(|| "-".into()),
                state_label(row.state).to_string(),
                row.pid.map(|p| p.to_string()).unwrap_or_default(),
                row.process_name.clone().unwrap_or_default(),
            ])
            .style(selected_row(start + index, app.selected()))
        })
        .collect();
    frame.render_widget(
        table(
            &[
                ("proto", 5),
                ("local", 40),
                ("remote", 40),
                ("state", 12),
                ("pid", 7),
                ("process", 20),
            ],
            &titled("connections", app),
        )
        .rows(connections),
        panes[1],
    );
    scrollbar(frame, app, panes[1]);
}

fn services(frame: &mut Frame, app: &mut App, area: Rect) {
    let inner_height = inner_rows(app, area);
    let start = app.scroll();
    let rows: Vec<Row> = app
        .services()
        .iter()
        .enumerate()
        .skip(start)
        .take(inner_height.max(1))
        .map(|(index, row): (usize, &ServiceInfo)| {
            Row::new(vec![
                row.name.clone(),
                service_state_label(row.state).to_string(),
                row.pid.map(|p| p.to_string()).unwrap_or_default(),
                row.display_name.clone().unwrap_or_default(),
                match row.enabled {
                    Some(true) => "yes".to_string(),
                    Some(false) => "no".to_string(),
                    None => "-".to_string(),
                },
            ])
            .style(selected_row(start + index, app.selected()))
        })
        .collect();
    frame.render_widget(
        table(
            &[
                ("service", 28),
                ("state", 10),
                ("pid", 7),
                ("display name", 30),
                ("enabled", 8),
            ],
            &titled("services", app),
        )
        .rows(rows),
        area,
    );
    scrollbar(frame, app, area);
}

fn system(frame: &mut Frame, app: &mut App, area: Rect) {
    let mut facts: Vec<Line> = Vec::new();
    if let Some(info) = app.system() {
        facts.push(Line::raw(format!(
            "{} {} ({})",
            info.os_name, info.os_version, info.arch
        )));
        facts.push(Line::raw(format!("host {}", info.hostname)));
        facts.push(Line::raw(format!(
            "cpu  {} x {}",
            info.cpu_count,
            info.cpu_brand.clone().unwrap_or_else(|| "unknown".into())
        )));
        facts.push(Line::raw(format!(
            "up   {}",
            format_duration(info.uptime_seconds)
        )));
    }
    facts.push(Line::from(""));
    facts.push(Line::raw(format!("cpu  {:.1}%", app.cpu())));
    if let Some(usage) = app.memory_usage() {
        facts.push(Line::raw(format!(
            "mem  {:.1}% ({} free)",
            usage.percent,
            format_bytes(usage.available_bytes)
        )));
        if usage.swap_total_bytes > 0 {
            facts.push(Line::raw(format!(
                "swap {} of {}",
                format_bytes(usage.swap_used_bytes),
                format_bytes(usage.swap_total_bytes)
            )));
        }
        if let Some(pressure) = usage.pressure {
            facts.push(Line::raw(format!("press {pressure}")));
        }
    }
    frame.render_widget(
        Paragraph::new(facts).block(Block::default().borders(Borders::ALL).title(" system ")),
        area,
    );
}

/// Mounted filesystems on top, the usage tree of the launch directory below.
fn disks(frame: &mut Frame, app: &mut App, area: Rect) {
    let panes = Layout::vertical([
        Constraint::Length((app.disks().len() as u16 + 4).clamp(5, 12)),
        Constraint::Min(3),
    ])
    .split(area);

    let mounts: Vec<Row> = app
        .disks()
        .iter()
        .map(|row| {
            Row::new(vec![
                row.mount_point.clone(),
                row.file_system.clone().unwrap_or_default(),
                row.media_type
                    .map(|media| media.to_string())
                    .unwrap_or_default(),
                format_bytes(row.total_bytes),
                format_bytes(row.used_bytes()),
                format!("{:.0}%", row.percent),
            ])
        })
        .collect();
    frame.render_widget(
        table(
            &[
                ("mount", 6),
                ("fs", 8),
                ("media", 9),
                ("size", 10),
                ("used", 10),
                ("use%", 5),
            ],
            "filesystems",
        )
        .rows(mounts),
        panes[0],
    );

    let inner_height = inner_rows(app, panes[1]);
    let start = app.scroll();
    let rows: Vec<Row> = app
        .usage_tree()
        .into_iter()
        .enumerate()
        .skip(start)
        .take(inner_height.max(1))
        .map(|(index, row)| {
            let name = row
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| row.path.display().to_string());
            Row::new(vec![
                format!("{}{name}", "  ".repeat(row.depth)),
                format_bytes(row.total_bytes),
                row.files.to_string(),
                row.dirs.to_string(),
            ])
            .style(selected_row(start + index, app.selected()))
        })
        .collect();
    let title = if app.usage_scanning() {
        format!("usage {} scanning", app.usage_root().display())
    } else {
        format!("usage {} enter toggles", app.usage_root().display())
    };
    frame.render_widget(
        table(
            &[("directory", 48), ("size", 10), ("files", 7), ("dirs", 6)],
            &title,
        )
        .rows(rows),
        panes[1],
    );
    scrollbar(frame, app, panes[1]);
}

fn scrollbar(frame: &mut Frame, app: &App, area: Rect) {
    let total = app.rows();
    if total <= area.height.saturating_sub(2) as usize {
        return;
    }
    let mut state = ScrollbarState::new(total).position(app.selected());
    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight).begin_symbol(None),
        area,
        &mut state,
    );
}

fn status(frame: &mut Frame, app: &App, area: Rect) {
    let text = if let Modal::Prompt { input, label } = app.modal() {
        format!("{label}: {input}_")
    } else {
        app.status().to_string()
    };
    frame.render_widget(
        Paragraph::new(text).style(crate::theme::current().accent(false)),
        area,
    );
}

/// The remote page: pick an SSH host, then its read-only snapshot.
fn remote(frame: &mut Frame, app: &App, area: Rect) {
    let theme = crate::theme::current();
    let mut lines: Vec<Line> = Vec::new();

    if let Some(host) = app.remote_host() {
        lines.push(Line::styled(
            format!("snapshot from {host}"),
            theme.accent(true),
        ));
        if let Some(error) = app.remote_error() {
            lines.push(Line::styled(error.to_string(), theme.danger()));
        }
        for row in app.remote_rows() {
            lines.push(Line::styled(row.clone(), theme.text()));
        }
        lines.push(Line::styled(
            "esc: back to host list".to_string(),
            theme.dim(),
        ));
    } else {
        lines.push(Line::styled(
            "hosts from ~/.ssh/config — enter fetches a read-only snapshot:",
            theme.warning(),
        ));
        if app.remote_hosts().is_empty() {
            lines.push(Line::styled(
                "  (none found; add hosts to ~/.ssh/config or use `x remote connect`)",
                theme.dim(),
            ));
        }
        for (index, host) in app.remote_hosts().iter().enumerate() {
            let selected = index == app.selected();
            let marker = if selected { "> " } else { "  " };
            lines.push(Line::styled(
                format!("{marker}{host}"),
                if selected {
                    theme.accent(true)
                } else {
                    theme.text()
                },
            ));
        }
        if app.remote_fetching() {
            lines.push(Line::styled("fetching…", theme.dim()));
        }
        if let Some(error) = app.remote_error() {
            lines.push(Line::styled(error.to_string(), theme.danger()));
        }
    }

    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" remote ")
                .style(theme.text()),
        ),
        area,
    );
}

/// Rates render as human bytes per second; an absent rate stays `-`.
fn rate_label(rate: Option<f64>) -> String {
    match rate {
        Some(bps) => format!("{}/s", format_bytes(bps as u64)),
        None => "-".to_string(),
    }
}

fn net_top(frame: &mut Frame, app: &mut App, area: Rect) {
    if !app.net_top_available() {
        frame.render_widget(
            Paragraph::new("net top: no sampler wired in this session")
                .style(crate::theme::current().dim()),
            area,
        );
        return;
    }
    let inner_height = inner_rows(app, area);
    let start = app.scroll();
    let Some(report) = app.net_top_report() else {
        frame.render_widget(
            Paragraph::new("collecting baseline: per-process rates appear from the next round")
                .style(crate::theme::current().dim()),
            area,
        );
        return;
    };

    let mut rows: Vec<(String, String, String, String, String, String)> = report
        .processes
        .iter()
        .map(|rate| {
            let process = app
                .net_top_process_name(rate.pid)
                .unwrap_or_else(|| "?".to_string());
            (
                rate.pid.to_string(),
                process,
                rate_label(rate.rx_bps),
                rate_label(rate.tx_bps),
                rate.conns.to_string(),
                rate.source.to_string(),
            )
        })
        .collect();
    // Unmapped = host total minus the attributed sum; derived, not a
    // process, so it renders last without a pid.
    if report.unmapped.rx_bps.is_some() || report.unmapped.tx_bps.is_some() {
        rows.push((
            "-".to_string(),
            "(unmapped)".to_string(),
            rate_label(report.unmapped.rx_bps),
            rate_label(report.unmapped.tx_bps),
            "-".to_string(),
            "host counters only".to_string(),
        ));
    }
    let rows: Vec<Row> = rows
        .into_iter()
        .enumerate()
        .skip(start)
        .take(inner_height.max(1))
        .map(|(index, (pid, process, rx, tx, conns, source))| {
            Row::new(vec![pid, process, rx, tx, conns, source])
                .style(selected_row(start + index, app.selected()))
        })
        .collect();

    let widget = table(
        &[
            ("pid", 7),
            ("process", 24),
            ("rx/s", 12),
            ("tx/s", 12),
            ("conns", 7),
            ("source", 16),
        ],
        &titled("net top", app),
    )
    .rows(rows);
    frame.render_widget(widget, area);
    scrollbar(frame, app, area);
}

fn footer(frame: &mut Frame, app: &App, area: Rect) {
    let keys = match app.view() {
        View::Dashboard => "1-9 pages   / search   ctrl+p commands   r refresh   q quit",
        View::Ports => "enter details   k kill   / search   f filter   r refresh   q quit",
        View::Processes => {
            if app.tree_mode() {
                "enter details   k kill   space fold   t tree off   s sort   p ports   / search   f filter   r refresh   q quit"
            } else {
                "enter details   k kill   t tree   s sort   p ports   / search   f filter   r refresh   q quit"
            }
        }
        View::Network => "enter details   k kill   / search   f filter   r refresh   q quit",
        View::Services => "enter details   / search   f filter   r refresh   q quit",
        View::System => "/ search   ctrl+p commands   r refresh   q quit",
        View::Disks => "enter fold   / search   ctrl+p commands   r refresh   q quit",
        View::Remote => "enter fetch snapshot   up/down choose host   r refresh   q quit",
        View::NetTop => "r resample   ctrl+p commands   q quit",
    };
    frame.render_widget(
        Paragraph::new(keys).style(crate::theme::current().dim()),
        area,
    );
}

// ---------------------------------------------------------------------------
// Dialogs
// ---------------------------------------------------------------------------

fn dialog(frame: &mut Frame, app: &App, area: Rect) {
    let width = area.width.saturating_sub(8).min(76);
    let height = match app.modal() {
        Modal::Confirm {
            target: Target::Sockets(plan),
            ..
        } => (plan.sockets.len() as u16 + 7).min(area.height.saturating_sub(4)),
        Modal::Confirm { .. } => 8.min(area.height.saturating_sub(4)),
        Modal::Prompt { .. } => 8.min(area.height.saturating_sub(4)),
        Modal::Palette { .. } => 18.min(area.height.saturating_sub(4)),
        Modal::Search { .. } => 22.min(area.height.saturating_sub(4)),
        Modal::Detail { rows, .. } => (rows.len() as u16 + 4).min(area.height.saturating_sub(4)),
        Modal::None => 8,
    };
    let popup = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };

    frame.render_widget(Clear, popup);

    match app.modal() {
        Modal::None => {}
        Modal::Prompt { label, input } => {
            let text = vec![
                Line::styled(label.to_string(), crate::theme::current().warning()),
                Line::raw(input.clone()),
            ];
            frame.render_widget(
                Paragraph::new(text)
                    .block(Block::default().borders(Borders::ALL).title(" filter ")),
                popup,
            );
        }
        Modal::Confirm { title, target, .. } => {
            let mut lines: Vec<Line> = vec![Line::styled(
                title.clone(),
                crate::theme::current().warning(),
            )];
            match target {
                Target::Sockets(plan) => {
                    for row in &plan.sockets {
                        lines.push(Line::raw(format!(
                            "  {:<6} {} {}  {} ({})",
                            row.local_port,
                            row.protocol.name(),
                            state_label(row.state),
                            row.process_name.clone().unwrap_or_else(|| "-".into()),
                            row.pid.map(|p| p.to_string()).unwrap_or_default(),
                        )));
                    }
                    lines.push(Line::from(""));
                    lines.push(Line::raw(format!(
                        "terminate {} process(es) to free this port",
                        plan.target_pids().len()
                    )));
                }
                Target::Process { pid, name } => {
                    lines.push(Line::raw(format!("  pid  {pid}")));
                    lines.push(Line::raw(format!("  name {name}")));
                }
            }
            lines.push(Line::from(""));
            lines.push(Line::styled(
                "y confirm    any other key cancels",
                crate::theme::current().accent(false),
            ));
            frame.render_widget(
                Paragraph::new(lines)
                    .block(Block::default().borders(Borders::ALL).title(" confirm ")),
                popup,
            );
        }
        Modal::Palette { input, selected } => {
            let matches = app.palette_matches();
            let visible = popup.height.saturating_sub(3) as usize;
            let start = selected.saturating_sub(visible.saturating_sub(1));
            let mut lines: Vec<Line> = vec![Line::styled(
                format!("> {input}_"),
                crate::theme::current().accent(false),
            )];
            for (index, command) in matches.iter().enumerate().skip(start).take(visible) {
                let style = if index == *selected {
                    crate::theme::current().header()
                } else {
                    Style::default()
                };
                lines.push(
                    Line::raw(format!("{:<36} {}", command.label, command.hint)).style(style),
                );
            }
            frame.render_widget(
                Paragraph::new(lines)
                    .block(Block::default().borders(Borders::ALL).title(" commands ")),
                popup,
            );
        }
        Modal::Search { input, selected } => {
            let hits = app.search_hits();
            let visible = popup.height.saturating_sub(4) as usize;
            let start = selected.saturating_sub(visible.saturating_sub(1));
            let mut lines: Vec<Line> = vec![Line::styled(
                format!("> {input}_"),
                crate::theme::current().accent(false),
            )];
            let mut family = "";
            for (index, hit) in hits.iter().enumerate().skip(start).take(visible) {
                if hit.family != family {
                    family = hit.family;
                    lines.push(Line::styled(
                        format!(" {family}"),
                        crate::theme::current().warning(),
                    ));
                }
                let style = if index == *selected {
                    crate::theme::current().header()
                } else {
                    Style::default()
                };
                lines.push(Line::raw(format!("  {:<32} {}", hit.label, hit.detail)).style(style));
            }
            for note in app.search_notes() {
                lines.push(Line::styled(
                    format!("! {note}"),
                    crate::theme::current().danger(),
                ));
            }
            frame.render_widget(
                Paragraph::new(lines)
                    .block(Block::default().borders(Borders::ALL).title(" search ")),
                popup,
            );
        }
        Modal::Detail {
            title,
            rows,
            target,
        } => {
            let mut lines: Vec<Line> = Vec::new();
            for (key, value) in rows {
                lines.push(Line::raw(format!("  {key:<12} {value}")));
            }
            lines.push(Line::from(""));
            lines.push(Line::styled(
                if target.is_some() {
                    "k kill    any other key closes"
                } else {
                    "any key closes"
                },
                crate::theme::current().accent(false),
            ));
            frame.render_widget(
                Paragraph::new(lines).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(format!(" {title} ")),
                ),
                popup,
            );
        }
    }
}

#[cfg(test)]
mod tests;
