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
    let lines: Vec<_> = log.iter().map(log_line).collect();
    let block = Block::default()
        .title(" Activity ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(MUTED));
    let inner = block.inner(columns[0]);
    let paragraph = Paragraph::new(lines)
        .style(Style::default().fg(WHITE))
        .wrap(Wrap { trim: false })
        .block(block);
    let height = paragraph.line_count(inner.width);
    let scroll = height
        .saturating_sub(inner.height as usize)
        .saturating_sub(app.log_offset as usize)
        .min(u16::MAX as usize) as u16;
    frame.render_widget(paragraph.scroll((scroll, 0)), columns[0]);
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
    let pot = Rect::new(
        inner.x + inner.width / 2 - 6,
        inner.y + inner.height / 2,
        12,
        1,
    );
    let board = Rect::new(
        inner.x + inner.width / 2 - 13,
        inner.y + inner.height / 2 - 2,
        26,
        1,
    );
    let board_text = state
        .board
        .iter()
        .map(|card| format!("[{card}]"))
        .collect::<Vec<_>>()
        .join(" ");
    frame.render_widget(
        Paragraph::new(board_text)
            .alignment(Alignment::Center)
            .style(Style::default().fg(WHITE).add_modifier(Modifier::BOLD)),
        board,
    );
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
            let cards = if let Some(cards) = visible {
                format!("│{:<3}││{:<3}│", cards[0].to_string(), cards[1].to_string())
            } else if player.viewer || state.hand_number == 0 {
                "│ - ││ - │".to_owned()
            } else {
                "│###││###│".to_owned()
            };
            let lines = vec![
                Line::raw(label),
                Line::raw("┌───┐┌───┐"),
                Line::raw(cards),
                Line::raw("└───┘└───┘"),
                Line::styled(
                    if player.online {
                        format!("{status} {}", player.stack)
                    } else {
                        format!("off/{status}")
                    },
                    Style::default().fg(if player.status == PlayerStatus::Turn {
                        GOLD
                    } else {
                        MUTED
                    }),
                ),
            ];
            frame.render_widget(
                Paragraph::new(lines)
                    .alignment(Alignment::Center)
                    .style(Style::default().fg(WHITE)),
                seat,
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
        format!("Raise to: {}   Enter submit  Esc cancel", app.raise_input)
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
        } else if matches!(state.street, Street::Showdown | Street::Finished) {
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
            Some(cards) => format!("Your cards: {}  {}", cards[0], cards[1]),
            None => "Waiting for cards".into(),
        }));
        lines.push(Line::styled(
            format!("Enlarge terminal for table view ({min_width}x{min_height})."),
            Style::default().fg(RED),
        ));
        lines.push(Line::raw(format!(
            "Board: {}",
            state
                .board
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(" ")
        )));
        lines.push(Line::raw("Activity:"));
        let available = area.height.saturating_sub(9) as usize;
        lines.extend(
            state
                .log
                .iter()
                .skip(state.log.len().saturating_sub(available))
                .map(log_line),
        );
    } else {
        lines.push(Line::raw("Connecting..."));
    }
    let names = ["Check", "Raise", "Call", "Fold", "All-in", "Cash out"];
    lines.push(Line::styled(
        format!("Action: {} (h/l)", names[app.selected_action]),
        Style::default().fg(GOLD),
    ));
    let hint = if app.confirming_cash_out {
        "Cash out? y/n".into()
    } else if app.editing_raise {
        format!("Raise to: {}", app.raise_input)
    } else {
        app.action_message
            .clone()
            .unwrap_or_else(|| "Enter: act".into())
    };
    lines.push(Line::styled(hint, Style::default().fg(MUTED)));
    lines.push(Line::styled(
        "h/l select  Enter act  Esc/q leave",
        Style::default().fg(MUTED),
    ));
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().fg(WHITE).bg(FELT)),
        area,
    );
}
