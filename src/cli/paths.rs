use std::path::PathBuf;

use clap::{Args, ValueEnum};

use crate::config::{self, Config};

#[derive(Args, Debug)]
pub struct PathsArgs {
    #[arg(value_enum, help = "Print only this path")]
    pub which: Option<Which>,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    Config,
    State,
    Cache,
    Library,
}

fn path(which: Which) -> PathBuf {
    match which {
        Which::Config => Config::path(),
        Which::State => config::state_file(),
        Which::Cache => config::cache_dir(),
        Which::Library => config::library_dir(),
    }
}

pub fn run(args: PathsArgs) -> i32 {
    if let Some(which) = args.which {
        println!("{}", path(which).display());
        return 0;
    }
    if let Some(base) = config::base_dir() {
        println!("{:<8} {}", "base", base.display());
    }
    for which in Which::value_variants() {
        let name = which.to_possible_value().map(|v| v.get_name().to_string()).unwrap_or_default();
        println!("{name:<8} {}", path(*which).display());
    }
    0
}
