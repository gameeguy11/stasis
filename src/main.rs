#[macro_use]
extern crate tracing;

mod combat;
mod config;
mod ui;

use anyhow::Result;
use azalea::block::{BlockState, BlockTrait};
use azalea::entity::Position;
use azalea::prelude::*;
use azalea::protocol::packets::game::ClientboundGamePacket;
use azalea::pathfinder::PathfinderClientExt;
use azalea::pathfinder::goals::RadiusGoal;
use azalea::{BlockPos, Vec3, WalkDirection};
use clap::Parser;
use once_cell::sync::OnceCell;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::config::Config;

const EXIT_VERIFY: i32 = 2;
const EXIT_COOLDOWN: i32 = 3;
const EXIT_PREMIUM: i32 = 4;
const EXIT_LIMIT: i32 = 5;
const EXIT_WRONG_PASSWORD: i32 = 6;
const EXIT_NOT_REGISTERED: i32 = 7;
const EXIT_VERSION: i32 = 8;
const EXIT_SILENT: i32 = 9;
const EXIT_GAVE_UP: i32 = 10;
const EXIT_SETUP: i32 = 11;
const R_TOO_FAST: i32 = 20;
const R_ALREADY: i32 = 21;
const R_DDOS: i32 = 22;
const R_BACKEND: i32 = 23;
const R_CONN: i32 = 24;
const R_KICK: i32 = 25;
const R_NO_LOGIN: i32 = 26;
const R_STUCK: i32 = 27;
const R_BACKUP: i32 = 28;
const R_SILENT: i32 = 29;
const R_DROPPED: i32 = 30;
const MAIN_FLAG: i32 = 64;
const MAX_FAILURES: u32 = 8;
const PORTAL_SEARCH: Duration = Duration::from_secs(45);
const WALK_TRIES: u32 = 3;

#[derive(Parser)]
#[clap(author, version)]
struct Cli {
    #[clap(long)]
    run: bool,

    #[clap(long, default_value = config::DEFAULT_PATH)]
    config: PathBuf,

    #[clap(long, hide = true)]
    child: Option<String>,
}

fn banner(lines: &[String]) {
    let bar = "=".repeat(66);
    println!();
    println!("{bar}");
    for l in lines {
        println!(" {l}");
    }
    println!("{bar}");
    println!();
}

fn secs(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 60 { format!("{}m {}s", s / 60, s % 60) } else { format!("{s}s") }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Some(server) = cli.child.clone() {
        let Some(config) = ui::run(&cli.config, true)? else {
            std::process::exit(EXIT_SETUP);
        };
        run_child(server, config);
    }
    let Some(config) = ui::run(&cli.config, cli.run)? else {
        println!("Bye!");
        return Ok(());
    };
    supervise(&config, &cli.config)
}

struct Fails {
    idx: usize,
    fails: u32,
    on_server: u32,
    silent_holds: u32,
}

fn backoff(f: &Fails, base: u64) -> u64 {
    (base * (1u64 << f.fails.min(3))).min(60_000)
}

fn split_server(s: &str) -> (String, u16) {
    if let Some((h, p)) = s.rsplit_once(':')
        && let Ok(port) = p.parse::<u16>()
    {
        return (h.to_owned(), port);
    }
    (s.to_owned(), 25565)
}

fn probe(server: &str) -> Result<(), String> {
    use std::net::{TcpStream, ToSocketAddrs};
    let (host, port) = split_server(server);
    let addrs: Vec<_> = (host.as_str(), port).to_socket_addrs().map_err(|e| e.to_string())?.collect();
    if addrs.is_empty() {
        return Err("no address".into());
    }
    let mut last = String::new();
    for a in addrs {
        match TcpStream::connect_timeout(&a, Duration::from_secs(8)) {
            Ok(_) => return Ok(()),
            Err(e) => last = e.to_string(),
        }
    }
    Err(last)
}

fn retry(f: &mut Fails, servers: &[String], announced: bool, delay_ms: u64, why: &str, rotate: Option<bool>) {
    if announced {
        f.fails = 0;
        f.on_server = 0;
        f.silent_holds = 0;
    } else {
        f.fails += 1;
        f.on_server += 1;
    }
    if f.fails >= MAX_FAILURES {
        banner(&[
            format!("Tried {} times in a row without reaching main. Last problem: {why}.", f.fails),
            "Check your internet connection, then restart the bot later.".into(),
        ]);
        std::process::exit(EXIT_GAVE_UP);
    }
    let mut note = String::new();
    let rotate_now = rotate == Some(true) || (rotate != Some(false) && f.on_server >= 2);
    if rotate_now && servers.len() > 1 {
        f.idx += 1;
        f.on_server = 0;
        note = format!(" via {}", servers[f.idx % servers.len()]);
    }
    println!("{why}. Reconnecting in {}{note}...", secs(Duration::from_millis(delay_ms)));
    std::thread::sleep(Duration::from_millis(delay_ms));
}

