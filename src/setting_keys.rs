use serde_json::Value;

use crate::config::{Config, Preset};
use crate::ui::settings::{self, Item, Kind};

pub struct Setting {
    pub group: String,
    pub item: Item,
}

impl Setting {
    pub fn key(&self) -> &'static str {
        self.item.key
    }

    pub fn is_provider(&self) -> bool {
        self.item.key.starts_with("providers.")
    }
}

pub fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(c, '-' | '_' | ' '))
        .flat_map(char::to_lowercase)
        .collect()
}

pub fn slug(s: &str) -> String {
    s.trim().to_lowercase().replace(' ', "-")
}

pub fn all() -> Vec<Setting> {
    settings::keyed_items()
        .into_iter()
        .map(|(group, item)| Setting { group, item })
        .collect()
}

pub fn find(key: &str) -> Result<Setting, String> {
    let want = normalize(key);
    let all = all();
    let last = want.rsplit('.').next().unwrap_or(&want).to_string();
    let mut similar: Vec<&'static str> = all
        .iter()
        .filter(|s| !last.is_empty() && normalize(s.key()).rsplit('.').next().is_some_and(|k| k.contains(&last) || last.contains(k)))
        .map(Setting::key)
        .collect();
    if let Some(s) = all.into_iter().find(|s| normalize(s.key()) == want) {
        return Ok(s);
    }
    similar.truncate(3);
    Err(if similar.is_empty() {
        format!("unknown setting “{key}”")
    } else {
        format!("unknown setting “{key}”, did you mean {}?", similar.join(" or "))
    })
}

pub fn parse_bool(v: &str) -> Option<bool> {
    match v.trim().to_lowercase().as_str() {
        "on" | "true" | "yes" | "y" | "1" | "enable" | "enabled" => Some(true),
        "off" | "false" | "no" | "n" | "0" | "disable" | "disabled" => Some(false),
        _ => None,
    }
}

pub fn choice_index(options: &[&str], value: &str) -> Result<usize, String> {
    let want = normalize(value);
    if let Some(i) = options.iter().position(|o| normalize(o) == want) {
        return Ok(i);
    }
    let prefixed: Vec<usize> = (0..options.len())
        .filter(|i| !want.is_empty() && normalize(options[*i]).starts_with(&want))
        .collect();
    match prefixed.as_slice() {
        [i] => Ok(*i),
        _ => Err(format!(
            "“{value}” is not one of {}",
            options.iter().map(|o| slug(o)).collect::<Vec<_>>().join(", ")
        )),
    }
}

fn preset_labels() -> Vec<&'static str> {
    Preset::ALL.iter().map(|p| p.label()).collect()
}

pub fn apply(cfg: &mut Config, item: &Item, value: &str) -> Result<(), String> {
    match &item.kind {
        Kind::Toggle { get, set } => {
            let v = if value.trim().eq_ignore_ascii_case("toggle") {
                !get(cfg)
            } else {
                parse_bool(value).ok_or_else(|| format!("“{value}” is not on or off"))?
            };
            set(cfg, v);
        }
        Kind::Choice { options, set, .. } => set(cfg, choice_index(options, value)?),
        Kind::Number {
            min,
            max,
            decimals,
            unit,
            set,
            ..
        } => {
            let text = value.trim();
            let text = text.strip_suffix(unit).unwrap_or(text).trim();
            let n: f64 = text.parse().map_err(|_| format!("“{value}” is not a number"))?;
            if !n.is_finite() || n < *min || n > *max {
                return Err(format!("{} must be between {}", item.label, range(*min, *max, *decimals, unit)));
            }
            let scale = 10f64.powi(*decimals as i32);
            set(cfg, (n * scale).round() / scale);
        }
        Kind::Text { set, .. } => set(cfg, value.to_string()),
        Kind::Preset { .. } => Preset::ALL[choice_index(&preset_labels(), value)?].apply(cfg),
        _ => return Err(format!("{} can't be set", item.label)),
    }
    Ok(())
}

fn number(v: f64, decimals: usize) -> String {
    format!("{v:.decimals$}")
}

fn range(min: f64, max: f64, decimals: usize, unit: &str) -> String {
    let r = format!("{} and {}", number(min, decimals), number(max, decimals));
    if unit.is_empty() { r } else { format!("{r} {unit}") }
}

pub fn mask(v: &str) -> String {
    if v.is_empty() {
        return String::new();
    }
    let tail: String = v.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    format!("••••••{tail}")
}

pub fn value(cfg: &Config, item: &Item, reveal: bool) -> String {
    match &item.kind {
        Kind::Toggle { get, .. } => if get(cfg) { "on" } else { "off" }.into(),
        Kind::Choice { options, get, .. } => options.get(get(cfg)).map(|o| slug(o)).unwrap_or_default(),
        Kind::Number { get, decimals, .. } => number(get(cfg), *decimals),
        Kind::Text { get, secret, .. } => {
            let v = get(cfg);
            if *secret && !reveal { mask(&v) } else { v }
        }
        Kind::Preset { .. } => Preset::detect(cfg, true).map(|p| slug(p.label())).unwrap_or_else(|| "custom".into()),
        _ => String::new(),
    }
}

