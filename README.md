# SnakeTUI

A highly configurable snake game for your terminal, built in Rust with
[ratatui](https://ratatui.rs).

## Features

- **Responsive controls** — direction keys move the snake immediately, ahead of
  the tick timer. Press an arrow three times within one tick and the snake
  moves three times.
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

## Development

```sh
cargo test    # wall generation invariants (mirroring, connectivity, no bottleneck cells)
cargo build --release
```
