mod now_playing;
mod paths;
mod providers;
mod settings;

use clap::{Args, Parser, Subcommand};

use crate::config::Config;

#[derive(Parser, Debug)]
#[command(
    name = "lyrminal",
    version,
    about = "Synced lyrics in your terminal",
    after_help = "Without a command, Lyrminal starts and shows the lyrics of the playing song.\nEverything else is configured inside the app (press Esc)."
)]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalArgs,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Args, Debug, Default, Clone)]
#[command(next_help_heading = "Global options")]
pub struct GlobalArgs {
    #[arg(
        long,
        global = true,
        value_name = "DIR",
        value_parser = non_empty,
        help = "Use DIR as the home folder for Lyrminal's files ($BASE/.config/lyrminal, $BASE/.cache/lyrminal, ...); created if missing"
    )]
    pub base_directory: Option<String>,
    #[arg(long, global = true, help = "Same as --base-directory, in a new randomly named temporary folder")]
    pub temp_base_directory: bool,
    #[arg(long, global = true, value_name = "FILE", value_parser = non_empty, help = "Read and save the settings in FILE instead of config.json")]
    pub config: Option<String>,
    #[arg(long, global = true, value_name = "FILE", value_parser = non_empty, help = "Keep per-track offsets, chosen syncs and the tour status in FILE instead of state.json")]
    pub state_file: Option<String>,
    #[arg(long, global = true, value_name = "DIR", value_parser = non_empty, help = "Keep cached lyrics and lookups in DIR")]
    pub cache_dir: Option<String>,
    #[arg(long, global = true, value_name = "DIR", value_parser = non_empty, help = "Use DIR as the local library (index.json and the saved syncs)")]
    pub library_dir: Option<String>,
    #[arg(long, global = true, value_name = "FILE", value_parser = non_empty, help = "Socket the running app listens on, used by now-playing and to tell the app about changes")]
    pub socket: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    #[command(about = "Show where Lyrminal keeps its files")]
    Paths(paths::PathsArgs),
    #[command(about = "Read and change the settings (everything except the lyrics providers)")]
    Settings(settings::SettingsArgs),
    #[command(about = "Order, turn on/off and configure the lyrics providers")]
    Providers(providers::ProvidersArgs),
    #[command(about = "Show what the running Lyrminal is playing and where it is in the lyrics")]
    NowPlaying(now_playing::NowPlayingArgs),
}

pub fn non_empty(s: &str) -> Result<String, String> {
    if s.trim().is_empty() {
        Err("needs a value".into())
    } else {
        Ok(s.to_string())
    }
}

pub fn fail(msg: impl std::fmt::Display) -> ! {
    eprintln!("lyrminal: {msg}");
    std::process::exit(1);
}

pub fn load_config() -> Config {
    Config::load().unwrap_or_else(|e| fail(format!("{e:#}")))
}

pub fn save_config(cfg: &Config) {
    if let Err(e) = cfg.save() {
        fail(format!("could not save {}: {e:#}", Config::path().display()));
    }
    tell_app(&crate::ipc::Request::ReloadConfig);
}

pub fn tell_app(request: &crate::ipc::Request) {
    if let Err(e) = crate::ipc::call_blocking(request)
        && e != crate::ipc::NOT_RUNNING
    {
        eprintln!("lyrminal: saved, but the running app could not take the change: {e}");
    }
}

pub fn stdin_piped() -> bool {
    !std::io::IsTerminal::is_terminal(&std::io::stdin())
}

pub fn read_stdin() -> String {
    let mut text = String::new();
    if let Err(e) = std::io::Read::read_to_string(&mut std::io::stdin(), &mut text) {
        fail(format!("could not read stdin: {e}"));
    }
    text
}

pub fn read_list(values: Vec<String>, what: &str) -> Vec<String> {
    if !values.is_empty() && values != ["-"] {
        return values;
    }
    if !stdin_piped() {
        fail(format!("give at least one {what}, as arguments or one per line on stdin"));
    }
    let list: Vec<String> = read_stdin()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect();
    if list.is_empty() {
        fail(format!("stdin had no {what}"));
    }
    list
}

pub fn read_value(value: Option<String>, what: &str) -> String {
    match value {
        Some(v) if v != "-" => v,
        _ if stdin_piped() => read_stdin().trim_end_matches(['\n', '\r']).to_string(),
        _ => fail(format!("{what} needs a value, as an argument or on stdin")),
    }
}

pub async fn run(command: Command) -> i32 {
    match command {
        Command::Paths(args) => paths::run(args),
        Command::Settings(args) => settings::run(args),
        Command::Providers(args) => providers::run(args),
        Command::NowPlaying(args) => now_playing::run(args).await,
    }
}
