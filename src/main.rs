<<<<<<< HEAD
#![feature(error_generic_member_access)]
#![feature(map_try_insert)]

mod blockpos_string;
pub mod commands;
pub mod devnet;
pub mod entity_util;
pub mod module;
pub mod nbs_format;
pub mod openauthmod;
pub mod task;
pub mod util;

#[macro_use]
extern crate tracing;

use crate::blockpos_string::BlockPosString;
use crate::module::Module;
use crate::module::auto_tpa::AutoTpaModule;
use crate::module::autoeat::AutoEatModule;
use crate::module::chat::ChatModule;
use crate::module::devnet_handler::DevNetIntegrationModule;
use crate::module::disc_jockey::DiscJockeyModule;
use crate::module::emergency_quit::EmergencyQuitModule;
use crate::module::killaura::KillauraModule;
use crate::module::legacy_stasis::LegacyStasisModule;
use crate::module::look_at_players::LookAtPlayersModule;
use crate::module::periodic_swing::PeriodicSwingModule;
use crate::module::server_tps::ServerTpsModule;
use crate::module::soundness::SoundnessModule;
use crate::module::stasis::StasisModule;
use crate::module::visual_range::VisualRangeModule;
use crate::module::webhook::WebhookModule;
use crate::openauthmod::OpenAuthModPlugin;
use crate::task::group::TaskGroup;
use crate::task::{Task, TaskOutcome};
use anyhow::{Context, Result};
use azalea::app::PluginGroup;
use azalea::entity::Position;
use azalea::swarm::DefaultSwarmPlugins;
use azalea::task_pool::{TaskPoolOptions, TaskPoolPlugin, TaskPoolThreadAssignmentPolicy};
use azalea::{
    DefaultBotPlugins, DefaultPlugins, JoinOpts,
    auth::AuthResult,
    prelude::*,
    swarm::{Swarm, SwarmEvent},
};
use bevy_log::LogPlugin;
use clap::Parser;
use logroller::{Compression, LogRollerBuilder, Rotation, RotationAge, TimeZone};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::{
    collections::VecDeque,
    io::IsTerminal,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use strip_ansi_escapes::Writer as StripAnsiWriter;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::prelude::*;

#[allow(dead_code)]
pub const EXITCODE_OTHER: i32 = 1; // Due to errors returned and propagated to main result
pub const EXITCODE_CONFLICTING_CLI_OPTS: i32 = 2;
pub const EXITCODE_AUTH_FAILED: i32 = 3;
pub const EXITCODE_NO_ACCESS_TOKEN: i32 = 4;
pub const EXITCODE_PATHFINDER_DEADLOCKED: i32 = 5;
pub const EXITCODE_USER_REQUESTED_STOP: i32 = 20; // Using an error code to prevent automatic relaunching in some configurations or scripts
pub const EXITCODE_LOW_HEALTH_OR_TOTEM_POP: i32 = 69;

/// A simple stasis bot, using azalea!
#[derive(Parser)]
#[clap(author, version)]
struct Opts {
    /// What server (and port) to connect to
    server_address: String,

    /// Player names who are considered more trustworthy for certain commands
    #[clap(short, long)]
    admin: Vec<String>,

    /// Automatically logout/quit, once getting to this HP or lower (or popping a totem)
    #[clap(short = 'H', long, alias = "autolog-hp")]
    emergency_quit: Option<f32>,

    /// Be more paranoid about quitting (old behavior which often prevents rejoining and regenerating).
    #[clap(long)]
    emergency_quit_strict: bool,

    /// Workaround for crashes: Forbid the bot from sending any messages to players.
    #[clap(short = 'q', long)]
    quiet: bool,

    /// Make the stasis bot, not do any stasis-related duties. So you can abuse him easier as an afk bot.
    #[clap(short = 'S', long)]
    no_stasis: bool,

    /// Specify a simple, single logfile to log everything into as well
    #[clap(short = 'l', long)]
    log_file: Option<PathBuf>,

    /// Specify a directory to put automatically rotated and compressed logfiles into
    #[clap(long)]
    log_directory: Option<PathBuf>,

    /// Use a different filter in log files (instead of RUST_LOG, but same syntax)
    #[clap(long)]
    log_filter: Option<String>,

    /// Disable color. Can fix some issues of still persistent escape codes in log files.
    #[clap(short = 'C', long)]
    no_color: bool,

    /// Allow chat messages to be signed.
    #[clap(short = 's', long)]
    sign_chat: bool,

    /// Use an offline account with specified user name.
    #[clap(long)]
    offline_username: Option<String>,

    /// Forbid pathfinding to mine blocks to reach its goal
    #[clap(short = 'M', long)]
    no_mining: bool,

    /// Enable looking at the closest player which is no more than N blocks away.
    #[clap(short = 'L', long)]
    look_at_players: Option<u32>,

    /// Enable a command, that allows admins to get the position of the bot. Might be dangerous!
    #[clap(long)]
    enable_pos_command: bool,

    /// Enables Automatic Eating food items in hotbar, when appropriate
    #[clap(long)]
    auto_eat: bool,

    /// File, used to store authentication information in. Ignored if --offline-username is used.
    #[clap(long, default_value = "login-secrets.json")]
    auth_file: PathBuf,

    /// Which key to use within the auth-file
    #[clap(long, default_value = "default")]
    auth_file_key: String,

    /// Only print access token, then quit. Fancy account refresher for something else.
    #[clap(long)]
    just_print_access_token: bool,

    /// 2b Anti AFK
    #[clap(long)]
    periodic_swing: bool,

    /*
    /// How many tokio worker threads to use. 0 = Automatic
    #[clap(short = 't', long, default_value = "0")]
    worker_threads: usize,
    */
    /// How many async compute tasks to create (might be pathfinding). 0 = Automatic
    #[clap(short = 'c', long, default_value = "0")]
    compute_threads: usize,

    /// How many tasks to create for Bevys AsyncComputeTaskPool (might be pathfinding). 0 = Automatic
    #[clap(short = 'A', long, default_value = "0")]
    async_compute_threads: usize,

    /// How many tasks to create for IO. 0 = Automatic
    #[clap(short = 'i', long, default_value = "0")]
    io_threads: usize,

    /// Base URL to devnet API
    #[clap(long)]
    devnet_url: Option<String>,

    /// Token to access devnet API (as destination)
    #[clap(long)]
    devnet_access_token: Option<String>,

    /// Don't re-open trapdoor after teleport. Use if teleports fail or you're paranoid.
    #[clap(long)]
    no_trapdoor_reopen: bool,

    /// Allow users to only set a certain amount of pearls
    #[clap(long)]
    max_pearls: Option<u32>,

    #[clap(long, default_value = "/msg")]
    message_command: String,

    #[clap(long)]
    reply_command: Option<String>,

    /// Attempt to bypass anti-spam measures
    #[clap(long)]
    anti_anti_spam: bool,

    /// If a pearl spawns below this position, it will get ignored (X,Y,Z)
    #[clap(long)]
    pearls_min_pos: Option<BlockPosString>,

    /// If a pearl spawns above this position, it will get ignored (X,Y,Z)
    #[clap(long)]
    pearls_max_pos: Option<BlockPosString>,

    /// Path to folder containing *.nbs song files (enables DiscJockey)
    #[clap(long)]
    songs: Option<PathBuf>,

    /// BlockPos of the noteblocks are to play at (for !dj)
    #[clap(long)]
    dj_pos: Option<BlockPosString>,

    /// Specifiy to only allow any dj commands from admins (otherwise anyone in render distance can use it)
    #[clap(long)]
    dj_admin_only: bool,

    /// Automatically resume DJ playback when Idle for X seconds (no tasks), goes back if --dj-pos set
    #[clap(long)]
    dj_resume_after_idle_for: Option<usize>,

    #[clap(long)]
    webhook_url: Option<String>,

    #[clap(long)]
    webhook_alert_role_id: Option<u64>,

    #[clap(long)]
    /// Seems that azalea currently has issues getting the correct damage types.
    /// Use this as a workaround on 1.21.4
    use_hardcoded_damage_types: bool,

    #[clap(long)]
    /// Automatically attack dangerous entities
    killaura: bool,

    #[clap(long)]
    /// Allows defining trusted players who can request a /tpahere via !tpahere.
    /// Additionally the bot will try to auto-accept tpa requests from such users.
    /// Manage trusted players with "!tpatrust add/rm NAME" (admins only)
    auto_tpa: bool,

    #[clap(long, default_value = "/tpaccept {NAME}")]
    /// Command used by AutoTpa for accepting a teleport.
    /// %NAME% will be replaced by the sender if present.
    tpa_accept_command: String,

    #[clap(long, default_value = "/tpdeny")]
    /// Command used by AutoTpa for denying a teleport.
    /// {NAME} will be replaced by the sender if present.
    tpa_deny_command: String,

    #[clap(long, default_value = "255")]
    /// Max length for any commands (will get truncated if too long)
    max_command_length: usize,

    #[clap(long)]
    /// Allow signs to be used to infer chamber owners with unknown pearls with.
    /// Can be handy sometimes, but also is more insecure and should
    /// hopefully never be needed in a "normal" scenario.
    use_chamber_signs: bool, // Fix your shit, 2b!

    #[clap(long, use_value_delimiter = true)]
    /// Allow given commands to be used from public chat
    public_chat: Vec<String>,

    #[clap(long)]
    /// Enable open auth mod support.
    /// The target server will be able to authenticate your account against any server of its choosing!
    /// So be sure to trust it!
    open_auth_mod: bool,

    #[clap(long)]
    /// Send a command once entering a lobby (to join queue to main server)
    queue_command: Option<String>,
}

static OPTS: Lazy<Opts> = Lazy::new(|| Opts::parse());
static INPUTLINE_QUEUE: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());
static DEVNET_RX_QUEUE: Mutex<VecDeque<devnet::Message>> = Mutex::new(VecDeque::new());
static DEVNET_TX: Mutex<Option<tokio::sync::mpsc::Sender<devnet::Message>>> = Mutex::new(None);

