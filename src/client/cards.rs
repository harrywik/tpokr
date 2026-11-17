use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, BorderType, Borders, Paragraph, Widget},
};
use tpokr::protocol::{Card, Suit};

#[derive(Clone, Copy)]
pub(super) struct CardTheme {
    pub ascii: bool,
    pub color: bool,
}

impl CardTheme {
    pub fn from_env() -> Self {
        Self {
            ascii: std::env::var_os("TPOKR_ASCII").is_some(),
            color: std::env::var_os("NO_COLOR").is_none(),
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum CardFace {
    Up(Card),
    Down,
    Empty,
}

#[derive(Clone, Copy)]
pub(super) enum CardSize {
    Large,
    Compact,
}

impl CardSize {
    pub fn dimensions(self) -> (u16, u16) {
        match self {
            Self::Large => (9, 7),
            Self::Compact => (5, 3),
        }
    }
}

pub(super) struct CardWidget {
    pub face: CardFace,
    pub size: CardSize,
    pub theme: CardTheme,
}

impl Widget for CardWidget {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let (width, height) = self.size.dimensions();
        if area.width < width || area.height < height {
            return;
        }
        let area = Rect::new(area.x, area.y, width, height);
        let (foreground, background) = match self.face {
            CardFace::Up(Card {
                suit: Suit::Hearts | Suit::Diamonds,
                ..
            }) => (Color::Rgb(175, 45, 51), Color::Rgb(244, 239, 220)),
            CardFace::Up(_) => (Color::Rgb(20, 42, 39), Color::Rgb(244, 239, 220)),
            CardFace::Down => (Color::Rgb(245, 215, 158), Color::Rgb(102, 35, 47)),
            CardFace::Empty => (Color::Rgb(148, 181, 166), Color::Rgb(14, 54, 46)),
        };
        let style = if self.theme.color {
            Style::default().fg(foreground).bg(background)
        } else {
            Style::default()
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(if self.theme.ascii {
                BorderType::Plain
            } else {
                BorderType::Rounded
            })
            .style(style)
            .border_style(style);
        // Plain uses box-drawing glyphs too. Choose ASCII borders explicitly.
        let block = if self.theme.ascii {
            block.border_set(ratatui::symbols::border::Set {
                top_left: "+",
                top_right: "+",
                bottom_left: "+",
                bottom_right: "+",
                vertical_left: "|",
                vertical_right: "|",
                horizontal_top: "-",
                horizontal_bottom: "-",
            })
        } else {
            block
        };
        let lines = match (self.face, self.size) {
            (CardFace::Up(card), size) => {
                let rank = match card.rank {
                    11 => "J".into(),
                    12 => "Q".into(),
                    13 => "K".into(),
                    14 => "A".into(),
                    n => n.to_string(),
                };
                let suit = match (card.suit, self.theme.ascii) {
                    (Suit::Clubs, false) => "♣",
                    (Suit::Diamonds, false) => "♦",
                    (Suit::Hearts, false) => "♥",
                    (Suit::Spades, false) => "♠",
                    (Suit::Clubs, true) => "C",
                    (Suit::Diamonds, true) => "D",
                    (Suit::Hearts, true) => "H",
                    (Suit::Spades, true) => "S",
                };
                match size {
                    CardSize::Compact => vec![Line::from(format!("{rank}{suit}"))],
                    CardSize::Large => vec![
                        Line::from(format!("{rank}{suit}")),
                        Line::from(""),
                        Line::from(format!("   {suit}")),
                        Line::from(""),
                        Line::from(format!("{rank:>6}{suit}")),
                    ],
                }
            }
            (CardFace::Down, CardSize::Compact) => vec![Line::from(if self.theme.ascii {
                "###"
            } else {
                "▒░▒"
            })],
            (CardFace::Down, CardSize::Large) => {
                let (a, b) = if self.theme.ascii {
                    ("#.#.#.#", ".#.#.#.")
                } else {
                    ("▒░▒░▒░▒", "░▒░▒░▒░")
                };
                vec![
                    Line::from(a),
                    Line::from(b),
                    Line::from(a),
                    Line::from(b),
                    Line::from(a),
                ]
            }
            (CardFace::Empty, _) => vec![Line::from(" ")],
        };
        Paragraph::new(lines)
            .style(style.add_modifier(Modifier::BOLD))
            .alignment(Alignment::Left)
            .block(block)
            .render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_suits_and_colors_and_ascii_fallback() {
        for (suit, expected, color) in [
            (Suit::Clubs, "♣", Color::Rgb(20, 42, 39)),
            (Suit::Diamonds, "♦", Color::Rgb(175, 45, 51)),
            (Suit::Hearts, "♥", Color::Rgb(175, 45, 51)),
            (Suit::Spades, "♠", Color::Rgb(20, 42, 39)),
        ] {
            let area = Rect::new(0, 0, 9, 7);
            let mut buf = Buffer::empty(area);
            CardWidget {
                face: CardFace::Up(Card { rank: 10, suit }),
                size: CardSize::Large,
                theme: CardTheme {
                    ascii: false,
                    color: true,
                },
            }
            .render(area, &mut buf);
            assert_eq!(buf[(3, 1)].symbol(), expected);
            assert_eq!(buf[(3, 1)].fg, color);
            assert_eq!(buf[(3, 1)].bg, Color::Rgb(244, 239, 220));
        }
        let area = Rect::new(0, 0, 5, 3);
        let mut buf = Buffer::empty(area);
        CardWidget {
            face: CardFace::Up(Card {
                rank: 14,
                suit: Suit::Hearts,
            }),
            size: CardSize::Compact,
            theme: CardTheme {
                ascii: true,
                color: false,
            },
        }
        .render(area, &mut buf);
        assert_eq!(buf[(0, 0)].symbol(), "+");
        assert_eq!(buf[(2, 1)].symbol(), "H");
        assert_eq!(buf[(2, 1)].fg, Color::Reset);
    }

    #[test]
    fn backs_and_empty_cards_do_not_expose_values() {
        let area = Rect::new(0, 0, 5, 3);
        let mut buf = Buffer::empty(area);
        CardWidget {
            face: CardFace::Down,
            size: CardSize::Compact,
            theme: CardTheme {
                ascii: true,
                color: false,
            },
        }
        .render(area, &mut buf);
        assert_eq!(buf[(1, 1)].symbol(), "#");
        CardWidget {
            face: CardFace::Empty,
            size: CardSize::Compact,
            theme: CardTheme {
                ascii: true,
                color: false,
            },
        }
        .render(area, &mut buf);
        assert_eq!(buf[(1, 1)].symbol(), " ");
    }
}
