use clap::{Args, ValueEnum};
use serde_json::Value;

use crate::ipc::{self, Request};

#[derive(Args, Debug)]
pub struct NowPlayingArgs {
    #[arg(short, long, value_name = "TEXT", help = "Put TEXT between the fields instead of \" · \"")]
    pub separator: Option<String>,
    #[arg(
        short,
        long,
        value_enum,
        value_delimiter = ',',
        value_name = "FIELD,...",
        help = "Print these fields, in this order"
    )]
    pub fields: Vec<Field>,
    #[arg(long, conflicts_with = "fields", help = "Print every field: album, line numbers, progress, word, sync and provider too")]
    pub full: bool,
    #[arg(long, help = "Print unformatted values (seconds, progress from 0 to 1) separated by tabs")]
    pub raw: bool,
    #[arg(long, conflicts_with = "raw", help = "Print a JSON object with every field, or only those given with --fields")]
    pub json: bool,
    #[arg(long, help = "Print nothing and exit with code 3 when nothing is playing or there are no lyrics")]
    pub strict: bool,
    #[arg(short, long, help = "Keep running and print a new line whenever the track, status, line or word changes")]
    pub watch: bool,
    #[arg(
        long,
        requires = "watch",
        value_name = "MS",
        value_parser = clap::value_parser!(u64).range(50..),
        help = "With --watch, also print every MS milliseconds, e.g. to keep the position up to date"
    )]
    pub interval: Option<u64>,
}

pub const EMPTY_EXIT: i32 = 3;

pub fn empty_reason(data: &Value) -> Option<String> {
    let status = data["status"].as_str().unwrap_or("");
    if !matches!(status, "playing" | "paused") {
        return Some(if status.is_empty() { "nothing is playing".into() } else { format!("nothing is playing ({status})") });
    }
    match data["lyrics"].as_str().unwrap_or("") {
        "ready" => None,
        state => Some(format!("there are no lyrics ({state})")),
    }
}

impl NowPlayingArgs {
    fn fields(&self) -> Vec<Field> {
        if self.full {
            FULL.to_vec()
        } else if self.fields.is_empty() {
            COMPACT.to_vec()
        } else {
            self.fields.clone()
        }
    }
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Status,
    Player,
    Artist,
    Title,
    Album,
    Time,
    Position,
    Length,
    Line,
    Lines,
    Text,
    Progress,
    Word,
    Next,
    Sync,
    Provider,
    Source,
    Lyrics,
}

pub const COMPACT: &[Field] = &[Field::Status, Field::Artist, Field::Title, Field::Time, Field::Text];
pub const FULL: &[Field] = &[
    Field::Status,
    Field::Artist,
    Field::Title,
    Field::Album,
    Field::Position,
    Field::Length,
    Field::Line,
    Field::Lines,
    Field::Text,
    Field::Progress,
    Field::Word,
    Field::Sync,
    Field::Provider,
];

impl Field {
    pub fn key(self) -> &'static str {
        match self {
            Field::Status => "status",
            Field::Player => "player",
            Field::Artist => "artist",
            Field::Title => "title",
            Field::Album => "album",
            Field::Time => "time",
            Field::Position => "position",
            Field::Length => "length",
            Field::Line => "line",
            Field::Lines => "lines",
            Field::Text => "text",
            Field::Progress => "progress",
            Field::Word => "word",
            Field::Next => "next",
            Field::Sync => "sync",
            Field::Provider => "provider",
            Field::Source => "source",
            Field::Lyrics => "lyrics",
        }
    }
}

