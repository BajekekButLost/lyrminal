use clap::{Args, Parser, Subcommand};

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
}

#[derive(Subcommand, Debug)]
pub enum Command {}

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

pub async fn run(command: Command) -> i32 {
    match command {}
}