fn stop_with(code: i32, lines: &[&str]) -> ! {
    let v: Vec<String> = lines.iter().map(|s| (*s).to_owned()).collect();
    banner(&v);
    std::process::exit(code);
}

fn supervise(cfg: &Config, config_path: &PathBuf) -> ! {
    let servers = cfg.server_list();
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Cannot locate the bot executable: {e}");
            std::process::exit(EXIT_SETUP);
        }
    };
    let journey = Instant::now();
    let mut f = Fails { idx: 0, fails: 0, on_server: 0, silent_holds: 0 };
    loop {
        let server = servers[f.idx % servers.len()].clone();
        if let Err(e) = probe(&server) {
            let delay = if (f.fails as usize) < servers.len() { 3000 } else { backoff(&f, 10_000) };
            retry(&mut f, &servers, false, delay, &format!("Could not reach {server} ({e})"), Some(true));
            continue;
        }
        println!("Connecting to {server} as {}...", cfg.username);
        if !cfg.traps.is_empty() {
            println!("Chat trigger: {}", cfg.trigger_text());
            println!("Press Enter in this window to go to and click trapdoor 1.");
        }
        let started = Instant::now();
        let status = match Command::new(&exe).arg("--child").arg(&server).arg("--config").arg(config_path).status() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Could not start the connection: {e}");
                std::process::exit(1);
            }
        };
        let raw = status.code().unwrap_or(-1);
        if raw < 0 || raw == 130 {
            std::process::exit(0);
        }
        let (code, announced) = if (MAIN_FLAG..MAIN_FLAG + 32).contains(&raw) { (raw - MAIN_FLAG, true) } else { (raw, false) };
        let b = backoff(&f, 10_000);
        match code {
            0 => {
                println!("Took {} overall.", secs(journey.elapsed()));
                std::process::exit(0);
            }
            EXIT_VERIFY => stop_with(
                EXIT_VERIFY,
                &[
                    "6b6t needs you to verify this connection (VPN / anti-bot check).",
                    "",
                    "The verify link is printed above.",
                    "Open it in a browser on the SAME computer or network this bot runs on,",
                    "complete the check, then restart the bot.",
                ],
            ),
            EXIT_COOLDOWN => stop_with(
                EXIT_COOLDOWN,
                &[
                    "The verification link expired and 6b6t asks you to wait (see the message above).",
                    "Wait that long, then restart the bot and verify the new link it shows.",
                    "Restarting sooner makes 6b6t lengthen the wait, up to 120 minutes.",
                ],
            ),
            EXIT_PREMIUM => stop_with(
                EXIT_PREMIUM,
                &["This username belongs to a paid Minecraft account, so it cannot join as a cracked account.", "Change the account name in the settings."],
            ),
            EXIT_LIMIT => stop_with(
                EXIT_LIMIT,
                &["6b6t allows 3 accounts per IP, and your IP already has that many connected.", "Disconnect one of them, then restart the bot."],
            ),
            EXIT_WRONG_PASSWORD => stop_with(
                EXIT_WRONG_PASSWORD,
                &[
                    "6b6t says the password is wrong, or the login was never confirmed.",
                    "Fix the password in the settings, then run the bot again. (Accounts with 2FA cannot use this bot.)",
                ],
            ),
            EXIT_NOT_REGISTERED => stop_with(
                EXIT_NOT_REGISTERED,
                &[
                    "This account is not registered on 6b6t yet.",
                    "Set Registering to auto or always, or register once yourself with a normal Minecraft client.",
                ],
            ),
            EXIT_VERSION => stop_with(EXIT_VERSION, &["6b6t rejected Minecraft 1.21.11. The server has probably updated, and this bot needs updating too."]),
            EXIT_SETUP => stop_with(EXIT_SETUP, &["Setup problem: the saved config is incomplete."]),
            R_TOO_FAST => retry(&mut f, &servers, announced, b, "Logging in too fast for 6b6t", Some(false)),
            R_ALREADY => retry(&mut f, &servers, announced, 60_000, "This name is still online from an earlier session", Some(false)),
            R_DDOS => retry(&mut f, &servers, announced, 30_000, "Blocked by this address's DDoS protection", Some(true)),
            R_BACKEND => retry(&mut f, &servers, announced, 60_000, "6b6t could not reach its own server", None),
            R_CONN => retry(&mut f, &servers, announced, 10_000, "Connection error on 6b6t", None),
            R_KICK => retry(&mut f, &servers, announced, b, "Kicked", None),
            R_NO_LOGIN => {
                let held = started.elapsed();
                if held >= Duration::from_secs(25) {
                    f.silent_holds += 1;
                    if f.silent_holds >= 2 {
                        stop_with(
                            EXIT_SILENT,
                            &[
                                "6b6t held the connection without letting the bot in, twice in a row.",
                                "This usually means your IP is in a verification cooldown.",
                                "Wait 10-30 minutes, then restart the bot.",
                            ],
                        );
                    }
                    let why = format!("6b6t held the connection for {} without letting the bot in", secs(held));
                    retry(&mut f, &servers, announced, 60_000, &why, Some(true));
                } else {
                    retry(&mut f, &servers, announced, b, "The connection closed before login", None);
                }
            }
            R_STUCK => retry(&mut f, &servers, announced, 10_000, "Stuck before main", None),
            R_BACKUP => retry(&mut f, &servers, announced, 10_000, "Reconnecting from the backup server", Some(false)),
            R_SILENT => retry(&mut f, &servers, announced, b, "Nothing received from 6b6t; the connection is dead", None),
            R_DROPPED => retry(&mut f, &servers, announced, b, "Disconnected", None),
            other => retry(&mut f, &servers, announced, b, &format!("The connection process exited unexpectedly (code {other})"), None),
        }
    }
}

