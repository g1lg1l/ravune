# Install and run

## Download

The [releases page](https://github.com/g1lg1l/ravune/releases) has the 0.9.3 beta:

| System | File | |
|---|---|---|
| Windows 10 or 11 (x64) | `ravune-0.9.3-beta-windows-x64-setup.exe` | the installer: Start menu entry, uninstaller, `.ravune` files open in Ravune. Or the `.zip`, to run it from any folder |
| macOS 11 or newer (Apple Silicon) | `ravune-0.9.3-beta-macos-arm64.dmg` | drag Ravune to Applications |
| Linux (x64) | `ravune-0.9.3-beta-linux-x64.tar.gz` | unpack and run `ravune`; needs ALSA |

The builds aren't signed yet. On Windows, SmartScreen may stop the installer: More info →
Run anyway. On macOS, the first time, right-click Ravune → Open.

You download Ravune once. After that it updates itself: when a new version is out, it says
so at start, and Preferences → Extensions → Update swaps it in. The installer puts Ravune in
your user folder for that (no admin rights needed); if you pick "all users", updates run the
next installer instead.

## Build it

Ravune builds from source with [Rust](https://rustup.rs) (stable). On Windows you need the
MSVC toolchain; on Linux, `libasound2-dev`.

```bash
git clone https://github.com/g1lg1l/ravune
cd ravune
cargo run --release -p ravune
```

File → New from Template gives you a song to start from: `Space` plays it, `F9` opens the mixer.

The program is `target/release/ravune`. Keep the two helpers next to it:
`ravune-host` runs plugins (one process each) and `ravune-mcp` lets agents in.

## Where things live

| What | Where |
|---|---|
| Settings, autosave, the plugin list | `%APPDATA%\Ravune\data` on Windows |
| Your sounds and instruments | `Documents/Ravune/Library` (and any folder you add in the browser) |
| Sounds kept from the Online tab, with their credits | `Documents/Ravune/Online Sounds` |
| Your scripts | `Documents/Ravune/Scripts` |
| Installed extensions | `Documents/Ravune/Extensions` |

`RAVUNE_DATA=<folder>` runs Ravune with a separate profile: its own settings, autosave and
extensions.
