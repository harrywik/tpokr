use super::super::cards::{CardFace, CardSize, CardWidget};
use super::{App, FELT, GOLD, MUTED, PANEL, RED, WHITE};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use tpokr::protocol::{PlayerStatus, RoomState, Street};

pub(super) fn draw_table(
    frame: &mut Frame<'_>,
    area: Rect,
    name: &str,
    state: Option<&RoomState>,
    app: &App,
) {
    if app.card_theme.ascii || !app.card_theme.color {
        draw_small(frame, area, name, state, 0, 0, app);
        return;
    }
    let (min_width, base_height) = match state.map(|s| s.capacity).unwrap_or(2) {
        7.. => (110, 40),
        4.. => (90, 34),
        _ => (72, 28),
    };
    let min_height = base_height + 3;
    if area.width < min_width || area.height < min_height {
        draw_small(frame, area, name, state, min_width, min_height, app);
        return;
    }
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .split(area);
    frame.render_widget(
        Paragraph::new(format!(" tPOKR  /  {name}"))
            .style(Style::default().fg(GOLD).add_modifier(Modifier::BOLD)),
        rows[0],
    );
    frame.render_widget(
        Paragraph::new(" h/l action  Enter act  j/k log  Esc/q leave  |  status number = stack")
            .style(Style::default().fg(MUTED)),
        rows[3],
    );
    draw_action_bar(frame, rows[2], app);
    let columns = Layout::horizontal([Constraint::Length(23), Constraint::Min(1)]).split(rows[1]);
    let log = state.map(|state| state.log.as_slice()).unwrap_or(&[]);
    let block = Block::default()
        .title(" Activity ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(MUTED));
    draw_activity(frame, columns[0], log, app.log_offset, Some(block), true);
    let Some(state) = state else {
        frame.render_widget(
            Paragraph::new("Connecting...")
                .alignment(Alignment::Center)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(GOLD)),
                ),
            columns[1],
        );
        return;
    };
    let table = columns[1];
    frame.render_widget(
        Block::default()
            .title(format!(
                " Table  {}/{} ",
                state.players.len(),
                state.capacity
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(GOLD))
            .style(Style::default().bg(PANEL)),
        table,
    );
    let inner = Rect::new(table.x + 1, table.y + 1, table.width - 2, table.height - 2);
    let rich_board = inner.width >= 65 && inner.height >= 29;
    let (board_size, board_y) = if rich_board {
        (CardSize::Large, inner.y + inner.height / 2 - 5)
    } else {
        (CardSize::Compact, inner.y + inner.height / 2 - 3)
    };
    let (board_card_width, board_card_height) = board_size.dimensions();
    // Place the pot below the whole board, not inside the larger cards.
    let pot = Rect::new(
        inner.x + inner.width / 2 - 6,
        board_y + board_card_height,
        12,
        1,
    );
    let board_width = 5 * board_card_width + 4;
    let board_x = inner.x + (inner.width - board_width) / 2;
    for index in 0..5 {
        let face = state
            .board
            .get(index)
            .copied()
            .map_or(CardFace::Empty, CardFace::Up);
        frame.render_widget(
            CardWidget {
                face,
                size: board_size,
                theme: app.card_theme,
            },
            Rect::new(
                board_x + index as u16 * (board_card_width + 1),
                board_y,
                board_card_width,
                board_card_height,
            ),
        );
    }
    frame.render_widget(
        Paragraph::new(format!("POT  {}", state.pot))
            .alignment(Alignment::Center)
            .style(Style::default().fg(GOLD).add_modifier(Modifier::BOLD)),
        pot,
    );
    // Fixed seats in join order, rotated so this client is always at the bottom.
    // The seat rectangles are spaced around an ellipse, never over the pot.
    let Some(me) = state
        .players
        .iter()
        .position(|player| player.id == state.you)
    else {
        return;
    };
    let n = state.capacity;
    let seat_w = 12u16;
    let seat_h = 5u16;
    let rx = (inner.width.saturating_sub(seat_w) as f64) / 2.0;
    let ry = (inner.height.saturating_sub(seat_h) as f64) / 2.0;
    for offset in 0..n {
        let angle = std::f64::consts::FRAC_PI_2 + offset as f64 * std::f64::consts::TAU / n as f64;
        let x = inner.x + (rx + rx * angle.cos()).round() as u16;
        let y = inner.y + (ry + ry * angle.sin()).round() as u16;
        let seat = Rect::new(x, y, seat_w, seat_h);
        // Empty slots come after the joined players. Rotate relative to the local player.
        let index = (me + offset) % n;
        if let Some(player) = state.players.get(index) {
            let label = if player.id == state.you {
                "You".to_owned()
            } else {
                format!("Player {}", player.id + 1)
            };
            let markers = [
                player.dealer.then_some("D"),
                player.small_blind.then_some("SB"),
                player.big_blind.then_some("BB"),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("/");
            let label = if markers.is_empty() {
                label
            } else {
                let full = format!("{label} {markers}");
                if full.len() > seat_w as usize {
                    format!("P{} {markers}", player.id + 1)
                } else {
                    full
                }
            };
            let status = if player.viewer {
                "viewer"
            } else if player.cash_out_pending {
                "cash out"
            } else {
                match player.status {
                    PlayerStatus::Waiting => "waiting",
                    PlayerStatus::Ready => "ready",
                    PlayerStatus::Disconnected => "offline",
                    PlayerStatus::Turn => "turn",
                    PlayerStatus::Folded => "folded",
                    PlayerStatus::Checked => "checked",
                    PlayerStatus::Raised => "raised",
                    PlayerStatus::Called => "called",
                    PlayerStatus::AllIn => "all-in",
                }
            };
            let visible = if player.id == state.you {
                state.cards
            } else {
                player.revealed_cards
            };
            frame.render_widget(
                Paragraph::new(label)
                    .alignment(Alignment::Center)
                    .style(Style::default().fg(WHITE)),
                Rect::new(seat.x, seat.y, seat.width, 1),
            );
            for index in 0..2 {
                let face = if let Some(cards) = visible {
                    CardFace::Up(cards[index])
                } else if player.viewer || state.hand_number == 0 {
                    CardFace::Empty
                } else {
                    CardFace::Down
                };
                frame.render_widget(
                    CardWidget {
                        face,
                        size: CardSize::Compact,
                        theme: app.card_theme,
                    },
                    Rect::new(seat.x + 1 + index as u16 * 5, seat.y + 1, 5, 3),
                );
            }
            frame.render_widget(
                Paragraph::new(if player.online {
                    format!("{status} {}", player.stack)
                } else {
                    format!("off/{status}")
                })
                .alignment(Alignment::Center)
                .style(Style::default().fg(
                    if player.status == PlayerStatus::Turn {
                        GOLD
                    } else {
                        MUTED
                    },
                )),
                Rect::new(seat.x, seat.y + 4, seat.width, 1),
            );
        } else {
            frame.render_widget(
                Paragraph::new("Empty seat")
                    .alignment(Alignment::Center)
                    .style(Style::default().fg(MUTED)),
                seat,
            );
        }
    }
}

fn draw_activity(
    frame: &mut Frame<'_>,
    area: Rect,
    entries: &[tpokr::protocol::LogEntry],
    offset: u16,
    block: Option<Block<'_>>,
    color: bool,
) {
    let mut lines: Vec<_> = entries.iter().map(log_line).collect();
    if !color {
        for line in &mut lines {
            line.style = Style::default();
            for span in &mut line.spans {
                span.style = Style::default();
            }
        }
    }
    let inner = block.as_ref().map_or(area, |block| block.inner(area));
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let mut paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    if color {
        paragraph = paragraph.style(Style::default().fg(WHITE));
    }
    if let Some(block) = block {
        paragraph = paragraph.block(block);
    }
    let height = paragraph.line_count(inner.width);
    let scroll = height
        .saturating_sub(inner.height as usize)
        .saturating_sub(offset as usize)
        .min(u16::MAX as usize) as u16;
    frame.render_widget(paragraph.scroll((scroll, 0)), area);
}

fn log_line(entry: &tpokr::protocol::LogEntry) -> Line<'static> {
    let (sender, color) = match (entry.player, &entry.name) {
        (Some(id), Some(name)) => (format!("[P{} - {}]: ", id + 1, name), WHITE),
        _ => ("[System]: ".into(), GOLD),
    };
    Line::from(vec![
        Span::styled(
            sender,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(entry.message.clone(), Style::default().fg(color)),
    ])
}

fn draw_action_bar(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let chosen = Style::default()
        .fg(FELT)
        .bg(GOLD)
        .add_modifier(Modifier::BOLD);
    let normal = Style::default().fg(MUTED);
    let actions = [
        " Check ",
        " Raise ",
        " Call ",
        " Fold ",
        " All-in ",
        " Cash out ",
    ];
    let spans = actions
        .iter()
        .enumerate()
        .map(|(index, label)| {
            Span::styled(
                *label,
                if index == app.selected_action {
                    chosen
                } else {
                    normal
                },
            )
        })
        .collect::<Vec<_>>();
    let hint = if app.confirming_cash_out {
        "Cash out after this hand? y/n".into()
    } else if app.editing_raise {
        let current = match &app.screen {
            super::Screen::Room {
                state: Some(state), ..
            } => Some(state.current_bet),
            _ => None,
        };
        let total = current.and_then(|bet| {
            app.raise_input
                .parse::<u64>()
                .ok()
                .and_then(|increase| bet.checked_add(increase))
        });
        format!(
            "Raise by: {}  Total: {}  Enter submit  Esc cancel",
            app.raise_input,
            total.map_or_else(|| "-".into(), |value| value.to_string())
        )
    } else if let Some(message) = &app.action_message {
        message.clone()
    } else if let super::Screen::Room {
        state: Some(state), ..
    } = &app.screen
    {
        if state.game_over {
            "Game over. See activity for results.".into()
        } else if state
            .players
            .iter()
            .find(|p| p.id == state.you)
            .is_some_and(|p| p.viewer)
        {
            "You are viewing. No more actions are available.".into()
        } else if state.paused {
            "Game paused: a player disconnected".into()
        } else if state.street == Street::Showdown {
            "Showdown. Next hand starts after one minute.".into()
        } else if state.street == Street::Finished {
            "Hand complete. Next hand starts shortly if enough players remain.".into()
        } else if state.cards.is_none() {
            "Waiting for players".into()
        } else {
            format!(
                "{:?}  |  Bet {}  |  Minimum raise to {}",
                state.street, state.current_bet, state.min_raise_to
            )
        }
    } else {
        "Waiting for players".into()
    };
    let lines = vec![Line::from(spans), Line::styled(hint, normal)];
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title(" Actions ")
                .borders(Borders::TOP)
                .border_style(Style::default().fg(GOLD)),
        ),
        area,
    );
}