fn main() -> Result<()> {
    // Parse cli args, handle --help, etc.
    let _ = *OPTS;

    // Setup logging
    if std::env::var("RUST_LOG").is_err() {
        unsafe {
            std::env::set_var("RUST_LOG", "info,azalea::pathfinder=warn");
        }
    }

    let (non_blocking_stdout, _stdout_guard) = tracing_appender::non_blocking(std::io::stdout());
    let reg = tracing_subscriber::registry().with(
        // Default layer outputting to console/tty
        tracing_subscriber::fmt::layer()
            .with_ansi(!OPTS.no_color)
            .with_thread_names(true)
            .with_writer(non_blocking_stdout)
            .with_filter(EnvFilter::from_default_env()),
    );

    let _file_guard: WorkerGuard; // Extend lifetime of workerguard or logs will be empty due to it being dropped early

    // Log either directly to log file or rotated using logrotate into a directory
    // The filter may be different from default, if --log-filter was specified
    match (&OPTS.log_file, &OPTS.log_directory) {
        (Some(_log_file_path), Some(_log_directory_path)) => {
            eprintln!("Fatal: --log-file and --log-directory are not supported together. Choose one!");
            std::process::exit(EXITCODE_CONFLICTING_CLI_OPTS);
        }
        (None, Some(log_directory_path)) => {
            let appender = LogRollerBuilder::new(log_directory_path, &PathBuf::from("stasis-bot"))
                .rotation(Rotation::AgeBased(RotationAge::Daily))
                .time_zone(TimeZone::UTC)
                .compression(Compression::Gzip)
                .graceful_shutdown(true)
                .suffix("log".to_owned())
                .build()?;

            let (non_blocking, worker_guard) = tracing_appender::non_blocking(StripAnsiWriter::new(appender));
            reg.with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_thread_names(true)
                    .with_writer(non_blocking)
                    .with_filter(if let Some(filter) = &OPTS.log_filter {
                        filter.parse::<EnvFilter>().expect("Parsing --log-filter")
                    } else {
                        EnvFilter::from_default_env()
                    }),
            )
            .init();
            _file_guard = worker_guard;
        }
        (Some(log_file_path), None) => {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(log_file_path)
                .context("Open logfile for appending")?;
            let (non_blocking, worker_guard) = tracing_appender::non_blocking(StripAnsiWriter::new(file));

            reg.with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_thread_names(true)
                    .with_writer(non_blocking)
                    .with_filter(if let Some(filter) = &OPTS.log_filter {
                        filter.parse::<EnvFilter>().expect("Parsing --log-filter")
                    } else {
                        EnvFilter::from_default_env()
                    }),
            )
            .init();
            _file_guard = worker_guard;
        }
        (None, None) => {
            reg.init();
        }
    }

    //let worker_threads = if OPTS.worker_threads == 0 { 4 } else { OPTS.worker_threads };
    //info!("Worker threads: {worker_threads}");

    let worker_thread_num = AtomicU32::new(1);
    tokio::runtime::Builder::new_current_thread() // https://github.com/azalea-rs/azalea/tree/main/azalea#using-a-single-threaded-tokio-runtime
        //.worker_threads(worker_threads) // Does nothing rn
        .thread_name_fn(move || format!("Tokio Worker {}", worker_thread_num.fetch_add(1, std::sync::atomic::Ordering::Relaxed),))
        .enable_all()
        .build()?
        .block_on(async_main())
}

