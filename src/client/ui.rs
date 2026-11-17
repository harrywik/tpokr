use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

#[path = "table.rs"]
mod table;

use super::{App, Screen};

const FELT: Color = Color::Rgb(10, 42, 37);
const PANEL: Color = Color::Rgb(14, 54, 46);
const GOLD: Color = Color::Rgb(237, 194, 108);
const MUTED: Color = Color::Rgb(148, 181, 166);
const WHITE: Color = Color::Rgb(244, 239, 220);
const RED: Color = Color::Rgb(205, 79, 77);

const LOGO: [&str; 6] = [
    "       ____   ___  _  ______",
    "  _   |  _ \\ / _ \\| |/ /  _ \\",
    " | |_ | |_) | | | | ' /| |_) |",
    " | __||  __/| |_| | . \\|  _ <",
    " | |_ | |    \\___/|_|\\_\\_| \\_\\",
    "  \\__||_|",
];

pub(super) fn draw(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();
    frame.render_widget(Block::default().style(Style::default().bg(FELT)), area);

    if let Screen::Room { name, state } = &app.screen {
        table::draw_table(frame, area, name, state.as_ref(), app);
        return;
    }

    if area.width < 48 || area.height < 27 {
        draw_compact(frame, area, app);
        return;
    }

    let content = centered(area, 70, 27);
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(GOLD))
            .style(Style::default().bg(PANEL)),
        content,
    );
    let inner = Rect::new(
        content.x + 2,
        content.y + 1,
        content.width - 4,
        content.height - 2,
    );
    let rows = Layout::vertical([
        Constraint::Length(6),
        Constraint::Length(2),
        Constraint::Length(9),
        Constraint::Length(5),
        Constraint::Min(1),
    ])
    .split(inner);

    // Pad every line to the same width before centering the logo as one block.
    let width = LOGO.iter().map(|line| line.len()).max().unwrap_or(0);
    let logo = LOGO
        .iter()
        .map(|line| Line::from(format!("{line:<width$}")))
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(logo)
            .alignment(Alignment::Center)
            .style(Style::default().fg(GOLD).add_modifier(Modifier::BOLD)),
        rows[0],
    );
    frame.render_widget(
        Paragraph::new("WELCOME TO THE TABLE")
            .alignment(Alignment::Center)
            .style(Style::default().fg(MUTED).add_modifier(Modifier::BOLD)),
        rows[1],
    );
    draw_cards(frame, rows[2], app.frame);
    draw_prompt(frame, rows[3], app);
    draw_footer(frame, rows[4], app);
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

fn draw_cards(frame: &mut Frame<'_>, area: Rect, tick: u64) {
    const CARDS: [(&str, &str, Color); 4] = [
        ("A", "♠", FELT),
        ("K", "♥", RED),
        ("Q", "♣", FELT),
        ("J", "♦", RED),
    ];
    const WIDTH: u16 = 9;
    const GAP: u16 = 2;
    let total_width = WIDTH * 4 + GAP * 3;
    if area.width < total_width || area.height < 8 {
        return;
    }
    let x = area.x + (area.width - total_width) / 2;
    for (index, (rank, suit, color)) in CARDS.iter().enumerate() {
        // A small travelling wave lifts one card at a time, without blocking input.
        let lift = u16::from((tick as usize % 4) == index);
        let card = Rect::new(
            x + index as u16 * (WIDTH + GAP),
            area.y + 1 - lift,
            WIDTH,
            7,
        );
        let style = Style::default().fg(*color).bg(WHITE);
        let lines = vec![
            Line::from(format!("{rank} {suit}")),
            Line::from(""),
            Line::from(format!("  {suit}")),
            Line::from(""),
            Line::from(format!("    {rank}")),
        ];
        frame.render_widget(
            Paragraph::new(lines)
                .style(style)
                .block(Block::default().borders(Borders::ALL).border_style(style)),
            card,
        );
    }
}

fn prompt_lines(app: &App) -> Vec<Line<'static>> {
    let heading = Style::default().fg(WHITE).add_modifier(Modifier::BOLD);
    let selected = Style::default()
        .fg(GOLD)
        .bg(FELT)
        .add_modifier(Modifier::BOLD);
    let mut lines = match &app.screen {
        Screen::Select => {
            let choice = app
                .servers
                .get(app.selected)
                .map(|server| format!("  {}  ({})  ", server.name, server.address))
                .unwrap_or_else(|| "  No servers. Press a to add one.  ".into());
            vec![
                Line::styled("Select a server", heading),
                Line::from(""),
                Line::styled(choice, selected),
                Line::styled(
                    format!(
                        "{} / {}",
                        if app.servers.is_empty() {
                            0
                        } else {
                            app.selected + 1
                        },
                        app.servers.len()
                    ),
                    Style::default().fg(MUTED),
                ),
            ]
        }
        Screen::Edit {
            index,
            name,
            address,
            field,
        } => vec![
            Line::styled(
                if index.is_some() {
                    "Edit server"
                } else {
                    "Add server"
                },
                heading,
            ),
            Line::from(vec![
                Span::styled(if *field == 0 { "> Name: " } else { "  Name: " }, selected),
                Span::raw(name.clone()),
            ]),
            Line::from(vec![
                Span::styled(
                    if *field == 1 {
                        "> Address: "
                    } else {
                        "  Address: "
                    },
                    selected,
                ),
                Span::raw(address.clone()),
            ]),
            Line::styled(
                "Tab: switch field   Enter: save   Esc: cancel",
                Style::default().fg(MUTED),
            ),
        ],
        Screen::Delete => vec![
            Line::styled("Delete server?", heading),
            Line::from(app.servers[app.selected].name.clone()),
            Line::styled("y: confirm   n / Esc: cancel", Style::default().fg(MUTED)),
        ],
        Screen::Room { .. } => Vec::new(),
    };
    if let Some(message) = &app.message {
        lines.push(Line::styled(message.clone(), Style::default().fg(RED)));
    }
    lines
}

