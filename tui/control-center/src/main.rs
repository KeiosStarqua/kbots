use std::io;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Row, Table, Wrap},
};

const NAVIGATION: [&str; 6] = [
    "Overview",
    "Projects",
    "Tasks",
    "Agents",
    "Approvals",
    "Activity log",
];

struct App {
    selected: usize,
}

impl App {
    fn new() -> Self {
        Self { selected: 0 }
    }

    fn handle_key(&mut self, key: KeyCode) -> bool {
        match key {
            KeyCode::Char('q') | KeyCode::Esc => return true,
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = (self.selected + 1) % NAVIGATION.len();
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.checked_sub(1).unwrap_or(NAVIGATION.len() - 1);
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.selected = (self.selected + 1) % NAVIGATION.len();
            }
            KeyCode::Left | KeyCode::Char('h') => {
                self.selected = self.selected.checked_sub(1).unwrap_or(NAVIGATION.len() - 1);
            }
            KeyCode::Char('1'..='6') => {
                if let KeyCode::Char(number) = key {
                    self.selected = (number as usize) - ('1' as usize);
                }
            }
            _ => {}
        }
        false
    }
}

fn main() -> io::Result<()> {
    let mut app = App::new();
    ratatui::run(|terminal| {
        loop {
            terminal.draw(|frame| render(frame, &app))?;
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press && app.handle_key(key.code) {
                    break Ok(());
                }
            }
        }
    })
}

fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);

    render_header(frame, sections[0]);
    render_body(frame, sections[1], app);
    render_footer(frame, sections[2]);
}

fn render_header(frame: &mut Frame, area: Rect) {
    let line = if area.width < 70 {
        Line::from(vec![
            Span::styled(
                " kbots ",
                Style::default().fg(Color::Black).bg(Color::Green).bold(),
            ),
            Span::styled(" · DEMO", Style::default().fg(Color::Yellow)),
        ])
    } else if area.width < 112 {
        Line::from(vec![
            Span::styled(
                " kbots ",
                Style::default().fg(Color::Black).bg(Color::Green).bold(),
            ),
            Span::styled(
                " · Control Center · ",
                Style::default().fg(Color::White).bold(),
            ),
            Span::styled("DEMO", Style::default().fg(Color::Yellow)),
        ])
    } else {
        Line::from(vec![
            Span::styled(
                " kbots ",
                Style::default().fg(Color::Black).bg(Color::Green).bold(),
            ),
            Span::styled(
                "  CONTROL CENTER  ",
                Style::default().fg(Color::White).bold(),
            ),
            Span::raw("  │  "),
            Span::styled(
                "DEMO DATA · daemon not connected",
                Style::default().fg(Color::Yellow),
            ),
            Span::raw("  │  LOCAL WORKSPACE "),
        ])
    };
    let header = Paragraph::new(line).block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    frame.render_widget(header, area);
}

fn render_body(frame: &mut Frame, area: Rect, app: &App) {
    if area.width < 112 {
        render_compact_body(frame, area, app);
        return;
    }

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(22), Constraint::Min(0)])
        .split(area);

    let items = NAVIGATION
        .iter()
        .enumerate()
        .map(|(index, item)| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!(" {} ", index + 1),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::raw(*item),
            ]))
        })
        .collect::<Vec<_>>();
    let navigation = List::new(items)
        .block(
            Block::default()
                .title(" WORKSPACE ")
                .borders(Borders::RIGHT),
        )
        .highlight_style(
            Style::default()
                .fg(Color::LightGreen)
                .bg(Color::Rgb(35, 49, 40))
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("› ");
    let mut selected = ratatui::widgets::ListState::default();
    selected.select(Some(app.selected));
    frame.render_stateful_widget(navigation, columns[0], &mut selected);

    if app.selected == 0 {
        render_overview(frame, columns[1]);
    } else {
        render_placeholder(frame, columns[1], NAVIGATION[app.selected]);
    }
}

fn render_compact_body(frame: &mut Frame, area: Rect, app: &App) {
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(area);
    let nav = Paragraph::new(Line::from(vec![
        Span::styled("‹ ", Style::default().fg(Color::LightGreen).bold()),
        Span::styled(
            format!(
                "{}/{}  {}",
                app.selected + 1,
                NAVIGATION.len(),
                NAVIGATION[app.selected]
            ),
            Style::default().fg(Color::White).bold(),
        ),
        Span::styled("  ›", Style::default().fg(Color::LightGreen).bold()),
    ]));
    frame.render_widget(nav, sections[0]);

    if app.selected == 0 {
        render_compact_overview(frame, sections[1]);
    } else {
        render_placeholder(frame, sections[1], NAVIGATION[app.selected]);
    }
}