async fn async_main() -> Result<()> {
    if !OPTS.just_print_access_token {
        if OPTS.offline_username.is_none() {
            info!(
                "File used to store authentication information: {:?} (key: {:?})",
                OPTS.auth_file, OPTS.auth_file_key
            );
        }

        if OPTS.no_color {
            info!("Will not use colored chat messages and disabled ansi formatting in console.");
        }

        if OPTS.quiet {
            info!("Will not automatically send any chat commands (workaround for getting kicked because of broken ChatCommand packet).");
        }

        if let Some(emergency_quit) = OPTS.emergency_quit {
            info!("Will automatically logout and quit, when reaching {emergency_quit} HP or popping a totem.");
        }

        if OPTS.no_stasis {
            info!("Will not perform any stasis duties!");
        }

        if OPTS.enable_pos_command {
            info!("The command !pos has been enabled for admins!");
        }

        if OPTS.auto_eat {
            info!("Automatic Eating is enabled.");
        }

        if OPTS.no_stasis && OPTS.pearls_min_pos.is_some() {
            error!("--pearls-min-pos has no use when --no-stasis is specified!");
            std::process::exit(EXITCODE_CONFLICTING_CLI_OPTS);
        }

        if OPTS.no_stasis && OPTS.pearls_max_pos.is_some() {
            error!("--pearls-max-pos has no use when --no-stasis is specified!");
            std::process::exit(EXITCODE_CONFLICTING_CLI_OPTS);
        }

        if let Some(pearls_min_pos) = &OPTS.pearls_min_pos {
            info!("Ignoring any pearls that spawn at a block pos below: {pearls_min_pos}");
        }

        if let Some(pearls_max_pos) = &OPTS.pearls_max_pos {
            info!("Ignoring any pearls that spawn at a block pos above: {pearls_max_pos}");
        }

        if OPTS.devnet_url.is_some() != OPTS.devnet_access_token.is_some() {
            error!("--devnet-url and --devnet-access-token must both be specified or omitted!");
            std::process::exit(EXITCODE_CONFLICTING_CLI_OPTS);
        }

        if !OPTS.public_chat.is_empty() {
            if OPTS.public_chat.contains(&"*".to_owned()) {
                info!("All commands are allowed from public chat!");
            } else {
                info!("The following commands are allowed from public chat: {:?}", OPTS.public_chat);
            }
        }

        info!("Admins: {}", OPTS.admin.join(", "));
        info!("Logging in...");
    }

    // parking_lot deadlock detection:
    // Create a background thread which checks for deadlocks every 10s
    std::thread::Builder::new().name("Deadlock-Checker".to_owned()).spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(30));
            let deadlocks = parking_lot::deadlock::check_deadlock();
            if deadlocks.is_empty() {
                continue;
            }

            error!("{} DEADLOCKS DETECTED!!!", deadlocks.len());
            for (i, threads) in deadlocks.iter().enumerate() {
                error!("Deadlock #{}", i);
                for t in threads {
                    error!("Thread Id {:#?}\n{:#?}", t.thread_id(), t.backtrace());
                }
            }
        }
    })?;

    let mut account = if let Some(offline_username) = &OPTS.offline_username {
        if OPTS.just_print_access_token {
            error!("Can't print an access token for an offline account!");
            std::process::exit(EXITCODE_CONFLICTING_CLI_OPTS);
        }
        info!("Using an offline account with username {offline_username:?}!");
        Account::offline(&offline_username)
    } else {
        let auth_result = match auth().await {
            Ok(result) => result,
            Err(err) => {
                error!("Quiting because failed to authenticate: {err:?}");
                std::process::exit(EXITCODE_AUTH_FAILED);
            }
        };
        Account {
            username: auth_result.profile.name,
            access_token: Some(Arc::new(Mutex::new(auth_result.access_token))),
            uuid: Some(auth_result.profile.id),
            account_opts: azalea::AccountOpts::Microsoft {
                email: OPTS.auth_file_key.to_owned(),
            },
            // we don't do chat signing by default unless the user asks for it
            certs: None,
        }
    };

    // Print access token and exit, if requested
    if OPTS.just_print_access_token {
        if let Some(access_token) = account.access_token {
            println!("{}", access_token.lock());
            std::process::exit(0);
        } else {
            error!("Failed to find access token!");
            std::process::exit(EXITCODE_NO_ACCESS_TOKEN);
        }
    }

    if OPTS.sign_chat {
        account.request_certs().await.context("Request certs for chat signing")?;
        info!("Chat signing is enabled. Retreived certs for it.");
    }

    info!("Logged in as {}. Connecting to \"{}\"...", account.username, OPTS.server_address);

    if let Some(ref url) = OPTS.devnet_url
        && let Some(ref access_token) = OPTS.devnet_access_token
    {
        let url = url.to_owned();
        let access_token = access_token.to_owned();
        let (msg_tx_tx, msg_tx_rx) = tokio::sync::mpsc::channel(512);
        //msg_tx_tx.send(devnet::Message::DestinationsRequest).await?;
        *DEVNET_TX.lock() = Some(msg_tx_tx);

        let (msg_rx_tx, mut msg_rx_rx) = tokio::sync::mpsc::channel(512);
        tokio::spawn(async move {
            while let Some(message) = msg_rx_rx.recv().await {
                DEVNET_RX_QUEUE.lock().push_back(message);
            }
            error!("Receiver for devnet messages ended unexpectedly!");
        });
        tokio::spawn(devnet::run_forever(url, access_token, msg_tx_rx, msg_rx_tx));
    }

    // Read input and put in queue
    if std::io::stdin().is_terminal() {
        std::thread::spawn(|| {
            loop {
                let mut line = String::new();
                if let Err(err) = std::io::stdin().read_line(&mut line) {
                    warn!("Not accepting any input anymore because reading failed: Err: {err}");
                    return;
                }
                let line: &str = line.trim();

                INPUTLINE_QUEUE.lock().push_back(line.to_owned());
            }
        });
    }

    let account_count = 1;
    let task_opts = TaskPoolOptions {
        // By default, use however many cores are available on the system
        min_total_threads: 1,
        max_total_threads: usize::MAX,

        // Use 25% of cores for IO, at least 1, no more than 4
        io: TaskPoolThreadAssignmentPolicy {
            min_threads: if OPTS.io_threads == 0 { (account_count / 12).max(2) } else { OPTS.io_threads },
            max_threads: if OPTS.io_threads == 0 { (account_count / 12).max(2) } else { OPTS.io_threads },
            percent: 0.25,
        },

        // Use 25% of cores for async compute, at least 1, no more than 4
        async_compute: TaskPoolThreadAssignmentPolicy {
            min_threads: if OPTS.async_compute_threads == 0 {
                account_count.max(4)
            } else {
                OPTS.async_compute_threads
            },
            max_threads: if OPTS.async_compute_threads == 0 {
                account_count.max(4)
            } else {
                OPTS.async_compute_threads
            },
            percent: 0.25,
        },

        // Use all remaining cores for compute (at least 1)
        compute: TaskPoolThreadAssignmentPolicy {
            min_threads: if OPTS.compute_threads == 0 {
                (account_count / 12).max(2)
            } else {
                OPTS.compute_threads
            },
            max_threads: if OPTS.compute_threads == 0 {
                (account_count / 12).max(2)
            } else {
                OPTS.compute_threads
            },
            percent: 1.0, // This 1.0 here means "whatever is left over"
        },
    };
    let mut builder = azalea::swarm::SwarmBuilder::new_without_plugins()
        .add_plugins(DefaultPlugins.build().disable::<TaskPoolPlugin>().disable::<LogPlugin>())
        .add_plugins(DefaultBotPlugins)
        .add_plugins(DefaultSwarmPlugins)
        .add_plugins(TaskPoolPlugin { task_pool_options: task_opts })
        .set_handler(handle)
        .set_swarm_handler(swarm_handle)
        .add_account(account.clone());
    if OPTS.open_auth_mod {
        builder = builder.add_plugins(OpenAuthModPlugin::start().await);
        info!(
            "OpenAuthMod support is active. The target server can authenticate your account against any server of its choosing! Make sure to only use this on servers you trust!"
        );
    }
    builder.start(OPTS.server_address.as_str()).await.context("Running swarm")?
}