pub fn json_value(cfg: &Config, item: &Item, reveal: bool) -> Value {
    match &item.kind {
        Kind::Toggle { get, .. } => Value::Bool(get(cfg)),
        Kind::Number { get, decimals, .. } => {
            let scale = 10f64.powi(*decimals as i32);
            serde_json::Number::from_f64((get(cfg) * scale).round() / scale)
                .map(|n| {
                    if *decimals == 0 {
                        Value::from(n.as_f64().unwrap_or(0.0) as i64)
                    } else {
                        Value::Number(n)
                    }
                })
                .unwrap_or(Value::Null)
        }
        _ => Value::String(value(cfg, item, reveal)),
    }
}

pub fn allowed(item: &Item) -> String {
    match &item.kind {
        Kind::Toggle { .. } => "on, off".into(),
        Kind::Choice { options, .. } => options.iter().map(|o| slug(o)).collect::<Vec<_>>().join(", "),
        Kind::Number {
            min,
            max,
            step,
            decimals,
            unit,
            ..
        } => format!("{}, in steps of {} in the app", range(*min, *max, *decimals, unit), number(*step, *decimals)),
        Kind::Text { folder: true, .. } => "a folder".into(),
        Kind::Text { secret: true, .. } => "a secret key".into(),
        Kind::Text { .. } => "any text".into(),
        Kind::Preset { .. } => preset_labels().iter().map(|l| slug(l)).collect::<Vec<_>>().join(", "),
        _ => String::new(),
    }
}

pub fn kind_name(item: &Item) -> &'static str {
    match &item.kind {
        Kind::Toggle { .. } => "on/off",
        Kind::Choice { .. } | Kind::Preset { .. } => "choice",
        Kind::Number { .. } => "number",
        Kind::Text { .. } => "text",
        _ => "",
    }
}

pub fn reset(cfg: &mut Config, item: &Item) {
    let d = Config::default();
    match &item.kind {
        Kind::Toggle { get, set } => set(cfg, get(&d)),
        Kind::Choice { get, set, .. } => set(cfg, get(&d)),
        Kind::Number { get, set, .. } => set(cfg, get(&d)),
        Kind::Text { get, set, .. } => set(cfg, get(&d)),
        Kind::Preset { .. } => {
            if let Some(p) = Preset::detect(&d, true) {
                p.apply(cfg)
            }
        }
        _ => {}
    }
}

pub fn is_default(cfg: &Config, item: &Item) -> bool {
    value(cfg, item, true) == value(&Config::default(), item, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(v: &Value, key: &str) -> Option<Value> {
        key.split('.').try_fold(v.clone(), |v, part| v.get(part).cloned())
    }

    #[test]
    fn every_key_is_unique_and_points_into_the_config() {
        let all = all();
        let json = serde_json::to_value(Config::default()).unwrap();
        let mut seen = std::collections::HashSet::new();
        for s in &all {
            assert!(seen.insert(normalize(s.key())), "duplicate key {}", s.key());
            let virtual_key = s.key() == settings::PRESET_KEY || s.key().ends_with(".enabled") && s.is_provider();
            if !virtual_key {
                assert!(lookup(&json, s.key()).is_some(), "{} is not in the config", s.key());
            }
        }
        assert!(all.iter().any(|s| s.key() == "appearance.fontScale"));
        assert!(all.iter().any(|s| s.key() == "providers.spicyLyrics.apiKey"));
    }

    #[test]
    fn sets_values_like_the_app() {
        let mut cfg = Config::default();
        let set = |cfg: &mut Config, key: &str, v: &str| apply(cfg, &find(key).unwrap().item, v);
        set(&mut cfg, "appearance.font-scale", "3.14").unwrap();
        assert_eq!(cfg.appearance.font_scale, 3.1);
        assert!(set(&mut cfg, "appearance.fontScale", "9").is_err());
        set(&mut cfg, "APPEARANCE.RENDERER", "cell").unwrap();
        assert_eq!(cfg.appearance.renderer, crate::config::Renderer::Cell);
        set(&mut cfg, "player.mode", "fixed").unwrap();
        assert_eq!(cfg.player.mode, crate::config::PlayerMode::Fixed);
        assert_eq!(value(&cfg, &find("player.mode").unwrap().item, false), "fixed-process");
        set(&mut cfg, "sync.offsetMs", "-120 ms").unwrap();
        assert_eq!(cfg.sync.offset_ms, -120);
        set(&mut cfg, "lyrics.translation", "yes").unwrap();
        assert!(cfg.lyrics.translation);
        set(&mut cfg, "lyrics.translation", "toggle").unwrap();
        assert!(!cfg.lyrics.translation);
        set(&mut cfg, "appearance.preset", "spicy-lyrics").unwrap();
        assert_eq!(Preset::detect(&cfg, true), Some(Preset::SpicyLyrics));
        set(&mut cfg, "player.preferred", "a, b").unwrap();
        assert_eq!(cfg.player.preferred, vec!["a", "b"]);
        assert!(set(&mut cfg, "appearance.style", "s").is_ok());
        assert!(choice_index(&["staggered", "smooth"], "s").is_err());
        assert!(find("appearance.fontScal").err().unwrap().contains("appearance.fontScale"));
    }
}