fn card_text(card: tpokr::protocol::Card, ascii: bool) -> String {
    if !ascii {
        return card.to_string();
    }
    let suit = match card.suit {
        tpokr::protocol::Suit::Clubs => 'C',
        tpokr::protocol::Suit::Diamonds => 'D',
        tpokr::protocol::Suit::Hearts => 'H',
        tpokr::protocol::Suit::Spades => 'S',
    };
    let rank = match card.rank {
        11 => "J".into(),
        12 => "Q".into(),
        13 => "K".into(),
        14 => "A".into(),
        n => n.to_string(),
    };
    format!("{rank}{suit}")
}

fn draw_small(
    frame: &mut Frame<'_>,
    area: Rect,
    name: &str,
    state: Option<&RoomState>,
    min_width: u16,
    min_height: u16,
    app: &App,
) {
    let mut lines = vec![Line::styled(
        format!("tPOKR / {name}"),
        Style::default().fg(GOLD).add_modifier(Modifier::BOLD),
    )];
    if let Some(state) = state {
        lines.push(Line::raw(format!(
            "Players: {}/{}  Pot: {}",
            state.players.len(),
            state.capacity,
            state.pot
        )));
        lines.push(Line::raw(match state.cards {
            Some(cards) => format!(
                "Your cards: {}  {}",
                card_text(cards[0], app.card_theme.ascii),
                card_text(cards[1], app.card_theme.ascii)
            ),
            None => "Waiting for cards".into(),
        }));
        if min_width > 0 {
            lines.push(Line::styled(
                format!("Enlarge terminal for table view ({min_width}x{min_height})."),
                Style::default().fg(RED),
            ));
        }
        if app.card_theme.ascii || !app.card_theme.color {
            for player in &state.players {
                let status = if player.viewer {
                    "viewer"
                } else if !player.online {
                    "offline"
                } else {
                    match player.status {
                        PlayerStatus::Waiting => "waiting",
                        PlayerStatus::Ready => "ready",
                        PlayerStatus::Disconnected => "offline",
                        PlayerStatus::Turn => "turn",
                        PlayerStatus::Folded => "folded",
                        PlayerStatus::Checked => "checked",
                        PlayerStatus::Raised => "raised",
                        PlayerStatus::Called => "called",
                        PlayerStatus::AllIn => "all-in",
                    }
                };
                let cards = if player.id == state.you {
                    state.cards
                } else {
                    player.revealed_cards
                };
                let hand = if let Some(cards) = cards {
                    format!(
                        "{} {}",
                        card_text(cards[0], app.card_theme.ascii),
                        card_text(cards[1], app.card_theme.ascii)
                    )
                } else if player.viewer || state.hand_number == 0 {
                    "-- --".into()
                } else {
                    "## ##".into()
                };
                lines.push(Line::raw(format!(
                    "P{}: {status} {} [{hand}]",
                    player.id + 1,
                    player.stack
                )));
            }
        }
        lines.push(Line::raw(format!(
            "Board: {}",
            state
                .board
                .iter()
                .map(|card| card_text(*card, app.card_theme.ascii))
                .collect::<Vec<_>>()
                .join(" ")
        )));
    } else {
        lines.push(Line::raw("Connecting..."));
    }
    let top_height = (lines.len() as u16).min(area.height.saturating_sub(4));
    let rows = Layout::vertical([
        Constraint::Length(top_height),
        Constraint::Min(0),
        Constraint::Length(3),
    ])
    .split(area);
    let style = if app.card_theme.color {
        Style::default().fg(WHITE).bg(FELT)
    } else {
        Style::default()
    };
    if !app.card_theme.color {
        for line in &mut lines {
            line.style = Style::default();
            for span in &mut line.spans {
                span.style = Style::default();
            }
        }
    }
    frame.render_widget(Paragraph::new(lines).style(style), rows[0]);
    let log = state.map(|state| state.log.as_slice()).unwrap_or(&[]);
    draw_activity(
        frame,
        rows[1],
        log,
        app.log_offset,
        None,
        app.card_theme.color,
    );
    let names = ["Check", "Raise", "Call", "Fold", "All-in", "Cash out"];
    let mut lines = vec![Line::styled(
        format!("Action: {} (h/l)", names[app.selected_action]),
        Style::default().fg(GOLD),
    )];
    let hint = if app.confirming_cash_out {
        "Cash out? y/n".into()
    } else if app.editing_raise {
        let total = state.and_then(|s| {
            app.raise_input
                .parse::<u64>()
                .ok()
                .and_then(|increase| s.current_bet.checked_add(increase))
        });
        format!(
            "Raise by: {}  Total: {}",
            app.raise_input,
            total.map_or_else(|| "-".into(), |value| value.to_string())
        )
    } else {
        app.action_message
            .clone()
            .unwrap_or_else(|| "Enter: act".into())
    };
    lines.push(Line::styled(hint, Style::default().fg(MUTED)));
    lines.push(Line::styled(
        "h/l select  j/k log  Esc/q leave",
        Style::default().fg(MUTED),
    ));
    let style = if app.card_theme.color {
        Style::default().fg(WHITE).bg(FELT)
    } else {
        Style::default()
    };
    if !app.card_theme.color {
        for line in &mut lines {
            line.style = Style::default();
            for span in &mut line.spans {
                span.style = Style::default();
            }
        }
    }
    frame.render_widget(Paragraph::new(lines).style(style), rows[2]);
}