fn render_compact_overview(frame: &mut Frame, area: Rect) {
    if area.height < 9 {
        let summary = Paragraph::new(vec![
            Line::from(Span::styled(
                "Projects 3 · Tasks 12 · Agents 4 · Approvals 2",
                Style::default().fg(Color::LightGreen).bold(),
            )),
            Line::from(Span::styled(
                "Enlarge terminal for activity details",
                Style::default().fg(Color::Gray),
            )),
        ]);
        frame.render_widget(summary, area);
        return;
    }

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(6),
            Constraint::Length(5),
            Constraint::Min(4),
        ])
        .split(area);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("P ", Style::default().fg(Color::Cyan)),
            Span::styled("03   ", Style::default().fg(Color::White).bold()),
            Span::styled("T ", Style::default().fg(Color::LightGreen)),
            Span::styled("12   ", Style::default().fg(Color::White).bold()),
            Span::styled("A ", Style::default().fg(Color::Magenta)),
            Span::styled("04   ", Style::default().fg(Color::White).bold()),
            Span::styled("! ", Style::default().fg(Color::Yellow)),
            Span::styled("02", Style::default().fg(Color::White).bold()),
        ])),
        rows[0],
    );

    let agents = Paragraph::new(vec![
        Line::from(Span::styled(
            "AGENTS · DEMO",
            Style::default().fg(Color::Gray).bold(),
        )),
        Line::from(vec![
            Span::styled("CEO  ", Style::default().fg(Color::Magenta)),
            Span::raw("Planning · api-service"),
        ]),
        Line::from(vec![
            Span::styled("W01  ", Style::default().fg(Color::Cyan)),
            Span::raw("OAuth callback"),
        ]),
        Line::from(vec![
            Span::styled("GIT  ", Style::default().fg(Color::Yellow)),
            Span::raw("Awaiting integration"),
        ]),
        Line::from(vec![
            Span::styled("OPS  ", Style::default().fg(Color::LightGreen)),
            Span::raw("Running tests"),
        ]),
    ])
    .block(Block::default().borders(Borders::TOP));
    frame.render_widget(agents, rows[1]);

    let queue = Paragraph::new(vec![
        Line::from(Span::styled(
            "QUEUE",
            Style::default().fg(Color::Gray).bold(),
        )),
        Line::from("01 · Authentication [CODE]"),
        Line::from("02 · Billing dashboard [REVIEW]"),
        Line::from("03 · Webhook retries [QUEUED]"),
    ])
    .block(Block::default().borders(Borders::TOP));
    frame.render_widget(queue, rows[2]);

    let events = Paragraph::new(vec![
        Line::from(Span::styled(
            "RECENT EVENTS",
            Style::default().fg(Color::Gray).bold(),
        )),
        Line::from("10:42 worker.done · TASK-104"),
        Line::from("10:39 review.pending · TASK-102"),
    ])
    .block(Block::default().borders(Borders::TOP));
    frame.render_widget(events, rows[3]);
}

fn render_overview(frame: &mut Frame, area: Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(5),
            Constraint::Min(10),
            Constraint::Length(7),
        ])
        .split(area);

    let title = Paragraph::new(Line::from(vec![
        Span::styled("Overview", Style::default().fg(Color::White).bold()),
        Span::styled(
            "  Projects, agents, and workflow status",
            Style::default().fg(Color::Gray),
        ),
    ]))
    .block(Block::default().borders(Borders::BOTTOM));
    frame.render_widget(title, rows[0]);

    let metrics = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Ratio(1, 4); 4])
        .split(rows[1]);
    let cards = [
        ("PROJECTS", "03", "2 running", Color::Cyan),
        ("OPEN TASKS", "12", "4 ready", Color::LightGreen),
        (
            "ACTIVE AGENTS",
            "04",
            "3 coding · 1 planning",
            Color::Magenta,
        ),
        ("APPROVALS", "02", "waiting for review", Color::Yellow),
    ];
    for (area, (label, value, detail, accent)) in metrics.iter().zip(cards) {
        let card = Paragraph::new(vec![
            Line::from(Span::styled(label, Style::default().fg(Color::Gray).bold())),
            Line::from(vec![
                Span::styled(value, Style::default().fg(Color::White).bold()),
                Span::styled(format!("  {detail}"), Style::default().fg(accent)),
            ]),
        ])
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        );
        frame.render_widget(card, *area);
    }

    let panels = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
        .split(rows[2]);
    render_agents(frame, panels[0]);
    render_queue(frame, panels[1]);
    render_events(frame, rows[3]);
}

