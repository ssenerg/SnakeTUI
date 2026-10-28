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
- **Which game.** `ssh snake@play.example.com` opens Snake. Plain
  `ssh play.example.com` opens the game you had open last time, kept for
  your key or account. Once a server has more than one game, new players
  pick one, and `g` on the home menu switches. Chess is the second, on
  servers that turn it on (see [Chess](#chess)).
- **Only the game.** Commands (`ssh host ls`), file transfer and port
  forwarding are refused with a message.
- **Limits.** Connections are capped in total and per IP address. Logins time
  out, idle players are logged out, and clients that stop reading are
  disconnected.

### Multiplayer

Rooms hold 2 to 8 players on one board. Every snake moves on the same tick,
so in multiplayer turns are always queued for the next tick, and super food
gives no speed boost.

- **Quick play** puts you in a queue. A match starts as soon as 8 players
  are waiting, or after 10 seconds with at least 2. It plays one round on a
  30×18 board, which fits an 80×23 terminal. Matches are rated. After 20
  seconds with nobody to play, press `b` for a match against 3 bots
  instead; it isn't rated.
- **Private rooms** use your single-player settings and get a 6-character
  code to share. Anyone with the code can join. A round starts 15 seconds
  after the second player arrives, or as soon as everyone presses Enter.
  Rounds keep going until everyone leaves.
- **Room rules.** In a private room's lobby, press `e` to see the room's
  rules: the board, food and gameplay settings. The host (whoever has been in
  the room longest) changes any of them there with ←→, and makes the next
  rounds faster or slower with ←→ right in the lobby. Everyone else is told
  what changed. A change un-readies everyone and restarts the 15 seconds, so
  nobody starts by rules they didn't see. A board too small for everyone
  already in the room is refused. The changes belong to the room; the host's
  own settings stay as they were.
- **Bots.** In a private room's lobby, the host adds a bot with `b` and takes
  the last one out with `x`. Bots are always ready, so a player alone can
  start a round with them. Someone joining a full room takes a bot's seat.
  A bot heads for the nearest food it can reach and stays clear of walls,
  bodies, poison and dead ends. Bots are named after snakes (Viper, Cobra,
  and so on), and accounts can't take those names. Once every person's snake
  is out, the round ends: the longest bot left wins.
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
- **Round trips.** Every player's round trip to the server shows next to
  their name: in green while turns feel instant, yellow from 80 ms, red from
  200 ms. A turn shows up a round trip after the key, plus the wait for the
  next tick. The scoreboard shows the number when it's wide enough, and a
  colored dot otherwise. The server times it by asking the terminal for a
  status report every 2 seconds, which every terminal answers.

### Chess

Chess is off unless the server's `server.toml` turns it on:
`enabled_games = ["snake", "chess"]`. Then `ssh chess@play.example.com`
opens it, and new players choose between the two.

- **Challenges.** Make a private room and share its code, or invite a
  friend. The host's challenge says the time control and which side they
  play: ←→ in the lobby steps through 1+0, 2+1, 3+0, 3+2, 5+0, 5+3, 10+0,
  10+5, 15+10, 30+0 and 30+20, and `e` sets any starting time from a
  quarter of a minute to 3 hours, any increment up to 3 minutes, and the
  host's side: white, black or drawn. A game starts once both players are
  ready, and the room's next game is the rematch. Challenges are unrated
  for now, and quick pairing comes later.
- **The board** fits an 80×24 terminal with the clocks, what each side has
  taken and the moves beside it. Smaller terminals get a smaller board.
- **Moves.** Type them, as `Nf3`, `exd5`, `O-O` or `e7e8q`: as you type,
  the squares the move could mean light up, and Enter plays it once only
  one move fits. Or pick a piece with the arrows and Enter, then its
  square. Esc opens a menu to offer or accept a draw, resign (it asks
  first), abort before both sides have moved, and flip the board.
- **Clocks** are the server's, with the increment added after each move
  and a little of each move's lag given back. Each side has 30 seconds for
  their first move, or the game is aborted.
- **Settings**, on chess's home menu: the board's colors, glyphs or letters
  for the pieces (some terminals draw the glyphs badly), coordinates,
  promoting to a queen without asking, whether letter keys type moves or
  move the cursor like WASD and HJKL, and asking before resigning.
- **Watching** works as in Snake: `v` on a friend who's playing.

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
- **Account** shows your rating, games and wins in the game you have open,
  and the machines that sign you in, each of which you can remove. You can change your password there,
  or sign out, which also forgets this machine.
- **Friends.** Add a friend by username. They accept in their own Friends
  screen, and if they're online they hear about it at once. Your list shows
  which friends are online and what they're doing.
- **Playing with a friend.** Pick an online friend to invite them. You're put
  in a new private room, or press `i` in a room you're already in. Your
  friend sees the invite on whatever screen they're on: Enter joins, Esc
  dismisses it. Invites last 2 minutes.
- **Watching a friend.** In Friends, press `v` on a friend who's in a match
  or a private room to watch their game live, with their snake in your
  colors. You don't play and nothing you press moves anything; Esc takes you
  back to Friends. Players see how many are watching. Up to 20 people can
  watch one room, whichever servers they're on.
- **Ratings.** Every account starts at 1000. Only quick play is rated: each
  match moves your rating by where you placed and whom you beat, and the
  results show by how much. Quick play looks for players of a close rating
  first, and widens the range the longer you wait. Only accounts are rated,
  against each other: guests can join rated matches, but beating them, or
  losing to them, doesn't count. Each game keeps its own ratings, and a game
  can keep several apart, as chess will its time controls.
- **Leaderboard**, on the home screen, lists the open game's top 20 accounts
  by rating, and your own place; ←→ picks another of its ratings, when it has
  several. You're listed after 5 rated games.
- **Passwords** are typed into the game's own masked form, never into ssh's
  password prompt, which stays off. The server keeps only an Argon2id hash.
  A connection closes after 5 wrong tries, and 5 wrong tries for one username
  within 15 minutes lock it for 15 minutes. There's no email, so a forgotten
  password can only be replaced from a machine that still signs you in, under
  **Account → Change password**.

To host it on Ubuntu 24.04, follow [deploy/INSTALL.md](deploy/INSTALL.md).
For more players, or players in several regions, run several servers as one
with [deploy/CLUSTER.md](deploy/CLUSTER.md). Prebuilt Linux servers are on the
[releases page](../../releases) next to the game. To run it in Docker instead,
deployed by CI, see [Deploying from CI](#deploying-from-ci).

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

### Deploying from CI

Every push to `main` that passes CI is deployed to a Linux server with
Docker Compose. The `image` job publishes the server's image to
`ghcr.io/ssenerg/game`, for amd64, tagged `latest` and `sha-COMMIT`; the
`deploy` job sends its digest over SSH. The key GitHub signs in with runs
nothing but [`deploy.sh`](deploy/docker/deploy.sh) there, which pulls
exactly that image and restarts the game with it:

- The old game lets rounds in play finish first, for up to 200 seconds.
  Until then, new players can't connect.
- If the new game doesn't come up healthy, the script puts the image that
  ran before back, and the job fails.
- The game's logs name players and their addresses, so they go to the
  server's journal (`journalctl -t snaketui-deploy`), not to the public CI
  log.

A leaked key can therefore only deploy images published to that
repository, in the setup the server has: the script, `docker-compose.yml`
and `.env` belong to root, and the key can't change them. When
`deploy/docker/deploy.sh` or `deploy/docker/docker-compose.yml` change here,
install them on the server again, as below.

**On the server.** Players connect on port 22, so move the machine's own
SSH to another port first, as in [INSTALL.md](deploy/INSTALL.md) steps 2 to
4. Docker opens the ports it publishes ahead of ufw, so ufw needs no rule
for the game, but a cloud provider's firewall still needs port 22 open.
Health and metrics are on `127.0.0.1:9022`.

Once, as root, in a clone of this repository: a user to deploy as (in the
`docker` group, which is as good as root on that server), and the directory
the game runs from, which that user may read but not change:

```sh
useradd --create-home --groups docker deploy
install -d -o root -g deploy -m 750 /opt/apps/snaketui
install -o root -g deploy -m 640 deploy/docker/docker-compose.yml /opt/apps/snaketui/
install -o root -g deploy -m 750 deploy/docker/deploy.sh /opt/apps/snaketui/
```

To change the ports, put `SNAKETUI_PORT` or `SNAKETUI_ADMIN_PORT` in
`/opt/apps/snaketui/.env`, owned the same way (see
[`docker-compose.yml`](deploy/docker/docker-compose.yml)).

**DNS.** Point the game's name, such as `play.example.com`, at the server
with an `A` record. On Cloudflare, make it DNS only (a grey cloud).
Cloudflare's proxy and its tunnels carry web traffic, not a player's `ssh`.

**Keys.** On your computer, make a key for GitHub to sign in with, and get
the server's key for GitHub to check it by. Check the key `ssh-keyscan`
prints against `ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub` on the
server:

```sh
ssh-keygen -t ed25519 -N '' -C snaketui-deploy -f snaketui-deploy
ssh-keyscan -t ed25519 -p 2222 SERVER > known_hosts   # the admin SSH's port
```

Add `snaketui-deploy.pub` to `~deploy/.ssh/authorized_keys` on the server,
with the image it may deploy in front of it. `restrict` takes away shells,
terminals and forwarding; `command` runs whatever GitHub asks for, with what
it asked for as the digest:

```
restrict,command="/opt/apps/snaketui/deploy.sh ghcr.io/ssenerg/game" ssh-ed25519 AAAA... snaketui-deploy
```

**GitHub.** Under Settings → Environments, create the environment
`production`, let only `main` deploy to it (Deployment branches → Selected
branches), and give it these secrets and variables:

```sh
gh secret set DEPLOY_HOST --env production --body SERVER
gh secret set DEPLOY_SSH_KEY --env production < snaketui-deploy
gh secret set DEPLOY_KNOWN_HOSTS --env production < known_hosts
gh variable set DEPLOY_PORT --env production --body 2222
rm snaketui-deploy   # GitHub has it now
```

The other variables are optional: `DEPLOY_USER` (`deploy`) and
`DEPLOY_URL`, the address shown with each deploy. Until the secrets are
set, the job skips deploying with a warning. If the package on `ghcr.io` is
private, sign the server in to it once with a token that may read packages:
`sudo -u deploy docker login ghcr.io`.

**Keep the volume.** The Docker volume `snaketui_data` holds the game's host
key and its database, with every account. Back it up as INSTALL.md step 8
explains. To copy it safely, stop the game first:

```sh
cd /opt/apps/snaketui && sudo -u deploy docker compose stop
sudo tar czf /root/snaketui-data.tgz -C /var/lib/docker/volumes/snaketui_data/_data .
sudo -u deploy docker compose start
```

To deploy again, run the CI workflow on `main` (Actions → CI → Run
workflow). To go back one deploy, on the server:

```sh
cd /opt/apps/snaketui && sudo -u deploy sh -c 'docker image tag snaketui:previous snaketui:latest && docker compose up -d --force-recreate snaketui'
```

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
| `crates/game-api` | What every game shares: players, the `Game` and `Round` traits a game implements, and `Room`, which runs any game's lobby, countdown and results |
| `crates/snake-core` | Game rules: board generation, any number of snakes moving together (`World`), single player (`SoloGame`), multiplayer rounds (`Snake`, a `Game`), settings, records. No terminal or file access |
| `crates/chess-core` | Chess rules: moves, SAN, clocks, endings and Glicko-2 ratings, on cozy-chess; and rooms' chess (`Chess`, a `Game`) |
| `crates/shell-tui` | The app around every game, drawn through any ratatui backend: the home menu, accounts, friends, the leaderboard, the game picker, and rooms' lobby and results. Each game adds its screens through `GameUi` |
| `crates/snake-tui` | Snake's screens: single player, and its part of rooms (`SnakeUi`, a `GameUi`). The local game drives them on their own |
| `crates/chess-tui` | Chess's screens: the board, typed and picked moves, the game menu, a room's challenge and the settings (`ChessUi`, a `GameUi`) |
| `crates/snaketui-server` | The SSH server: sign-in, accounts, friends and ratings, one actor per player session and per room of any game (actix), the games it serves (`games/`), the quick-play matcher with a pool per game and kind of match, private-room codes, the store (SQLite, or Postgres for several servers), rooms and presence shared between servers through NATS, draining on shutdown, health and metrics over HTTP, the PROXY protocol, `server.toml`; and `snaketui-matchmaker`, the quick-play queue for a cluster of servers |
| `crates/loadbot` | `snaketui-loadbot`: thousands of bot players, to see how servers hold up |
| `crates/test-services` | Postgres and NATS for the tests, in throwaway Docker containers |
| `deploy/` | systemd units, example `server.toml`, the Ubuntu install and cluster guides, the image and what CI deploys it with (`deploy/docker/`), and a docker-compose cluster to try (`deploy/cluster/`) |

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
