# jotbar

A tiny, fast notes app that lives in your tray. Press a key, jot, press it again.

**Your notes are yours.** They are plain Markdown files on your machine, and if
you want them on other devices **you pick where they sync to**: a folder you
already sync, your own server, or any WebDAV share. No account, no
subscription, no cloud you don't control.

- **Tray + hotkey popover:** opens instantly, hides when you click away
- **Tabs:** one note per tab; drag to reorder, `Ctrl+1…9` to jump
- **Search across every note:** `Ctrl+F`
- **Magic mode:** inline AI completion from a model running *locally* through
  [Ollama](https://ollama.com). Press `Tab` to accept. Nothing leaves your machine.
- **Four window sizes:** S / M / L / XL, plus adjustable text size
- **Bring-your-own sync:** folder, self-hosted server, or WebDAV (below)
- **Picks up your [pywal](https://github.com/dylanaraps/pywal) palette** if you have one
- Linux, macOS and Windows (Tauri 2, about 10 MB)

## Where your notes live

```
~/.local/share/jotbar/          (macOS: ~/Library/Application Support/jotbar)
  notes/<id>.md                 one plain Markdown file per tab
  index.json                    tab order + sync bookkeeping
```

You can change the folder in Settings. Files you drop into `notes/` show up as tabs.

## Sync: you choose where

Open Settings (`Ctrl+,`), go to **Sync**, pick a target and press *Test connection*.

| Target | Use it when | What you need |
|---|---|---|
| **Folder** | You already run Syncthing, the Nextcloud/Dropbox/OneDrive desktop client, or have a NAS mount or USB drive | A path, e.g. `~/Sync/jotbar` |
| **jotbar server** | You want a small always-on server on a VPS, home server or Raspberry Pi | `jotbar-server` (below) |
| **WebDAV** | You have Nextcloud, ownCloud, Synology, Fastmail files, `rclone serve webdav` … | URL + username + app password |

Every target stores notes as readable `.md` files, so nothing is locked in.

**Conflicts never lose text.** jotbar compares each note three ways (this
device, the target, and the last version both agreed on). Whichever side
changed wins. If both changed, the target's version stays in the tab and your
local text is kept in a new tab marked `⚠ Conflicted copy`.

### Self-hosting the server

Docker:

```sh
cd crates/server
export JOTBAR_TOKEN=$(openssl rand -hex 32)   # put the same token in the app
docker compose up -d                          # listens on :8787, data in ./data
```

Bare binary:

```sh
cargo build --release -p jotbar-server
JOTBAR_TOKEN=... JOTBAR_ADDR=0.0.0.0:8787 JOTBAR_DATA=/var/lib/jotbar ./target/release/jotbar-server
```

Put it behind HTTPS (Caddy, nginx, Tailscale, Cloudflare Tunnel) if it is
reachable from the internet. The data directory is plain files, so a backup is a copy of it.

API (Bearer token): `GET /api/notes`, `GET /api/notes/{id}`,
`PUT /api/notes/{id}` (compare-and-swap via `base_hash`, `409` on conflict), `GET /health`.

## Magic mode

```sh
ollama pull qwen2.5:1.5b      # any small model works; bigger = smarter but slower
```

Then go to Settings, then **Magic mode**, enable it and pick the model. You can
also toggle it from the `✦ magic` chip or with `Ctrl+Shift+M`. Suggestions show
as grey text at the end of a line: `Tab` accepts all of it, `Ctrl+→` accepts
one word, `Esc` dismisses it.

## Keys

| | |
|---|---|
| `Ctrl+T` / `Ctrl+N` | new note |
| `Ctrl+W` / middle-click tab | delete note (asks first if it isn't empty) |
| `Ctrl+Tab`, `Ctrl+PgUp/PgDn` | next / previous tab |
| `Ctrl+1…8`, `Ctrl+9` | jump to tab, last tab |
| `Ctrl+F` / `Ctrl+K` | search all notes |
| `Ctrl+S` | sync now |
| `Ctrl+Shift+M` | toggle magic mode |
| `Ctrl+=` / `Ctrl+-` / `Ctrl+0` | text size |
| `Ctrl+,` | settings |
| `Esc` | close panel / hide window |

### Global hotkey

The default is `Ctrl+Shift+Space` (change it in Settings). That works on macOS,
Windows and X11. **Wayland** doesn't let apps grab global keys, so bind the
toggle in your compositor instead. A second launch is forwarded to the running app:

```sh
jotbar --toggle     # show/hide
jotbar --hidden     # start in the tray (for autostart)
```

Hyprland (Lua config):

```lua
hl.bind(mainMod .. " + SHIFT + N", hl.dsp.exec_cmd("jotbar --toggle"))
hl.window_rule({
    name  = "jotbar-popover",
    match = { class = "jotbar" },
    float = true,
    pin   = true,
    move  = "monitor_w-window_w-12 44",   -- top-right, under the bar
})
```

Classic `hyprland.conf`: `bind = SUPER SHIFT, N, exec, jotbar --toggle` plus
`windowrule = float, pin, move 100%-w-12 44, class:jotbar`. sway:
`bindsym $mod+Shift+n exec jotbar --toggle` plus `for_window [app_id="jotbar"] floating enable, sticky enable`.

## Building

You need Rust (stable) and the [Tauri 2 prerequisites](https://v2.tauri.app/start/prerequisites/)
(on Arch: `webkit2gtk-4.1 gtk3 libappindicator-gtk3`). You don't need Node: the UI is plain HTML/JS.

```sh
cargo run -p jotbar                       # dev run
cargo build --release -p jotbar           # binary at target/release/jotbar
cargo test                                # core + server + sync tests

# installable packages (.deb/.rpm/.AppImage/.dmg/.msi):
cargo install tauri-cli --version '^2' && cd app/src-tauri && cargo tauri build
```

## Layout

```
crates/core     note store, sync algorithm, sync targets (folder / server / WebDAV)
crates/server   jotbar-server, the self-hosted sync server (axum)
app/src-tauri   desktop app (Tauri 2): tray, window, commands
app/ui          frontend: index.html, style.css, app.js (no build step)
```

## Branches

- `dev`: active development; PRs go here
- `prod`: stable releases; only fast-forwarded from `dev`

## License

MIT