static CONFIG: OnceCell<Config> = OnceCell::new();
static SERVER: OnceCell<String> = OnceCell::new();
static START: OnceCell<Instant> = OnceCell::new();
static PLAY_LOGINS: AtomicU32 = AtomicU32::new(0);
static LAST_PACKET_MS: AtomicU64 = AtomicU64::new(0);
static BACKUP_SINCE_MS: AtomicU64 = AtomicU64::new(0);
static ANNOUNCED: AtomicBool = AtomicBool::new(false);
static TRAP_REQUEST: AtomicUsize = AtomicUsize::new(0);
static PENDING_TRAP: Mutex<Option<(usize, Instant)>> = Mutex::new(None);
static LAST_TRIGGER_MS: AtomicU64 = AtomicU64::new(0);

fn cfg() -> &'static Config {
    CONFIG.get().expect("config not initialized")
}

fn now_ms() -> u64 {
    START.get().map(|s| s.elapsed().as_millis() as u64).unwrap_or(0)
}

fn quit(code: i32) -> ! {
    std::thread::sleep(Duration::from_millis(150));
    let flagged = if code != 0 && ANNOUNCED.load(Ordering::Relaxed) { code + MAIN_FLAG } else { code };
    std::process::exit(flagged);
}

fn redact(s: &str) -> String {
    let p = &cfg().password;
    if p.chars().count() >= 3 { s.replace(p.as_str(), "********") } else { s.to_owned() }
}

fn clean_text(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '§' {
            it.next();
            continue;
        }
        let u = c as u32;
        if (0xE000..=0xF8FF).contains(&u) || u == 0xFFFD || (0xF0000..=0x10FFFF).contains(&u) {
            continue;
        }
        out.push(c);
    }
    out
}

fn short(id: &str) -> &str {
    id.rsplit(':').next().unwrap_or(id)
}

fn block_id(state: BlockState) -> &'static str {
    Box::<dyn BlockTrait>::from(state).id()
}

fn find_link(h: &str) -> Option<String> {
    let mut from = 0;
    while let Some(rel) = h[from..].find("http") {
        let i = from + rel;
        let tail = &h[i..];
        if tail.starts_with("https://") || tail.starts_with("http://") {
            let end = tail.find(|c: char| c.is_whitespace() || "\"'\\<>".contains(c)).unwrap_or(tail.len());
            let cand = tail[..end].trim_end_matches(['.', ',', ';', ')']);
            if cand.to_lowercase().contains("verif") {
                return Some(cand.to_owned());
            }
            from = i + end.max(4);
        } else {
            from = i + 4;
        }
    }
    None
}

enum Kick {
    Vpn(Option<String>),
    VerifyExpired,
    Premium,
    Limit,
    WrongPassword,
    Version,
    Backend,
    Ddos,
    TooFast,
    AlreadyOnline,
    ConnError,
    Other,
}

fn classify_kick(text: &str, raw: &str) -> Kick {
    let l = format!("{text}\n{raw}").to_lowercase();
    let link = find_link(text).or_else(|| find_link(raw));
    if link.is_some()
        || l.contains("anti bot protection")
        || l.contains("prove you're a human")
        || l.contains("prove you’re a human")
        || l.contains("flagged for further verification")
        || l.contains("vpn/proxy/unregistered")
    {
        return Kick::Vpn(link);
    }
    if l.contains("verification link expired") {
        return Kick::VerifyExpired;
    }
    if l.contains("own the mojang account") || l.contains("choose another username") {
        return Kick::Premium;
    }
    if l.contains("connected account limit") {
        return Kick::Limit;
    }
    if l.contains("wrong password") || l.contains("incorrect password") || l.contains("invalid password") {
        return Kick::WrongPassword;
    }
    if l.contains("unsupported minecraft version") || l.contains("outdated client") || l.contains("outdated server") {
        return Kick::Version;
    }
    if l.contains("unable to connect to ") || l.contains("internal server connection error") {
        return Kick::Backend;
    }
    if l.contains("ddos protection") || l.contains("connection blocked") {
        return Kick::Ddos;
    }
    if l.contains("too fast") {
        return Kick::TooFast;
    }
    if l.contains("already online") || l.contains("already connected") || l.contains("logged in from another location") {
        return Kick::AlreadyOnline;
    }
    if l.contains("internal error occurred") || l.contains("encountered a problem") {
        return Kick::ConnError;
    }
    Kick::Other
}

