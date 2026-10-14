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
| Trapdoors | Any number of trapdoors, each with its own coordinates and chat trigger word. Add, edit or remove them in the submenu. |
| Chat trigger users | Players whose chat messages are watched. Add or remove them in the submenu; duplicates are refused. An empty list disables the trigger. |
| Combat (totem/eat/aura) | Submenu: auto totem, auto eat and its food threshold, auto armor, auto best weapon, kill aura on/off, range, hostile mobs, neutral mobs, players, never-attack list, always-attack list. |

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
  "trigger_users": ["SomePlayer", "OtherPlayer"],
  "traps": [
    { "pos": [12, 64, -30], "word": "NOW" },
    { "pos": [14, 64, -30], "word": "LATER" }
  ],
  "auto_totem": true,
  "auto_eat": true,
  "eat_below": 14,
  "auto_armor": true,
  "auto_weapon": true,
  "kill_aura": true,
  "kill_aura_range": 3.0,
  "kill_aura_hostiles": true,
  "kill_aura_neutral": false,
  "kill_aura_players": false,
  "kill_aura_never": ["creeper", "SomeFriend"],
  "kill_aura_always": ["iron_golem"]
}
```

Missing fields fall back to defaults. Old `trapdoor` / `trigger_word` fields are converted into `traps`, and an old `trigger_user` into `trigger_users`, automatically.

### Combat helpers

- `auto_totem` keeps a totem in the offhand.
- `auto_eat` eats when the food level is at or below `eat_below` (0-20). Food comes from the hotbar or inventory; junk food is skipped.
- `auto_armor` wears the best armor found in the inventory by material (netherite, diamond, iron, chainmail, gold, leather). An elytra in the chest slot is kept.
- `auto_weapon` holds the best sword or axe before each kill aura attack.

### Kill aura filters

- `kill_aura_never` wins over everything: mob types (`creeper`, `zombie`) or player names.
- `kill_aura_always` attacks listed mob types even if they are not monsters, e.g. `wolf`.
- Neutral mobs are endermen, piglins and zombified piglins. Players are only attacked when `kill_aura_players` is on and the name is not in `kill_aura_never`.

### Trapdoor trigger

When any of the `trigger_users` sends a chat message containing a trapdoor's word (case-insensitive, whole word only), the bot walks to within reach of that trapdoor and clicks it. Pressing **Enter** in the bot's console does the same for trapdoor 1. Walking only happens on the main server; elsewhere the bot just clicks.

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
