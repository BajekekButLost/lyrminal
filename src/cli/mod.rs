mod library;
mod now_playing;
mod paths;
pub mod providers;
mod settings;
mod update;

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
    #[command(flatten)]
    pub launch: LaunchArgs,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Args, Debug, Default)]
#[command(next_help_heading = "Temporary settings (locked in the app, never saved)")]
pub struct LaunchArgs {
    #[arg(
        long = "set",
        value_name = "KEY=VALUE",
        allow_hyphen_values = true,
        help = "Start with a setting changed just for this session, e.g. --set appearance.style=bloom (see lyrminal settings list); repeat it for more"
    )]
    pub set: Vec<String>,
    #[arg(long, value_name = "cell|pixel", help = "Use this renderer for this session")]
    pub renderer: Option<String>,
}

impl LaunchArgs {
    fn shortcuts(&self) -> Vec<(&'static str, &'static str, String)> {
        let mut out = Vec::new();
        if let Some(v) = &self.renderer {
            out.push(("--renderer", "appearance.renderer", v.clone()));
        }
        out
    }

    pub fn overrides(&self) -> Result<Vec<crate::overrides::Override>, String> {
        let mut list = Vec::new();
        for (flag, key, value) in self.shortcuts() {
            list.push(crate::overrides::make(key, &value, flag)?);
        }
        for s in &self.set {
            list.push(crate::overrides::parse(s, "--set")?);
        }
        Ok(list)
    }

    pub fn any(&self) -> bool {
        !self.set.is_empty() || !self.shortcuts().is_empty()
    }
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
    #[command(about = "Add, list, change and remove the syncs in the local library")]
    Library(library::LibraryArgs),
    #[command(about = "Check for a new release and install it")]
    Update(update::UpdateArgs),
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
    let _ = tell_app(&crate::ipc::Request::ReloadConfig);
}

pub fn tell_app(request: &crate::ipc::Request) -> bool {
    match crate::ipc::call_blocking(request) {
        Ok(_) => true,
        Err(e) => {
            if e != crate::ipc::NOT_RUNNING {
                eprintln!("lyrminal: saved, but the running app could not take the change: {e}");
            }
            false
        }
    }
}

pub fn library_changed(change: crate::ipc::LibraryChange) {
    if tell_app(&crate::ipc::Request::LibraryChanged(change.clone())) {
        return;
    }
    let mut state = crate::state::State::load();
    if change.forget_choices(&mut state.choices) {
        state.save();
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

pub fn confirm(question: &str) -> bool {
    use std::io::Write;
    print!("{question} [y/N] ");
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    if std::io::stdin().read_line(&mut answer).is_err() {
        return false;
    }
    matches!(answer.trim().to_lowercase().as_str(), "y" | "yes")
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
        Command::Library(args) => library::run(args).await,
        Command::Update(args) => update::run(args).await,
    }
}