fn is_worker_line(text: &str) -> bool {
    let l = text.to_lowercase();
    let key = "now playing on worker-";
    let Some(i) = l.find(key) else { return false };
    let pre = l[..i].trim_end();
    let Some(head) = pre.strip_suffix("you're").or_else(|| pre.strip_suffix("you’re")) else {
        return false;
    };
    if head.chars().any(|c| c.is_alphanumeric() || c == '_' || c == '»') {
        return false;
    }
    let rest = &l[i + key.len()..];
    let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 {
        return false;
    }
    match rest[digits..].strip_prefix(", hosted on node-") {
        Some(n) => n.chars().next().is_some_and(|c| c.is_ascii_digit()),
        None => false,
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Connecting,
    Auth,
    Lobby,
    Backup,
    Main,
    Unknown,
}

fn describe(k: Kind) -> &'static str {
    match k {
        Kind::Connecting => "connecting",
        Kind::Auth => "login server",
        Kind::Lobby => "lobby",
        Kind::Backup => "backup server (queue)",
        Kind::Main => "MAIN server",
        Kind::Unknown => "an unrecognised server",
    }
}

#[derive(Clone)]
enum Walk {
    Idle,
    Search { since: Instant },
    Go { goal: (f64, f64), since: Instant, start_y: f64 },
    Retry { at: Instant },
}

struct Conn {
    play_logins: u32,
    kind: Kind,
    kind_since: Instant,
    login_at: Instant,
    mode: String,
    slots: i64,
    authed: bool,
    login_sends: u32,
    last_login_send: Option<Instant>,
    register_sent: bool,
    auth_sent_at: Option<Instant>,
    fallback_walk_done: bool,
    walk_at: Option<Instant>,
    walk: Walk,
    walk_label: &'static str,
    walk_tries: HashMap<&'static str, u32>,
    walking: bool,
    scan: u32,
    on_main: bool,
    main_since: Instant,
    main_announced: bool,
    worker_login: u32,
    worker_line: String,
    last_line: String,
}

impl Conn {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            play_logins: 0,
            kind: Kind::Connecting,
            kind_since: now,
            login_at: now,
            mode: String::new(),
            slots: 0,
            authed: false,
            login_sends: 0,
            last_login_send: None,
            register_sent: false,
            auth_sent_at: None,
            fallback_walk_done: false,
            walk_at: None,
            walk: Walk::Idle,
            walk_label: "portal",
            walk_tries: HashMap::new(),
            walking: false,
            scan: 0,
            on_main: false,
            main_since: now,
            main_announced: false,
            worker_login: 0,
            worker_line: String::new(),
            last_line: String::new(),
        }
    }
}

#[derive(Clone, Component)]
struct BotState {
    conn: Arc<Mutex<Conn>>,
    combat: Arc<Mutex<combat::Combat>>,
}

impl Default for BotState {
    fn default() -> Self {
        Self { conn: Arc::new(Mutex::new(Conn::new())), combat: Arc::new(Mutex::new(combat::Combat::default())) }
    }
}

fn run_child(server: String, config: Config) -> ! {
    let _ = CONFIG.set(config);
    let _ = SERVER.set(server);
    let _ = START.set(Instant::now());
    spawn_watchdog();
    spawn_key_listener();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
    rt.block_on(async_main());
    if PLAY_LOGINS.load(Ordering::Relaxed) == 0 { quit(R_NO_LOGIN) } else { quit(R_DROPPED) }
}

async fn async_main() {
    let account = Account::offline(&cfg().username);
    let server = SERVER.get().expect("server").clone();
    let _ = ClientBuilder::new().set_handler(handle).start(account, server.as_str()).await;
}

fn spawn_watchdog() {
    std::thread::spawn(|| {
        loop {
            std::thread::sleep(Duration::from_secs(2));
            let now = now_ms();
            if PLAY_LOGINS.load(Ordering::Relaxed) == 0 {
                if now > 75_000 {
                    warn!("6b6t has not let the bot in after 75s.");
                    quit(R_NO_LOGIN);
                }
                continue;
            }
            let idle = now.saturating_sub(LAST_PACKET_MS.load(Ordering::Relaxed));
            if idle > 30_000 {
                warn!("Nothing received from 6b6t for {}s.", idle / 1000);
                quit(R_SILENT);
            }
            let b = BACKUP_SINCE_MS.load(Ordering::Relaxed);
            if b != 0 && now.saturating_sub(b) > 30 * 60 * 1000 {
                warn!("Still on the backup server after 30 minutes; reconnecting.");
                quit(R_BACKUP);
            }
        }
    });
}

