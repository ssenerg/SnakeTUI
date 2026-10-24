# SnakeTUI

A highly configurable snake game for your terminal, built in Rust with
[ratatui](https://ratatui.rs).

## Features

- **Two move timings**
  - `instant` — direction keys move the snake immediately, ahead of the tick
    timer. Press an arrow three times within one tick and the snake moves
    three times.
  - `on tick` — the snake moves only on the tick. Keys pressed in between are
    queued (up to three) and used one per tick, so a quick up-then-left still
    turns twice.
- **Three edge-wall modes**
  - `solid` — the board is enclosed; hitting the border ends the game
  - `none (wrap)` — the snake wraps around to the opposite side
  - `gaps` — random border walls with gaps; gaps are generated in
    mirrored pairs, so wrapping through a gap always lands on an open cell
- **Inner wall generation** that never boxes you in: every free cell stays
  reachable, and no single cell is a bottleneck. Every part of the board has
  at least two separate ways in and out, so there are no dead ends and no
  rooms behind a one-cell doorway that the snake could enter but never leave.
- **Food, super food, and poison**, each with a configurable spawn frequency
  and an independent time-to-live — uneaten items disappear.
- **Super food** grows the snake by a configurable amount and can optionally
  double the game speed for a configurable duration.
- **Two growth modes** — grow instantly on eating, or only when the eaten food
  reaches the tail.
- **Optional pass-through-self** — decide whether biting yourself is fatal.
- **Fully customizable colors** for every cell type (head, body, food, super
  food, poison, walls, background).
- **Per-configuration records** — your best length is tracked separately for
  every combination of gameplay settings and persisted across sessions.
- The last-used configuration is saved and restored on the next launch.

## Installation

### Prebuilt binaries

Download the archive for your platform from the
[releases page](../../releases), extract it, and run the `snaketui` binary
(`snaketui.exe` on Windows).

### From source

Requires a recent stable Rust toolchain (edition 2024).

```sh
cargo install --path .
# or just run it:
cargo run --release
```

## Serving it over SSH

`snaketui-server` lets anyone play from their own terminal with
`ssh play.example.com`. The game is the SSH server: players get no shell and
no account on the machine, and quitting the game ends their connection at once.

- **Guests and accounts.** Anyone can play as a guest. Signing up in the
  game adds a username, a rating and friends (see below). Even as a guest, a
  player who connects with an SSH key keeps their nickname, settings and
  records between visits.
- **Only the game.** Commands (`ssh host ls`), file transfer and port
  forwarding are refused with a message.
- **Limits.** Connections are capped in total and per IP address. Logins time
  out, idle players are logged out, and clients that stop reading are
  disconnected.

### Multiplayer

Rooms hold 2 to 8 players on one board. Every snake moves on the same tick,
so in multiplayer turns are always queued for the next tick.

- **Quick play** puts you in a queue. A match starts as soon as 8 players
  are waiting, or after 10 seconds with at least 2. It plays one round on a
  30×18 board, which fits an 80×23 terminal. Matches are rated.
- **Private rooms** use your single-player settings and get a 6-character
  code to share. Anyone with the code can join. A round starts 15 seconds
  after the second player arrives, or as soon as everyone presses Enter.
  Rounds keep going until everyone leaves.
- **Winning.** The last snake alive wins. If all the snakes still in play
  crash on the same tick, it's a draw. After 3 minutes, the longest snake
  still alive wins.
- **Dead snakes turn into food.** Their players watch the rest of the round.
- **Leaving.** Esc asks before you leave a round you're still playing. The
  others play on.
- **Telling snakes apart.** Your snake keeps your colors. Other players'
  snakes get colors that don't clash with your food or walls, and every head
  shows its player's number.
- **Small terminals.** If the board doesn't fit, you see the part around
  your snake, with a warning.

### Accounts, friends and ratings

Guests can play everything except friends and ratings. To get those, pick
**Sign up** on the home screen and choose a username and a password.

- **Signing in.** The SSH key you signed up with signs you in by itself on
  later visits. On another machine, pick **Sign in** and type your password.
  Leave "remember this machine" ticked to skip that next time. An account
  remembers up to 10 machines.
- **Your settings and records come along.** If you played as a guest with a
  key before signing up, your single-player settings and records move into
  the new account.
- **Account** shows your rating, games and wins, and the machines that sign
  you in, each of which you can remove. You can change your password there,
  or sign out, which also forgets this machine.
- **Friends.** Add a friend by username. They accept in their own Friends
  screen, and if they're online they hear about it at once. Your list shows
  which friends are online and what they're doing.
- **Playing with a friend.** Pick an online friend to invite them. You're put
  in a new private room, or press `i` in a room you're already in. Your
  friend sees the invite on whatever screen they're on: Enter joins, Esc
  dismisses it. Invites last 2 minutes.
- **Ratings.** Every account starts at 1000. Only quick play is rated: each
  match moves your rating by where you placed and whom you beat, and the
  results show by how much. Quick play looks for players of a close rating
  first, and widens the range the longer you wait. Guests can join rated
  matches; they count as 1000, and nothing is saved for them.
- **Passwords** are typed into the game's own masked form, never into ssh's
  password prompt, which stays off. The server keeps only an Argon2id hash.
  A connection closes after 5 wrong tries, and 5 wrong tries for one username
  within 15 minutes lock it for 15 minutes. There's no email, so a forgotten
  password can only be replaced from a machine that still signs you in, under
  **Account → Change password**.

To host it on Ubuntu 24.04, follow [deploy/INSTALL.md](deploy/INSTALL.md).
For more players, or players in several regions, run several servers as one
with [deploy/CLUSTER.md](deploy/CLUSTER.md). Prebuilt Linux servers are on the
[releases page](../../releases) next to the game.

To try it on your own machine:

```sh
cat > /tmp/snaketui-dev.toml <<'TOML'
listen = "127.0.0.1:2022"
host_key = "/tmp/snaketui-dev/host_ed25519_key"
database = "sqlite:///tmp/snaketui-dev/snaketui.db"
TOML
cargo run --release -p snaketui-server -- --config /tmp/snaketui-dev.toml
```

Then, in another terminal, run `ssh -p 2022 localhost`.

## How to play

1. Adjust the settings in the config menu (see below), then press **Enter**.
2. The game waits frozen until your first direction key — that key both starts
   the game and makes the first move.
3. Eat food to grow. Poison and walls kill you; so does your own body unless
   pass-through-self is enabled.

If the terminal gets too small to show the whole board, the game pauses until
you make the window bigger again and resume.

### Controls

| Key | Action |
| --- | --- |
| `↑ ↓ ← →` / `WASD` | Move; in the settings menu, select (`↑ ↓`) and change a value (`← →`). The menu also takes `hjkl` |
| `Enter` | Start game / confirm |
| `Esc` / `p` | Pause (resume or quit to menu) |
| `r` `r` | Reset all settings to defaults (in the menu; press `r` twice) |
| `r` | Play again with the same settings (after game over) |
| `q` | Quit (from the pause menu: back to the settings) |
| `Ctrl+C` | Quit right away, from any screen |

Letter keys work the same with Shift or Caps Lock on.

### Config options

| Option | Description |
| --- | --- |
| board width / height | Board size in cells |
| tick speed | Milliseconds between automatic moves |
| edge walls | `solid` (enclosed), `none (wrap)`, or `gaps` (mirrored openings) |
| inner walls | Amount of randomly generated interior walls |
| food / super food / poison spawns every | Spawn interval per item type (`off` disables) |
| food / super food / poison lifetime | Seconds before an uneaten item disappears (`off` = forever) |
| move timing | `instant` (a key moves the snake right away) or `on tick` (keys queue up, one per tick) |
| growth mode | `instant` or `when food reaches tail` |
| pass through self | If `yes`, the snake can cross its own body |
| super food growth | Length gained from super food |
| super food speed boost | Duration of the 2× speed boost (`off` disables) |
| food / super food / poison icon | Glyph drawn on the board (dot, star, heart, cross, …) |
| colors | Per-cell-type colors (head, body, food, super food, poison, walls, background) |

The board's frame shows your current length, the record for the active
configuration, and the elapsed play time in its heading.

## Data files

Everything lives in `~/.snaketui/`:

| File | Contents |
| --- | --- |
| `config` | Last-used configuration (plain `key=value` text) |
| `records` | Best length per gameplay-configuration combination |

Colors are cosmetic and don't affect which record a game counts toward.
On-tick games keep records separate from instant ones.

## Development

The code is a Cargo workspace:

| Crate | What's in it |
| --- | --- |
| `snaketui` (repository root) | The local game: terminal loop and `~/.snaketui` files |
| `crates/snake-core` | Game rules: board generation, any number of snakes moving together (`World`), single player (`SoloGame`), settings, records. No terminal or file access |
| `crates/snake-tui` | Screens and the app state machine, drawn through any ratatui backend |
| `crates/snaketui-server` | The SSH server: sign-in, accounts, friends and ratings, one actor per player session and per room (actix), the quick-play matcher, private-room codes, the store (SQLite, or Postgres for several servers), rooms and presence shared between servers through NATS, draining on shutdown, health and metrics over HTTP, the PROXY protocol, `server.toml`; and `snaketui-matchmaker`, the quick-play queue for a cluster of servers |
| `crates/loadbot` | `snaketui-loadbot`: thousands of bot players, to see how servers hold up |
| `crates/test-services` | Postgres and NATS for the tests, in throwaway Docker containers |
| `deploy/` | systemd units, example `server.toml`, the Ubuntu install and cluster guides, and a docker-compose cluster to try (`deploy/cluster/`) |

```sh
cargo test --workspace   # rules, wall invariants, key handling, every screen at
                         # every size, and the SSH server end to end
cargo build --release    # builds the `snaketui` binary
cargo build --release -p snaketui-server
```

Some server tests need Postgres and NATS. They start them in Docker on their
own, and remove the containers when they finish. To use servers you already
run, set `SNAKETUI_TEST_POSTGRES` to the URL of a database whose user may
create databases, and `SNAKETUI_TEST_NATS` to a NATS server.
