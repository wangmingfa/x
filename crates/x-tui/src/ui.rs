//! Drawing.
//!
//! Layout is fixed on purpose: a header with the tabs, a body that is one table
//! per view, a status line and a footer with the keys that currently do
//! something. Anything the user can act on is visible without scrolling back.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, Paragraph, Row, Scrollbar, ScrollbarOrientation, ScrollbarState,
    Table, Tabs,
};
use ratatui::Frame;

use x_core::network::is_default_route;
use x_core::port::ConnectionState;
use x_core::process::ProcessInfo;
use x_core::service::ServiceInfo;
use x_core::{format_bytes, format_duration};

use crate::app::{App, Modal, Target, View};

/// Draw the whole interface.
pub fn draw(frame: &mut Frame, app: &mut App) {
    let rows = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(3),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(frame.area());

    tabs(frame, app, rows[0]);
    body(frame, app, rows[1]);
    status(frame, app, rows[2]);
    footer(frame, rows[3]);

    if !matches!(app.modal(), Modal::None) {
        dialog(frame, app, frame.area());
    }
}

fn tabs(frame: &mut Frame, app: &App, area: Rect) {
    let titles = View::ALL.iter().map(|view| view.title().to_string());
    let index = View::ALL.iter().position(|view| *view == app.view());
    let tabs = Tabs::new(titles)
        .select(index)
        .block(Block::default().borders(Borders::ALL).title(" x "))
        .highlight_style(Style::default().fg(Color::Cyan).bold());
    frame.render_widget(tabs, area);
}

fn body(frame: &mut Frame, app: &mut App, area: Rect) {
    match app.view() {
        View::Ports => ports(frame, app, area),
        View::Processes => processes(frame, app, area),
        View::Network => network(frame, app, area),
        View::System => system(frame, app, area),
        View::Disk => disk(frame, app, area),
    }
}

fn table<'a>(headers: &[(&'a str, usize)], title: &str) -> Table<'a> {
    let widths: Vec<Constraint> = headers
        .iter()
        .map(|(_, width)| Constraint::Length(*width as u16))
        .collect();
    let header = Row::new(headers.iter().map(|(label, _)| Cell::from(*label)))
        .style(Style::default().fg(Color::Yellow).bold());
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
        Style::default().bg(Color::Blue).fg(Color::White)
    } else {
        Style::default()
    }
}

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
                row.endpoint(),
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
            ("local address", 40),
        ],
        "listening sockets",
    )
    .rows(rows);
    frame.render_widget(widget, area);
    scrollbar(frame, app, area);
}

fn processes(frame: &mut Frame, app: &mut App, area: Rect) {
    let inner_height = inner_rows(app, area);
    let start = app.scroll();
    let rows: Vec<Row> = app
        .processes()
        .iter()
        .enumerate()
        .skip(start)
        .take(inner_height.max(1))
        .map(|(index, row)| process_row(start + index, app.selected(), row))
        .collect();

    let widget = table(
        &[
            ("pid", 7),
            ("user", 12),
            ("cpu%", 7),
            ("mem", 10),
            ("name", 22),
            ("command", 60),
        ],
        "processes by cpu",
    )
    .rows(rows);
    frame.render_widget(widget, area);
    scrollbar(frame, app, area);
}

fn process_row(index: usize, selected: usize, row: &ProcessInfo) -> Row<'_> {
    Row::new(vec![
        row.pid.to_string(),
        row.user.clone().unwrap_or_default(),
        format!("{:.1}", row.cpu_usage.unwrap_or_default()),
        row.memory_bytes.map(format_bytes).unwrap_or_default(),
        row.name.clone(),
        row.command_line.clone().unwrap_or_default(),
    ])
    .style(selected_row(index, selected))
}