fn spawn_key_listener() {
    std::thread::spawn(|| {
        let stdin = std::io::stdin();
        let mut line = String::new();
        loop {
            line.clear();
            match stdin.read_line(&mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => TRAP_REQUEST.store(1, Ordering::Relaxed),
            }
        }
    });
}

const CLICK_REACH: f64 = 4.2;
const TRAP_WALK_TIMEOUT: Duration = Duration::from_secs(90);

fn trap_center(pos: [i32; 3]) -> Vec3 {
    Vec3::new(pos[0] as f64 + 0.5, pos[1] as f64 + 0.5, pos[2] as f64 + 0.5)
}

fn trap_distance(bot: &Client, center: Vec3) -> f64 {
    let eye = bot.eye_position();
    ((eye.x - center.x).powi(2) + (eye.y - center.y).powi(2) + (eye.z - center.z).powi(2)).sqrt()
}

fn click_trapdoor(bot: &Client, idx: usize) {
    let Some(trap) = cfg().traps.get(idx) else {
        warn!("Trapdoor {} is not set. Add it in the menu under 'Trapdoors'.", idx + 1);
        return;
    };
    let [x, y, z] = trap.pos;
    bot.look_at(trap_center(trap.pos));
    bot.block_interact(BlockPos::new(x, y, z));
    info!("Clicked trapdoor {}.", idx + 1);
}

fn trap_tick(bot: &Client, on_main: bool) {
    let request = TRAP_REQUEST.swap(0, Ordering::Relaxed);
    let mut pending = PENDING_TRAP.lock();
    if request > 0 {
        let idx = request - 1;
        let Some(trap) = cfg().traps.get(idx) else {
            warn!("Trapdoor {request} is not set. Add it in the menu under 'Trapdoors'.");
            return;
        };
        if !on_main || trap_distance(bot, trap_center(trap.pos)) <= CLICK_REACH {
            *pending = None;
            click_trapdoor(bot, idx);
            return;
        }
        info!("Walking to trapdoor {request}...");
        bot.start_goto(RadiusGoal::new(trap_center(trap.pos), 3.0));
        *pending = Some((idx, Instant::now()));
        return;
    }
    let Some((idx, since)) = *pending else {
        return;
    };
    let Some(trap) = cfg().traps.get(idx) else {
        *pending = None;
        return;
    };
    if trap_distance(bot, trap_center(trap.pos)) <= CLICK_REACH {
        *pending = None;
        bot.stop_pathfinding();
        click_trapdoor(bot, idx);
    } else if since.elapsed() > TRAP_WALK_TIMEOUT || bot.is_goto_target_reached() && since.elapsed() > Duration::from_secs(2) {
        *pending = None;
        bot.stop_pathfinding();
        warn!("Could not get within reach of trapdoor {}.", idx + 1);
    }
}

fn has_phrase(hay: &str, needle: &str) -> bool {
    let mut from = 0;
    while let Some(rel) = hay[from..].find(needle) {
        let start = from + rel;
        let end = start + needle.len();
        let before_ok = hay[..start].chars().next_back().is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
        let after_ok = hay[end..].chars().next().is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
        if before_ok && after_ok {
            return true;
        }
        from = end;
    }
    false
}

fn split_chat(text: &str) -> Option<(String, String)> {
    let (idx, sep) = text.char_indices().skip(1).find(|(_, c)| "\u{bb}\u{203a}:".contains(*c))?;
    let header = &text[..idx];
    let message = &text[idx + sep.len_utf8()..];
    let mut plain = String::new();
    let mut in_bracket = false;
    for c in header.chars() {
        match c {
            '[' => in_bracket = true,
            ']' => {
                in_bracket = false;
                plain.push(' ');
            }
            _ if !in_bracket => plain.push(c),
            _ => {}
        }
    }
    let sender = plain.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).filter(|t| !t.is_empty()).next_back()?;
    Some((sender.to_owned(), message.to_owned()))
}

fn chat_triggers(text: &str) -> Option<usize> {
    if cfg().trigger_users.is_empty() {
        return None;
    }
    let (sender, message) = split_chat(text)?;
    if !cfg().trigger_users.iter().any(|u| u.trim().eq_ignore_ascii_case(&sender)) {
        return None;
    }
    let message = message.to_lowercase();
    cfg().traps.iter().position(|t| {
        let word = t.word.trim().to_lowercase();
        !word.is_empty() && has_phrase(&message, &word)
    })
}

fn send_command(bot: &Client, command: String) {
    info!("Sending command: /{}", command.split(' ').next().unwrap_or(""));
    bot.chat(format!("/{command}"));
}