fn draw_prompt(frame: &mut Frame<'_>, area: Rect, app: &App) {
    frame.render_widget(
        Paragraph::new(prompt_lines(app)).alignment(Alignment::Center),
        area,
    );
}

fn draw_footer(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let text = match app.screen {
        Screen::Select => "j/k  select   Enter  join   a  add   e  edit   d  delete   q  quit",
        Screen::Room { .. } => "Esc / q  leave room",
        _ => "Ctrl+C  quit",
    };
    let footer = Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1);
    frame.render_widget(
        Paragraph::new(text)
            .alignment(Alignment::Center)
            .style(Style::default().fg(MUTED)),
        footer,
    );
}

fn draw_compact(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let mut lines = vec![
        Line::styled(
            "tPOKR",
            Style::default().fg(GOLD).add_modifier(Modifier::BOLD),
        ),
        Line::from(""),
    ];
    lines.extend(prompt_lines(app));
    lines.push(Line::from(""));
    lines.push(Line::styled(
        match app.screen {
            Screen::Select => "j/k select  Enter join  a/e/d  q quit",
            Screen::Room { .. } => "Esc/q leave",
            _ => "Ctrl+C quit",
        },
        Style::default().fg(MUTED),
    ));
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().fg(WHITE)),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn render(width: u16, height: u16, app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        let mut result = String::new();
        for y in 0..height {
            for x in 0..width {
                result.push_str(buffer[(x, y)].symbol());
            }
            result.push('\n');
        }
        result
    }

    #[test]
    fn renders_table_with_private_cards_and_opponents() {
        use tpokr::protocol::{Card, PlayerStatus, PlayerView, RoomState, Suit};
        let mut app = App::new(vec![]);
        app.screen = Screen::Room {
            name: "Local".into(),
            state: Some(RoomState {
                capacity: 2,
                you: 0,
                players: vec![
                    PlayerView {
                        id: 0,
                        status: PlayerStatus::Ready,
                        online: true,
                        name: "alice".into(),
                        revealed_cards: None,
                        viewer: false,
                        cash_out_pending: false,
                        cashed_out: 0,
                        dealer: true,
                        small_blind: true,
                        big_blind: false,
                        stack: 1000,
                        bet: 0,
                    },
                    PlayerView {
                        id: 1,
                        status: PlayerStatus::Turn,
                        online: true,
                        name: "bob".into(),
                        revealed_cards: None,
                        viewer: false,
                        cash_out_pending: false,
                        cashed_out: 0,
                        dealer: false,
                        small_blind: false,
                        big_blind: true,
                        stack: 1000,
                        bet: 0,
                    },
                ],
                cards: Some([
                    Card {
                        rank: 14,
                        suit: Suit::Spades,
                    },
                    Card {
                        rank: 13,
                        suit: Suit::Hearts,
                    },
                ]),
                pot: 42,
                current_bet: 10,
                min_raise_to: 20,
                big_blind_amount: 10,
                betting_complete: false,
                paused: false,
                street: tpokr::protocol::Street::Flop,
                board: vec![
                    Card {
                        rank: 2,
                        suit: Suit::Clubs,
                    },
                    Card {
                        rank: 3,
                        suit: Suit::Diamonds,
                    },
                    Card {
                        rank: 4,
                        suit: Suit::Hearts,
                    },
                ],
                hand_number: 1,
                game_over: false,
                pots: vec![],
                log: vec![tpokr::protocol::LogEntry {
                    player: Some(1),
                    name: Some("bob".into()),
                    message: "checked".into(),
                }],
                payouts: vec![],
            }),
        };
        let screen = render(80, 33, &app);
        for text in [
            "Player 2 BB",
            "You D/SB",
            "A♠",
            "K♥",
            "2♣",
            "3♦",
            "4♥",
            "###",
            "POT  42",
            "turn",
            "bob]: checked",
        ] {
            assert!(screen.contains(text), "missing {text}:\n{screen}");
        }
        let bar = render(80, 33, &app);
        assert!(bar.contains("Check"));
        assert!(bar.contains("Raise"));
        assert!(bar.contains("Minimum raise to 20"));
        let compact = render(40, 12, &app);
        assert!(compact.contains("Your cards: A♠  K♥"));
        assert!(compact.contains("Enlarge terminal"));
        render(1, 1, &app);
    }

    #[test]
    fn activity_wraps_and_colors_senders() {
        use tpokr::protocol::{LogEntry, RoomState, Street};
        let mut app = App::new(vec![]);
        app.screen = Screen::Room {
            name: "test".into(),
            state: Some(RoomState {
                capacity: 2,
                you: 0,
                players: vec![],
                cards: None,
                pot: 0,
                current_bet: 0,
                min_raise_to: 10,
                big_blind_amount: 10,
                betting_complete: false,
                paused: false,
                street: Street::Preflop,
                board: vec![],
                hand_number: 1,
                game_over: false,
                pots: vec![],
                payouts: vec![],
                log: vec![
                    LogEntry {
                        player: Some(0),
                        name: Some("alice".into()),
                        message: "joined".into(),
                    },
                    LogEntry {
                        player: None,
                        name: None,
                        message: "turn dealt with a long message that wraps".into(),
                    },
                ],
            }),
        };
        let mut terminal = Terminal::new(TestBackend::new(80, 33)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let buffer = terminal.backend().buffer();
        let mut text = String::new();
        for y in 0..33 {
            for x in 0..80 {
                text.push_str(buffer[(x, y)].symbol());
            }
            text.push('\n');
        }
        assert!(text.contains("[P1 - alice]: joined"));
        assert!(text.contains("turn dealt"));
        assert!(text.contains("message"), "{text}");
        assert!(text.contains("that wraps"), "{text}");
        let system_y = (0..33)
            .find(|&y| {
                (0..80).any(|x| {
                    buffer[(x, y)].symbol() == "S" && x > 0 && buffer[(x - 1, y)].symbol() == "["
                })
            })
            .unwrap();
        let player_y = (0..33)
            .find(|&y| {
                (0..80).any(|x| {
                    buffer[(x, y)].symbol() == "P" && x > 0 && buffer[(x - 1, y)].symbol() == "["
                })
            })
            .unwrap();
        assert_eq!(buffer[(2, system_y)].fg, GOLD);
        assert_eq!(buffer[(2, player_y)].fg, WHITE);
    }

    #[test]
    fn all_player_labels_fit_at_minimum_table_sizes() {
        use tpokr::protocol::{PlayerStatus, PlayerView, RoomState};
        for capacity in 2..=10 {
            let mut app = App::new(vec![]);
            app.screen = Screen::Room {
                name: "Local".into(),
                state: Some(RoomState {
                    capacity,
                    you: 0,
                    players: (0..capacity as u64)
                        .map(|id| PlayerView {
                            id,
                            status: PlayerStatus::Ready,
                            online: true,
                            name: "test".into(),
                            revealed_cards: None,
                            viewer: false,
                            cash_out_pending: false,
                            cashed_out: 0,
                            dealer: false,
                            small_blind: false,
                            big_blind: false,
                            stack: 1000,
                            bet: 0,
                        })
                        .collect(),
                    cards: None,
                    pot: 0,
                    current_bet: 0,
                    min_raise_to: 10,
                    big_blind_amount: 10,
                    betting_complete: false,
                    paused: false,
                    street: tpokr::protocol::Street::Preflop,
                    board: vec![],
                    hand_number: 1,
                    game_over: false,
                    pots: vec![],
                    log: vec![],
                    payouts: vec![],
                }),
            };
            let (width, height) = match capacity {
                7.. => (110, 43),
                4.. => (90, 37),
                _ => (72, 31),
            };
            let screen = render(width, height, &app);
            for id in 2..=capacity {
                assert!(
                    screen.contains(&format!("Player {id}")),
                    "capacity {capacity}, missing player {id}:\n{screen}"
                );
            }
            assert!(screen.contains("POT  0"));
        }
    }

    #[test]
    fn renders_selector_and_animation() {
        let mut app = App::new(vec![super::super::config::Server {
            name: "Local".into(),
            address: "localhost:7878".into(),
        }]);
        let first = render(80, 30, &app);
        let logo_width = LOGO.iter().map(|line| line.len()).max().unwrap();
        let lines: Vec<_> = first.lines().collect();
        let first_row = lines
            .iter()
            .position(|line| line.contains(LOGO[0].trim()))
            .unwrap();
        let start = lines[first_row].find(LOGO[0].trim()).unwrap()
            - (LOGO[0].len() - LOGO[0].trim_start().len());
        for (row, logo) in LOGO.iter().enumerate() {
            let expected = format!("{logo:<logo_width$}");
            assert_eq!(&lines[first_row + row][start..start + logo_width], expected);
        }
        assert!(first.contains("Select a server"));
        assert!(first.contains("Local"));
        assert!(first.contains('♠'));
        app.frame = 1;
        assert_ne!(first, render(80, 30, &app));
        assert!(render(30, 8, &app).contains("Select a server"));
        render(1, 1, &app);
    }
}
