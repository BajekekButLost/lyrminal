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
        Value::String(s) => s.clone(),
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
    let text = match field {
        Field::Time => format!("{}/{}", plain(&data["position"]), plain(&data["length"])),
        f => plain(&data[f.key()]),
    };
    text.replace(['\t', '\n', '\r'], " ")
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

pub async fn run(args: NowPlayingArgs) -> i32 {
    match ipc::call(&Request::NowPlaying).await {
        Ok(data) => {
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
        let picked = to_json(&json!({"ok": true, "title": "T", "position": 1.0, "length": 2.0, "text": "x"}), &[Field::Time, Field::Title]);
        assert_eq!(picked, json!({"title": "T", "position": 1.0, "length": 2.0}));
        assert_eq!(to_json(&json!({"ok": true, "text": "x"}), &[]), json!({"text": "x"}));
    }
}