async fn handle(bot: Client, event: Event, state: BotState) -> Result<()> {
    match event {
        Event::Packet(packet) => {
            LAST_PACKET_MS.store(now_ms(), Ordering::Relaxed);
            if let ClientboundGamePacket::Login(p) = &*packet {
                let mode = format!("{:?}", p.common.game_type).to_lowercase();
                on_play_login(&bot, &state, mode, p.max_players as i64);
            }
        }
        Event::Chat(packet) => on_chat(&bot, &state, packet.message().to_string()),
        Event::Tick => {
            if state.conn.lock().on_main {
                combat::tick(&bot, &state.combat);
            }
            on_tick(&bot, &state)
        }
        Event::Disconnect(reason) => {
            let text = reason.as_ref().map(|r| clean_text(&r.to_string())).unwrap_or_default();
            let raw = reason.as_ref().map(|r| format!("{r:?}")).unwrap_or_default();
            on_disconnect(&state, text, raw);
        }
        _ => {}
    }
    Ok(())
}

fn classify_server(c: &Conn) -> Kind {
    if c.play_logins == 1 {
        Kind::Auth
    } else if c.slots == 1 {
        Kind::Backup
    } else if c.mode == "survival" && c.slots == 1000 {
        Kind::Main
    } else if c.mode == "adventure" && c.slots == 10000 {
        Kind::Lobby
    } else {
        Kind::Unknown
    }
}

fn enter_main(c: &mut Conn) {
    c.on_main = true;
    c.main_announced = false;
    c.kind = Kind::Main;
    c.main_since = Instant::now();
}

fn on_play_login(bot: &Client, state: &BotState, mode: String, slots: i64) {
    bot.walk(WalkDirection::None);
    let mut c = state.conn.lock();
    c.play_logins += 1;
    PLAY_LOGINS.store(c.play_logins, Ordering::Relaxed);
    BACKUP_SINCE_MS.store(0, Ordering::Relaxed);
    let now = Instant::now();
    c.walk = Walk::Idle;
    c.walk_tries.clear();
    c.walking = false;
    c.walk_at = None;
    c.login_at = now;
    c.mode = mode;
    c.slots = slots;
    let kind = classify_server(&c);
    let was_main = c.on_main;
    c.kind = kind;
    c.kind_since = now;
    info!("Now on: {} ({}, {} slots)", describe(kind), c.mode, c.slots);
    if kind == Kind::Main {
        if was_main && c.main_announced {
            info!("Still on main (moved to another main worker).");
            c.main_since = now;
            return;
        }
        enter_main(&mut c);
        return;
    }
    if was_main {
        c.on_main = false;
        if c.main_announced {
            warn!("Left main -> {}. Waiting to get back...", describe(kind));
        }
        c.main_announced = false;
        ANNOUNCED.store(false, Ordering::Relaxed);
    }
    match kind {
        Kind::Auth => info!("Waiting for the login prompt..."),
        Kind::Lobby => start_walk(&mut c, "lobby portal"),
        Kind::Backup => {
            BACKUP_SINCE_MS.store(now_ms() + 1, Ordering::Relaxed);
            info!("Waiting on the backup server. 6b6t moves you to main by itself when a spot is free, usually within a minute or two.");
        }
        Kind::Unknown => {
            if c.mode == "adventure" {
                start_walk(&mut c, "portal");
            } else {
                warn!("Not sure which server this is; waiting to be moved.");
            }
        }
        _ => {}
    }
}

fn announce(c: &mut Conn) {
    c.main_announced = true;
    ANNOUNCED.store(true, Ordering::Relaxed);
    let with_line = c.worker_login == c.play_logins;
    let mut lines = vec![format!("REACHED MAIN - {} is on 6b6t's main server.", cfg().username)];
    if with_line {
        lines.push(c.worker_line.clone());
    }
    let mut ev = vec![format!("{} mode", c.mode), format!("{} player slots", c.slots)];
    if with_line {
        ev.push("\"now playing on\" message".to_owned());
    }
    lines.push(format!("Confirmed by: {}.", ev.join(", ")));
    if !cfg().stay {
        lines.push("Disconnecting now (stay connected is off).".to_owned());
    }
    banner(&lines);
    if !cfg().stay {
        quit(0);
    }
}