async fn auth() -> Result<AuthResult> {
    Ok(azalea::auth::auth(
        &OPTS.auth_file_key,
        azalea::auth::AuthOpts {
            cache_file: Some(OPTS.auth_file.clone()),
            ..Default::default()
        },
    )
    .await?)
}

#[derive(Clone, Component)]
pub struct BotState {
    last_dm_handled_at: Arc<Mutex<Option<Instant>>>,
    root_task_group: Arc<Mutex<TaskGroup>>,
    root_task_last_display: Arc<Mutex<String>>,

    auto_eat: Option<AutoEatModule>,
    periodic_swing: Option<PeriodicSwingModule>,
    legacy_stasis: Option<LegacyStasisModule>,
    visual_range: Option<VisualRangeModule>,
    look_at_players: Option<LookAtPlayersModule>,
    soundness: Option<SoundnessModule>,
    emergency_quit: Option<EmergencyQuitModule>,
    devnet_integration: Option<DevNetIntegrationModule>,
    server_tps: Option<ServerTpsModule>,
    stasis: Option<StasisModule>,
    chat: Option<ChatModule>,
    disc_jockey: Option<DiscJockeyModule>,
    webhook: Option<WebhookModule>,
    killaura: Option<KillauraModule>,
    auto_tpa: Option<AutoTpaModule>,
}

