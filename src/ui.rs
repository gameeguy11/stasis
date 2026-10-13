use crate::config::Config;
use anyhow::Result;
use console::{Key, Term};
use dialoguer::{Confirm, Input, Select, theme::ColorfulTheme};
use std::path::Path;

fn ask_text(prompt: &str, current: &str) -> Result<String> {
    let theme = ColorfulTheme::default();
    let mut input = Input::<String>::with_theme(&theme).with_prompt(prompt);
    if !current.is_empty() {
        input = input.default(current.to_owned());
    }
    Ok(input.interact_text()?.trim().to_owned())
}

fn read_masked(hint: &str, validate: impl Fn(&str) -> Result<(), String>) -> Result<String> {
    let term = Term::stdout();
    term.write_line(hint)?;
    let mut buf = String::new();
    loop {
        term.clear_line()?;
        term.write_str(&format!("> {}", "*".repeat(buf.chars().count())))?;
        match term.read_key()? {
            Key::Enter => match validate(&buf) {
                Ok(()) => {
                    term.write_line("")?;
                    return Ok(buf);
                }
                Err(e) => {
                    term.write_line("")?;
                    term.write_line(&e)?;
                }
            },
            Key::Backspace => {
                buf.pop();
            }
            Key::Char(c) if !c.is_control() => buf.push(c),
            Key::CtrlC => return Err(std::io::Error::from(std::io::ErrorKind::Interrupted).into()),
            _ => {}
        }
    }
}

fn ask_password(current: &str) -> Result<String> {
    let keep = !current.is_empty();
    let hint = if keep { "Password (leave empty to keep current)" } else { "Password" };
    let new = read_masked(hint, |v| {
        if v.is_empty() && !keep { Err("Password can't be empty.".to_owned()) } else { Ok(()) }
    })?;
    Ok(if new.is_empty() { current.to_owned() } else { new })
}

fn ask_number(prompt: &str, current: f64) -> Result<f64> {
    let theme = ColorfulTheme::default();
    Ok(Input::<f64>::with_theme(&theme)
        .with_prompt(prompt)
        .default(current)
        .validate_with(|v: &f64| if *v >= 0.0 && v.is_finite() { Ok(()) } else { Err("must be 0 or more") })
        .interact_text()?)
}

fn ask_optional(prompt: &str, current: &str) -> Result<String> {
    let theme = ColorfulTheme::default();
    Ok(Input::<String>::with_theme(&theme)
        .with_prompt(prompt)
        .with_initial_text(current.to_owned())
        .allow_empty(true)
        .interact_text()?
        .trim()
        .to_owned())
}

fn parse_trapdoor(text: &str) -> Result<[i32; 3], String> {
    let tokens: Vec<&str> = text.split(|c: char| c.is_whitespace() || c == ',').filter(|t| !t.is_empty()).collect();
    if tokens.len() != 3 {
        return Err(format!("Expected 3 numbers (X Y Z), but got {}. Example: 12 64 -30", tokens.len()));
    }
    let mut out = [0i32; 3];
    for (i, (token, axis)) in tokens.iter().zip(["X", "Y", "Z"]).enumerate() {
        out[i] = token.parse::<i32>().map_err(|_| format!("{axis} must be a whole number, but got '{token}'."))?;
    }
    if !(-64..=320).contains(&out[1]) {
        return Err(format!("Y must be between -64 and 320, but got {}.", out[1]));
    }
    if out[0].abs() > 30_000_000 || out[2].abs() > 30_000_000 {
        return Err("X and Z must be within 30000000 of the origin.".to_owned());
    }
    Ok(out)
}

fn ask_trapdoor(current: Option<[i32; 3]>) -> Result<Option<[i32; 3]>> {
    let hint = if current.is_some() {
        "Trapdoor X Y Z (empty keeps current, type 'clear' to remove)"
    } else {
        "Trapdoor X Y Z (for example: 12 64 -30)"
    };
    let text = read_masked(hint, |v| {
        let t = v.trim();
        if t.is_empty() || t.eq_ignore_ascii_case("clear") {
            return Ok(());
        }
        parse_trapdoor(t).map(|_| ())
    })?;
    let t = text.trim();
    if t.is_empty() {
        return Ok(current);
    }
    if t.eq_ignore_ascii_case("clear") {
        return Ok(None);
    }
    Ok(parse_trapdoor(t).ok())
}

