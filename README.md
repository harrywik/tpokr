# tPOKR

A terminal poker client and a separate single-room server. It plays consecutive Texas Hold'em hands until fewer than two players remain.

## Run locally

Start a server for two players:

```sh
cargo run --bin tpokr-server -- 127.0.0.1:7878 2
```

Open two other terminals. Run `cargo run -- --profile alice` in one and `cargo run -- --profile bob` in the other. Select the local server and press Enter. Each player needs a separate profile. The server shuffles one 52-card deck and deals two private cards to each player when the room is full. It deals subsequent hands after a five-second result display.

Release archives contain `tpokr-client` and `tpokr-server`. Run them directly as `./tpokr-client --profile alice` and `./tpokr-server 127.0.0.1:7878 2`.

The server accepts an address and a room size between 2 and 10. It defaults to `127.0.0.1:7878` and two players. A waiting player who leaves frees a seat. Once the hand starts, a player can rejoin their seat by selecting the same server with the same profile. An offline player checks when possible and otherwise folds. Rejoining does not undo a fold.

## Controls

On the server selector, use `j` and `k` or the arrow keys to change the selection, Enter to join, `a` to add, `e` to edit, and `d` to delete. Confirm deletion with `y`. In the editor, Tab switches fields, Enter saves, and Esc cancels. In a room, use `h` and `l` to select an action and Enter to submit it. Use `k` and `j` to scroll the activity log. For Raise, enter the total wager for the round, not the increase, and press Enter. Select Cash out and confirm with `y` to leave the game as a viewer after the current hand settles. Esc cancels raise entry. Otherwise, Esc or `q` leaves the room. Ctrl+C quits.

Set `TPOKR_ASCII=1` to use the plain-text table and ASCII suit letters. Set `NO_COLOR=1` to use the plain-text table without colors.

Server entries are stored in `$XDG_CONFIG_HOME/tpokr/servers.json`, or `~/.config/tpokr/servers.json` if `XDG_CONFIG_HOME` is unset. The local server is the default when no file exists. Each profile has a separate session token in the same directory. The default profile is `default`. Use distinct profiles when running multiple players on the same machine.

## Protocol and security

The client and server use newline-delimited JSON-RPC 2.0 over TCP. A connection calls `join` once and submits actions with `act`. The server pushes `room.updated` notifications after joins, deals, actions, and disconnects. Before showdown, each notification contains only the recipient's hole cards. At showdown it also contains the cards of players who have not folded. Requests are limited to 4096 bytes. A session token identifies the seat on rejoin. Keep the token private. The protocol has no encryption, so use it only on a trusted network. Anyone who captures or obtains a token can claim its seat after it disconnects. Anyone who can connect can occupy an empty seat.

The room view shows the activity log, player seats, private cards, stacks, and pot. Once the room fills, the first player in join order gets the dealer button. The button moves to the next remaining player each hand. The next two players are small blind and big blind, except heads-up, where the dealer is also the small blind. Each player starts with 1000 chips and posts blinds of 5 and 10. The server checks turn order, calls, minimum raises, stacks, and whether a short all-in reopens raising. It tracks pot layers and eligible players across streets. At showdown it evaluates the best five-card hand, reveals the remaining players' cards, returns uncalled chips, and pays each pot. Ties split evenly, with odd chips awarded clockwise from the dealer. The activity log summarizes net transfers between players. When multiple players win or lose, the server matches losses to gains in seat order. Pots do not track which player's individual chips went to which winner. When all but one player folds, the remaining player wins without revealing cards. The server burns a card before the flop, turn, and river. A bankrupt player becomes a viewer after settlement. Cashing out removes the player's remaining chips after settlement and also makes them a viewer. A viewer can remain connected or rejoin but cannot play or receive private cards in later hands. The game ends when fewer than two players remain. If one player remains after others cash out, that player can also cash out. Restart the server to start a new game. Larger rooms need a larger terminal to display every seat.
