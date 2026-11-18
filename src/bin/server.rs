use std::{env, io, net::TcpListener};

fn main() -> io::Result<()> {
    let mut args = env::args().skip(1);
    let address = args.next().unwrap_or_else(|| "0.0.0.0:7878".into());
    if args.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: tpokr-server [address:port]",
        ));
    }
    let listener = TcpListener::bind(&address)?;
    println!("Listening on {}", listener.local_addr()?);
    tpokr::server::serve(listener)
}
