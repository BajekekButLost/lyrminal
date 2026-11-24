use clap::{Args, Subcommand};
use serde_json::json;

use crate::config::Config;
use crate::setting_keys::{self, Setting};

#[derive(Args, Debug)]
pub struct SettingsArgs {
    #[command(subcommand)]
    pub command: SettingsCommand,
}

#[derive(Subcommand, Debug)]
pub enum SettingsCommand {
    #[command(about = "List every setting with its key and value, grouped like the settings tabs")]
    List(ListArgs),
    #[command(about = "Print the value of a setting")]
    Get(GetArgs),
    #[command(about = "Change a setting; the value can also come from stdin")]
    Set(SetArgs),
    #[command(about = "Turn on/off settings the other way; keys can also come from stdin")]
    Toggle(KeysArgs),
}

#[derive(Args, Debug)]
pub struct KeysArgs {
    #[arg(help = "Setting keys; leave them out or use - to read them from stdin, one per line")]
    pub keys: Vec<String>,
}

#[derive(Args, Debug)]
pub struct SetArgs {
    #[arg(help = "The setting's key, e.g. appearance.fontScale")]
    pub key: String,
    #[arg(allow_hyphen_values = true, help = "The new value; leave it out or use - to read it from stdin")]
    pub value: Option<String>,
}

#[derive(Args, Debug)]
pub struct GetArgs {
    #[arg(help = "The setting's key, e.g. appearance.fontScale")]
    pub key: String,
    #[arg(long, help = "Show secret values such as API keys instead of masking them")]
    pub reveal: bool,
}

#[derive(Args, Debug)]
pub struct ListArgs {
    #[arg(help = "Only list the settings of this tab, e.g. appearance")]
    pub tab: Option<String>,
    #[arg(long, help = "Print the settings as JSON")]
    pub json: bool,
    #[arg(long, help = "Show secret values such as API keys instead of masking them")]
    pub reveal: bool,
}

pub fn settings() -> Vec<Setting> {
    setting_keys::all().into_iter().filter(|s| !s.is_provider()).collect()
}

pub fn print_list(cfg: &Config, list: &[Setting], json: bool, reveal: bool) {
    let defaults = Config::default();
    if json {
        let rows: Vec<_> = list
            .iter()
            .map(|s| {
                json!({
                    "key": s.key(),
                    "group": s.group,
                    "label": s.item.label,
                    "value": setting_keys::json_value(cfg, &s.item, reveal),
                    "default": setting_keys::json_value(&defaults, &s.item, reveal),
                    "changed": !setting_keys::is_default(cfg, &s.item),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows).unwrap_or_default());
        return;
    }
    let width = list.iter().map(|s| s.key().len()).max().unwrap_or(0);
    let mut group = None;
    for s in list {
        if group != Some(&s.group) {
            if group.is_some() {
                println!();
            }
            println!("{}", s.group);
            group = Some(&s.group);
        }
        let value = setting_keys::value(cfg, &s.item, reveal);
        if setting_keys::is_default(cfg, &s.item) {
            println!("  {:<width$}  {value}", s.key());
        } else {
            let default = setting_keys::value(&defaults, &s.item, reveal);
            println!("  {:<width$}  {value}  (default: {})", s.key(), if default.is_empty() { "empty" } else { &default });
        }
    }
}

fn list(args: ListArgs) -> i32 {
    let cfg = super::load_config();
    let mut all = settings();
    if let Some(tab) = &args.tab {
        let want = setting_keys::normalize(tab);
        all.retain(|s| setting_keys::normalize(&s.group) == want);
        if all.is_empty() {
            let mut tabs: Vec<String> = settings().into_iter().map(|s| setting_keys::slug(&s.group)).collect();
            tabs.dedup();
            super::fail(format!("there is no “{tab}” tab, use one of {}", tabs.join(", ")));
        }
    }
    print_list(&cfg, &all, args.json, args.reveal);
    0
}

pub fn find(key: &str) -> Setting {
    let s = setting_keys::find(key).unwrap_or_else(|e| super::fail(e));
    if s.is_provider() {
        super::fail(format!("{} is a provider setting, use lyrminal providers", s.key()));
    }
    s
}

fn get(args: GetArgs) -> i32 {
    let s = find(&args.key);
    println!("{}", setting_keys::value(&super::load_config(), &s.item, args.reveal));
    0
}

pub fn change(s: &Setting, f: impl FnOnce(&mut Config) -> Result<(), String>) {
    let mut cfg = super::load_config();
    f(&mut cfg).unwrap_or_else(|e| super::fail(e));
    super::save_config(&cfg);
    println!("{} = {}", s.key(), setting_keys::value(&cfg, &s.item, false));
}

fn set(args: SetArgs) -> i32 {
    let s = find(&args.key);
    let value = super::read_value(args.value, s.key());
    change(&s, |cfg| setting_keys::apply(cfg, &s.item, &value));
    0
}

pub fn change_many(list: &[Setting], f: impl Fn(&mut Config, &Setting) -> Result<(), String>) {
    let mut cfg = super::load_config();
    for s in list {
        f(&mut cfg, s).unwrap_or_else(|e| super::fail(e));
    }
    super::save_config(&cfg);
    for s in list {
        println!("{} = {}", s.key(), setting_keys::value(&cfg, &s.item, false));
    }
}

fn toggle(args: KeysArgs) -> i32 {
    let list: Vec<Setting> = super::read_list(args.keys, "setting key").iter().map(|k| find(k)).collect();
    if let Some(s) = list.iter().find(|s| !matches!(s.item.kind, crate::ui::settings::Kind::Toggle { .. })) {
        super::fail(format!("{} is not an on/off setting", s.key()));
    }
    change_many(&list, |cfg, s| setting_keys::apply(cfg, &s.item, "toggle"));
    0
}

pub fn run(args: SettingsArgs) -> i32 {
    match args.command {
        SettingsCommand::List(a) => list(a),
        SettingsCommand::Get(a) => get(a),
        SettingsCommand::Set(a) => set(a),
        SettingsCommand::Toggle(a) => toggle(a),
    }
}
