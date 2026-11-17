use crate::protocol::Card;

/// Lexicographically ordered poker hand: category followed by kickers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct HandRank(pub [u8; 6]);

impl HandRank {
    pub fn name(self) -> &'static str {
        [
            "high card",
            "pair",
            "two pair",
            "three of a kind",
            "straight",
            "flush",
            "full house",
            "four of a kind",
            "straight flush",
        ][self.0[0] as usize]
    }
}

/// Best five-card hand from seven cards.
pub fn evaluate(cards: &[Card; 7]) -> HandRank {
    let mut best = HandRank([0; 6]);
    for a in 0..3 {
        for b in a + 1..4 {
            for c in b + 1..5 {
                for d in c + 1..6 {
                    for e in d + 1..7 {
                        best = best.max(evaluate_five([
                            cards[a], cards[b], cards[c], cards[d], cards[e],
                        ]));
                    }
                }
            }
        }
    }
    best
}

fn evaluate_five(cards: [Card; 5]) -> HandRank {
    let mut counts = [0u8; 15];
    for card in cards {
        counts[card.rank as usize] += 1;
    }
    let flush = cards.iter().all(|card| card.suit == cards[0].suit);
    let straight = (5..=14).rev().find(|&high| {
        (0..5).all(|offset| {
            let rank = high - offset;
            counts[if rank == 1 { 14 } else { rank }] > 0
        })
    });
    let mut groups: Vec<(u8, u8)> = (2..=14)
        .rev()
        .filter(|&rank| counts[rank] > 0)
        .map(|rank| (counts[rank], rank as u8))
        .collect();
    groups.sort_unstable_by(|a, b| b.cmp(a));
    let mut rank = [0u8; 6];
    if let Some(high) = straight.filter(|_| flush) {
        rank[0] = 8;
        rank[1] = high as u8;
    } else if groups[0].0 == 4 {
        rank[..3].copy_from_slice(&[7, groups[0].1, groups[1].1]);
    } else if groups[0].0 == 3 && groups[1].0 == 2 {
        rank[..3].copy_from_slice(&[6, groups[0].1, groups[1].1]);
    } else if flush {
        rank[0] = 5;
        for (i, (_, r)) in groups.iter().enumerate() {
            rank[i + 1] = *r;
        }
    } else if let Some(high) = straight {
        rank[0] = 4;
        rank[1] = high as u8;
    } else if groups[0].0 == 3 {
        rank[..4].copy_from_slice(&[3, groups[0].1, groups[1].1, groups[2].1]);
    } else if groups[0].0 == 2 && groups[1].0 == 2 {
        rank[..4].copy_from_slice(&[2, groups[0].1, groups[1].1, groups[2].1]);
    } else if groups[0].0 == 2 {
        rank[..5].copy_from_slice(&[1, groups[0].1, groups[1].1, groups[2].1, groups[3].1]);
    } else {
        for (i, (_, r)) in groups.iter().enumerate() {
            rank[i + 1] = *r;
        }
    }
    HandRank(rank)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Suit::{Clubs, Diamonds, Hearts, Spades};
    fn hand(ranks: [u8; 5], suits: [crate::protocol::Suit; 5]) -> HandRank {
        evaluate_five(std::array::from_fn(|i| Card {
            rank: ranks[i],
            suit: suits[i],
        }))
    }
    #[test]
    fn categories_and_kickers() {
        let mixed = [Clubs, Diamonds, Hearts, Spades, Clubs];
        let values = [
            hand([14, 13, 9, 7, 2], mixed),
            hand([14, 14, 9, 7, 2], mixed),
            hand([14, 14, 9, 9, 2], mixed),
            hand([14, 14, 14, 7, 2], mixed),
            hand([6, 5, 4, 3, 2], mixed),
            hand([14, 13, 9, 7, 2], [Hearts; 5]),
            hand([14, 14, 14, 2, 2], mixed),
            hand([14, 14, 14, 14, 2], mixed),
            hand([6, 5, 4, 3, 2], [Hearts; 5]),
        ];
        assert!(values.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(hand([14, 14, 13, 7, 2], mixed) > hand([14, 14, 12, 11, 10], mixed));
        assert!(hand([6, 5, 4, 3, 2], mixed) > hand([14, 5, 4, 3, 2], mixed));
    }
    #[test]
    fn exhaustive_five_card_category_counts() {
        let deck: Vec<_> = [Clubs, Diamonds, Hearts, Spades]
            .into_iter()
            .flat_map(|suit| (2..=14).map(move |rank| Card { rank, suit }))
            .collect();
        let mut counts = [0u32; 9];
        for a in 0..48 {
            for b in a + 1..49 {
                for c in b + 1..50 {
                    for d in c + 1..51 {
                        for e in d + 1..52 {
                            counts[evaluate_five([deck[a], deck[b], deck[c], deck[d], deck[e]]).0[0]
                                as usize] += 1;
                        }
                    }
                }
            }
        }
        assert_eq!(
            counts,
            [
                1_302_540, 1_098_240, 123_552, 54_912, 10_200, 5_108, 3_744, 624, 40
            ]
        );
    }

    #[test]
    fn best_of_seven_and_board_ties() {
        let board = [
            Card {
                rank: 10,
                suit: Hearts,
            },
            Card {
                rank: 11,
                suit: Hearts,
            },
            Card {
                rank: 12,
                suit: Hearts,
            },
            Card {
                rank: 13,
                suit: Hearts,
            },
            Card {
                rank: 14,
                suit: Hearts,
            },
        ];
        let a = [
            board[0],
            board[1],
            board[2],
            board[3],
            board[4],
            Card {
                rank: 2,
                suit: Clubs,
            },
            Card {
                rank: 3,
                suit: Clubs,
            },
        ];
        let b = [
            board[0],
            board[1],
            board[2],
            board[3],
            board[4],
            Card {
                rank: 9,
                suit: Spades,
            },
            Card {
                rank: 9,
                suit: Clubs,
            },
        ];
        assert_eq!(evaluate(&a), evaluate(&b));
        assert_eq!(evaluate(&a).name(), "straight flush");
    }
}
