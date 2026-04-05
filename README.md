# Clipbro

A clipboard manager for the COSMIC desktop environment. Clipbro runs as a background daemon, keeps a history of everything you copy, and shows a card-based overlay when you need to find something.

## Features

- **Card-based overlay** with image previews and syntax-highlighted code
- **Favorites** pin entries so they survive clears and deletions. Toggle with Ctrl+F or click the star on any card. Favorited cards get a gold border and filled star
- **Syntax highlighting** for 200+ languages including Rust, Python, Go, JavaScript, TypeScript, TOML, YAML, JSON, Dockerfile, Bash, SQL, CSS, Markdown, and more
- **Language detection** identifies what you copied and labels each card (e.g., "Python", "TOML", "JSON")
- **Image thumbnails** for copied images and optionally for image URLs
- **Multi-term search** across content, language, and type. Type `python quickvm` to find Python entries containing "quickvm". Type `image` to filter to images only
- **Instant search** starts filtering as you type, no need to click the search box
- **Delete entries** with the Delete key. Favorites are protected and cannot be deleted
- **Configurable hotkeys** for favorite toggle and entry deletion
- **Clipboard and primary selection sync** keeps both buffers in sync
- **Encrypted database** with SQLCipher, key stored in your system keyring
- **Configurable overlay position** at the top, bottom, left, or right edge of your screen
- **COSMIC theme support** matches your system dark/light mode
- **systemd integration** for starting on login

## Getting Started

### Prerequisites

- COSMIC desktop environment
- Rust toolchain (install from [rustup.rs](https://rustup.rs))
- `wl-clipboard` (`wl-copy` and `wl-paste`)
- A secret service provider (GNOME Keyring, KDE Wallet, or oo7) for database encryption

On Fedora:

```sh
sudo dnf install wl-clipboard
```

On Ubuntu/Debian:

```sh
sudo apt install wl-clipboard
```

### Install

```sh
cargo install --git https://github.com/jdoss/clipbro
```

### Set Up

Initialize the config file and database:

```sh
clipbro init
```

This creates:
- `~/.config/clipbro/config.toml` with commented defaults
- `~/.local/share/clipbro/clipbro.db` (encrypted by default)

Install and enable the systemd user service:

```sh
clipbro install
```

Start the service:

```sh
clipbro start
```

Clipbro is now running in the background and recording your clipboard history.

### Bind a Hotkey

Add a keyboard shortcut in COSMIC Settings that runs:

```sh
clipbro toggle
```

This opens and closes the overlay. When the overlay is open:

- **Arrow keys** navigate between cards (left/right for top/bottom bars, up/down for sidebars)
- **Type** to search and filter cards instantly
- **Backspace** to edit your search
- **Escape** or **Enter** selects the focused card and copies it to your clipboard
- **Ctrl+F** toggles favorite on the focused card
- **Delete** removes the focused card (no-op on favorites)
- **Click** a card to select it directly
- **Click the star** on any card to toggle its favorite status
- Clicking outside the overlay or losing focus also selects the focused card

## Configuration

Edit `~/.config/clipbro/config.toml`:

```toml
# Maximum number of clipboard entries to keep
max_entries = 100

# Sync clipboard and primary selection
sync_selections = true

# Encrypt the database using the system keyring
encrypt_db = true

# Show image thumbnails in the overlay
show_thumbnails = true

# Fetch and cache thumbnails for image URLs
show_remote_thumbnails = false

# Maximum size in bytes for remote thumbnail downloads (5 MB)
max_thumbnail_bytes = 5242880

# Overlay position: "top", "bottom", "left", "right"
position = "top"

[hotkeys]
# Toggle favorite on the focused entry
toggle_favorite = "ctrl+f"

# Delete the focused entry (favorites are protected)
delete_entry = "delete"
```

### Overlay Position

The `position` setting controls where the overlay appears and how cards are arranged:

| Position | Layout | Navigation |
|----------|--------|------------|
| `top` | Horizontal cards along the top edge | Arrow Left/Right |
| `bottom` | Horizontal cards along the bottom edge | Arrow Left/Right |
| `left` | Vertical cards along the left edge | Arrow Up/Down |
| `right` | Vertical cards along the right edge | Arrow Up/Down |

### Image URL Thumbnails

Set `show_remote_thumbnails = true` to automatically fetch and cache thumbnail previews for image URLs you copy. Thumbnails are stored in the database so they only get downloaded once. The `max_thumbnail_bytes` setting limits how large a remote image can be before it's skipped.

### Hotkeys

The `[hotkeys]` section lets you rebind overlay keyboard shortcuts. Values are modifier+key strings like `"ctrl+f"`, `"alt+d"`, or `"delete"`. Supported modifiers: `ctrl`, `alt`, `shift`. Hotkeys match on the logical key, so remapped keyboard layouts (Colemak, Dvorak, etc.) work correctly.

### Database Encryption

By default, the clipboard database is encrypted with SQLCipher. The encryption key is stored in your system keyring. If you don't have a secret service provider running, either install one or set `encrypt_db = false`.

## Commands

```
clipbro              Start the daemon (foreground)
clipbro init         Create default config and database
clipbro install      Install and enable systemd user service
clipbro start        Start the systemd service
clipbro stop         Stop the systemd service
clipbro restart      Restart the systemd service
clipbro status       Show systemd service status
clipbro toggle       Toggle the overlay (bind this to a hotkey)
clipbro show         Open the overlay
clipbro hide         Close the overlay
clipbro clear        Delete all non-favorite clipboard entries
```

## Files

| Path | Purpose |
|------|---------|
| `~/.config/clipbro/config.toml` | Configuration |
| `~/.local/share/clipbro/clipbro.db` | Clipboard database |
| `~/.local/share/clipbro/clipbro.log` | Log file |
| `~/.config/systemd/user/clipbro.service` | systemd unit (created by `clipbro install`) |

## License

MIT. See [LICENSE](LICENSE) for details.