pub fn clock(secs: f64) -> String {
    let total = secs.max(0.0) as u64;
    let (h, m, s) = (total / 3600, total / 60 % 60, total % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

fn plain(v: &Value) -> String {
    match v {
        Value::String(s) => super::printable(s),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

pub fn format(data: &Value, field: Field) -> String {
    let num = |k: &str| data[k].as_f64().unwrap_or(0.0);
    match field {
        Field::Time => format!("{}/{}", clock(num("position")), clock(num("length"))),
        Field::Position | Field::Length => clock(num(field.key())),
        Field::Progress => format!("{}%", (num("progress") * 100.0).round() as i64),
        f => plain(&data[f.key()]),
    }
}

pub fn raw(data: &Value, field: Field) -> String {
    match field {
        Field::Time => format!("{}/{}", plain(&data["position"]), plain(&data["length"])),
        f => plain(&data[f.key()]),
    }
}

pub fn render(data: &Value, fields: &[Field], separator: &str, unformatted: bool) -> String {
    fields
        .iter()
        .map(|f| if unformatted { raw(data, *f) } else { format(data, *f) })
        .collect::<Vec<_>>()
        .join(separator)
}

pub fn to_json(data: &Value, fields: &[Field]) -> Value {
    let mut data = data.clone();
    if let Some(map) = data.as_object_mut() {
        map.remove("ok");
        if !fields.is_empty() {
            let keep: Vec<&str> = fields
                .iter()
                .flat_map(|f| match f {
                    Field::Time => vec!["position", "length"],
                    f => vec![f.key()],
                })
                .collect();
            map.retain(|k, _| keep.contains(&k.as_str()));
        }
    }
    data
}

impl NowPlayingArgs {
    fn line(&self, data: &Value) -> String {
        if self.json {
            let fields = if self.full { FULL.to_vec() } else { self.fields.clone() };
            return to_json(data, &fields).to_string();
        }
        let separator = self.separator.as_deref().unwrap_or(if self.raw { "\t" } else { " · " });
        render(data, &self.fields(), separator, self.raw)
    }
}

fn steady(data: &Value) -> Value {
    let mut data = data.clone();
    for key in ["position", "progress", "offset"] {
        data[key] = Value::from(0);
    }
    data
}

pub const EVENT_KEYS: &[&str] = &["status", "player", "title", "artist", "album", "lyrics", "line", "word", "interlude"];

pub fn event(data: &Value) -> String {
    EVENT_KEYS.iter().map(|k| data[*k].to_string()).collect::<Vec<_>>().join("\u{1f}")
}

impl NowPlayingArgs {
    fn shown(&self, data: &Value) -> String {
        if self.strict && empty_reason(data).is_some() { String::new() } else { self.line(data) }
    }

    fn change_key(&self, data: &Value) -> String {
        format!("{}\u{1e}{}", event(data), self.shown(&steady(data)))
    }
}

async fn watch(args: NowPlayingArgs) -> i32 {
    let interval = args.interval.map(std::time::Duration::from_millis);
    let poll = interval.map_or(POLL, |i| i.min(POLL));
    let path = crate::config::socket_path();
    let mut client: Option<ipc::Client> = None;
    let mut last: Option<String> = None;
    let mut printed_at = std::time::Instant::now();
    loop {
        if client.is_none() {
            client = ipc::Client::connect(&path).await.ok();
        }
        let answer = match client.as_mut() {
            Some(c) => c.call(&Request::NowPlaying).await,
            None => Err(ipc::NOT_RUNNING.to_string()),
        };
        let (line, key) = match answer {
            Ok(data) => (args.shown(&data), args.change_key(&data)),
            Err(_) => {
                client = None;
                (String::new(), String::new())
            }
        };
        let due = interval.is_some_and(|i| printed_at.elapsed() >= i);
        if last.as_ref() != Some(&key) || due {
            println!("{line}");
            last = Some(key);
            printed_at = std::time::Instant::now();
        }
        tokio::time::sleep(poll).await;
    }
}

const POLL: std::time::Duration = std::time::Duration::from_millis(100);

pub async fn run(args: NowPlayingArgs) -> i32 {
    if args.watch {
        return watch(args).await;
    }
    match ipc::call(&Request::NowPlaying).await {
        Ok(data) => {
            if args.strict
                && let Some(reason) = empty_reason(&data)
            {
                eprintln!("lyrminal: {reason}");
                return EMPTY_EXIT;
            }
            println!("{}", args.line(&data));
            0
        }
        Err(e) => {
            eprintln!("lyrminal: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn prints_the_compact_line() {
        let data = json!({
            "status": "playing", "artist": "Childish Gambino", "title": "III. Telegraph Ave.",
            "position": 83.4, "length": 3725.0, "text": "Oakland, Oakland", "progress": 0.426, "line": 12
        });
        assert_eq!(
            render(&data, COMPACT, " · ", false),
            "playing · Childish Gambino · III. Telegraph Ave. · 1:23/1:02:05 · Oakland, Oakland"
        );
        assert_eq!(
            render(&data, &[Field::Position, Field::Progress, Field::Time, Field::Text], "\t", true),
            "83.4\t0.426\t83.4/3725.0\tOakland, Oakland"
        );
        assert_eq!(format(&data, Field::Progress), "43%");
        assert_eq!(format(&data, Field::Line), "12");
        assert_eq!(format(&data, Field::Album), "");
        let evil = json!({"title": "Song\u{1b}[2J\u{9b}31m\tB\nC"});
        assert_eq!(format(&evil, Field::Title), "Song[2J31m B C");
        assert_eq!(raw(&evil, Field::Title), "Song[2J31m B C");
        assert_eq!(to_json(&evil, &[Field::Title])["title"], "Song\u{1b}[2J\u{9b}31m\tB\nC");
        let picked = to_json(&json!({"ok": true, "title": "T", "position": 1.0, "length": 2.0, "text": "x"}), &[Field::Time, Field::Title]);
        assert_eq!(picked, json!({"title": "T", "position": 1.0, "length": 2.0}));
        assert_eq!(to_json(&json!({"ok": true, "text": "x"}), &[]), json!({"text": "x"}));
        assert_eq!(empty_reason(&json!({"status": "playing", "lyrics": "ready"})), None);
        assert!(empty_reason(&json!({"status": "paused", "lyrics": "not found"})).is_some());
        assert!(empty_reason(&json!({"status": "no player", "lyrics": "no player"})).is_some());
    }

    #[test]
    fn watch_notices_line_and_word_changes_whatever_is_printed() {
        let a = json!({"status": "playing", "title": "T", "line": 1, "word": "one", "position": 1.0});
        let b = json!({"status": "playing", "title": "T", "line": 1, "word": "two", "position": 1.2});
        let c = json!({"status": "playing", "title": "T", "line": 1, "word": "two", "position": 1.4});
        assert_ne!(event(&a), event(&b));
        assert_eq!(event(&b), event(&c));
    }
}
