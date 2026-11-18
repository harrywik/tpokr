use serde::{Deserialize, Serialize};
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    io,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket},
    thread,
    time::Duration,
};

pub const PORT: u16 = 47878;
const GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 77, 78);
const MAGIC: &str = "tpokr-discovery-v1";

#[derive(Serialize, Deserialize)]
struct Beacon {
    protocol: String,
    port: u16,
}

pub fn beacon(address: SocketAddr) -> io::Result<()> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    socket.set_multicast_ttl_v4(1)?;
    if let SocketAddr::V4(address) = address
        && !address.ip().is_unspecified()
        && !address.ip().is_loopback()
    {
        socket2::SockRef::from(&socket).set_multicast_if_v4(address.ip())?;
    }
    let bytes = serde_json::to_vec(&Beacon {
        protocol: MAGIC.into(),
        port: address.port(),
    })?;
    thread::spawn(move || {
        loop {
            if address.is_ipv4() && !address.ip().is_loopback() {
                let _ = socket.send_to(&bytes, SocketAddrV4::new(GROUP, PORT));
            }
            thread::sleep(Duration::from_secs(30));
        }
    });
    Ok(())
}

pub fn listener() -> io::Result<UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    socket.set_reuse_port(true)?;
    socket.bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, PORT).into())?;
    socket.join_multicast_v4(&GROUP, &Ipv4Addr::UNSPECIFIED)?;
    socket.set_read_timeout(Some(Duration::from_millis(500)))?;
    Ok(socket.into())
}

pub fn parse(bytes: &[u8], source: SocketAddr) -> Option<SocketAddr> {
    let beacon: Beacon = serde_json::from_slice(bytes).ok()?;
    if beacon.protocol != MAGIC
        || beacon.port == 0
        || !source.is_ipv4()
        || source.ip().is_loopback()
    {
        return None;
    }
    Some(SocketAddr::new(source.ip(), beacon.port))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_discovery_messages() {
        let source = "192.168.1.4:1234".parse().unwrap();
        assert_eq!(
            parse(br#"{"protocol":"tpokr-discovery-v1","port":7878}"#, source),
            Some("192.168.1.4:7878".parse().unwrap())
        );
        assert!(parse(br#"{"protocol":"wrong","port":7878}"#, source).is_none());
        assert!(parse(br#"{"protocol":"tpokr-discovery-v1","port":0}"#, source).is_none());
    }
}
