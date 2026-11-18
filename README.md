# tPOKR

A terminal poker client and a separate server that hosts multiple Texas Hold'em rooms. Each room accepts 2 to 10 players.

## Run locally

Start a server:

```sh
cargo run --bin tpokr-server
```

Open two other terminals. Run `cargo run -- --profile alice` in one and `cargo run -- --profile bob` in the other. Select the local server, create a room with `c`, and join it. The other player can refresh the room list with `r` and join the same room. Once at least two players have joined, press `s` to start. Each player needs a separate profile.

Release archives contain `tpokr-client` and `tpokr-server` for Linux x86-64 and Apple Silicon macOS, and `tpokr-client.exe` and `tpokr-server.exe` for Windows x86-64. Run them directly as `./tpokr-client --profile alice` and `./tpokr-server 0.0.0.0:7878`. On Windows, use Windows Terminal and run `./tpokr-client.exe --profile alice` and `./tpokr-server.exe 0.0.0.0:7878` in PowerShell. The Windows binaries are unsigned and may trigger a SmartScreen warning.

The server accepts an optional bind address and defaults to `0.0.0.0:7878`. New players may join a running room and receive cards on the next hand. A player who leaves before the game starts frees a seat. Once the game starts, disconnected players keep their seats and can rejoin with the same profile. An offline player checks when possible and otherwise folds. Rejoining does not undo a fold.

Servers bound to an IPv4 LAN address send a multicast beacon every 30 seconds on UDP port 47878. Press `f` on the server selector to browse discovered servers. Discovery may take up to 30 seconds and requires multicast and firewall access on the LAN. It does not cross network boundaries, and loopback-only servers do not advertise. Add a server manually if discovery is unavailable. Discovered entries expire after 95 seconds without a beacon.

## Controls

On the server selector, use `j` and `k` or the arrow keys to change the selection, Enter to open the room list, `f` to browse LAN servers, `a` to add, `e` to edit, and `d` to delete. In the room list, press `c` to create a named room, `r` to refresh, or Enter to join. Any player can press `s` to start once at least two players have joined. Confirm deletion with `y`. In the editor, Tab switches fields, Enter saves, and Esc cancels. In a room, use `h` and `l` to select an action and Enter to submit it. Use `k` and `j` to scroll the activity log. For Raise, enter the increase above the current bet and press Enter. For example, raising a bet of 10 by 10 makes the total wager 20. Select Cash out and confirm with `y` to leave the game as a viewer after the current hand settles. Esc cancels raise entry. Otherwise, Esc or `q` leaves the room. Ctrl+C quits.

Set `TPOKR_ASCII=1` to use the plain-text table and ASCII suit letters. Set `NO_COLOR=1` to use the plain-text table without colors.

Server entries are stored in `$XDG_CONFIG_HOME/tpokr/servers.json`, or `~/.config/tpokr/servers.json` if `XDG_CONFIG_HOME` is unset. On Windows, they are stored in `%APPDATA%\tpokr\servers.json`. The local server is the default when no file exists. Each profile has a separate session token in the same directory. New session files restrict access to the current user and system on Windows, or to the current user on Unix. The default profile is `default`. Use distinct profiles when running multiple players on the same machine.

## Protocol and security

The client and server use newline-delimited JSON-RPC 2.0 over TCP. The lobby uses `rooms` and `create_room` requests. A game connection calls `join` with a room name and submits actions with `act`. The server pushes `room.updated` notifications after joins, deals, actions, and disconnects. Before showdown, each notification contains only the recipient's hole cards. At showdown it also contains the cards of players who have not folded. Requests are limited to 4096 bytes. A session token identifies the seat on rejoin. Keep the token private. The protocol has no encryption, so use it only on a trusted network. Anyone who captures or obtains a token can claim its seat after it disconnects. Anyone who can connect can occupy an empty seat.

The room view shows the activity log, player seats, private cards, stacks, and pot. When a game starts, the first player in join order gets the dealer button. The button moves to the next remaining player each hand. The next two players are small blind and big blind, except heads-up, where the dealer is also the small blind. Each player starts with 1000 chips and posts blinds of 5 and 10. The server checks turn order, calls, minimum raises, stacks, and whether a short all-in reopens raising. It tracks pot layers and eligible players across streets. At showdown it evaluates the best five-card hand, reveals the remaining players' cards, returns uncalled chips, and pays each pot. The result remains visible for one minute before the next hand. Ties split evenly, with odd chips awarded clockwise from the dealer. The activity log summarizes net transfers between players. When multiple players win or lose, the server matches losses to gains in seat order. Pots do not track which player's individual chips went to which winner. When all but one player folds, the remaining player wins without revealing cards. The server burns a card before the flop, turn, and river. A bankrupt player becomes a viewer after settlement. Cashing out removes the player's remaining chips after settlement and also makes them a viewer. A viewer can remain connected or rejoin but cannot play or receive private cards in later hands. The game ends when fewer than two players remain. If one player remains after others cash out, that player can also cash out. Create another room to start a new game. Rooms and game state live only in memory and disappear when the server restarts. The server retains disconnected seats until restart and limits itself to 100 rooms. Empty, unstarted rooms expire after five minutes when another room is created. Larger rooms need a larger terminal to display every seat.
