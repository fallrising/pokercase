//! Minimal TUI: connections / routes / recent usage.

use std::io::{self, stdout};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Tabs};
use ratatui::Terminal;

use crate::config::AppConfig;
use crate::store::Store;

enum Tab {
    Connections,
    Routes,
    Usage,
}

impl Tab {
    fn next(&self) -> Self {
        match self {
            Tab::Connections => Tab::Routes,
            Tab::Routes => Tab::Usage,
            Tab::Usage => Tab::Connections,
        }
    }

    fn prev(&self) -> Self {
        match self {
            Tab::Connections => Tab::Usage,
            Tab::Routes => Tab::Connections,
            Tab::Usage => Tab::Routes,
        }
    }

    fn index(&self) -> usize {
        match self {
            Tab::Connections => 0,
            Tab::Routes => 1,
            Tab::Usage => 2,
        }
    }
}

pub fn run_tui(cfg: &AppConfig) -> Result<()> {
    let store = Store::open(&cfg.db_path(), cfg.secrets_key.clone())?;

    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;

    let mut tab = Tab::Connections;
    let result = loop {
        let conns = store.list_connections().unwrap_or_default();
        let routes = store.list_routes().unwrap_or_default();
        let usage = store.recent_usage(30).unwrap_or_default();
        let stats = store.stats().unwrap_or_else(|_| serde_json::json!({}));

        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Min(5),
                    Constraint::Length(2),
                ])
                .split(f.area());

            let titles = vec!["Connections", "Routes", "Usage"];
            let tabs = Tabs::new(titles)
                .select(tab.index())
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" thinrouter TUI "),
                )
                .highlight_style(
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                );
            f.render_widget(tabs, chunks[0]);

            let items: Vec<ListItem> = match tab {
                Tab::Connections => conns
                    .iter()
                    .map(|c| {
                        ListItem::new(Line::from(vec![
                            Span::styled(
                                format!("{:<16}", c.name),
                                Style::default().fg(Color::Green),
                            ),
                            Span::raw(format!("  {}  {}", c.base_url, c.default_model.as_deref().unwrap_or("—"))),
                        ]))
                    })
                    .collect(),
                Tab::Routes => routes
                    .iter()
                    .map(|r| {
                        let targets: Vec<_> = r
                            .targets
                            .iter()
                            .map(|t| {
                                format!(
                                    "{}{}",
                                    t.connection_name.as_deref().unwrap_or(&t.connection_id),
                                    t.model_override
                                        .as_ref()
                                        .map(|m| format!("→{m}"))
                                        .unwrap_or_default()
                                )
                            })
                            .collect();
                        ListItem::new(format!(
                            "{:<16} [{}]  {}",
                            r.public_model,
                            r.strategy,
                            targets.join(" | ")
                        ))
                    })
                    .collect(),
                Tab::Usage => usage
                    .iter()
                    .map(|e| {
                        ListItem::new(format!(
                            "{}  model={}  status={}  {}ms  {}",
                            e.ts,
                            e.public_model.as_deref().unwrap_or("—"),
                            e.status.map(|s| s.to_string()).unwrap_or_else(|| "—".into()),
                            e.latency_ms.unwrap_or(0),
                            e.error.as_deref().unwrap_or("")
                        ))
                    })
                    .collect(),
            };

            let list = List::new(items).block(Block::default().borders(Borders::ALL).title(
                match tab {
                    Tab::Connections => " Connections ",
                    Tab::Routes => " Routes ",
                    Tab::Usage => " Recent usage ",
                },
            ));
            f.render_widget(list, chunks[1]);

            let cost = stats
                .get("estimated_cost_usd")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let help = Paragraph::new(format!(
                "←/→ or Tab: switch  |  q: quit  |  db: {}  |  est. cost: ${cost:.6}",
                cfg.db_path().display()
            ))
            .style(Style::default().fg(Color::DarkGray));
            f.render_widget(help, chunks[2]);
        })?;

        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break Ok(()),
                    KeyCode::Right | KeyCode::Tab | KeyCode::Char('l') => tab = tab.next(),
                    KeyCode::Left | KeyCode::BackTab | KeyCode::Char('h') => tab = tab.prev(),
                    _ => {}
                }
            }
        }
    };

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    // ensure restore even on error
    let _ = io::Result::Ok(());
    result
}