fn render_agents(frame: &mut Frame, area: Rect) {
    let rows = [
        Row::new(["CEO Bot", "Planning · api-service", "Planning"])
            .style(Style::default().fg(Color::Magenta)),
        Row::new(["Worker 01", "Implement OAuth callback", "Working"])
            .style(Style::default().fg(Color::Cyan)),
        Row::new(["GIT Bot", "Waiting for integration gate", "Idle"])
            .style(Style::default().fg(Color::Yellow)),
        Row::new(["OPS Bot", "api-service · test suite", "Running"])
            .style(Style::default().fg(Color::LightGreen)),
    ];
    let table = Table::new(
        rows,
        [
            Constraint::Length(14),
            Constraint::Min(20),
            Constraint::Length(12),
        ],
    )
    .header(
        Row::new(["AGENT", "CURRENT WORK", "STATE"]).style(Style::default().fg(Color::Gray).bold()),
    )
    .column_spacing(1)
    .block(
        Block::default()
            .title(" Agent activity ")
            .borders(Borders::ALL),
    );
    frame.render_widget(table, area);
}

fn render_queue(frame: &mut Frame, area: Rect) {
    let items = [
        "01  Implement authentication\n     api-service · CODING",
        "02  Update billing dashboard\n     merchant-portal · REVIEW",
        "03  Fix webhook retries\n     api-service · QUEUED",
    ];
    let queue = Paragraph::new(items.join("\n\n"))
        .style(Style::default().fg(Color::White))
        .wrap(Wrap { trim: false })
        .block(
            Block::default()
                .title(" Workflow queue ")
                .borders(Borders::ALL),
        );
    frame.render_widget(queue, area);
}

fn render_events(frame: &mut Frame, area: Rect) {
    let events = Paragraph::new(vec![
        Line::from(vec![
            Span::styled(
                "10:42:18  worker.completed  ",
                Style::default().fg(Color::LightGreen),
            ),
            Span::raw("Worker 01 finished TASK-104"),
        ]),
        Line::from(vec![
            Span::styled(
                "10:39:51  review.requested  ",
                Style::default().fg(Color::Magenta),
            ),
            Span::raw("TASK-102 is ready for CEO review"),
        ]),
    ])
    .block(
        Block::default()
            .title(" Recent events · sample ")
            .borders(Borders::ALL),
    );
    frame.render_widget(events, area);
}

fn render_placeholder(frame: &mut Frame, area: Rect, section: &str) {
    let placeholder = Paragraph::new(vec![
        Line::from(Span::styled(
            section,
            Style::default().fg(Color::White).bold(),
        )),
        Line::from("This view is ready for the daemon snapshot and event stream."),
        Line::from(Span::styled(
            "No backend connection is configured yet; displayed state is demo-only.",
            Style::default().fg(Color::Yellow),
        )),
    ])
    .wrap(Wrap { trim: true })
    .block(Block::default().borders(Borders::ALL));
    frame.render_widget(placeholder, area);
}

fn render_footer(frame: &mut Frame, area: Rect) {
    let line = if area.width < 70 {
        Line::from(vec![
            Span::styled("←/→ ", Style::default().fg(Color::LightGreen).bold()),
            Span::raw("sections  "),
            Span::styled("q ", Style::default().fg(Color::LightGreen).bold()),
            Span::raw("quit"),
        ])
    } else if area.width < 112 {
        Line::from(vec![
            Span::styled("←/→ or j/k ", Style::default().fg(Color::LightGreen).bold()),
            Span::raw("sections   "),
            Span::styled("1–6 ", Style::default().fg(Color::LightGreen).bold()),
            Span::raw("jump   "),
            Span::styled("q ", Style::default().fg(Color::LightGreen).bold()),
            Span::raw("quit"),
        ])
    } else {
        Line::from(vec![
            Span::styled(
                " ↑/↓ or j/k ",
                Style::default().fg(Color::LightGreen).bold(),
            ),
            Span::raw("navigate   "),
            Span::styled("1–6 ", Style::default().fg(Color::LightGreen).bold()),
            Span::raw("jump to section   "),
            Span::styled("q / Esc ", Style::default().fg(Color::LightGreen).bold()),
            Span::raw("quit and restore terminal"),
        ])
    };
    let footer = Paragraph::new(line);
    frame.render_widget(footer, area);
}
