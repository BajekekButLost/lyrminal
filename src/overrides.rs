use std::sync::OnceLock;

use crate::config::Config;
use crate::setting_keys::{self, normalize};
use crate::ui::settings::{Item, Kind, PRESET_KEY};

pub const ORDER_KEY: &str = "providers.order";

pub const PRESET_KEYS: &[&str] = &[
    "appearance.renderer",
    "appearance.style",
    "appearance.albumTheming",
    "appearance.background",
    "appearance.albumArt",
    "appearance.showHeader",
    "appearance.alignment",
    "appearance.lineGap",
    "appearance.depthDimming",
    "appearance.fadePrevious",
    "appearance.contrast",
    "appearance.fontWeight",
    "appearance.fontScale",
    "appearance.glow",
    "appearance.emphasis",
    "appearance.syllableSpacing",
    "scroll.animation",
    "scroll.speed",
    "scroll.leadMs",
    "lyrics.backgroundVocals",
    "lyrics.interludes",
    "lyrics.interludeThresholdSecs",
    "lyrics.reserveInterludeSpace",
    "lyrics.joinSplitWords",
];

#[derive(Debug, Clone, PartialEq)]
pub struct Override {
    pub key: String,
    pub value: String,
    pub flag: String,
}

static OVERRIDES: OnceLock<Vec<Override>> = OnceLock::new();

pub fn parse(pair: &str, flag: &str) -> Result<Override, String> {
    let (key, value) = pair.split_once('=').ok_or_else(|| format!("{flag} needs KEY=VALUE, e.g. appearance.style=bloom"))?;
    make(key.trim(), value.trim(), flag)
}

pub fn make(key: &str, value: &str, flag: &str) -> Result<Override, String> {
    let key = if normalize(key) == normalize(ORDER_KEY) {
        crate::cli::providers::order_from(&[value.to_string()])?;
        ORDER_KEY.to_string()
    } else {
        let s = setting_keys::find(key)?;
        setting_keys::apply(&mut Config::default(), &s.item, value).map_err(|e| format!("{flag}: {e}"))?;
        s.key().to_string()
    };
    Ok(Override {
        key,
        value: value.to_string(),
        flag: flag.to_string(),
    })
}

pub fn set(list: Vec<Override>) {
    let _ = OVERRIDES.set(list);
}

pub fn all() -> &'static [Override] {
    OVERRIDES.get().map(Vec::as_slice).unwrap_or(&[])
}

pub fn apply_list(list: &[Override], cfg: &mut Config) {
    let presets = list.iter().filter(|o| o.key == PRESET_KEY);
    for o in presets.chain(list.iter().filter(|o| o.key != PRESET_KEY)) {
        if o.key == ORDER_KEY {
            if let Ok(ids) = crate::cli::providers::order_from(std::slice::from_ref(&o.value)) {
                cfg.providers.order = ids;
                cfg.providers.order = crate::providers::ordered_ids(cfg);
            }
        } else if let Ok(s) = setting_keys::find(&o.key) {
            let _ = setting_keys::apply(cfg, &s.item, &o.value);
        }
    }
}

pub fn apply(cfg: &mut Config) {
    apply_list(all(), cfg);
}

fn lock_in<'a>(list: &'a [Override], key: &str) -> Option<&'a Override> {
    list.iter().rev().find(|o| o.key == key).or_else(|| {
        PRESET_KEYS
            .contains(&key)
            .then(|| list.iter().rev().find(|o| o.key == PRESET_KEY))
            .flatten()
    })
}

pub fn lock_of(key: &str) -> Option<&'static Override> {
    lock_in(all(), key)
}

pub fn enabled_lock(id: &str) -> Option<&'static Override> {
    lock_of(&format!("providers.{}.enabled", crate::cli::providers::json_name(id)))
}

pub fn order_lock() -> Option<&'static Override> {
    lock_of(ORDER_KEY)
}

pub fn item_lock(item: &Item) -> Option<&'static Override> {
    match &item.kind {
        Kind::Provider(id) => enabled_lock(id).or_else(order_lock),
        _ if item.key.is_empty() => None,
        _ => lock_of(item.key),
    }
}

fn copy(key: &str, from: &Config, into: &mut Config) {
    let Ok(s) = setting_keys::find(key) else { return };
    match &s.item.kind {
        Kind::Toggle { get, set } => set(into, get(from)),
        Kind::Choice { get, set, .. } => set(into, get(from)),
        Kind::Number { get, set, .. } => set(into, get(from)),
        Kind::Text { get, set, .. } => set(into, get(from)),
        _ => {}
    }
}

pub fn persisted_with(list: &[Override], effective: &Config, base: &Config) -> Config {
    let mut out = effective.clone();
    for o in list {
        match o.key.as_str() {
            ORDER_KEY => out.providers.order = base.providers.order.clone(),
            PRESET_KEY => {
                for k in PRESET_KEYS {
                    copy(k, base, &mut out);
                }
            }
            k => copy(k, base, &mut out),
        }
    }
    out
}

pub fn persisted(effective: &Config, base: &Config) -> Config {
    persisted_with(all(), effective, base)
}

pub fn relaunch_args() -> Vec<std::ffi::OsString> {
    all()
        .iter()
        .flat_map(|o| ["--set".into(), format!("{}={}", o.key, o.value).into()])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrides_apply_lock_and_stay_out_of_the_file() {
        let list = vec![
            parse("appearance.style=bloom", "--set").unwrap(),
            parse("providers.order=lrclib,spicy", "--set").unwrap(),
            make("appearance.preset", "spicy-lyrics", "--preset").unwrap(),
        ];
        assert!(parse("appearance.style", "--set").is_err());
        assert!(parse("appearance.fontScale=99", "--set").is_err());
        assert!(parse("nope=1", "--set").is_err());
        let mut base = Config::default();
        base.sync.offset_ms = 40;
        let mut cfg = base.clone();
        apply_list(&list, &mut cfg);
        assert_eq!(cfg.appearance.style, crate::config::VisualStyle::Bloom);
        assert_eq!(crate::providers::ordered_ids(&cfg)[1], "lrclib");
        assert!(cfg.appearance.glow);
        assert_eq!(lock_in(&list, "appearance.glow").map(|o| o.flag.as_str()), Some("--preset"));
        assert_eq!(lock_in(&list, "appearance.style").map(|o| o.flag.as_str()), Some("--set"));
        assert!(lock_in(&list, "sync.offsetMs").is_none());
        cfg.sync.offset_ms = 90;
        let saved = persisted_with(&list, &cfg, &base);
        assert_eq!(saved.appearance.style, base.appearance.style);
        assert_eq!(saved.appearance.glow, base.appearance.glow);
        assert_eq!(saved.providers.order, base.providers.order);
        assert_eq!(saved.sync.offset_ms, 90);
    }
}
