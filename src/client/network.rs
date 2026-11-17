use std::{
    io::{self, Read, Write},
    net::{TcpStream, ToSocketAddrs},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, Sender},
    },
    thread,
    time::{Duration, Instant},
};
use tpokr::protocol::{Action, Notification, Request, Response, RoomState};

pub(super) enum Update {
    State(RoomState),
    Error(String),
    ActionError(String),
}

pub(super) fn connect(
    address: String,
    token: String,
    profile: String,
    sender: Sender<Update>,
    commands: Receiver<Action>,
    stop: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        if let Err(error) = run(&address, &token, &profile, &sender, &commands, &stop)
            && !stop.load(Ordering::Relaxed)
        {
            let _ = sender.send(Update::Error(error.to_string()));
        }
    });
}

fn run(
    address: &str,
    token: &str,
    profile: &str,
    sender: &Sender<Update>,
    commands: &Receiver<Action>,
    stop: &AtomicBool,
) -> io::Result<()> {
    let addresses: Vec<_> = address.to_socket_addrs()?.collect();
    let mut stream = None;
    for addr in addresses {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        if let Ok(connected) = TcpStream::connect_timeout(&addr, Duration::from_secs(3)) {
            stream = Some(connected);
            break;
        }
    }
    let mut stream = stream.ok_or_else(|| io::Error::other("could not connect to server"))?;
    stream.set_read_timeout(Some(Duration::from_millis(250)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    serde_json::to_writer(
        &mut stream,
        &Request {
            jsonrpc: "2.0".into(),
            id: 1,
            method: "join".into(),
            params: None,
            session: Some(token.to_owned()),
            profile: Some(profile.to_owned()),
        },
    )?;
    stream.write_all(b"\n")?;
    let mut pending = Vec::new();
    let mut joined = false;
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut buffer = [0u8; 2048];
    let mut next_id = 2u64;
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        if joined {
            while let Ok(action) = commands.try_recv() {
                serde_json::to_writer(
                    &mut stream,
                    &Request {
                        jsonrpc: "2.0".into(),
                        id: next_id,
                        method: "act".into(),
                        params: Some(action),
                        session: None,
                        profile: None,
                    },
                )?;
                stream.write_all(b"\n")?;
                next_id += 1;
            }
        }
        if !joined && Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "server did not answer join",
            ));
        }
        match stream.read(&mut buffer) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "server closed connection",
                ));
            }
            Ok(count) => {
                for &byte in &buffer[..count] {
                    if byte == b'\n' {
                        if !joined {
                            let response: Response = serde_json::from_slice(&pending)?;
                            if response.jsonrpc != "2.0" || response.id != 1 {
                                return Err(io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    "unexpected join response",
                                ));
                            }
                            if let Some(error) = response.error {
                                return Err(io::Error::other(error.message));
                            }
                            let state = response.result.ok_or_else(|| {
                                io::Error::new(io::ErrorKind::InvalidData, "missing RPC result")
                            })?;
                            joined = true;
                            if sender.send(Update::State(state)).is_err() {
                                return Ok(());
                            }
                        } else {
                            let value: serde_json::Value = serde_json::from_slice(&pending)?;
                            if value.get("id").is_some() {
                                let response: Response = serde_json::from_value(value)?;
                                if response.jsonrpc != "2.0"
                                    || response.id < 2
                                    || response.id >= next_id
                                {
                                    return Err(io::Error::new(
                                        io::ErrorKind::InvalidData,
                                        "unexpected action response",
                                    ));
                                }
                                if let Some(error) = response.error {
                                    if sender.send(Update::ActionError(error.message)).is_err() {
                                        return Ok(());
                                    }
                                } else if let Some(state) = response.result {
                                    if sender.send(Update::State(state)).is_err() {
                                        return Ok(());
                                    }
                                } else {
                                    return Err(io::Error::new(
                                        io::ErrorKind::InvalidData,
                                        "missing action response",
                                    ));
                                }
                            } else {
                                let notification: Notification = serde_json::from_value(value)?;
                                if notification.jsonrpc != "2.0"
                                    || notification.method != "room.updated"
                                {
                                    return Err(io::Error::new(
                                        io::ErrorKind::InvalidData,
                                        "unexpected notification",
                                    ));
                                }
                                if sender.send(Update::State(notification.params)).is_err() {
                                    return Ok(());
                                }
                            }
                        }
                        pending.clear();
                    } else {
                        pending.push(byte);
                        if pending.len() > 16 * 1024 {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "server message too large",
                            ));
                        }
                    }
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) =>
            {
                continue;
            }
            Err(error) => return Err(error),
        }
    }
}