fn default_if<T: Default>(enabled: bool) -> Option<T> {
    if enabled { Some(Default::default()) } else { None }
=======
#[macro_use]
extern crate tracing;

mod config;
mod ui;

use anyhow::Result;
use azalea::block::{BlockState, BlockTrait};
use azalea::entity::Position;
use azalea::prelude::*;
use azalea::protocol::packets::game::ClientboundGamePacket;
use azalea::{BlockPos, Vec3, WalkDirection};
use clap::Parser;
use once_cell::sync::OnceCell;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
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
        if cfg.trapdoor.is_some() {
            println!("Chat trigger: {}", cfg.trigger_text());
            println!("Press Enter in this window to click the trapdoor.");
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
static CLICK_REQUEST: AtomicBool = AtomicBool::new(false);
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
>>>>>>> 160cc57 (making it pub)
}

impl Default for BotState {
    fn default() -> Self {
<<<<<<< HEAD
        Self {
            last_dm_handled_at: Default::default(),
            root_task_group: Arc::new(Mutex::new(TaskGroup::new_root())),
            root_task_last_display: Default::default(),

            auto_eat: default_if(OPTS.auto_eat),
            periodic_swing: default_if(OPTS.periodic_swing),
            legacy_stasis: default_if(!OPTS.no_stasis), // Just left to migrate locations, does not do anything anymore
            visual_range: Some(Default::default()),
            look_at_players: OPTS.look_at_players.map(|dist| LookAtPlayersModule::new(dist)),
            soundness: Some(Default::default()),
            emergency_quit: OPTS.emergency_quit.map(|hp| EmergencyQuitModule::new(hp, OPTS.emergency_quit_strict)),
            devnet_integration: default_if(OPTS.devnet_url.is_some() && OPTS.devnet_access_token.is_some()),
            server_tps: Some(Default::default()),
            stasis: if !OPTS.no_stasis {
                Some(StasisModule::new(!OPTS.no_trapdoor_reopen, OPTS.max_pearls))
            } else {
                None
            },
            chat: Some(ChatModule::default()),
            disc_jockey: if let Some(songs) = &OPTS.songs {
                Some(DiscJockeyModule::new(songs))
            } else {
                None
            },
            webhook: if let Some(url) = &OPTS.webhook_url {
                Some(WebhookModule::new(url, OPTS.webhook_alert_role_id))
            } else {
                None
            },
            killaura: default_if(OPTS.killaura),
            auto_tpa: default_if(OPTS.auto_tpa),
        }
    }
}

impl BotState {
    pub fn modules(&self) -> Vec<&dyn Module> {
        let mut modules: Vec<&dyn Module> = vec![];
        if let Some(module) = &self.auto_eat {
            modules.push(module);
        };
        if let Some(module) = &self.webhook {
            modules.push(module);
        };
        if let Some(module) = &self.periodic_swing {
            modules.push(module);
        };
        if let Some(module) = &self.legacy_stasis {
            modules.push(module);
        };
        if let Some(module) = &self.visual_range {
            modules.push(module);
        };
        if let Some(module) = &self.look_at_players {
            modules.push(module);
        };
        if let Some(module) = &self.soundness {
            modules.push(module);
        };
        if let Some(module) = &self.emergency_quit {
            modules.push(module);
        };
        if let Some(module) = &self.devnet_integration {
            modules.push(module);
        };
        if let Some(module) = &self.server_tps {
            modules.push(module);
        };
        if let Some(module) = &self.stasis {
            modules.push(module);
        };
        if let Some(module) = &self.chat {
            modules.push(module);
        };
        if let Some(module) = &self.disc_jockey {
            modules.push(module);
        };
        if let Some(module) = &self.killaura {
            modules.push(module);
        }
        if let Some(module) = &self.auto_tpa {
            modules.push(module);
        };
        modules
    }

    pub fn add_task(&self, task: impl Into<Box<dyn Task>>) {
        self.root_task_group.lock().add(task);
    }

    pub fn add_task_now(&self, bot: Client, bot_state: &BotState, task: impl Into<Box<dyn Task>>) -> Result<()> {
        self.root_task_group.lock().add_now(bot.clone(), bot_state, task)?;
        Ok(())
    }

    pub fn tasks(&self) -> u64 {
        self.root_task_group.lock().remaining()
    }

    pub fn webhook_alert(&self, message: impl AsRef<str>) {
        if let Some(webhook) = &self.webhook {
            webhook.webhook_alert(message);
        } else {
            debug!("Alert Webhook-Message (not enabled): {}", message.as_ref());
        }
    }

    pub fn webhook_silent(&self, message: impl AsRef<str>) {
        if let Some(webhook) = &self.webhook {
            webhook.webhook_silent(message);
        } else {
            debug!("Silent Webhook-Message (not enabled): {}", message.as_ref());
        }
    }

    pub fn webhook(&self, message: impl AsRef<str>) {
        if let Some(webhook) = &self.webhook {
            webhook.webhook(message);
        } else {
            debug!("Webhook-Message (not enabled): {}", message.as_ref());
        }
    }

    pub fn wait_on_webhooks_and_exit(&self, exit_code: i32) {
        if let Some(webhook) = self.webhook.clone() {
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(300)).await;
                webhook.queue_open.store(false, Ordering::Relaxed);
                let started = tokio::time::Instant::now();
                loop {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    if webhook.queue.lock().is_empty() && !webhook.sending_message.load(Ordering::Relaxed) {
                        info!("Webhook is done sending messages. Quitting now!");
                        std::process::exit(exit_code);
                    }
                    if started.elapsed() > Duration::from_secs(3) {
                        warn!("Waited too long on webhook. Quitting now!");
                        std::process::exit(exit_code);
                    }
                }
            });
        } else {
            std::process::exit(exit_code);
        }
    }
}

