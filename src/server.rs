use std::{
    collections::BTreeMap,
    io::{self, BufRead, BufReader, Read, Write},
    net::{Shutdown, TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        mpsc::{self, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};

use crate::betting::{BIG_BLIND, Betting, SMALL_BLIND, STARTING_STACK};
use crate::protocol::{
    Action, Card, LogEntry, Notification, Payout, PlayerStatus, PlayerView, Request, Response,
    RoomState, RpcError, Street, Suit,
};
use rand::seq::SliceRandom;

const MAX_MESSAGE: u64 = 4096;
const MAX_LOG: usize = 40;
const RESULT_DELAY: Duration = Duration::from_secs(5);

type Outbox = SyncSender<Vec<u8>>;

// Pots do not record whose chips go to whom. Match net losses to net gains in
// seat order to provide a deterministic summary without changing payouts.
fn net_transfers(changes: &[(u64, u64, u64)]) -> Vec<String> {
    let mut losses: Vec<_> = changes
        .iter()
        .filter_map(|&(id, before, after)| {
            (before > after).then_some((id, before.saturating_sub(after)))
        })
        .collect();
    let mut gains: Vec<_> = changes
        .iter()
        .filter_map(|&(id, before, after)| {
            (after > before).then_some((id, after.saturating_sub(before)))
        })
        .collect();
    let mut result = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < losses.len() && j < gains.len() {
        let amount = losses[i].1.min(gains[j].1);
        result.push(format!(
            "P{} -> P{}: {amount} chips",
            losses[i].0 + 1,
            gains[j].0 + 1
        ));
        losses[i].1 -= amount;
        gains[j].1 -= amount;
        if losses[i].1 == 0 {
            i += 1;
        }
        if gains[j].1 == 0 {
            j += 1;
        }
    }
    debug_assert_eq!(i, losses.len());
    debug_assert_eq!(j, gains.len());
    result
}

struct Player {
    cards: Option<[Card; 2]>,
    outbox: Option<Outbox>,
    socket: Option<TcpStream>,
    token: String,
    name: String,
    stack: u64,
    viewer: bool,
    cash_out_pending: bool,
    cashed_out: u64,
}

struct Room {
    capacity: usize,
    next_id: u64,
    players: BTreeMap<u64, Player>,
    deck: Vec<Card>,
    dealt: bool,
    betting: Option<Betting>,
    board: Vec<Card>,
    street: Street,
    log: Vec<LogEntry>,
    payouts: Vec<Payout>,
    dealer: Option<u64>,
    hand_number: u64,
    game_over: bool,
    next_hand_at: Option<Instant>,
}

impl Room {
    fn new(capacity: usize) -> io::Result<Self> {
        if !(2..=10).contains(&capacity) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "room size must be between 2 and 10",
            ));
        }
        Ok(Self {
            capacity,
            next_id: 0,
            players: BTreeMap::new(),
            deck: [Suit::Clubs, Suit::Diamonds, Suit::Hearts, Suit::Spades]
                .into_iter()
                .flat_map(|suit| (2..=14).map(move |rank| Card { rank, suit }))
                .collect(),
            dealt: false,
            betting: None,
            board: Vec::new(),
            street: Street::Preflop,
            log: Vec::new(),
            payouts: Vec::new(),
            dealer: None,
            hand_number: 0,
            game_over: false,
            next_hand_at: None,
        })
    }

    fn record(&mut self, entry: String) {
        self.log.push(LogEntry {
            player: None,
            name: None,
            message: entry,
        });
        if self.log.len() > MAX_LOG {
            self.log.remove(0);
        }
    }

    fn record_player(&mut self, id: u64, message: String) {
        let name = self.players[&id].name.clone();
        self.log.push(LogEntry {
            player: Some(id),
            name: Some(name),
            message,
        });
        if self.log.len() > MAX_LOG {
            self.log.remove(0);
        }
    }

    fn join(
        &mut self,
        outbox: Outbox,
        socket: TcpStream,
        token: &str,
        name: &str,
    ) -> Result<u64, &'static str> {
        if name.is_empty()
            || name.len() > 64
            || !name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        {
            return Err("invalid profile name");
        }
        if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("invalid session token");
        }
        if let Some((&id, player)) = self
            .players
            .iter_mut()
            .find(|(_, player)| player.token == token)
        {
            if player.outbox.is_some() {
                return Err("seat is already connected");
            }
            player.outbox = Some(outbox);
            player.socket = Some(socket);
            self.record_player(id, "rejoined".into());
            return Ok(id);
        }
        if self.dealt || self.players.len() >= self.capacity {
            return Err("room is full or the hand has already started");
        }
        let id = self.next_id;
        self.next_id += 1;
        self.players.insert(
            id,
            Player {
                cards: None,
                outbox: Some(outbox),
                socket: Some(socket),
                token: token.to_owned(),
                name: name.to_owned(),
                stack: STARTING_STACK,
                viewer: false,
                cash_out_pending: false,
                cashed_out: 0,
            },
        );
        self.record_player(id, "joined".into());
        if self.players.len() == self.capacity {
            self.dealt = true;
            self.start_hand();
            self.advance();
        }
        Ok(id)
    }

    fn start_hand(&mut self) {
        let stacks: BTreeMap<_, _> = self
            .players
            .iter()
            .filter_map(|(&id, p)| (!p.viewer && p.stack > 0).then_some((id, p.stack)))
            .collect();
        if stacks.len() < 2 {
            self.schedule_next_hand();
            return;
        }
        let dealer = match self.dealer {
            None => *stacks.keys().next().unwrap(),
            Some(previous) => stacks
                .range((
                    std::ops::Bound::Excluded(previous),
                    std::ops::Bound::Unbounded,
                ))
                .next()
                .or_else(|| stacks.first_key_value())
                .map(|(&id, _)| id)
                .unwrap(),
        };
        self.dealer = Some(dealer);
        self.hand_number += 1;
        self.next_hand_at = None;
        self.street = Street::Preflop;
        self.board.clear();
        self.payouts.clear();
        self.deck = [Suit::Clubs, Suit::Diamonds, Suit::Hearts, Suit::Spades]
            .into_iter()
            .flat_map(|suit| (2..=14).map(move |rank| Card { rank, suit }))
            .collect();
        self.deck.shuffle(&mut rand::rng());
        for player in self.players.values_mut() {
            player.cards = None;
        }
        let ids: Vec<_> = stacks.keys().copied().collect();
        let first: Vec<_> = ids.iter().map(|_| self.deck.pop().unwrap()).collect();
        for (id, card) in ids.into_iter().zip(first) {
            self.players.get_mut(&id).unwrap().cards = Some([card, self.deck.pop().unwrap()]);
        }
        self.betting = Some(Betting::with_stacks(&stacks, dealer));
        self.record(format!(
            "Hand {}: blinds {SMALL_BLIND}/{BIG_BLIND}",
            self.hand_number
        ));
        let betting = self.betting.as_ref().unwrap();
        let (sb, bb) = (betting.small_blind, betting.big_blind);
        let (sb_amount, bb_amount) = (betting.seats[&sb].bet, betting.seats[&bb].bet);
        self.record_player(sb, format!("posted small blind {sb_amount}"));
        self.record_player(bb, format!("posted big blind {bb_amount}"));
        self.record("Hole cards dealt".into());
    }

    fn schedule_next_hand(&mut self) {
        let active: Vec<_> = self
            .players
            .iter()
            .filter_map(|(&id, player)| (!player.viewer && player.stack > 0).then_some(id))
            .collect();
        if active.len() < 2 {
            self.next_hand_at = None;
            if !self.game_over {
                self.game_over = true;
                if let Some(id) = active.first() {
                    self.record(format!("Game over. P{} is the last player.", id + 1));
                } else {
                    self.record("Game over. No players remain.".into());
                }
            }
        } else if self.next_hand_at.is_none() {
            self.next_hand_at = Some(Instant::now() + RESULT_DELAY);
        }
    }

    fn tick(&mut self, now: Instant) -> bool {
        if self.next_hand_at.is_some_and(|deadline| now >= deadline) {
            self.start_hand();
            self.advance();
            self.broadcast();
            true
        } else {
            false
        }
    }

    fn complete_cash_out(&mut self, id: u64) {
        let player = self.players.get_mut(&id).unwrap();
        let amount = player.stack;
        player.stack = 0;
        player.cashed_out += amount;
        player.cash_out_pending = false;
        player.viewer = true;
        if let Some(betting) = &mut self.betting
            && let Some(seat) = betting.seats.get_mut(&id)
        {
            seat.stack = 0;
        }
        self.record_player(id, format!("cashed out {amount} chips; now viewing"));
    }

    fn cash_out(&mut self, id: u64) -> Result<(), &'static str> {
        let player = self.players.get_mut(&id).ok_or("unknown player")?;
        if !self.dealt {
            return Err("cash out is available after the game starts");
        }
        if player.viewer {
            return Err("you are already a viewer");
        }
        if player.cash_out_pending {
            return Err("cash out already requested");
        }
        if matches!(self.street, Street::Showdown | Street::Finished) || self.game_over {
            self.complete_cash_out(id);
            self.schedule_next_hand();
        } else {
            player.cash_out_pending = true;
            self.record_player(id, "will cash out after this hand".into());
        }
        Ok(())
    }

    fn state(&self, id: u64) -> Option<RoomState> {
        let dealer = self.dealer;
        let small_blind = self.betting.as_ref().map(|b| b.small_blind);
        let big_blind = self.betting.as_ref().map(|b| b.big_blind);
        self.players.get(&id).map(|player| RoomState {
            capacity: self.capacity,
            you: id,
            players: self
                .players
                .iter()
                .map(|(&id, player)| PlayerView {
                    id,
                    online: player.outbox.is_some(),
                    name: player.name.clone(),
                    revealed_cards: if self.street == Street::Showdown
                        && self
                            .betting
                            .as_ref()
                            .and_then(|b| b.seats.get(&id))
                            .is_some_and(|seat| !seat.folded)
                    {
                        player.cards
                    } else {
                        None
                    },
                    viewer: player.viewer,
                    cash_out_pending: player.cash_out_pending,
                    cashed_out: player.cashed_out,
                    dealer: dealer == Some(id),
                    small_blind: small_blind == Some(id),
                    big_blind: big_blind == Some(id),
                    stack: self
                        .betting
                        .as_ref()
                        .and_then(|b| b.seats.get(&id))
                        .map_or(player.stack, |s| s.stack),
                    bet: self
                        .betting
                        .as_ref()
                        .and_then(|b| b.seats.get(&id))
                        .map_or(0, |s| s.bet),
                    status: if let Some(b) = &self.betting {
                        if b.turn == Some(id) && !b.paused {
                            PlayerStatus::Turn
                        } else {
                            b.seats
                                .get(&id)
                                .map_or(PlayerStatus::Waiting, |seat| seat.status.clone())
                        }
                    } else {
                        PlayerStatus::Waiting
                    },
                })
                .collect(),
            cards: player.cards,
            pot: self.betting.as_ref().map_or(0, Betting::pot),
            current_bet: self.betting.as_ref().map_or(0, |b| b.current_bet),
            min_raise_to: self
                .betting
                .as_ref()
                .map_or(BIG_BLIND, Betting::min_raise_to),
            big_blind_amount: BIG_BLIND,
            betting_complete: self.betting.as_ref().is_some_and(|b| b.complete),
            paused: false,
            street: self.street,
            board: self.board.clone(),
            hand_number: self.hand_number,
            game_over: self.game_over,
            pots: self.betting.as_ref().map_or_else(Vec::new, Betting::pots),
            log: self.log.clone(),
            payouts: self.payouts.clone(),
        })
    }

    fn settle(&mut self, showdown: bool) {
        let betting = self.betting.as_mut().expect("betting started");
        let dealer = self.dealer.expect("dealer set");
        let starting: BTreeMap<_, _> = betting
            .seats
            .iter()
            .map(|(&id, seat)| (id, seat.stack + seat.total))
            .collect();
        // A contribution unmatched by any opponent is returned, not won as a side pot.
        let mut totals: Vec<_> = betting
            .seats
            .iter()
            .map(|(&id, seat)| (seat.total, id))
            .collect();
        totals.sort_unstable_by(|a, b| b.cmp(a));
        let excess = totals[0].0.saturating_sub(totals[1].0);
        if excess > 0 {
            let seat = betting.seats.get_mut(&totals[0].1).unwrap();
            seat.total -= excess;
            seat.bet = seat.bet.saturating_sub(excess);
            seat.stack += excess;
            self.payouts.push(Payout {
                player: totals[0].1,
                amount: excess,
                reason: "uncalled bet returned".into(),
            });
        }
        let ranks: BTreeMap<_, _> = if showdown {
            betting
                .seats
                .iter()
                .filter(|(_, seat)| !seat.folded)
                .map(|(&id, _)| {
                    let hole = self.players[&id].cards.unwrap();
                    let cards = [
                        hole[0],
                        hole[1],
                        self.board[0],
                        self.board[1],
                        self.board[2],
                        self.board[3],
                        self.board[4],
                    ];
                    (id, crate::hand::evaluate(&cards))
                })
                .collect()
        } else {
            BTreeMap::new()
        };
        for pot in betting.pots() {
            let best = pot
                .eligible
                .iter()
                .filter_map(|id| ranks.get(id))
                .max()
                .copied();
            let mut winners: Vec<_> = pot
                .eligible
                .iter()
                .copied()
                .filter(|id| !showdown || ranks.get(id).copied() == best)
                .collect();
            // Odd chips go clockwise starting with the first seat after the dealer.
            winners.sort_by_key(|id| if *id > dealer { (0, *id) } else { (1, *id) });
            assert!(!winners.is_empty(), "pot must have an eligible winner");
            let share = pot.amount / winners.len() as u64;
            let remainder = pot.amount % winners.len() as u64;
            for (index, id) in winners.into_iter().enumerate() {
                let amount = share + u64::from((index as u64) < remainder);
                betting.seats.get_mut(&id).unwrap().stack += amount;
                self.payouts.push(Payout {
                    player: id,
                    amount,
                    reason: best.map_or("won by fold", |rank| rank.name()).into(),
                });
            }
        }
        let changes: Vec<_> = betting
            .seats
            .iter()
            .map(|(&id, seat)| (id, starting[&id], seat.stack))
            .collect();
        let messages = net_transfers(&changes);
        for (&id, seat) in &mut betting.seats {
            seat.total = 0;
            seat.bet = 0;
            self.players.get_mut(&id).unwrap().stack = seat.stack;
        }
        if showdown {
            self.record("Showdown".into());
        }
        if messages.is_empty() {
            self.record("No net chip transfers".into());
        }
        for message in messages {
            self.record(message);
        }
        let ids: Vec<_> = self.players.keys().copied().collect();
        for id in ids {
            if self.players[&id].cash_out_pending {
                self.complete_cash_out(id);
            } else if !self.players[&id].viewer && self.players[&id].stack == 0 {
                self.players.get_mut(&id).unwrap().viewer = true;
                self.record_player(id, "is bankrupt and is now a viewer".into());
            }
        }
        self.schedule_next_hand();
    }

    fn advance(&mut self) {
        loop {
            let Some(betting) = &self.betting else { return };
            if betting.complete {
                if matches!(self.street, Street::Finished | Street::Showdown) {
                    return;
                }
                if betting.seats.values().filter(|seat| !seat.folded).count() <= 1 {
                    self.street = Street::Finished;
                    self.settle(false);
                    return;
                }
                let (street, count, label) = match self.street {
                    Street::Preflop => (Street::Flop, 3, "Flop"),
                    Street::Flop => (Street::Turn, 1, "Turn"),
                    Street::Turn => (Street::River, 1, "River"),
                    Street::River => {
                        self.street = Street::Showdown;
                        self.settle(true);
                        return;
                    }
                    _ => return,
                };
                self.deck.pop().expect("burn card");
                for _ in 0..count {
                    self.board.push(self.deck.pop().expect("community card"));
                }
                self.street = street;
                self.record(format!("{label} dealt"));
                let dealer = self.dealer.expect("dealer set");
                self.betting.as_mut().unwrap().next_round(dealer);
                continue;
            }
            let Some(id) = betting.turn else { return };
            if self.players[&id].outbox.is_some() {
                return;
            }
            let seat = &betting.seats[&id];
            let action = if seat.bet == betting.current_bet {
                Action::Check
            } else {
                Action::Fold
            };
            let entry = self
                .betting
                .as_mut()
                .unwrap()
                .act(id, action)
                .expect("offline action is legal");
            self.record_player(id, format!("{entry} (offline)"));
        }
    }

    // Called while holding the room lock. Queueing preserves ordering without socket I/O
    // under the lock. Slow clients are disconnected instead of blocking everyone else.
    fn broadcast(&self) {
        for (&id, player) in &self.players {
            if let Some(outbox) = &player.outbox {
                let notification = Notification {
                    jsonrpc: "2.0".into(),
                    method: "room.updated".into(),
                    params: self.state(id).unwrap(),
                };
                let mut bytes =
                    serde_json::to_vec(&notification).expect("room state is serializable");
                bytes.push(b'\n');
                if outbox.try_send(bytes).is_err()
                    && let Some(socket) = &player.socket
                {
                    let _ = socket.shutdown(Shutdown::Both);
                }
            }
        }
    }

    fn leave(&mut self, id: u64) {
        if self.dealt {
            if let Some(player) = self.players.get_mut(&id) {
                player.outbox = None;
                player.socket = None;
            }
            self.record_player(id, "disconnected".into());
            self.advance();
        } else if self.players.contains_key(&id) {
            self.record_player(id, "left".into());
            self.players.remove(&id);
        }
        self.broadcast();
    }
}

