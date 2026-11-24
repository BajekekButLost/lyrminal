use clap::{Args, ValueEnum};
use serde_json::Value;

use crate::ipc::{self, Request};

#[derive(Args, Debug)]
pub struct NowPlayingArgs {}

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

pub fn render(data: &Value, fields: &[Field], separator: &str) -> String {
    fields.iter().map(|f| format(data, *f)).collect::<Vec<_>>().join(separator)
}

pub async fn run(_args: NowPlayingArgs) -> i32 {
    match ipc::call(&Request::NowPlaying).await {
        Ok(data) => {
            println!("{}", render(&data, COMPACT, " · "));
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
            render(&data, COMPACT, " · "),
            "playing · Childish Gambino · III. Telegraph Ave. · 1:23/1:02:05 · Oakland, Oakland"
        );
        assert_eq!(format(&data, Field::Progress), "43%");
        assert_eq!(format(&data, Field::Line), "12");
        assert_eq!(format(&data, Field::Album), "");
    }
}