async fn handle(bot: Client, event: Event, bot_state: BotState) -> Result<()> {
    if let Some(ref module) = bot_state.auto_eat {
        module
            .handle(bot.clone(), &event, &bot_state)
            .await
            .with_context(|| format!("Handling {}", module.name()))?;
    }
    if let Some(ref module) = bot_state.webhook {
        module
            .handle(bot.clone(), &event, &bot_state)
            .await
            .with_context(|| format!("Handling {}", module.name()))?;
    }
    if let Some(ref module) = bot_state.periodic_swing {
        module
            .handle(bot.clone(), &event, &bot_state)
            .await
            .with_context(|| format!("Handling {}", module.name()))?;
    }
    if let Some(ref module) = bot_state.legacy_stasis {
        module
            .handle(bot.clone(), &event, &bot_state)
            .await
            .with_context(|| format!("Handling {}", module.name()))?;
    }
    if let Some(ref module) = bot_state.visual_range {
        module
            .handle(bot.clone(), &event, &bot_state)
            .await
            .with_context(|| format!("Handling {}", module.name()))?;
    }
    if let Some(ref module) = bot_state.look_at_players {
        module
            .handle(bot.clone(), &event, &bot_state)
            .await
            .with_context(|| format!("Handling {}", module.name()))?;
    }
    if let Some(ref module) = bot_state.soundness {
        module
            .handle(bot.clone(), &event, &bot_state)
            .await
            .with_context(|| format!("Handling {}", module.name()))?;
    }
    if let Some(ref module) = bot_state.emergency_quit {
        module
            .handle(bot.clone(), &event, &bot_state)
            .await
            .with_context(|| format!("Handling {}", module.name()))?;
    }
    if let Some(ref module) = bot_state.devnet_integration {
        module
            .handle(bot.clone(), &event, &bot_state)
            .await
            .with_context(|| format!("Handling {}", module.name()))?;
    }
    if let Some(ref module) = bot_state.server_tps {
        module
            .handle(bot.clone(), &event, &bot_state)
            .await
            .with_context(|| format!("Handling {}", module.name()))?;
    }
    if let Some(ref module) = bot_state.stasis {
        module
            .handle(bot.clone(), &event, &bot_state)
            .await
            .with_context(|| format!("Handling {}", module.name()))?;
    }
    if let Some(ref module) = bot_state.chat {
        module
            .handle(bot.clone(), &event, &bot_state)
            .await
            .with_context(|| format!("Handling {}", module.name()))?;
    }
    if let Some(ref module) = bot_state.disc_jockey {
        module
            .handle(bot.clone(), &event, &bot_state)
            .await
            .with_context(|| format!("Handling {}", module.name()))?;
    }
    if let Some(ref module) = bot_state.killaura {
        module
            .handle(bot.clone(), &event, &bot_state)
            .await
            .with_context(|| format!("Handling {}", module.name()))?;
    }
    if let Some(ref module) = bot_state.auto_tpa {
        module
            .handle(bot.clone(), &event, &bot_state)
            .await
            .with_context(|| format!("Handling {}", module.name()))?;
    }

    // Process task(s)
    {
        let mut task_root = bot_state.root_task_group.lock();
        if task_root.remaining() > 0 {
            if bot.get_component::<Position>().is_none() {
                warn!("Bot has no position associated anymore. Canceling all tasks just in case!");
                task_root.discard(bot.clone(), &bot_state)?;
                *task_root = TaskGroup::new_root();
            } else {
                let mut do_cleanup = false;
                match task_root.handle(bot.clone(), &bot_state, &event).context("Handling root TaskGroup") {
                    Ok(TaskOutcome::Ongoing) => {}
                    Ok(TaskOutcome::Succeeded) => {
                        do_cleanup = true;
                    }
                    Ok(TaskOutcome::Failed { reason }) => {
                        do_cleanup = true;
                        error!("Task Fail: {reason}");
                    }
                    Err(err) => {
                        error!("Got error while handling task {task_root}: {err:?}");
                        bot_state.root_task_last_display.lock().clear();
                        *task_root = TaskGroup::new_root();
                    }
                }

                if do_cleanup {
                    info!("All Tasks done ({}).", task_root.subtasks.len());
                    bot_state.root_task_last_display.lock().clear();
                    *task_root = TaskGroup::new_root();
                } else {
                    let mut last_task_display = bot_state.root_task_last_display.lock();
                    let current_task_display = task_root.to_string();
                    if current_task_display != *last_task_display {
                        info!("Task Status: {current_task_display}");
                        *last_task_display = current_task_display;
                    }
                }
            }
        }
    }

    match event {
        Event::Tick => {
            // Execute commands from input queue
            {
                let mut queue = INPUTLINE_QUEUE.lock();
                while let Some(line) = queue.pop_front() {
                    if let Some(chat) = bot_state.chat.as_ref() {
                        chat.chat(line);
                    } else {
                        if line.starts_with("/") {
                            info!("Sending command: {line}");
                            bot.send_command_packet(&format!("{}", &line[1..]));
                        } else {
                            info!("Sending chat message: {line}");
                            bot.send_chat_packet(&line);
                        }
                    }
                }
=======
        Self { conn: Arc::new(Mutex::new(Conn::new())) }
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
                Ok(_) => CLICK_REQUEST.store(true, Ordering::Relaxed),
            }
        }
    });
}

