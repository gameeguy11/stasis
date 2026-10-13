=======
# sixb6t-login-bot

A Minecraft bot for [6b6t.org](https://6b6t.org) that logs a cracked account in, gets it through the lobby/queue to the main server, and keeps it connected. It can also click a configured trapdoor on demand, either from a chat trigger or from the console.

Targets **Minecraft 1.21.11** and is built on [azalea](https://github.com/azalea-rs/azalea) 0.15.
Uses a locally patched copy of `azalea-chat` (in `vendor/`) so 1.21.11 chat NBT that azalea 0.15 can't parse no longer drops the packet.

## To do list:
Adding kill aura,auto totem,auto eat...

## Community

[![Discord](https://invidget.switchblade.xyz/fdHkyVYc8)](https://discord.gg/fdHkyVYc8)

## Credits

This project is a fork of [EnderKill98/stasis-bot](https://github.com/EnderKill98/stasis-bot) by **EnderKill98**. Many thanks to the original author for the foundation this is built on. Both projects are released under the MIT License (see [LICENSE](LICENSE)).

## Features

- Automatic `/register` and `/login`
- Walks through the lobby portal and waits out the backup-server queue until it reaches main
- Automatic reconnect with backoff after disconnects, cycling through the configured servers
- Optional trapdoor click, triggered by a specific player saying a specific word in chat, or by pressing Enter in the console
- Interactive setup wizard and edit menu, with settings saved to JSON
- Multiple accounts via separate config files

## Requirements

- [Rust](https://rustup.rs) via `rustup`. The nightly toolchain is pinned in `rust-toolchain.toml` and installed automatically on first build.
- A C toolchain / linker for your platform.
- Optional, for faster builds: the settings in `.cargo/.config` (clang + lld). Rename it to `.cargo/config.toml` to enable it.

## Setup

```sh
git clone <your-fork-url> sixb6t-login
cd sixb6t-login
cargo run --release
```

On first start a wizard asks for:

1. Server addresses (comma separated, tried in order)
2. Account name
3. Password
4. Register mode

After that you get a menu where every setting can be edited. Choose **Start the bot** to connect. Settings are saved to `sixb6t-config.json` after every edit.

Passwords and trapdoor coordinates are shown as `*` while you type them.

> `sixb6t-config.json` stores the password in plain text. It is listed in `.gitignore`; never commit or share it.

## Usage

```sh
cargo run --release
cargo run --release -- --run
cargo run --release -- --config alt2.json
cargo run --release -- --config alt2.json --run
```

| Flag | Effect |
| --- | --- |
| `--run` | Skip the menu and start with the saved settings. Requires a complete config (server, account name, password). |
| `--config <file>` | Use a different settings file. Use one file per account to run several accounts. |

### Menu settings

| Setting | Description |
| --- | --- |
| Server addresses | Comma-separated list, tried in order. `host:port` is supported, default port is 25565. |
| Account name | Cracked account username. |
| Password | Used for `/register` and `/login`. Leave empty when editing to keep the current one. |
| Registering | `auto` registers only when the server asks, `always` sends `/register` right after joining, `never` only sends `/login`. |
| Wait after /login | Seconds to wait after logging in before walking. |
| Walk timeout | Seconds before a walk phase is aborted. |
| Stay connected after | Keep the bot connected once it has finished. |
| Trapdoor (X Y Z) | Coordinates of the trapdoor to click, e.g. `12 64 -30`. Type `clear` to remove it. |
| Chat trigger user | Player whose chat messages are watched. Empty disables the trigger. |
| Chat trigger word | Word or phrase that makes the bot click the trapdoor. Empty disables the trigger. |

### Config file

```json
{
  "servers": ["alt.6b6t.org", "alt2.6b6t.org"],
  "username": "YourAccount",
  "password": "your-password",
  "force_register": false,
  "no_register": false,
  "walk_delay": 3.0,
  "walk_timeout": 15.0,
  "stay": true,
  "trapdoor": [12, 64, -30],
  "trigger_user": "SomePlayer",
  "trigger_word": "NOW"
}
```

`trapdoor` may be `null`. Missing fields fall back to defaults.

### Trapdoor trigger

When a trapdoor is set, the bot clicks it when:

- `trigger_user` sends a chat message containing `trigger_word` (case-insensitive, whole word only), or
- you press **Enter** in the bot's console window.

Repeated triggers within 1.5 seconds are ignored.

### Reconnecting

After a disconnect the bot reconnects on its own with increasing delay (up to 60 s), rotates through the configured servers, and repeats the register/login/walk sequence.

## Troubleshooting

- **Build fails with a simd-related error:** the latest nightly can break azalea. In `rust-toolchain.toml` set `channel` to an older dated nightly, e.g. `nightly-2025-12-01`.
- **`--run needs a complete saved config`:** start once without `--run` and fill in server, account name and password.
- **Stuck on "backup server":** this is the 6b6t queue. The server moves the bot to main when a slot frees up, usually within a minute or two.

## License

MIT. Original work Copyright (c) 2024 EnderKill98. See [LICENSE](LICENSE).

[discord]: https://discord.gg/HX6rSFg3k
[vidget-discord]: https://invidget.switchblade.xyz/fdHkyVYc8
>>>>>>> 160cc57 (making it pub)
