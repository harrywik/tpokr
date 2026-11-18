mod cards;
mod config;
mod lobby;
mod network;
mod ui;

use config::Server;
use crossterm::{
    cursor::{Hide, Show},
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use rand::{TryRng, rngs::SysRng};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::{
    io::{self, IsTerminal},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    time::{Duration, Instant},
};
use tpokr::protocol::{Action, RoomState};

const FRAME_TIME: Duration = Duration::from_millis(250);

fn new_session() -> String {
    let mut bytes = [0u8; 32];
    SysRng
        .try_fill_bytes(&mut bytes)
        .expect("system random source unavailable");
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

struct TerminalGuard;
impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        if let Err(error) = execute!(io::stdout(), EnterAlternateScreen, Hide) {
            let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
            let _ = terminal::disable_raw_mode();
            return Err(error);
        }
        Ok(Self)
    }
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }
}

enum Screen {
    Select,
    Edit {
        index: Option<usize>,
        name: String,
        address: String,
        field: usize,
    },
    Delete,
    Lobby {
        server: Server,
        rooms: Vec<tpokr::protocol::RoomInfo>,
        selected: usize,
    },
    CreateRoom {
        server: Server,
        name: String,
    },
    Discover,
    Room {
        name: String,
        state: Option<RoomState>,
    },
}

struct App {
    servers: Vec<Server>,
    session: String,
    profile: String,
    card_theme: cards::CardTheme,
    selected: usize,
    screen: Screen,
    message: Option<String>,
    frame: u64,
    selected_action: usize,
    log_offset: u16,
    raise_input: String,
    editing_raise: bool,
    confirming_cash_out: bool,
    commands: Option<Sender<Action>>,
    action_message: Option<String>,
    receiver: Option<Receiver<network::Update>>,
    stop: Option<Arc<AtomicBool>>,
    lobby_tx: Sender<lobby::Update>,
    lobby_rx: Receiver<lobby::Update>,
    discovery_tx: Sender<String>,
    discovery_rx: Receiver<String>,
    discovered: Vec<(Server, Instant)>,
    discovery_stop: Arc<AtomicBool>,
    room_server: Option<Server>,
}