pub fn serve(listener: TcpListener, capacity: usize) -> io::Result<()> {
    let room = Arc::new(Mutex::new(Room::new(capacity)?));
    let timer_room = Arc::downgrade(&room);
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_millis(100));
            let Some(room) = timer_room.upgrade() else {
                break;
            };
            let Ok(mut room) = room.lock() else { break };
            room.tick(Instant::now());
        }
    });
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let room = Arc::clone(&room);
                thread::spawn(move || {
                    if let Err(error) = handle_client(stream, &room) {
                        eprintln!("client disconnected: {error}");
                    }
                });
            }
            Err(error) => eprintln!("accept failed: {error}"),
        }
    }
    Ok(())
}

fn handle_client(stream: TcpStream, room: &Mutex<Room>) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream.try_clone()?;
    let (outbox, receiver) = mpsc::sync_channel::<Vec<u8>>(32);
    let writer_thread = thread::spawn(move || {
        for bytes in receiver {
            if writer.write_all(&bytes).is_err() {
                break;
            }
        }
        let _ = writer.shutdown(Shutdown::Both);
    });
    let mut player = None;
    let result = (|| -> io::Result<()> {
        loop {
            let mut bytes = Vec::new();
            let size = (&mut reader)
                .take(MAX_MESSAGE + 1)
                .read_until(b'\n', &mut bytes)?;
            if size == 0 {
                break;
            }
            if size as u64 > MAX_MESSAGE || !bytes.ends_with(b"\n") {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "request too long or incomplete",
                ));
            }
            let request: Request = serde_json::from_slice(&bytes)?;
            let mut room = room
                .lock()
                .map_err(|_| io::Error::other("room lock poisoned"))?;
            let outcome = if request.jsonrpc != "2.0" {
                Err("expected JSON-RPC 2.0")
            } else {
                match (request.method.as_str(), player) {
                    ("join", None) => match room.join(
                        outbox.clone(),
                        stream.try_clone()?,
                        request.session.as_deref().unwrap_or(""),
                        request.profile.as_deref().unwrap_or("default"),
                    ) {
                        Ok(id) => {
                            player = Some(id);
                            stream.set_read_timeout(None)?;
                            Ok(room.state(id).unwrap())
                        }
                        Err(error) => Err(error),
                    },
                    ("join", Some(_)) => Err("already joined"),
                    ("act", Some(id)) if request.params == Some(Action::CashOut) => {
                        room.cash_out(id).map(|()| room.state(id).unwrap())
                    }
                    ("act", Some(id)) if room.players[&id].viewer => Err("viewers cannot act"),
                    ("act", Some(id)) => match (&mut room.betting, request.params.clone()) {
                        (Some(betting), Some(action)) => match betting.act(id, action) {
                            Ok(entry) => {
                                room.record_player(id, entry);
                                room.advance();
                                Ok(room.state(id).unwrap())
                            }
                            Err(error) => Err(error),
                        },
                        (None, _) => Err("waiting for players"),
                        (_, None) => Err("missing action"),
                    },
                    ("act", None) => Err("join first"),
                    _ => Err("unknown method"),
                }
            };
            let response = match outcome {
                Ok(state) => Response {
                    jsonrpc: "2.0".into(),
                    id: request.id,
                    result: Some(state),
                    error: None,
                },
                Err(message) => Response {
                    jsonrpc: "2.0".into(),
                    id: request.id,
                    result: None,
                    error: Some(RpcError {
                        code: -32000,
                        message: message.into(),
                    }),
                },
            };
            let mut bytes = serde_json::to_vec(&response)?;
            bytes.push(b'\n');
            outbox
                .try_send(bytes)
                .map_err(|_| io::Error::other("client output queue full"))?;
            if matches!(request.method.as_str(), "join" | "act")
                && player.is_some()
                && response.error.is_none()
            {
                room.broadcast();
            }
        }
        Ok(())
    })();
    if let Some(id) = player {
        room.lock()
            .map_err(|_| io::Error::other("room lock poisoned"))?
            .leave(id);
        let _ = stream.shutdown(Shutdown::Both);
        drop(outbox);
        let _ = writer_thread.join();
    } else {
        // Drain rejection responses before closing an unjoined connection.
        drop(outbox);
        let _ = writer_thread.join();
        let _ = stream.shutdown(Shutdown::Both);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn receive(reader: &mut BufReader<TcpStream>) -> serde_json::Value {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }
    fn send_join_token(stream: &mut TcpStream, token: &str) {
        serde_json::to_writer(
            &mut *stream,
            &Request {
                jsonrpc: "2.0".into(),
                id: 1,
                method: "join".into(),
                params: None,
                session: Some(token.into()),
                profile: Some("test".into()),
            },
        )
        .unwrap();
        stream.write_all(b"\n").unwrap();
    }
    fn send_join(stream: &mut TcpStream) {
        use rand::TryRng;
        let mut token = [0u8; 32];
        rand::rngs::SysRng
            .try_fill_bytes(&mut token)
            .expect("system random source unavailable");
        send_join_token(
            stream,
            &token.iter().map(|b| format!("{b:02x}")).collect::<String>(),
        );
    }
    fn connect(
        listener: &TcpListener,
        room: &Arc<Mutex<Room>>,
    ) -> (TcpStream, BufReader<TcpStream>, thread::JoinHandle<()>) {
        let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let (server, _) = listener.accept().unwrap();
        let room = Arc::clone(room);
        let handler = thread::spawn(move || {
            let _ = handle_client(server, &room);
        });
        let reader = BufReader::new(stream.try_clone().unwrap());
        (stream, reader, handler)
    }

    #[test]
    fn assigns_blind_positions_only_after_room_fills() {
        for capacity in [2, 3, 10] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let mut room = Room::new(capacity).unwrap();
            let mut peers = Vec::new();
            let mut receivers = Vec::new();
            for index in 0..capacity {
                let peer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
                let (socket, _) = listener.accept().unwrap();
                let (sender, receiver) = mpsc::sync_channel(32);
                room.join(sender, socket, &format!("{index:064x}"), "test")
                    .unwrap();
                peers.push(peer);
                receivers.push(receiver);
                if index + 1 < capacity {
                    assert!(
                        room.state(0)
                            .unwrap()
                            .players
                            .iter()
                            .all(|p| !p.dealer && !p.small_blind && !p.big_blind)
                    );
                }
            }
            let players = room.state(0).unwrap().players;
            assert!(players[0].dealer);
            assert!(players[if capacity == 2 { 0 } else { 1 }].small_blind);
            assert!(players[if capacity == 2 { 1 } else { 2 }].big_blind);
            assert_eq!(players.iter().filter(|p| p.dealer).count(), 1);
            assert_eq!(players.iter().filter(|p| p.small_blind).count(), 1);
            assert_eq!(players.iter().filter(|p| p.big_blind).count(), 1);
        }
    }

    #[test]
    fn one_deck_deals_unique_cards_for_every_room_size() {
        for capacity in 2..=10 {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let mut room = Room::new(capacity).unwrap();
            let mut peers = Vec::new();
            let mut receivers = Vec::new();
            for index in 0..capacity {
                let peer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
                let (socket, _) = listener.accept().unwrap();
                let (sender, receiver) = mpsc::sync_channel(32);
                room.join(sender, socket, &format!("{index:064x}"), "test")
                    .unwrap();
                peers.push(peer);
                receivers.push(receiver);
            }
            let cards: HashSet<_> = room
                .players
                .values()
                .flat_map(|player| player.cards.unwrap())
                .collect();
            assert_eq!(cards.len(), capacity * 2);
            assert_eq!(room.deck.len(), 52 - capacity * 2);
            assert!(room.deck.iter().all(|card| !cards.contains(card)));
        }
    }

    #[test]
    fn pushed_updates_are_ordered_and_private() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let room = Arc::new(Mutex::new(Room::new(2).unwrap()));
        let (mut a, mut ar, ah) = connect(&listener, &room);
        send_join(&mut a);
        assert!(receive(&mut ar)["result"]["cards"].is_null());
        assert_eq!(receive(&mut ar)["method"], "room.updated");
        let (mut b, mut br, bh) = connect(&listener, &room);
        send_join(&mut b);
        let b_state: RoomState =
            serde_json::from_value(receive(&mut br)["result"].clone()).unwrap();
        let b_notification: Notification = serde_json::from_value(receive(&mut br)).unwrap();
        let a_notification: Notification = serde_json::from_value(receive(&mut ar)).unwrap();
        let ac = a_notification.params.cards.unwrap();
        let bc = b_state.cards.unwrap();
        assert_eq!(b_notification.params.cards, Some(bc));
        assert!(ac.iter().all(|card| !bc.contains(card)));
        assert_eq!(a_notification.params.players.len(), 2);
        assert_eq!(room.lock().unwrap().deck.len(), 48);
        assert_eq!(
            room.lock()
                .unwrap()
                .deck
                .iter()
                .chain(ac.iter())
                .chain(bc.iter())
                .copied()
                .collect::<HashSet<_>>()
                .len(),
            52
        );
        drop(a);
        drop(ar);
        ah.join().unwrap();
        let disconnected: Notification = serde_json::from_value(receive(&mut br)).unwrap();
        assert!(!disconnected.params.players[0].online);
        assert_eq!(disconnected.params.players[0].status, PlayerStatus::Folded);
        drop(b);
        drop(br);
        bh.join().unwrap();
    }

    #[test]
    fn actions_are_validated_and_broadcast_to_both_players() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let room = Arc::new(Mutex::new(Room::new(2).unwrap()));
        let (mut a, mut ar, ah) = connect(&listener, &room);
        send_join(&mut a);
        receive(&mut ar);
        receive(&mut ar);
        let (mut b, mut br, bh) = connect(&listener, &room);
        send_join(&mut b);
        receive(&mut br);
        receive(&mut br);
        receive(&mut ar);
        let request = |id, action| Request {
            jsonrpc: "2.0".into(),
            id,
            method: "act".into(),
            params: Some(action),
            session: None,
            profile: None,
        };
        serde_json::to_writer(&mut b, &request(2, crate::protocol::Action::Check)).unwrap();
        b.write_all(b"\n").unwrap();
        assert!(receive(&mut br)["error"].is_object());
        serde_json::to_writer(&mut a, &request(2, crate::protocol::Action::Call)).unwrap();
        a.write_all(b"\n").unwrap();
        assert_eq!(receive(&mut ar)["result"]["pot"], 20);
        let update_a: Notification = serde_json::from_value(receive(&mut ar)).unwrap();
        let update_b: Notification = serde_json::from_value(receive(&mut br)).unwrap();
        assert_eq!(update_a.params.pot, 20);
        assert_eq!(update_b.params.pot, 20);
        assert_eq!(update_a.params.players[1].status, PlayerStatus::Turn);
        drop(a);
        drop(ar);
        ah.join().unwrap();
        let update: Notification = serde_json::from_value(receive(&mut br)).unwrap();
        assert!(!update.params.players[0].online);
        assert!(!update.params.paused);
        drop(b);
        drop(br);
        bh.join().unwrap();
    }

    #[test]
    fn rejoin_restores_private_seat_and_disables_auto_play() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let room = Arc::new(Mutex::new(Room::new(3).unwrap()));
        let token = "ab".repeat(32);
        let (mut a, mut ar, ah) = connect(&listener, &room);
        send_join_token(&mut a, &token);
        receive(&mut ar);
        receive(&mut ar);
        let (mut b, mut br, bh) = connect(&listener, &room);
        send_join(&mut b);
        receive(&mut br);
        receive(&mut br);
        receive(&mut ar);
        let (mut c, mut cr, ch) = connect(&listener, &room);
        send_join(&mut c);
        receive(&mut cr);
        receive(&mut cr);
        let a_state: Notification = serde_json::from_value(receive(&mut ar)).unwrap();
        receive(&mut br);
        let cards = a_state.params.cards;
        let (mut duplicate, mut dr, dh) = connect(&listener, &room);
        send_join_token(&mut duplicate, &token);
        assert!(receive(&mut dr)["error"].is_object());
        drop(duplicate);
        drop(dr);
        dh.join().unwrap();
        drop(a);
        drop(ar);
        ah.join().unwrap();
        let gone: Notification = serde_json::from_value(receive(&mut br)).unwrap();
        receive(&mut cr);
        assert_eq!(gone.params.players[0].status, PlayerStatus::Folded);
        assert!(!gone.params.players[0].online);
        let (mut rejoined, mut rr, rh) = connect(&listener, &room);
        send_join_token(&mut rejoined, &token);
        let state: RoomState = serde_json::from_value(receive(&mut rr)["result"].clone()).unwrap();
        assert_eq!(state.you, 0);
        assert_eq!(state.cards, cards);
        assert!(state.players[0].online);
        assert_eq!(state.players[0].status, PlayerStatus::Folded);
        receive(&mut rr);
        receive(&mut br);
        receive(&mut cr);
        drop(rejoined);
        drop(rr);
        rh.join().unwrap();
        drop(b);
        drop(br);
        bh.join().unwrap();
        drop(c);
        drop(cr);
        ch.join().unwrap();
    }

    fn test_room(capacity: usize) -> (Room, Vec<TcpStream>, Vec<mpsc::Receiver<Vec<u8>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut room = Room::new(capacity).unwrap();
        let mut peers = Vec::new();
        let mut receivers = Vec::new();
        for id in 0..capacity {
            let peer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
            let (socket, _) = listener.accept().unwrap();
            let (sender, receiver) = mpsc::sync_channel(32);
            room.join(sender, socket, &format!("{id:064x}"), &format!("user{id}"))
                .unwrap();
            peers.push(peer);
            receivers.push(receiver);
        }
        (room, peers, receivers)
    }

    #[test]
    fn net_transfer_summary_matches_losses_and_gains() {
        assert_eq!(
            net_transfers(&[(0, 100, 180), (1, 100, 40), (2, 100, 80)]),
            vec!["P2 -> P1: 60 chips", "P3 -> P1: 20 chips"]
        );
        assert_eq!(
            net_transfers(&[(0, 100, 70), (1, 100, 110), (2, 100, 120)]),
            vec!["P1 -> P2: 10 chips", "P1 -> P3: 20 chips"]
        );
        assert!(net_transfers(&[(0, 100, 100), (1, 100, 100)]).is_empty());
    }

    #[test]
    fn payouts_side_pots_uncalled_chips_and_conserve_stacks() {
        use Suit::*;
        let (mut room, _peers, _receivers) = test_room(3);
        room.board = vec![
            Card {
                rank: 2,
                suit: Clubs,
            },
            Card {
                rank: 3,
                suit: Diamonds,
            },
            Card {
                rank: 7,
                suit: Hearts,
            },
            Card {
                rank: 9,
                suit: Spades,
            },
            Card {
                rank: 11,
                suit: Clubs,
            },
        ];
        room.players.get_mut(&0).unwrap().cards = Some([
            Card {
                rank: 14,
                suit: Hearts,
            },
            Card {
                rank: 14,
                suit: Spades,
            },
        ]);
        room.players.get_mut(&1).unwrap().cards = Some([
            Card {
                rank: 13,
                suit: Hearts,
            },
            Card {
                rank: 13,
                suit: Spades,
            },
        ]);
        room.players.get_mut(&2).unwrap().cards = Some([
            Card {
                rank: 12,
                suit: Hearts,
            },
            Card {
                rank: 12,
                suit: Spades,
            },
        ]);
        let betting = room.betting.as_mut().unwrap();
        for (id, total) in [(0, 50), (1, 100), (2, 150)] {
            let seat = betting.seats.get_mut(&id).unwrap();
            seat.total = total;
            seat.stack = 1000 - total;
        }
        room.street = Street::Showdown;
        room.settle(true);
        let b = room.betting.as_ref().unwrap();
        assert_eq!(b.seats[&0].stack, 1100);
        assert_eq!(b.seats[&1].stack, 1000);
        assert_eq!(b.seats[&2].stack, 900);
        assert_eq!(b.pot(), 0);
        assert_eq!(b.seats.values().map(|s| s.stack).sum::<u64>(), 3000);
        assert!(
            room.log
                .iter()
                .any(|entry| entry.message == "P3 -> P1: 100 chips")
        );
        assert!(
            room.payouts
                .iter()
                .any(|p| p.player == 2 && p.amount == 50 && p.reason == "uncalled bet returned")
        );
        assert!(
            room.state(0)
                .unwrap()
                .players
                .iter()
                .all(|p| p.revealed_cards.is_some())
        );
    }

    #[test]
    fn folded_overcontributor_does_not_win_a_pot() {
        use Suit::*;
        let (mut room, _peers, _receivers) = test_room(3);
        room.board = vec![
            Card {
                rank: 2,
                suit: Clubs,
            },
            Card {
                rank: 3,
                suit: Diamonds,
            },
            Card {
                rank: 7,
                suit: Hearts,
            },
            Card {
                rank: 9,
                suit: Spades,
            },
            Card {
                rank: 11,
                suit: Clubs,
            },
        ];
        room.players.get_mut(&0).unwrap().cards = Some([
            Card {
                rank: 14,
                suit: Hearts,
            },
            Card {
                rank: 14,
                suit: Spades,
            },
        ]);
        room.players.get_mut(&1).unwrap().cards = Some([
            Card {
                rank: 13,
                suit: Hearts,
            },
            Card {
                rank: 13,
                suit: Spades,
            },
        ]);
        let b = room.betting.as_mut().unwrap();
        for (id, total) in [(0, 50), (1, 75), (2, 100)] {
            let seat = b.seats.get_mut(&id).unwrap();
            seat.total = total;
            seat.stack = 1000 - total;
        }
        b.seats.get_mut(&2).unwrap().folded = true;
        room.street = Street::Showdown;
        room.settle(true);
        assert_eq!(
            room.betting
                .as_ref()
                .unwrap()
                .seats
                .values()
                .map(|s| s.stack)
                .sum::<u64>(),
            3000
        );
        assert!(
            room.payouts
                .iter()
                .all(|p| p.player != 2 || p.reason == "uncalled bet returned")
        );
    }

    #[test]
    fn ties_split_odd_chip_clockwise_and_folded_cards_stay_private() {
        use Suit::*;
        let (mut room, _peers, _receivers) = test_room(3);
        room.board = (10..=14).map(|rank| Card { rank, suit: Hearts }).collect();
        let betting = room.betting.as_mut().unwrap();
        for seat in betting.seats.values_mut() {
            seat.total = 5;
            seat.stack = 995;
        }
        betting.seats.get_mut(&2).unwrap().folded = true;
        room.street = Street::Showdown;
        room.settle(true);
        let b = room.betting.as_ref().unwrap();
        assert_eq!(b.seats[&0].stack, 1002);
        assert_eq!(b.seats[&1].stack, 1003);
        assert_eq!(b.seats[&2].stack, 995);
        assert!(room.state(0).unwrap().players[2].revealed_cards.is_none());
        assert_eq!(b.seats.values().map(|s| s.stack).sum::<u64>(), 3000);
    }

    #[test]
    fn fold_winner_receives_pot_and_uncalled_amount() {
        let (mut room, _peers, _receivers) = test_room(2);
        let b = room.betting.as_mut().unwrap();
        b.act(0, Action::Fold).unwrap();
        room.advance();
        assert_eq!(room.street, Street::Finished);
        let b = room.betting.as_ref().unwrap();
        assert_eq!(b.pot(), 0);
        assert_eq!(b.seats[&0].stack, 995);
        assert_eq!(b.seats[&1].stack, 1005);
        assert!(
            room.state(0)
                .unwrap()
                .players
                .iter()
                .all(|p| p.revealed_cards.is_none())
        );
    }

    #[test]
    fn blinds_remain_distinct_during_disconnect_and_next_hand() {
        for capacity in 2..=10 {
            for disconnected in 0..capacity as u64 {
                let (mut room, _peers, _receivers) = test_room(capacity);
                let before: Vec<_> = room
                    .state(0)
                    .unwrap()
                    .players
                    .iter()
                    .map(|p| (p.id, p.small_blind, p.big_blind))
                    .collect();
                room.leave(disconnected);
                let after: Vec<_> = room
                    .state(0)
                    .unwrap()
                    .players
                    .iter()
                    .map(|p| (p.id, p.small_blind, p.big_blind))
                    .collect();
                assert_eq!(
                    before, after,
                    "capacity {capacity}, disconnected {disconnected}"
                );
                for _ in 0..50 {
                    if matches!(room.street, Street::Finished | Street::Showdown) {
                        break;
                    }
                    let betting = room.betting.as_ref().unwrap();
                    let id = betting.turn.unwrap();
                    let action = if betting.seats[&id].bet == betting.current_bet {
                        Action::Check
                    } else {
                        Action::Call
                    };
                    room.betting.as_mut().unwrap().act(id, action).unwrap();
                    room.advance();
                }
                assert!(matches!(room.street, Street::Finished | Street::Showdown));
                if !room.game_over {
                    room.start_hand();
                    let betting = room.betting.as_ref().unwrap();
                    assert_ne!(betting.small_blind, betting.big_blind);
                    assert_eq!(
                        betting.seats[&betting.small_blind].bet,
                        SMALL_BLIND.min(room.players[&betting.small_blind].stack)
                    );
                    assert_eq!(
                        betting.seats[&betting.big_blind].bet,
                        BIG_BLIND.min(room.players[&betting.big_blind].stack)
                    );
                    let state = room.state(0).unwrap();
                    assert_eq!(
                        state
                            .players
                            .iter()
                            .filter(|p| p.small_blind)
                            .map(|p| p.id)
                            .collect::<Vec<_>>(),
                        vec![betting.small_blind]
                    );
                    assert_eq!(
                        state
                            .players
                            .iter()
                            .filter(|p| p.big_blind)
                            .map(|p| p.id)
                            .collect::<Vec<_>>(),
                        vec![betting.big_blind]
                    );
                }
            }
        }
    }

    #[test]
    fn hands_rotate_dealer_and_keep_stacks() {
        let (mut room, _peers, _receivers) = test_room(3);
        assert_eq!(room.dealer, Some(0));
        let entry = room.betting.as_mut().unwrap().act(0, Action::Fold).unwrap();
        room.record_player(0, entry);
        room.betting.as_mut().unwrap().act(1, Action::Fold).unwrap();
        room.advance();
        assert!(room.next_hand_at.is_some());
        assert_eq!(room.players[&0].stack, 1000);
        assert_eq!(room.players[&1].stack, 995);
        assert_eq!(room.players[&2].stack, 1005);
        room.start_hand();
        room.advance();
        assert_eq!(room.hand_number, 2);
        assert_eq!(room.dealer, Some(1));
        let state = room.state(0).unwrap();
        assert!(state.players[1].dealer);
        assert!(state.players[2].small_blind);
        assert!(state.players[0].big_blind);
        assert_eq!(
            state.players.iter().map(|p| p.stack + p.bet).sum::<u64>(),
            3000
        );
        assert!(state.board.is_empty());
        assert!(state.payouts.is_empty());
    }

    #[test]
    fn timer_broadcasts_next_hand_with_private_cards() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let room = Arc::new(Mutex::new(Room::new(2).unwrap()));
        let (mut a, mut ar, ah) = connect(&listener, &room);
        send_join(&mut a);
        receive(&mut ar);
        receive(&mut ar);
        let (mut b, mut br, bh) = connect(&listener, &room);
        send_join(&mut b);
        receive(&mut br);
        receive(&mut br);
        receive(&mut ar);
        serde_json::to_writer(
            &mut a,
            &Request {
                jsonrpc: "2.0".into(),
                id: 2,
                method: "act".into(),
                params: Some(Action::Fold),
                session: None,
                profile: None,
            },
        )
        .unwrap();
        a.write_all(b"\n").unwrap();
        let result: RoomState = serde_json::from_value(receive(&mut ar)["result"].clone()).unwrap();
        assert_eq!(result.street, Street::Finished);
        receive(&mut ar);
        receive(&mut br);
        let deadline = room.lock().unwrap().next_hand_at.unwrap();
        assert!(room.lock().unwrap().tick(deadline));
        let next_a: Notification = serde_json::from_value(receive(&mut ar)).unwrap();
        let next_b: Notification = serde_json::from_value(receive(&mut br)).unwrap();
        assert_eq!(next_a.params.hand_number, 2);
        assert_eq!(next_a.params.street, Street::Preflop);
        assert!(
            next_a
                .params
                .cards
                .unwrap()
                .iter()
                .all(|c| !next_b.params.cards.unwrap().contains(c))
        );
        assert!(
            next_a
                .params
                .players
                .iter()
                .all(|p| p.revealed_cards.is_none())
        );
        drop(a);
        drop(ar);
        ah.join().unwrap();
        drop(b);
        drop(br);
        bh.join().unwrap();
    }

    #[test]
    fn cash_out_waits_for_settlement_and_viewers_cannot_be_dealt_in() {
        let (mut room, _peers, _receivers) = test_room(3);
        room.cash_out(0).unwrap();
        assert!(room.players[&0].cash_out_pending);
        assert!(!room.players[&0].viewer);
        assert!(room.cash_out(0).is_err());
        room.betting.as_mut().unwrap().act(0, Action::Fold).unwrap();
        room.betting.as_mut().unwrap().act(1, Action::Fold).unwrap();
        room.advance();
        assert!(room.players[&0].viewer);
        assert_eq!(room.players[&0].cashed_out, 1000);
        assert_eq!(room.state(0).unwrap().players[0].stack, 0);
        assert!(room.next_hand_at.is_some());
        room.start_hand();
        assert_eq!(room.dealer, Some(1));
        assert_eq!(room.players[&0].cards, None);
        assert!(!room.betting.as_ref().unwrap().seats.contains_key(&0));
        assert_eq!(
            room.players.values().map(|p| p.cashed_out).sum::<u64>()
                + room
                    .betting
                    .as_ref()
                    .unwrap()
                    .seats
                    .values()
                    .map(|s| s.stack + s.total)
                    .sum::<u64>(),
            3000
        );
    }

    #[test]
    fn bankrupt_player_becomes_viewer_and_last_player_wins() {
        let (mut room, _peers, _receivers) = test_room(2);
        let b = room.betting.as_mut().unwrap();
        b.seats.get_mut(&0).unwrap().stack = 0;
        b.seats.get_mut(&0).unwrap().total = 1000;
        b.seats.get_mut(&1).unwrap().stack = 0;
        b.seats.get_mut(&1).unwrap().total = 1000;
        b.seats.get_mut(&0).unwrap().folded = true;
        b.complete = true;
        b.turn = None;
        room.advance();
        assert!(room.game_over);
        assert!(room.players[&0].viewer);
        assert_eq!(room.players[&1].stack, 2000);
        assert!(room.next_hand_at.is_none());
        assert!(room.state(0).unwrap().game_over);
    }

    #[test]
    fn one_remaining_player_ends_game_after_others_cash_out() {
        let (mut room, _peers, _receivers) = test_room(2);
        room.cash_out(0).unwrap();
        room.betting.as_mut().unwrap().act(0, Action::Fold).unwrap();
        room.advance();
        assert!(room.game_over);
        assert!(room.players[&0].viewer);
        assert!(!room.players[&1].viewer);
        assert!(room.next_hand_at.is_none());
        room.cash_out(1).unwrap();
        assert!(room.players[&1].viewer);
    }

    #[test]
    fn players_can_all_cash_out_and_end_game() {
        let (mut room, _peers, _receivers) = test_room(2);
        room.cash_out(0).unwrap();
        room.cash_out(1).unwrap();
        room.betting.as_mut().unwrap().act(0, Action::Fold).unwrap();
        room.advance();
        assert!(room.game_over);
        assert!(room.players.values().all(|p| p.viewer));
        assert_eq!(
            room.players.values().map(|p| p.cashed_out).sum::<u64>(),
            2000
        );
        assert!(room.next_hand_at.is_none());
    }

    #[test]
    fn rejoining_before_turn_prevents_automatic_action() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut room = Room::new(3).unwrap();
        let mut peers = Vec::new();
        let mut receivers = Vec::new();
        for id in 0..3 {
            let peer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
            let (socket, _) = listener.accept().unwrap();
            let (sender, receiver) = mpsc::sync_channel(32);
            room.join(sender, socket, &format!("{id:064x}"), "test")
                .unwrap();
            peers.push(peer);
            receivers.push(receiver);
        }
        let original = room.state(2).unwrap().cards;
        room.leave(2);
        let peer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (socket, _) = listener.accept().unwrap();
        let (sender, receiver) = mpsc::sync_channel(32);
        assert_eq!(
            room.join(sender, socket, &format!("{:064x}", 2), "test")
                .unwrap(),
            2
        );
        peers.push(peer);
        receivers.push(receiver);
        room.betting.as_mut().unwrap().act(0, Action::Call).unwrap();
        room.advance();
        room.betting.as_mut().unwrap().act(1, Action::Call).unwrap();
        room.advance();
        assert_eq!(room.betting.as_ref().unwrap().turn, Some(2));
        assert_eq!(room.state(2).unwrap().cards, original);
        assert!(room.state(2).unwrap().players[2].online);
    }

    #[test]
    fn offline_big_blind_checks_and_streets_progress() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let room = Arc::new(Mutex::new(Room::new(2).unwrap()));
        let (mut a, mut ar, ah) = connect(&listener, &room);
        send_join(&mut a);
        receive(&mut ar);
        receive(&mut ar);
        let (mut b, mut br, bh) = connect(&listener, &room);
        send_join(&mut b);
        receive(&mut br);
        receive(&mut br);
        receive(&mut ar);
        drop(b);
        drop(br);
        bh.join().unwrap();
        receive(&mut ar);
        let send = |stream: &mut TcpStream, id, action| {
            serde_json::to_writer(
                &mut *stream,
                &Request {
                    jsonrpc: "2.0".into(),
                    id,
                    method: "act".into(),
                    params: Some(action),
                    session: None,
                    profile: None,
                },
            )
            .unwrap();
            stream.write_all(b"\n").unwrap();
        };
        send(&mut a, 2, Action::Call);
        let flop: RoomState = serde_json::from_value(receive(&mut ar)["result"].clone()).unwrap();
        assert_eq!(flop.street, Street::Flop);
        assert_eq!(flop.board.len(), 3);
        assert!(
            flop.log
                .iter()
                .any(|line| line.message.contains("checked (offline)"))
        );
        receive(&mut ar);
        let mut old_board = flop.board.clone();
        for (id, street, length) in [
            (3, Street::Turn, 4),
            (4, Street::River, 5),
            (5, Street::Showdown, 5),
        ] {
            send(&mut a, id, Action::Check);
            let state: RoomState =
                serde_json::from_value(receive(&mut ar)["result"].clone()).unwrap();
            assert_eq!(state.street, street);
            assert_eq!(state.board.len(), length);
            assert!(state.board.starts_with(&old_board));
            old_board = state.board;
            receive(&mut ar);
        }
        assert_eq!(room.lock().unwrap().deck.len(), 40);
        drop(a);
        drop(ar);
        ah.join().unwrap();
    }

    #[test]
    fn waiting_disconnect_broadcasts_and_frees_seat() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let room = Arc::new(Mutex::new(Room::new(3).unwrap()));
        let (mut a, mut ar, ah) = connect(&listener, &room);
        send_join(&mut a);
        receive(&mut ar);
        receive(&mut ar);
        let (mut b, mut br, bh) = connect(&listener, &room);
        send_join(&mut b);
        receive(&mut br);
        receive(&mut br);
        receive(&mut ar);
        drop(a);
        drop(ar);
        ah.join().unwrap();
        let notification: Notification = serde_json::from_value(receive(&mut br)).unwrap();
        assert_eq!(notification.params.players.len(), 1);
        drop(b);
        drop(br);
        bh.join().unwrap();
    }
}
