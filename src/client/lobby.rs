use std::{
    io::{self, BufRead, BufReader, Write},
    net::{TcpStream, ToSocketAddrs},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
    thread,
    time::Duration,
};
use tpokr::protocol::{Request, RoomInfo};

pub enum Update {
    Rooms(Vec<RoomInfo>),
    Created,
    Error(String),
}

fn request(address: &str, method: &str, room: Option<String>) -> io::Result<serde_json::Value> {
    let addresses: Vec<_> = address.to_socket_addrs()?.collect();
    let mut stream = addresses
        .iter()
        .find_map(|addr| TcpStream::connect_timeout(addr, Duration::from_secs(3)).ok())
        .ok_or_else(|| io::Error::other("could not connect to server"))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    serde_json::to_writer(
        &mut stream,
        &Request {
            jsonrpc: "2.0".into(),
            id: 1,
            method: method.into(),
            params: None,
            session: None,
            profile: None,
            room,
        },
    )?;
    stream.write_all(b"\n")?;
    let mut line = String::new();
    use std::io::Read;
    BufReader::new(stream).take(65537).read_line(&mut line)?;
    if line.len() > 65536 || !line.ends_with('\n') {
        return Err(io::Error::other("invalid lobby response"));
    }
    let response: serde_json::Value = serde_json::from_str(&line)?;
    if response["jsonrpc"] != "2.0" || response["id"] != 1 {
        return Err(io::Error::other("invalid lobby response"));
    }
    if let Some(error) = response.get("error") {
        return Err(io::Error::other(
            error["message"]
                .as_str()
                .unwrap_or("server error")
                .to_owned(),
        ));
    }
    response
        .get("result")
        .cloned()
        .ok_or_else(|| io::Error::other("missing lobby result"))
}

pub fn refresh(address: String, sender: Sender<Update>) {
    thread::spawn(move || {
        let update = match request(&address, "rooms", None)
            .and_then(|v| serde_json::from_value(v).map_err(Into::into))
        {
            Ok(rooms) => Update::Rooms(rooms),
            Err(e) => Update::Error(e.to_string()),
        };
        let _ = sender.send(update);
    });
}
pub fn create(address: String, room: String, sender: Sender<Update>) {
    thread::spawn(move || {
        let update = match request(&address, "create_room", Some(room)) {
            Ok(_) => Update::Created,
            Err(e) => Update::Error(e.to_string()),
        };
        let _ = sender.send(update);
    });
}

pub fn discover(sender: Sender<String>, stop: Arc<AtomicBool>) {
    thread::spawn(move || {
        let Ok(socket) = tpokr::discovery::listener() else {
            let _ = sender.send(String::new());
            return;
        };
        let mut bytes = [0; 1024];
        while !stop.load(Ordering::Relaxed) {
            if let Ok((size, source)) = socket.recv_from(&mut bytes)
                && let Some(address) = tpokr::discovery::parse(&bytes[..size], source)
            {
                let _ = sender.send(address.to_string());
            }
        }
    });
}
