use crate::protocol::{Action, PlayerStatus, Pot};
use std::collections::{BTreeMap, BTreeSet};

pub const SMALL_BLIND: u64 = 5;
pub const BIG_BLIND: u64 = 10;
pub const STARTING_STACK: u64 = 1000;

#[derive(Debug, Clone)]
pub struct Seat {
    pub stack: u64,
    pub bet: u64,
    pub total: u64,
    pub folded: bool,
    pub status: PlayerStatus,
}

pub struct Betting {
    pub seats: BTreeMap<u64, Seat>,
    pub small_blind: u64,
    pub big_blind: u64,
    pub turn: Option<u64>,
    pub current_bet: u64,
    pub min_raise: u64,
    pub complete: bool,
    pub paused: bool,
    pending: BTreeSet<u64>,
    // Amount faced when the player last acted. Cumulative short raises may reopen action.
    acted_at: BTreeMap<u64, u64>,
}

impl Betting {
    #[cfg(test)]
    pub fn new(ids: &[u64]) -> Self {
        let stacks = ids.iter().map(|&id| (id, STARTING_STACK)).collect();
        Self::with_stacks(&stacks, ids[0])
    }

    pub fn with_stacks(stacks: &BTreeMap<u64, u64>, dealer: u64) -> Self {
        let ids: Vec<_> = stacks.keys().copied().collect();
        assert!(ids.len() >= 2 && stacks.values().all(|&stack| stack > 0));
        let dealer_index = ids
            .iter()
            .position(|&id| id == dealer)
            .expect("dealer in hand");
        let mut seats: BTreeMap<_, _> = ids
            .iter()
            .map(|&id| {
                (
                    id,
                    Seat {
                        stack: stacks[&id],
                        bet: 0,
                        total: 0,
                        folded: false,
                        status: PlayerStatus::Ready,
                    },
                )
            })
            .collect();
        let sb_index = if ids.len() == 2 {
            dealer_index
        } else {
            (dealer_index + 1) % ids.len()
        };
        let bb_index = (sb_index + 1) % ids.len();
        let sb = ids[sb_index];
        let bb = ids[bb_index];
        for (id, amount) in [(sb, SMALL_BLIND), (bb, BIG_BLIND)] {
            let seat = seats.get_mut(&id).unwrap();
            let posted = amount.min(seat.stack);
            seat.stack -= posted;
            seat.bet = posted;
            seat.total = posted;
            if seat.stack == 0 {
                seat.status = PlayerStatus::AllIn;
            }
        }
        let pending: BTreeSet<_> = seats
            .iter()
            .filter_map(|(&id, seat)| (seat.stack > 0).then_some(id))
            .collect();
        let start = if ids.len() == 2 {
            dealer_index
        } else {
            (bb_index + 1) % ids.len()
        };
        let first = (0..ids.len())
            .map(|i| ids[(start + i) % ids.len()])
            .find(|id| pending.contains(id));
        let current_bet = if pending.len() <= 1 {
            seats.values().map(|seat| seat.bet).max().unwrap_or(0)
        } else {
            BIG_BLIND
        };
        let only_matched =
            pending.len() == 1 && pending.iter().all(|id| seats[id].bet >= current_bet);
        let complete = pending.is_empty() || only_matched;
        Self {
            seats,
            small_blind: sb,
            big_blind: bb,
            turn: if complete { None } else { first },
            current_bet,
            min_raise: BIG_BLIND,
            complete,
            paused: false,
            pending,
            acted_at: BTreeMap::new(),
        }
    }

    pub fn pot(&self) -> u64 {
        self.seats.values().map(|s| s.total).sum()
    }
    pub fn min_raise_to(&self) -> u64 {
        self.current_bet.saturating_add(self.min_raise)
    }
    pub fn pots(&self) -> Vec<Pot> {
        let mut levels: Vec<_> = self
            .seats
            .values()
            .map(|s| s.total)
            .filter(|&b| b > 0)
            .collect();
        levels.sort_unstable();
        levels.dedup();
        let mut previous = 0;
        levels
            .into_iter()
            .map(|level| {
                let contributors = self.seats.values().filter(|s| s.total >= level).count() as u64;
                let amount = (level - previous) * contributors;
                previous = level;
                Pot {
                    amount,
                    eligible: self
                        .seats
                        .iter()
                        .filter_map(|(&id, s)| (s.total >= level && !s.folded).then_some(id))
                        .collect(),
                }
            })
            .collect()
    }
    pub fn next_round(&mut self, dealer: u64) {
        for seat in self.seats.values_mut() {
            seat.bet = 0;
            if !seat.folded && seat.stack > 0 {
                seat.status = PlayerStatus::Ready;
            }
        }
        self.current_bet = 0;
        self.min_raise = BIG_BLIND;
        self.acted_at.clear();
        self.pending = self
            .seats
            .iter()
            .filter_map(|(&id, seat)| (!seat.folded && seat.stack > 0).then_some(id))
            .collect();
        if self.pending.len() < 2 {
            self.complete = true;
            self.turn = None;
        } else {
            self.complete = false;
            self.turn = self
                .pending
                .range((
                    std::ops::Bound::Excluded(dealer),
                    std::ops::Bound::Unbounded,
                ))
                .next()
                .copied()
                .or_else(|| self.pending.first().copied());
        }
    }

