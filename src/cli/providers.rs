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

pub fn run(args: ProvidersArgs) -> i32 {
    match args.command {
        ProvidersCommand::List(a) => list(a),
        ProvidersCommand::Enable(a) => set_enabled(a, true),
        ProvidersCommand::Disable(a) => set_enabled(a, false),
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
}