fn click_trapdoor(bot: &Client) {
    let Some([x, y, z]) = cfg().trapdoor else {
        warn!("No trapdoor is set. Set its X Y Z in the menu under 'Trapdoor'.");
        return;
    };
    bot.look_at(Vec3::new(x as f64 + 0.5, y as f64 + 0.5, z as f64 + 0.5));
    bot.block_interact(BlockPos::new(x, y, z));
    info!("Clicked the trapdoor.");
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

fn chat_triggers(text: &str) -> bool {
    let user = cfg().trigger_user.trim().to_lowercase();
    let word = cfg().trigger_word.trim().to_lowercase();
    if user.is_empty() || word.is_empty() {
        return false;
    }
    let Some((sender, message)) = split_chat(text) else {
        return false;
    };
    sender.to_lowercase() == user && has_phrase(&message.to_lowercase(), &word)
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
        Event::Tick => on_tick(&bot, &state),
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
>>>>>>> 160cc57 (making it pub)
            }
        }
        _ => {}
    }
<<<<<<< HEAD

    Ok(())
}

#[derive(Clone, Component, Resource)]
pub struct SwarmState {
    last_account_refresh: Arc<Mutex<Instant>>,
}

impl Default for SwarmState {
    fn default() -> Self {
        Self {
            last_account_refresh: Arc::new(Mutex::new(Instant::now())),
        }
    }
}