fn on_chat(bot: &Client, state: &BotState, raw: String) {
    let text = clean_text(&raw).trim_end().to_owned();
    if text.trim().is_empty() {
        return;
    }
    if PLAY_LOGINS.load(Ordering::Relaxed) > 0
        && let Some(idx) = chat_triggers(&text)
    {
        let now = now_ms();
        if now.saturating_sub(LAST_TRIGGER_MS.load(Ordering::Relaxed)) > 1500 {
            LAST_TRIGGER_MS.store(now, Ordering::Relaxed);
            info!("Chat trigger matched; going to trapdoor {}.", idx + 1);
            TRAP_REQUEST.store(idx + 1, Ordering::Relaxed);
        }
    }
    let mut c = state.conn.lock();
    if c.last_line == text {
        return;
    }
    c.last_line = text.clone();
    let worker = is_worker_line(&text);
    if worker || (!c.on_main && !c.main_announced) {
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            info!("[6b6t] {}", redact(line.trim()));
        }
    }
    let lower = text.to_lowercase();
    if c.kind == Kind::Auth && !c.authed {
        if lower.contains("please register to 6b6t") || lower.contains("/register <password>") {
            if cfg().no_register {
                error!("Account is not registered and registering is set to never.");
                quit(EXIT_NOT_REGISTERED);
            }
            if !c.register_sent {
                c.register_sent = true;
                c.auth_sent_at.get_or_insert_with(Instant::now);
                let pw = &cfg().password;
                send_command(bot, format!("register {pw} {pw}"));
                info!("Account not registered yet; sent /register.");
            }
        }
        if lower.contains("wrong password") || lower.contains("incorrect password") || lower.contains("invalid password") {
            error!("6b6t says the password is wrong.");
            quit(EXIT_WRONG_PASSWORD);
        }
        if lower.contains("please login with the command") || lower.contains("/login <password>") {
            if c.login_sends == 0 {
                send_login(bot, &mut c, "prompted");
            }
        }
        if lower.contains("you are now logged in")
            || lower.contains("successfully logged in")
            || lower.contains("logged in successfully")
            || lower.contains("successfully registered")
            || lower.contains("registered successfully")
        {
            on_authed(&mut c);
        }
    }
    if worker {
        c.worker_line = text.trim_start_matches(|ch: char| !ch.is_alphabetic()).trim().to_owned();
        c.worker_login = c.play_logins;
        if !c.on_main && c.play_logins >= 2 {
            info!("6b6t says this is main.");
            bot.walk(WalkDirection::None);
            c.walk = Walk::Idle;
            c.walking = false;
            enter_main(&mut c);
        }
    }
}

fn send_login(bot: &Client, c: &mut Conn, why: &str) {
    if c.authed || c.login_sends >= 2 || c.kind != Kind::Auth {
        return;
    }
    c.login_sends += 1;
    c.last_login_send = Some(Instant::now());
    c.auth_sent_at.get_or_insert_with(Instant::now);
    send_command(bot, format!("login {}", cfg().password));
    info!("Sent /login ({why}).");
}

fn on_authed(c: &mut Conn) {
    if c.authed {
        return;
    }
    c.authed = true;
    info!("Logged in.");
    c.walk_at = Some(Instant::now() + Duration::from_secs_f64(cfg().walk_delay.max(0.0)));
}

fn start_walk(c: &mut Conn, label: &'static str) {
    *c.walk_tries.entry(label).or_insert(0) += 1;
    c.walk_label = label;
    c.walk = Walk::Search { since: Instant::now() };
    c.walking = false;
    c.scan = 0;
}

fn walk_failed(c: &mut Conn, why: &str) {
    c.walk = Walk::Idle;
    c.walking = false;
    if c.kind == Kind::Auth && !c.authed {
        error!("The login was never confirmed, and the login server would not let the bot through.");
        quit(EXIT_WRONG_PASSWORD);
    }
    let label = c.walk_label;
    let tries = *c.walk_tries.get(label).unwrap_or(&0);
    if tries < WALK_TRIES {
        warn!("{label}: {why}. Trying again in 5s...");
        c.walk = Walk::Retry { at: Instant::now() + Duration::from_secs(5) };
        return;
    }
    warn!("{label}: {why}, {tries} times. Reconnecting...");
    quit(R_STUCK);
}

fn ground_ready(bot: &Client) -> bool {
    let p = bot.component::<Position>();
    let pos = BlockPos::new(p.x.floor() as i32, (p.y - 1.0).floor() as i32, p.z.floor() as i32);
    let world = bot.world();
    let world = world.read();
    match world.get_block_state(pos) {
        Some(s) => !matches!(short(block_id(s)), "air" | "cave_air" | "void_air"),
        None => false,
    }
}

fn find_portal(bot: &Client) -> Option<(i32, i32, i32)> {
    let p = bot.component::<Position>();
    let (cx, cy, cz) = (p.x.floor() as i32, p.y.floor() as i32, p.z.floor() as i32);
    let world = bot.world();
    let world = world.read();
    let mut best: Option<(f64, (i32, i32, i32))> = None;
    for dy in -6..=6 {
        for dx in -24..=24 {
            for dz in -24..=24 {
                let pos = (cx + dx, cy + dy, cz + dz);
                let Some(state) = world.get_block_state(BlockPos::new(pos.0, pos.1, pos.2)) else {
                    continue;
                };
                if short(block_id(state)) != "nether_portal" {
                    continue;
                }
                let d = ((dx * dx + dy * dy + dz * dz) as f64).sqrt();
                if best.is_none_or(|b| d < b.0) {
                    best = Some((d, pos));
                }
            }
        }
    }
    best.map(|b| b.1)
}

