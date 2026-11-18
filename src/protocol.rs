use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Suit {
    Clubs,
    Diamonds,
    Hearts,
    Spades,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Card {
    pub rank: u8,
    pub suit: Suit,
}

impl fmt::Display for Card {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let rank = match self.rank {
            11 => "J".to_owned(),
            12 => "Q".to_owned(),
            13 => "K".to_owned(),
            14 => "A".to_owned(),
            n => n.to_string(),
        };
        let suit = match self.suit {
            Suit::Clubs => '♣',
            Suit::Diamonds => '♦',
            Suit::Hearts => '♥',
            Suit::Spades => '♠',
        };
        write!(f, "{rank}{suit}")
    }
}

// A small JSON-RPC 2.0 subset, with one JSON object per line.
#[derive(Debug, Serialize, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Action>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    Start,
    Check,
    Call,
    Fold,
    Raise { to: u64 },
    AllIn,
    CashOut,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Response {
    pub jsonrpc: String,
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<RoomState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomInfo {
    pub name: String,
    pub players: usize,
    pub started: bool,
    pub game_over: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Notification {
    pub jsonrpc: String,
    pub method: String,
    pub params: RoomState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayerStatus {
    Waiting,
    Ready,
    Disconnected,
    Turn,
    Folded,
    Checked,
    Raised,
    Called,
    AllIn,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerView {
    pub id: u64,
    pub status: PlayerStatus,
    pub online: bool,
    pub name: String,
    pub revealed_cards: Option<[Card; 2]>,
    pub viewer: bool,
    pub cash_out_pending: bool,
    pub cashed_out: u64,
    pub dealer: bool,
    pub small_blind: bool,
    pub big_blind: bool,
    pub stack: u64,
    pub bet: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pot {
    pub amount: u64,
    pub eligible: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    pub player: Option<u64>,
    pub name: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Payout {
    pub player: u64,
    pub amount: u64,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Street {
    Preflop,
    Flop,
    Turn,
    River,
    Showdown,
    Finished,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomState {
    pub capacity: usize,
    pub you: u64,
    pub players: Vec<PlayerView>,
    pub cards: Option<[Card; 2]>,
    pub pot: u64,
    pub current_bet: u64,
    pub min_raise_to: u64,
    pub big_blind_amount: u64,
    pub betting_complete: bool,
    pub paused: bool,
    pub street: Street,
    pub board: Vec<Card>,
    pub hand_number: u64,
    pub game_over: bool,
    pub pots: Vec<Pot>,
    pub log: Vec<LogEntry>,
    pub payouts: Vec<Payout>,
}