async fn swarm_rejoin(swarm: Swarm, state: SwarmState, account: Account, join_opts: JoinOpts) {
    let mut reconnect_after_secs = 5;
    loop {
        let last_refreshed = state.last_account_refresh.lock().elapsed();
        if last_refreshed > Duration::from_secs(/*3h*/ 60 * 60 * 3)
            && let Some(access_token) = account.access_token.clone()
        {
            info!("This account's access token is more than 3 hours old. Refreshing it!");
            let auth_result = auth().await;
            match auth_result {
                Ok(result) => {
                    info!("Got new access token!");
                    *access_token.lock() = result.access_token;
                    *state.last_account_refresh.lock() = Instant::now();
                }
                Err(err) => {
                    error!("Quitting, because could not get new access token: {err:?}");
                    std::process::exit(EXITCODE_AUTH_FAILED);
                }
            }
        }

        info!("Reconnecting after {} seconds...", reconnect_after_secs);

        tokio::time::sleep(Duration::from_secs(reconnect_after_secs)).await;
        reconnect_after_secs = (reconnect_after_secs * 2).min(60 * 15); // 2x or max 15 minutes

        info!("Joining again...");
        match swarm.add_with_opts(&account, state.clone(), &join_opts).await {
            Ok(_) => return,
            Err(join_err) => error!("Failed to rejoin: {join_err}"), // Keep rejoining
        }
    }
}

async fn swarm_handle(swarm: Swarm, event: SwarmEvent, state: SwarmState) -> anyhow::Result<()> {
    match event {
        SwarmEvent::Disconnect(account, join_opts) => {
            tokio::spawn(swarm_rejoin(swarm.clone(), state.clone(), (*account).clone(), join_opts.clone()));
        }
        _ => {}
    }

    Ok(())
}
=======
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
    if PLAY_LOGINS.load(Ordering::Relaxed) > 0 && chat_triggers(&text) {
        let now = now_ms();
        if now.saturating_sub(LAST_TRIGGER_MS.load(Ordering::Relaxed)) > 1500 {
            LAST_TRIGGER_MS.store(now, Ordering::Relaxed);
            info!("Chat trigger matched; clicking the trapdoor.");
            CLICK_REQUEST.store(true, Ordering::Relaxed);
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
    if CLICK_REQUEST.swap(false, Ordering::Relaxed) {
        click_trapdoor(bot);
    }
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
>>>>>>> 160cc57 (making it pub)