fn ask_register_mode(cfg: &mut Config) -> Result<()> {
    let items = [
        "auto (register when the server asks)",
        "always (send /register right after joining)",
        "never (only /login)",
    ];
    let default = if cfg.no_register {
        2
    } else if cfg.force_register {
        1
    } else {
        0
    };
    let theme = ColorfulTheme::default();
    let choice = Select::with_theme(&theme)
        .with_prompt("Registering")
        .items(&items)
        .default(default)
        .interact()?;
    cfg.force_register = choice == 1;
    cfg.no_register = choice == 2;
    Ok(())
}

fn ask_servers(current: &[String]) -> Result<Vec<String>> {
    let text = ask_text("Server addresses (comma separated, tried in order)", &current.join(", "))?;
    Ok(text.split(',').map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect())
}

fn mask(pw: &str) -> String {
    if pw.is_empty() { "(not set)".to_owned() } else { "*".repeat(pw.chars().count().min(12)) }
}
fn wizard(cfg: &mut Config) -> Result<()> {
    println!("\nFirst time setup - let's answer a few questions.\n");
    cfg.servers = ask_servers(&cfg.servers)?;
    cfg.username = ask_text("Account name", &cfg.username)?;
    cfg.password = ask_password(&cfg.password)?;
    ask_register_mode(cfg)?;
    Ok(())
}
pub fn run(path: &Path, skip_menu: bool) -> Result<Option<Config>> {
    let loaded = Config::load(path)?;
    let first_run = loaded.is_none();
    let mut cfg = loaded.unwrap_or_default();

    if skip_menu {
        if !cfg.is_complete() {
            anyhow::bail!("--run needs a complete saved config ({}). Start without --run to set it up.", path.display());
        }
        return Ok(Some(cfg));
    }

    if first_run || !cfg.is_complete() {
        wizard(&mut cfg)?;
        cfg.save(path)?;
        println!("Saved to {}.", path.display());
    }

    let theme = ColorfulTheme::default();
    let mut selected = 0;
    loop {
        let items = vec![
            "▶  Start the bot".to_owned(),
            format!("Server addresses     : {}", cfg.servers.join(", ")),
            format!("Account name         : {}", cfg.username),
            format!("Password             : {}", mask(&cfg.password)),
            format!("Registering          : {}", cfg.register_mode()),
            format!("Wait after /login    : {} s", cfg.walk_delay),
            format!("Walk timeout         : {} s", cfg.walk_timeout),
            format!("Stay connected after : {}", if cfg.stay { "yes" } else { "no" }),
            format!("Trapdoor (X Y Z)     : {}", if cfg.trapdoor.is_some() { "************" } else { "(not set)" }),
            format!("Chat trigger user   : {}", if cfg.trigger_user.is_empty() { "(not set)" } else { cfg.trigger_user.as_str() }),
            format!("Chat trigger word   : {}", if cfg.trigger_word.is_empty() { "(not set)" } else { cfg.trigger_word.as_str() }),
            "Redo setup questions".to_owned(),
            "Quit".to_owned(),
        ];
        let choice = Select::with_theme(&theme)
            .with_prompt("sixb6t login bot - pick something to edit")
            .items(&items)
            .default(selected)
            .interact()?;
        selected = choice;

        match choice {
            0 => {
                if !cfg.is_complete() {
                    println!("Server, account name and password must be set first.");
                    continue;
                }
                cfg.save(path)?;
                return Ok(Some(cfg));
            }
            1 => cfg.servers = ask_servers(&cfg.servers)?,
            2 => cfg.username = ask_text("Account name", &cfg.username)?,
            3 => cfg.password = ask_password(&cfg.password)?,
            4 => ask_register_mode(&mut cfg)?,
            5 => cfg.walk_delay = ask_number("Seconds to wait after /login", cfg.walk_delay)?,
            6 => cfg.walk_timeout = ask_number("Seconds before a walk is aborted", cfg.walk_timeout)?,
            7 => cfg.stay = Confirm::with_theme(&theme).with_prompt("Stay connected when finished?").default(cfg.stay).interact()?,
            8 => cfg.trapdoor = ask_trapdoor(cfg.trapdoor)?,
            9 => cfg.trigger_user = ask_optional("Username to watch in chat (empty to turn off)", &cfg.trigger_user)?,
            10 => cfg.trigger_word = ask_optional("Word or phrase that makes the bot click (empty to turn off)", &cfg.trigger_word)?,
            11 => wizard(&mut cfg)?,
            _ => {
                cfg.save(path)?;
                return Ok(None);
            }
        }
        cfg.save(path)?;
    }
}