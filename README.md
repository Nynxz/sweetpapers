# sweetpapers

Per-monitor wallpaper rotator for Wayland. Picks images from your wallpaper
packs, hands them to [awww](https://codeberg.org/LGFae/awww) one monitor at a
time, and exposes a control socket so wofi pickers, status bars, Qt apps, and
hyprland binds can drive it.

## Install

```sh
cargo build --release
cp target/release/sweetpapers ~/.local/bin/
```

Requires [awww](https://codeberg.org/LGFae/awww) on `PATH` with `awww-daemon`
running, plus a Wayland session.

## Quick start

1. Copy the example config to the default location:

   ```sh
   mkdir -p ~/.config/sweetpapers
   cp config.example.jsonc ~/.config/sweetpapers/config.jsonc
   $EDITOR ~/.config/sweetpapers/config.jsonc
   ```

2. Start the daemon (typically from your compositor's autostart):

   ```sh
   sweetpapers daemon -p Background1
   ```

3. Drive it from anywhere:

   ```sh
   sweetpapers status
   sweetpapers pack Background2
   sweetpapers next
   ```

## Commands

| Command | What it does |
|---|---|
| `daemon -p PACK` | Run the rotation loop + control socket |
| `status` | Current pack, paused state, per-monitor images |
| `list` | Available packs + thumbnail paths |
| `pack NAME` | Switch active pack |
| `next` / `prev` | Force a swap to next / previous directory |
| `pause` / `resume` | Stop / start automatic rotation |
| `reload` | Re-read the config file (keeps current pack) |
| `interval SECS` | Change the rotation interval at runtime |
| `thumbnail NAME [--force]` | Get or regenerate a pack's thumbnail |

Add `--json` to any command for raw response output.

## Configuration

The daemon reads `~/.config/sweetpapers/config.jsonc` by default
(`$XDG_CONFIG_HOME/sweetpapers/config.jsonc`). Override with `--config`.

See [`config.example.jsonc`](config.example.jsonc) for a commented
template. Key fields:

| Key | Meaning |
|---|---|
| `screens.<id>.name` | Monitor name (find via `hyprctl monitors`). |
| `screens.<id>.orientation` | `landscape` or `portrait`. |
| `defaults.auto` | `true`: pick images by orientation match. `false`: group files by numeric filename prefix (`1.jpg`, `2_3.png` → groups 1 and 2) and map group key to screen key. |
| `defaults.sequence` | `true`: swap monitors one at a time with `interval` between each. `false`: swap all at once, then sleep. |
| `defaults.packs_location` | Root directory of packs. `~` is expanded. |
| `defaults.pack` | Pack the daemon starts on when `-p` is omitted. |
| `defaults.screen_order` | Optional. Order monitors are swapped in. Defaults to sorted `screens` keys. |
| `defaults.swap_on_pack_change` | `true` (default): switching packs swaps the wallpaper now. `false`: waits for the next tick. |
| `transition.next` | `ordered` (cycle directories) or `random`. |
| `transition.fill_mode` | `crop`, `fit`, `stretch`, or `no`. Passed to `awww --resize`. |
| `transition.interval` | Seconds between swaps. |
| `transition.transition_type` / `_duration` / `_step` / `_fps` | See [awww docs](https://codeberg.org/LGFae/awww). |

### Pack layout

```
~/Wallpapers/packs/
├── Background1/
│   ├── Oceans/
│   │   ├── 1.jpg
│   │   └── 2.jpg
│   ├── Mountains/
│   │   ├── 1.jpg
│   │   └── 2_1.jpg
│   └── .sweet_thumb.jpg   ← optional manual thumbnail override
└── Background2/
    └── …
```

### Thumbnails

`sweetpapers list` returns per-pack thumbnails at
`$XDG_CACHE_HOME/sweetpapers/thumbs/<name>.jpg` (256×256 JPEG q80). The
daemon picks the first image alphabetically from the first subdirectory and
center-crops it. Drop a `.sweet_thumb.jpg` (or `.png`) at a pack's root to
override the source. Cache is invalidated when any pack file becomes newer
than the thumbnail.

## Hyprland integration

```lua
hl.on("hyprland.start", function()
    hl.exec_cmd("awww-daemon")
    hl.exec_cmd("sweetpapers daemon -p Background1")
end)

hl.bind("SUPER + N",         hl.dsp.exec_cmd("sweetpapers next"))
hl.bind("SUPER + SHIFT + N", hl.dsp.exec_cmd("sweetpapers prev"))
hl.bind("SUPER + P",         hl.dsp.exec_cmd("sweetpapers pause"))
```

## How it works

`sweetpapers daemon` runs in the background, rotates through your packs, and
calls `awww img` to set each monitor. It also listens on a Unix socket at
`$XDG_RUNTIME_DIR/sweetpapers.sock`.

Running `sweetpapers` with any other subcommand (`status`, `pack`, `next`,
...) opens that socket, sends one JSON message, prints the reply, and exits.

See [`docs/protocol.md`](docs/protocol.md) if you want to talk to the socket
from your own tools.