fn network(frame: &mut Frame, app: &mut App, area: Rect) {
    let columns =
        Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).split(area);

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
            Row::new(vec![
                row.name.clone(),
                format!("{:?}", row.state).to_lowercase(),
                row.mac_address.clone().unwrap_or_default(),
                row.mtu.map(|m| m.to_string()).unwrap_or_default(),
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
                ("interface", 12),
                ("state", 10),
                ("mac", 18),
                ("mtu", 6),
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
        Style::default().fg(Color::Yellow).bold(),
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
    text.push(Line::styled(
        "resolvers",
        Style::default().fg(Color::Yellow).bold(),
    ));
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
    text.push(Line::styled(
        "addresses",
        Style::default().fg(Color::Yellow).bold(),
    ));

    let addresses = app.context().network.addresses().unwrap_or_default();
    for address in addresses.iter().take(8) {
        text.push(Line::raw(format!(
            "  {:<10} {}/{}",
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
}

fn system(frame: &mut Frame, app: &mut App, area: Rect) {
    let columns =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(area);

    let mut facts: Vec<Line> = Vec::new();
    if let Some(info) = app.system() {
        facts.push(Line::raw(format!(
            "{} {} ({})",
            info.os_name, info.os_version, info.arch
        )));
        facts.push(Line::raw(format!("host {}", info.hostname)));
        facts.push(Line::raw(format!(
            "cpu   {} x {}",
            info.cpu_count,
            info.cpu_brand.clone().unwrap_or_else(|| "unknown".into())
        )));
        facts.push(Line::raw(format!(
            "mem   {} of {}",
            format_bytes(info.total_memory_bytes - info.available_memory_bytes),
            format_bytes(info.total_memory_bytes)
        )));
        facts.push(Line::raw(format!(
            "up    {}",
            format_duration(info.uptime_seconds)
        )));
    }
    facts.push(Line::from(""));
    facts.push(Line::raw(format!("cpu   {:.1}%", app.cpu())));
    if let Some(usage) = app.memory_usage() {
        facts.push(Line::raw(format!(
            "mem   {:.1}% ({} free)",
            usage.percent,
            format_bytes(usage.available_bytes)
        )));
        if usage.swap_total_bytes > 0 {
            facts.push(Line::raw(format!(
                "swap  {} of {}",
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
        columns[0],
    );

    let services: Vec<Row> = app
        .services()
        .iter()
        .map(|row: &ServiceInfo| {
            Row::new(vec![
                row.name.clone(),
                format!("{:?}", row.state).to_lowercase(),
                row.pid.map(|p| p.to_string()).unwrap_or_default(),
                match row.enabled {
                    Some(true) => "yes".to_string(),
                    Some(false) => "no".to_string(),
                    None => "unknown".to_string(),
                },
            ])
        })
        .collect();
    frame.render_widget(
        table(
            &[("service", 28), ("state", 10), ("pid", 7), ("enabled", 8)],
            "services",
        )
        .rows(services),
        columns[1],
    );
}

/// Mounted filesystems on top, the usage tree of the launch directory below.
fn disk(frame: &mut Frame, app: &mut App, area: Rect) {
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
        Paragraph::new(text).style(Style::default().fg(Color::Cyan)),
        area,
    );
}

fn footer(frame: &mut Frame, area: Rect) {
    let keys = "tab view   j/arrows move   / filter   r refresh   k kill   enter confirm   q quit";
    frame.render_widget(
        Paragraph::new(keys).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

fn dialog(frame: &mut Frame, app: &App, area: Rect) {
    let width = area.width.saturating_sub(8).min(76);
    let height = match app.modal() {
        Modal::Confirm {
            target: Target::Sockets(plan),
            ..
        } => (plan.sockets.len() as u16 + 7).min(area.height.saturating_sub(4)),
        _ => 8.min(area.height.saturating_sub(4)),
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
                Line::styled(label.to_string(), Style::default().fg(Color::Yellow).bold()),
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
                Style::default().fg(Color::Yellow).bold(),
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
                Style::default().fg(Color::Cyan),
            ));
            frame.render_widget(
                Paragraph::new(lines)
                    .block(Block::default().borders(Borders::ALL).title(" confirm ")),
                popup,
            );
        }
    }
}

fn state_label(state: ConnectionState) -> &'static str {
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
