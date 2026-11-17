use std::{env, io, net::TcpListener};

fn main() -> io::Result<()> {
    let mut args = env::args().skip(1);
    let address = args.next().unwrap_or_else(|| "127.0.0.1:7878".into());
    let capacity: usize = args
        .next()
        .unwrap_or_else(|| "2".into())
        .parse()
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "player count must be a number from 2 to 10",
            )
        })?;
    if args.next().is_some() || !(2..=10).contains(&capacity) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: tpokr-server [address:port] [players: 2-10]",
        ));
    }
    let listener = TcpListener::bind(&address)?;
    println!(
        "Listening on {} for {capacity} players",
        listener.local_addr()?
    );
    tpokr::server::serve(listener, capacity)
}