fn on_tick(bot: &Client, state: &BotState) {
    let mut c = state.conn.lock();
    if c.play_logins == 0 {
        return;
    }
    let now = Instant::now();
    trap_tick(bot, c.on_main);
    match c.kind {
        Kind::Auth => {
            if !c.authed {
                let since = now.duration_since(c.kind_since);
                if cfg().force_register && !cfg().no_register && !c.register_sent && since >= Duration::from_secs(2) {
                    c.register_sent = true;
                    c.auth_sent_at.get_or_insert(now);
                    let pw = &cfg().password;
                    send_command(bot, format!("register {pw} {pw}"));
                }
                if c.login_sends == 0 && !c.register_sent && since >= Duration::from_millis(2500) {
                    send_login(bot, &mut c, "no prompt seen yet");
                }
                if c.login_sends == 1 && c.last_login_send.is_some_and(|t| now.duration_since(t) >= Duration::from_secs(8)) {
                    send_login(bot, &mut c, "no answer yet, sending again");
                }
                if !c.fallback_walk_done && c.auth_sent_at.is_some_and(|t| now.duration_since(t) >= Duration::from_secs(20)) {
                    c.fallback_walk_done = true;
                    warn!("The login was not confirmed after 20s; trying the portal anyway...");
                    start_walk(&mut c, "login portal");
                }
            } else if c.walk_at.is_some_and(|t| now >= t) {
                c.walk_at = None;
                start_walk(&mut c, "login portal");
            }
        }
        Kind::Main => {
            if c.on_main && !c.main_announced && now.duration_since(c.main_since) >= Duration::from_secs(3) {
                announce(&mut c);
            }
        }
        _ => {}
    }
    walk_tick(bot, &mut c, now);
}

fn walk_tick(bot: &Client, c: &mut Conn, now: Instant) {
    match c.walk.clone() {
        Walk::Idle => {}
        Walk::Retry { at } => {
            if now >= at {
                let label = c.walk_label;
                start_walk(c, label);
            }
        }
        Walk::Search { since } => {
            if c.kind == Kind::Backup {
                return;
            }
            c.scan += 1;
            if c.scan % 10 == 0 && now.duration_since(c.login_at) >= Duration::from_secs(1) && ground_ready(bot) {
                if let Some(b) = find_portal(bot) {
                    let p = bot.component::<Position>();
                    let goal = (b.0 as f64 + 0.5, b.2 as f64 + 0.5);
                    let dist = ((goal.0 - p.x).powi(2) + (goal.1 - p.z).powi(2)).sqrt();
                    info!("Walking into the {} ({} blocks)...", c.walk_label, dist.round());
                    c.walk = Walk::Go { goal, since: now, start_y: p.y };
                    return;
                }
            }
            if now.duration_since(since) >= PORTAL_SEARCH {
                walk_failed(c, "no portal in sight");
            }
        }
        Walk::Go { goal, since, start_y } => {
            let p = bot.component::<Position>();
            if p.y < start_y - 3.0 {
                bot.walk(WalkDirection::None);
                walk_failed(c, "fell off the platform");
                return;
            }
            let d = ((goal.0 - p.x).powi(2) + (goal.1 - p.z).powi(2)).sqrt();
            bot.look_at(Vec3::new(goal.0, p.y + 1.62, goal.1));
            if d > 0.25 {
                if !c.walking {
                    bot.walk(WalkDirection::Forward);
                    c.walking = true;
                }
            } else if c.walking {
                bot.walk(WalkDirection::None);
                c.walking = false;
            }
            if now.duration_since(since) >= Duration::from_secs_f64(cfg().walk_timeout.max(1.0)) {
                bot.walk(WalkDirection::None);
                walk_failed(c, &format!("no server switch after {}s", cfg().walk_timeout));
            }
        }
    }
}

fn on_disconnect(state: &BotState, text: String, raw: String) {
    let logins = state.conn.lock().play_logins;
    if text.is_empty() && raw.is_empty() {
        warn!("Disconnected.");
        quit(if logins == 0 { R_NO_LOGIN } else { R_DROPPED });
    }
    error!("Kicked by the server:");
    for line in text.lines() {
        error!("    {}", redact(line));
    }
    let code = match classify_kick(&text, &raw) {
        Kick::Vpn(link) => {
            println!("  Verify here:  {}", link.unwrap_or_else(|| "(see the kick message above)".to_owned()));
            EXIT_VERIFY
        }
        Kick::VerifyExpired => EXIT_COOLDOWN,
        Kick::Premium => EXIT_PREMIUM,
        Kick::Limit => EXIT_LIMIT,
        Kick::WrongPassword => EXIT_WRONG_PASSWORD,
        Kick::Version => EXIT_VERSION,
        Kick::Backend => R_BACKEND,
        Kick::Ddos => R_DDOS,
        Kick::TooFast => R_TOO_FAST,
        Kick::AlreadyOnline => R_ALREADY,
        Kick::ConnError => R_CONN,
        Kick::Other => R_KICK,
    };
    quit(code);
}