    pub fn act(&mut self, id: u64, action: Action) -> Result<String, &'static str> {
        if self.paused {
            return Err("game paused: a player disconnected");
        }
        if self.complete {
            return Err("betting round is complete");
        }
        if self.turn != Some(id) {
            return Err("not your turn");
        }
        let seat = self.seats.get(&id).ok_or("unknown player")?;
        let to_call = self.current_bet.saturating_sub(seat.bet);
        let max = seat.bet + seat.stack;
        let target = match action {
            Action::Check => {
                if to_call != 0 {
                    return Err("cannot check while facing a bet");
                }
                None
            }
            Action::Fold => None,
            Action::Call => {
                if to_call == 0 {
                    return Err("nothing to call; check instead");
                }
                Some(max.min(self.current_bet))
            }
            Action::Raise { to } => {
                if to <= self.current_bet {
                    return Err("raise must exceed the current bet");
                }
                if to > max {
                    return Err("raise exceeds your stack");
                }
                if to < self.min_raise_to() && to != max {
                    return Err("raise is below the minimum");
                }
                Some(to)
            }
            Action::AllIn => Some(max),
            Action::CashOut => return Err("cash out is handled by the room"),
        };
        if let Some(to) = target
            && to > self.current_bet
        {
            if !self
                .seats
                .iter()
                .any(|(&other, seat)| other != id && !seat.folded && seat.stack > 0)
            {
                return Err("cannot raise without an opponent able to respond");
            }
            if self
                .acted_at
                .get(&id)
                .is_some_and(|at| self.current_bet - at < self.min_raise)
            {
                return Err("betting has not reopened for a raise");
            }
        }
        let old_bet = self.current_bet;
        let seat = self.seats.get_mut(&id).unwrap();
        if matches!(action, Action::Fold) {
            seat.folded = true;
            seat.status = PlayerStatus::Folded;
        }
        if let Some(to) = target {
            seat.stack -= to - seat.bet;
            seat.total += to - seat.bet;
            seat.bet = to;
        }
        let label = match action {
            Action::Check => {
                seat.status = PlayerStatus::Checked;
                "checked".to_owned()
            }
            Action::Fold => "folded".to_owned(),
            Action::Call => {
                seat.status = if seat.stack == 0 {
                    PlayerStatus::AllIn
                } else {
                    PlayerStatus::Called
                };
                format!("called {}", seat.bet)
            }
            Action::Raise { .. } => {
                seat.status = if seat.stack == 0 {
                    PlayerStatus::AllIn
                } else {
                    PlayerStatus::Raised
                };
                format!("raised to {}", seat.bet)
            }
            Action::AllIn => {
                seat.status = PlayerStatus::AllIn;
                format!("all-in to {}", seat.bet)
            }
            Action::CashOut => unreachable!(),
        };
        let new_bet = seat.bet;
        self.pending.remove(&id);
        if new_bet > old_bet {
            let increase = new_bet - old_bet;
            self.current_bet = new_bet;
            if increase >= self.min_raise {
                self.min_raise = increase;
                self.acted_at.clear();
                self.pending = self
                    .seats
                    .iter()
                    .filter_map(|(&other, seat)| {
                        (other != id && !seat.folded && seat.stack > 0).then_some(other)
                    })
                    .collect();
            } else {
                for (&other, seat) in &self.seats {
                    if other != id && !seat.folded && seat.stack > 0 && seat.bet < new_bet {
                        self.pending.insert(other);
                    }
                }
            }
        }
        self.acted_at.insert(id, self.current_bet);
        let active = self.seats.values().filter(|s| !s.folded).count();
        let actionable = self
            .seats
            .values()
            .filter(|s| !s.folded && s.stack > 0)
            .count();
        if active <= 1
            || self.pending.is_empty()
            || (actionable == 1
                && self
                    .pending
                    .iter()
                    .all(|id| self.seats[id].bet == self.current_bet))
        {
            self.complete = true;
            self.turn = None;
        } else {
            self.turn = self
                .pending
                .range((std::ops::Bound::Excluded(id), std::ops::Bound::Unbounded))
                .next()
                .copied()
                .or_else(|| self.pending.first().copied());
        }
        Ok(label)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn short_blinds_post_only_their_stacks() {
        let b = Betting::with_stacks(&BTreeMap::from([(0, 3), (1, 7), (2, 100)]), 0);
        assert_eq!(b.seats[&1].bet, 5);
        assert_eq!(b.seats[&2].bet, 10);
        assert_eq!(b.turn, Some(0));
        let b = Betting::with_stacks(&BTreeMap::from([(0, 3), (1, 7)]), 0);
        assert_eq!(b.pot(), 10);
        assert!(b.complete);
        let b = Betting::with_stacks(&BTreeMap::from([(0, 3), (1, 2)]), 0);
        assert!(b.complete);
        assert_eq!(b.pot(), 5);
    }

