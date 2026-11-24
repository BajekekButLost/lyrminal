use clap::{Args, Subcommand};
use serde_json::json;

use crate::config::Config;
use crate::providers::{KNOWN_PROVIDERS, ProviderInfo, ordered_ids, provider_name};
use crate::setting_keys::normalize;

#[derive(Args, Debug)]
pub struct ProvidersArgs {
    #[command(subcommand)]
    pub command: ProvidersCommand,
}

#[derive(Subcommand, Debug)]
pub enum ProvidersCommand {
    #[command(about = "List the providers in priority order and whether they are on")]
    List(ListArgs),
    #[command(about = "Turn providers on; ids can also come from stdin")]
    Enable(IdsArgs),
    #[command(about = "Turn providers off; ids can also come from stdin")]
    Disable(IdsArgs),
    #[command(about = "Change a provider's priority; the place can also come from stdin")]
    Move(MoveArgs),
}

#[derive(Args, Debug)]
pub struct MoveArgs {
    #[arg(help = "Provider id or name")]
    pub provider: String,
    #[arg(help = "A position (1 is tried first), up, down, top or bottom; leave it out or use - to read it from stdin")]
    pub to: Option<String>,
}

#[derive(Args, Debug)]
pub struct IdsArgs {
    #[arg(help = "Provider ids or names; leave them out or use - to read them from stdin, one per line")]
    pub providers: Vec<String>,
}

#[derive(Args, Debug)]
pub struct ListArgs {
    #[arg(long, help = "Print the providers as JSON")]
    pub json: bool,
}

fn plain(s: &str) -> String {
    normalize(s).replace(['.', '(', ')'], "")
}

pub fn json_name(id: &str) -> String {
    let mut out = String::new();
    for (i, part) in id.split('-').enumerate() {
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            if i == 0 {
                out.push(first);
            } else {
                out.extend(first.to_uppercase());
            }
            out.push_str(chars.as_str());
        }
    }
    out
}

pub fn resolve(name: &str) -> Result<&'static ProviderInfo, String> {
    let want = plain(name);
    let names = |p: &ProviderInfo| [plain(p.id), plain(p.name), plain(&json_name(p.id))];
    let prefixed: Vec<&'static ProviderInfo> = KNOWN_PROVIDERS
        .iter()
        .filter(|p| !want.is_empty() && names(p).iter().any(|n| n.starts_with(&want)))
        .collect();
    KNOWN_PROVIDERS
        .iter()
        .find(|p| names(p).contains(&want))
        .or(match prefixed.as_slice() {
            [p] => Some(*p),
            _ => None,
        })
        .ok_or_else(|| {
            format!(
                "unknown provider “{name}”, use one of {}",
                KNOWN_PROVIDERS.iter().map(|p| p.id).collect::<Vec<_>>().join(", ")
            )
        })
}

pub fn provider(name: &str) -> &'static ProviderInfo {
    resolve(name).unwrap_or_else(|e| super::fail(e))
}

fn print_order(cfg: &Config, json: bool) {
    let ids = ordered_ids(cfg);
    if json {
        let rows: Vec<_> = ids
            .iter()
            .enumerate()
            .map(|(i, id)| {
                json!({
                    "position": i + 1,
                    "id": id,
                    "name": provider_name(id),
                    "enabled": cfg.providers.is_enabled(id),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows).unwrap_or_default());
        return;
    }
    let width = ids.iter().map(|id| id.len()).max().unwrap_or(0);
    let names = ids.iter().map(|id| provider_name(id).len()).max().unwrap_or(0);
    for (i, id) in ids.iter().enumerate() {
        let state = if cfg.providers.is_enabled(id) { "on" } else { "off" };
        println!("{:>2}  {id:<width$}  {:<names$}  {state}", i + 1, provider_name(id));
    }
}

fn list(args: ListArgs) -> i32 {
    print_order(&super::load_config(), args.json);
    0
}

fn providers(args: IdsArgs) -> Vec<&'static ProviderInfo> {
    super::read_list(args.providers, "provider").iter().map(|p| provider(p)).collect()
}

fn set_enabled(args: IdsArgs, on: bool) -> i32 {
    let list = providers(args);
    let mut cfg = super::load_config();
    for p in &list {
        cfg.providers.set_enabled(p.id, on);
    }
    super::save_config(&cfg);
    for p in &list {
        println!("{} = {}", p.id, if on { "on" } else { "off" });
    }
    0
}

pub fn target_position(to: &str, current: usize, count: usize) -> Result<usize, String> {
    let last = count.saturating_sub(1);
    match to.trim().to_lowercase().as_str() {
        "up" => Ok(current.saturating_sub(1)),
        "down" => Ok((current + 1).min(last)),
        "top" | "first" => Ok(0),
        "bottom" | "last" => Ok(last),
        n => match n.parse::<usize>() {
            Ok(n) if (1..=count).contains(&n) => Ok(n - 1),
            _ => Err(format!("“{to}” is not a position from 1 to {count}, up, down, top or bottom")),
        },
    }
}

fn move_provider(args: MoveArgs) -> i32 {
    let p = provider(&args.provider);
    let to = super::read_value(args.to, "move");
    let mut cfg = super::load_config();
    let mut ids = ordered_ids(&cfg);
    let current = ids.iter().position(|id| id == p.id).unwrap_or(0);
    let target = target_position(&to, current, ids.len()).unwrap_or_else(|e| super::fail(e));
    let id = ids.remove(current);
    ids.insert(target, id);
    cfg.providers.order = ids;
    super::save_config(&cfg);
    print_order(&cfg, false);
    0
}

pub fn run(args: ProvidersArgs) -> i32 {
    match args.command {
        ProvidersCommand::List(a) => list(a),
        ProvidersCommand::Enable(a) => set_enabled(a, true),
        ProvidersCommand::Disable(a) => set_enabled(a, false),
        ProvidersCommand::Move(a) => move_provider(a),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_providers_by_id_name_or_config_name() {
        assert_eq!(resolve("lrc.red").unwrap().id, "lrc-red");
        assert_eq!(resolve("lrcRed").unwrap().id, "lrc-red");
        assert_eq!(resolve("Spicy Lyrics").unwrap().id, "spicy-lyrics");
        assert_eq!(resolve("LRCLIB").unwrap().id, "lrclib");
        assert_eq!(resolve("better-lyrics").unwrap().id, "unison");
        assert!(resolve("genius").is_err());
        assert_eq!(resolve("spicy").unwrap().id, "spicy-lyrics");
        assert!(resolve("lrc").is_err());
        assert_eq!(json_name("local-library"), "localLibrary");
        assert_eq!(json_name("lrcmux"), "lrcmux");
    }

    #[test]
    fn works_out_where_a_provider_moves() {
        assert_eq!(target_position("up", 0, 6), Ok(0));
        assert_eq!(target_position("down", 5, 6), Ok(5));
        assert_eq!(target_position("Top", 3, 6), Ok(0));
        assert_eq!(target_position("bottom", 0, 6), Ok(5));
        assert_eq!(target_position("2", 4, 6), Ok(1));
        assert!(target_position("7", 0, 6).is_err());
        assert!(target_position("0", 0, 6).is_err());
    }
}