impl App {
    fn new(servers: Vec<Server>) -> Self {
        let (lobby_tx, lobby_rx) = mpsc::channel();
        let (discovery_tx, discovery_rx) = mpsc::channel();
        Self {
            servers,
            session: new_session(),
            profile: "default".into(),
            card_theme: cards::CardTheme::from_env(),
            selected: 0,
            screen: Screen::Select,
            message: None,
            frame: 0,
            selected_action: 0,
            log_offset: 0,
            raise_input: String::new(),
            editing_raise: false,
            confirming_cash_out: false,
            commands: None,
            action_message: None,
            receiver: None,
            stop: None,
            lobby_tx,
            lobby_rx,
            discovery_tx,
            discovery_rx,
            discovered: Vec::new(),
            discovery_stop: Arc::new(AtomicBool::new(false)),
            room_server: None,
        }
    }
    fn disconnect(&mut self) {
        if let Some(stop) = self.stop.take() {
            stop.store(true, Ordering::Relaxed);
        }
        self.receiver = None;
        self.commands = None;
        self.editing_raise = false;
        self.confirming_cash_out = false;
        self.raise_input.clear();
        self.selected_action = 0;
        self.log_offset = 0;
        self.action_message = None;
    }
    fn poll_network(&mut self) {
        while let Ok(address) = self.discovery_rx.try_recv() {
            if address.is_empty() {
                self.message = Some("LAN discovery unavailable. Add a server manually.".into());
                continue;
            }
            if self.servers.iter().any(|server| server.address == address) {
                continue;
            }
            if let Some((_, seen)) = self
                .discovered
                .iter_mut()
                .find(|(s, _)| s.address == address)
            {
                *seen = Instant::now();
            } else if self.discovered.len() < 100 {
                self.discovered.push((
                    Server {
                        name: "LAN server".into(),
                        address,
                    },
                    Instant::now(),
                ));
            }
        }
        if matches!(self.screen, Screen::Select) {
            self.discovered
                .retain(|(_, seen)| seen.elapsed() < Duration::from_secs(95));
        }
        self.selected = self.selected.min(
            self.servers
                .len()
                .saturating_add(self.discovered.len())
                .saturating_sub(1),
        );
        while let Ok(update) = self.lobby_rx.try_recv() {
            match update {
                lobby::Update::Rooms(rooms) => {
                    if let Screen::Lobby {
                        rooms: current,
                        selected,
                        ..
                    } = &mut self.screen
                    {
                        *current = rooms;
                        *selected = (*selected).min(current.len().saturating_sub(1));
                        self.message = None;
                    }
                }
                lobby::Update::Created => {
                    if let Screen::CreateRoom { server, .. } = &self.screen {
                        let server = server.clone();
                        lobby::refresh(server.address.clone(), self.lobby_tx.clone());
                        self.screen = Screen::Lobby {
                            server,
                            rooms: Vec::new(),
                            selected: 0,
                        };
                    }
                }
                lobby::Update::Error(error) => self.message = Some(error),
            }
        }
        if let Some(receiver) = &self.receiver {
            while let Ok(update) = receiver.try_recv() {
                match update {
                    network::Update::State(state) => {
                        if let Screen::Room { state: current, .. } = &mut self.screen {
                            *current = Some(state);
                            if self
                                .action_message
                                .as_deref()
                                .is_some_and(|m| m.starts_with("Sending "))
                            {
                                self.action_message = None;
                            }
                        }
                    }
                    network::Update::ActionError(error) => {
                        self.action_message = Some(error);
                    }
                    network::Update::Error(error) => {
                        self.message = Some(format!("Connection: {error}"));
                        self.screen = Screen::Select;
                    }
                }
            }
        }
    }
    fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        if modifiers.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('c') {
            return false;
        }
        if matches!(self.screen, Screen::Room { .. })
            && modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return true;
        }
        match &mut self.screen {
            Screen::Select => match code {
                KeyCode::Char('q') | KeyCode::Esc => return false,
                KeyCode::Char('j') | KeyCode::Down => {
                    let count = self.servers.len() + self.discovered.len();
                    if count > 0 {
                        self.selected = (self.selected + 1) % count;
                    }
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    let count = self.servers.len() + self.discovered.len();
                    if count > 0 {
                        self.selected = (self.selected + count - 1) % count;
                    }
                }
                KeyCode::Char('f') => {
                    self.selected = self.servers.len();
                    self.screen = Screen::Discover;
                }
                KeyCode::Char('a') => {
                    self.message = None;
                    self.screen = Screen::Edit {
                        index: None,
                        name: String::new(),
                        address: String::new(),
                        field: 0,
                    };
                }
                KeyCode::Char('e') if self.selected < self.servers.len() => {
                    let server = &self.servers[self.selected];
                    self.message = None;
                    self.screen = Screen::Edit {
                        index: Some(self.selected),
                        name: server.name.clone(),
                        address: server.address.clone(),
                        field: 0,
                    };
                }
                KeyCode::Char('d') if self.selected < self.servers.len() => {
                    self.message = None;
                    self.screen = Screen::Delete;
                }
                KeyCode::Enter => {
                    let server = self.servers.get(self.selected).cloned().or_else(|| {
                        self.selected
                            .checked_sub(self.servers.len())
                            .and_then(|i| self.discovered.get(i).map(|(s, _)| s.clone()))
                    });
                    if let Some(server) = server {
                        lobby::refresh(server.address.clone(), self.lobby_tx.clone());
                        self.message = Some("Loading rooms...".into());
                        self.screen = Screen::Lobby {
                            server,
                            rooms: Vec::new(),
                            selected: 0,
                        };
                    }
                }
                _ => {}
            },
            Screen::Edit {
                index,
                name,
                address,
                field,
            } => match code {
                KeyCode::Esc => {
                    self.screen = Screen::Select;
                    self.message = None;
                }
                KeyCode::Tab | KeyCode::BackTab => *field = 1 - *field,
                KeyCode::Backspace => {
                    if *field == 0 {
                        name.pop();
                    } else {
                        address.pop();
                    }
                }
                KeyCode::Enter => {
                    if name.trim().is_empty() || address.trim().is_empty() || !address.contains(':')
                    {
                        self.message = Some("Enter a name and address (host:port).".into());
                    } else {
                        let mut servers = self.servers.clone();
                        let server = Server {
                            name: name.trim().into(),
                            address: address.trim().into(),
                        };
                        let selected = if let Some(index) = *index {
                            servers[index] = server;
                            index
                        } else {
                            servers.push(server);
                            servers.len() - 1
                        };
                        match config::save(&servers) {
                            Ok(()) => {
                                self.servers = servers;
                                self.selected = selected;
                                self.screen = Screen::Select;
                                self.message = None;
                            }
                            Err(error) => self.message = Some(format!("Could not save: {error}")),
                        }
                    }
                }
                KeyCode::Char(c)
                    if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    let value = if *field == 0 { name } else { address };
                    if value.len() < 200 && !c.is_control() {
                        value.push(c);
                    }
                }
                _ => {}
            },
            Screen::Delete => match code {
                KeyCode::Char('y') => {
                    let mut servers = self.servers.clone();
                    servers.remove(self.selected);
                    match config::save(&servers) {
                        Ok(()) => {
                            self.servers = servers;
                            self.selected = self.selected.min(self.servers.len().saturating_sub(1));
                            self.message = None;
                        }
                        Err(error) => self.message = Some(format!("Could not save: {error}")),
                    }
                    self.screen = Screen::Select;
                }
                KeyCode::Char('n') | KeyCode::Esc => self.screen = Screen::Select,
                _ => {}
            },
            Screen::Discover => match code {
                KeyCode::Esc | KeyCode::Char('q') => self.screen = Screen::Select,
                KeyCode::Char('j') | KeyCode::Down if !self.discovered.is_empty() => {
                    self.selected = self.servers.len()
                        + (self.selected.saturating_sub(self.servers.len()) + 1)
                            % self.discovered.len()
                }
                KeyCode::Char('k') | KeyCode::Up if !self.discovered.is_empty() => {
                    self.selected = self.servers.len()
                        + (self.selected.saturating_sub(self.servers.len()) + self.discovered.len()
                            - 1)
                            % self.discovered.len()
                }
                KeyCode::Enter if !self.discovered.is_empty() => {
                    let index = self
                        .selected
                        .checked_sub(self.servers.len())
                        .filter(|&i| i < self.discovered.len())
                        .unwrap_or(0);
                    let server = self.discovered[index].0.clone();
                    lobby::refresh(server.address.clone(), self.lobby_tx.clone());
                    self.message = Some("Loading rooms...".into());
                    self.screen = Screen::Lobby {
                        server,
                        rooms: Vec::new(),
                        selected: 0,
                    };
                }
                _ => {}
            },
            Screen::Lobby {
                server,
                rooms,
                selected,
            } => match code {
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.screen = Screen::Select;
                    self.message = None;
                }
                KeyCode::Char('j') | KeyCode::Down if !rooms.is_empty() => {
                    *selected = (*selected + 1) % rooms.len()
                }
                KeyCode::Char('k') | KeyCode::Up if !rooms.is_empty() => {
                    *selected = (*selected + rooms.len() - 1) % rooms.len()
                }
                KeyCode::Char('r') => {
                    lobby::refresh(server.address.clone(), self.lobby_tx.clone());
                    self.message = Some("Loading rooms...".into());
                }
                KeyCode::Char('c') => {
                    self.screen = Screen::CreateRoom {
                        server: server.clone(),
                        name: String::new(),
                    };
                    self.message = None;
                }
                KeyCode::Enter if !rooms.is_empty() => {
                    let room = rooms[*selected].name.clone();
                    self.room_server = Some(server.clone());
                    let (sender, receiver) = mpsc::channel();
                    let stop = Arc::new(AtomicBool::new(false));
                    let (commands, command_receiver) = mpsc::channel();
                    network::connect(
                        server.address.clone(),
                        self.session.clone(),
                        self.profile.clone(),
                        room.clone(),
                        sender,
                        command_receiver,
                        Arc::clone(&stop),
                    );
                    self.commands = Some(commands);
                    self.receiver = Some(receiver);
                    self.stop = Some(stop);
                    self.message = None;
                    self.screen = Screen::Room {
                        name: room,
                        state: None,
                    };
                }
                _ => {}
            },
            Screen::CreateRoom { server, name } => match code {
                KeyCode::Esc => {
                    let server = server.clone();
                    lobby::refresh(server.address.clone(), self.lobby_tx.clone());
                    self.screen = Screen::Lobby {
                        server,
                        rooms: Vec::new(),
                        selected: 0,
                    };
                    self.message = None;
                }
                KeyCode::Backspace => {
                    name.pop();
                }
                KeyCode::Char(c)
                    if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                        && (c.is_ascii_alphanumeric() || c == '-' || c == '_')
                        && name.len() < 32 =>
                {
                    name.push(c)
                }
                KeyCode::Enter if !name.is_empty() => {
                    lobby::create(server.address.clone(), name.clone(), self.lobby_tx.clone());
                    self.message = Some("Creating room...".into());
                }
                _ => {}
            },
            Screen::Room { state, .. } => {
                if self.confirming_cash_out {
                    match code {
                        KeyCode::Char('y') => {
                            if let Some(sender) = &self.commands {
                                let _ = sender.send(Action::CashOut);
                            }
                            self.action_message = Some("Sending cash out...".into());
                            self.confirming_cash_out = false;
                        }
                        KeyCode::Char('n') | KeyCode::Esc => self.confirming_cash_out = false,
                        _ => {}
                    }
                } else if self.editing_raise {
                    match code {
                        KeyCode::Esc => self.editing_raise = false,
                        KeyCode::Backspace => {
                            self.raise_input.pop();
                        }
                        KeyCode::Char(c) if c.is_ascii_digit() && self.raise_input.len() < 20 => {
                            self.raise_input.push(c)
                        }
                        KeyCode::Enter => {
                            match self.raise_input.parse::<u64>().ok().and_then(|increase| {
                                state
                                    .as_ref()
                                    .and_then(|s| s.current_bet.checked_add(increase))
                            }) {
                                Some(to) => {
                                    if let Some(sender) = &self.commands {
                                        let _ = sender.send(Action::Raise { to });
                                    }
                                    self.editing_raise = false;
                                    self.action_message = Some("Sending raise...".into());
                                }
                                None => {
                                    self.action_message = Some(
                                        "Enter a valid raise increase with an active game.".into(),
                                    )
                                }
                            }
                        }
                        _ => {}
                    }
                } else {
                    match code {
                        KeyCode::Char('s')
                            if state.as_ref().is_some_and(|s| s.hand_number == 0) =>
                        {
                            if let Some(sender) = &self.commands {
                                let _ = sender.send(Action::Start);
                            }
                            self.action_message = Some("Sending start...".into());
                        }
                        KeyCode::Char('k') | KeyCode::Up => {
                            self.log_offset = self.log_offset.saturating_add(1)
                        }
                        KeyCode::Char('j') | KeyCode::Down => {
                            self.log_offset = self.log_offset.saturating_sub(1)
                        }
                        KeyCode::Char('h') | KeyCode::Left => {
                            self.selected_action = (self.selected_action + 5) % 6;
                            self.action_message = None;
                        }
                        KeyCode::Char('l') | KeyCode::Right => {
                            self.selected_action = (self.selected_action + 1) % 6;
                            self.action_message = None;
                        }
                        KeyCode::Enter => {
                            if self.selected_action == 5 {
                                self.confirming_cash_out = true;
                            } else if self.selected_action == 1 {
                                self.raise_input = state
                                    .as_ref()
                                    .map(|s| {
                                        s.min_raise_to.saturating_sub(s.current_bet).to_string()
                                    })
                                    .unwrap_or_default();
                                self.editing_raise = true;
                            } else {
                                let action = match self.selected_action {
                                    0 => Action::Check,
                                    2 => Action::Call,
                                    3 => Action::Fold,
                                    _ => Action::AllIn,
                                };
                                if let Some(sender) = &self.commands {
                                    let _ = sender.send(action);
                                }
                                self.action_message = Some("Sending action...".into());
                            }
                        }
                        KeyCode::Esc | KeyCode::Char('q') => {
                            self.disconnect();
                            if let Some(server) = self.room_server.take() {
                                lobby::refresh(server.address.clone(), self.lobby_tx.clone());
                                self.screen = Screen::Lobby {
                                    server,
                                    rooms: Vec::new(),
                                    selected: 0,
                                };
                            } else {
                                self.screen = Screen::Select;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        true
    }
}
impl Drop for App {
    fn drop(&mut self) {
        self.disconnect();
        self.discovery_stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(name: &str) -> Server {
        Server {
            name: name.into(),
            address: "localhost:7878".into(),
        }
    }

    #[test]
    fn action_bar_keys_send_actions_and_edit_raise() {
        let mut app = App::new(vec![]);
        let (sender, receiver) = mpsc::channel();
        app.commands = Some(sender);
        app.screen = Screen::Room {
            name: "room".into(),
            state: Some(RoomState {
                capacity: 2,
                you: 0,
                players: vec![],
                cards: None,
                pot: 15,
                current_bet: 10,
                min_raise_to: 20,
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
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(receiver.try_recv().unwrap(), Action::Check);
        app.handle_key(KeyCode::Char('l'), KeyModifiers::NONE);
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.editing_raise);
        assert_eq!(app.raise_input, "10");
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(receiver.try_recv().unwrap(), Action::Raise { to: 20 });
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        app.raise_input = u64::MAX.to_string();
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.editing_raise);
        assert!(receiver.try_recv().is_err());
        app.handle_key(KeyCode::Esc, KeyModifiers::NONE);
        app.handle_key(KeyCode::Esc, KeyModifiers::NONE);
        assert!(matches!(app.screen, Screen::Select));
    }

    #[test]
    fn lobby_controls_and_start_game() {
        let mut app = App::new(vec![server("one")]);
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        assert!(matches!(app.screen, Screen::Lobby { .. }));
        app.handle_key(KeyCode::Char('c'), KeyModifiers::NONE);
        assert!(matches!(app.screen, Screen::CreateRoom { .. }));
        app.handle_key(KeyCode::Char('A'), KeyModifiers::NONE);
        assert!(matches!(&app.screen, Screen::CreateRoom { name, .. } if name == "A"));
        app.handle_key(KeyCode::Esc, KeyModifiers::NONE);
        assert!(matches!(app.screen, Screen::Lobby { .. }));
        app.screen = Screen::Room {
            name: "room".into(),
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
                street: tpokr::protocol::Street::Preflop,
                board: vec![],
                hand_number: 0,
                game_over: false,
                pots: vec![],
                log: vec![],
                payouts: vec![],
            }),
        };
        let (sender, receiver) = mpsc::channel();
        app.commands = Some(sender);
        app.handle_key(KeyCode::Char('s'), KeyModifiers::NONE);
        assert_eq!(receiver.try_recv().unwrap(), Action::Start);
    }

    #[test]
    fn cash_out_requires_confirmation() {
        let mut app = App::new(vec![]);
        let (sender, receiver) = mpsc::channel();
        app.commands = Some(sender);
        app.screen = Screen::Room {
            name: "room".into(),
            state: None,
        };
        app.handle_key(KeyCode::Char('h'), KeyModifiers::NONE);
        assert_eq!(app.selected_action, 5);
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.confirming_cash_out);
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE);
        assert!(receiver.try_recv().is_err());
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        app.handle_key(KeyCode::Char('y'), KeyModifiers::NONE);
        assert_eq!(receiver.try_recv().unwrap(), Action::CashOut);
    }

    #[test]
    fn selection_and_editor_keys_work() {
        let mut app = App::new(vec![server("one"), server("two")]);
        app.handle_key(KeyCode::Char('k'), KeyModifiers::NONE);
        assert_eq!(app.selected, 1);
        app.handle_key(KeyCode::Char('j'), KeyModifiers::NONE);
        assert_eq!(app.selected, 0);
        app.handle_key(KeyCode::Char('e'), KeyModifiers::NONE);
        assert!(matches!(app.screen, Screen::Edit { index: Some(0), .. }));
        app.handle_key(KeyCode::Tab, KeyModifiers::NONE);
        assert!(matches!(app.screen, Screen::Edit { field: 1, .. }));
        app.handle_key(KeyCode::Esc, KeyModifiers::NONE);
        assert!(matches!(app.screen, Screen::Select));
        app.handle_key(KeyCode::Char('d'), KeyModifiers::NONE);
        assert!(matches!(app.screen, Screen::Delete));
        app.handle_key(KeyCode::Char('n'), KeyModifiers::NONE);
        assert_eq!(app.servers.len(), 2);
        assert!(!app.handle_key(KeyCode::Char('q'), KeyModifiers::NONE));
    }
}

pub fn run() -> io::Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "tPOKR requires an interactive terminal",
        ));
    }
    let servers = config::load()?;
    let _guard = TerminalGuard::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut args = std::env::args().skip(1);
    let profile = match args.next().as_deref() {
        None => "default".to_owned(),
        Some("--profile") => args
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing profile name"))?,
        Some(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "usage: tpokr-client [--profile name]",
            ));
        }
    };
    if args.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: tpokr-client [--profile name]",
        ));
    }
    let mut app = App::new(servers);
    lobby::discover(app.discovery_tx.clone(), Arc::clone(&app.discovery_stop));
    app.session = config::session(&profile)?;
    app.profile = profile;
    let mut next_frame = Instant::now();
    loop {
        if Instant::now() >= next_frame {
            app.poll_network();
            app.frame = app.frame.wrapping_add(1);
            terminal.draw(|frame| ui::draw(frame, &app))?;
            next_frame = Instant::now() + FRAME_TIME;
        }
        if event::poll(next_frame.saturating_duration_since(Instant::now()))? {
            match event::read()? {
                Event::Key(key)
                    if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                {
                    if !app.handle_key(key.code, key.modifiers) {
                        break;
                    }
                    terminal.draw(|frame| ui::draw(frame, &app))?;
                }
                Event::Resize(_, _) => {
                    terminal.draw(|frame| ui::draw(frame, &app))?;
                }
                _ => {}
            }
        }
    }
    Ok(())
}