    #[test]
    fn heads_up_blinds_and_legality() {
        let mut b = Betting::new(&[0, 1]);
        assert_eq!((b.pot(), b.turn), (15, Some(0)));
        assert!(b.act(1, Action::Check).is_err());
        assert!(b.act(0, Action::Check).is_err());
        assert!(b.act(0, Action::Raise { to: 19 }).is_err());
        assert!(b.act(0, Action::Raise { to: 1001 }).is_err());
        b.act(0, Action::Call).unwrap();
        b.act(1, Action::Check).unwrap();
        assert!(b.complete);
        assert_eq!(b.pot(), 20);
    }
    #[test]
    fn full_raise_and_short_all_in_do_not_automatically_reopen() {
        let mut b = Betting::new(&[0, 1, 2]);
        b.seats.get_mut(&1).unwrap().stack = 20;
        b.act(0, Action::Raise { to: 20 }).unwrap();
        b.act(1, Action::AllIn).unwrap();
        assert_eq!(b.current_bet, 25);
        assert_eq!(b.min_raise_to(), 35);
        b.act(2, Action::Call).unwrap();
        assert!(b.act(0, Action::Raise { to: 35 }).is_err());
        b.act(0, Action::Call).unwrap();
        assert!(b.complete);
    }
    #[test]
    fn cumulative_short_all_ins_reopen_raising() {
        let mut b = Betting::new(&[0, 1, 2, 3]);
        b.seats.get_mut(&0).unwrap().stack = 25;
        b.seats.get_mut(&1).unwrap().stack = 30;
        b.act(3, Action::Raise { to: 20 }).unwrap();
        b.act(0, Action::AllIn).unwrap();
        b.act(1, Action::AllIn).unwrap();
        b.act(2, Action::Call).unwrap();
        assert_eq!(b.turn, Some(3));
        b.act(3, Action::Raise { to: 45 }).unwrap();
        assert_eq!(b.current_bet, 45);
    }

    #[test]
    fn no_raise_against_only_all_in_opponents() {
        let mut b = Betting::new(&[0, 1]);
        b.seats.get_mut(&0).unwrap().stack = 5;
        b.act(0, Action::AllIn).unwrap();
        assert!(b.complete);
        assert!(b.act(1, Action::Raise { to: 20 }).is_err());
    }

    #[test]
    fn next_round_preserves_contributions_and_starts_after_dealer() {
        let mut b = Betting::new(&[0, 1, 2]);
        b.act(0, Action::Call).unwrap();
        b.act(1, Action::Call).unwrap();
        b.act(2, Action::Check).unwrap();
        assert_eq!(b.pot(), 30);
        b.next_round(0);
        assert_eq!(b.turn, Some(1));
        assert_eq!(b.current_bet, 0);
        assert_eq!(b.min_raise_to(), 10);
        b.act(1, Action::Raise { to: 20 }).unwrap();
        b.act(2, Action::Call).unwrap();
        b.act(0, Action::Call).unwrap();
        assert_eq!(b.pot(), 90);
        assert_eq!(b.pots()[0].amount, 90);
    }

    #[test]
    fn side_pots_include_folded_contributions_but_not_folded_players() {
        let mut b = Betting::new(&[0, 1, 2]);
        b.seats.get_mut(&0).unwrap().total = 100;
        b.seats.get_mut(&1).unwrap().total = 50;
        b.seats.get_mut(&2).unwrap().total = 100;
        b.seats.get_mut(&2).unwrap().folded = true;
        assert_eq!(
            b.pots(),
            vec![
                Pot {
                    amount: 150,
                    eligible: vec![0, 1]
                },
                Pot {
                    amount: 100,
                    eligible: vec![0]
                }
            ]
        );
    }
}
