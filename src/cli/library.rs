use clap::{Args, Subcommand};
use serde_json::json;

use crate::library::{Entry, Library};
use crate::setting_keys::normalize;

#[derive(Args, Debug)]
pub struct LibraryArgs {
    #[command(subcommand)]
    pub command: LibraryCommand,
}

#[derive(Subcommand, Debug)]
pub enum LibraryCommand {
    #[command(about = "List the syncs saved in the local library")]
    List(ListArgs),
}

#[derive(Args, Debug)]
pub struct ListArgs {
    #[arg(short, long, value_name = "TEXT", help = "Only list syncs whose title, artist or album contains TEXT")]
    pub search: Option<String>,
    #[arg(long, help = "Print the syncs as JSON")]
    pub json: bool,
}

pub const SHORT_ID: usize = 8;

fn matches(entry: &Entry, search: &str) -> bool {
    let want = normalize(search);
    let t = &entry.track;
    [t.title.as_str(), &t.artist_line(), t.album.as_deref().unwrap_or("")]
        .iter()
        .any(|f| normalize(f).contains(&want))
}

pub fn entry_json(entry: &Entry) -> serde_json::Value {
    let mut v = serde_json::to_value(entry).unwrap_or_default();
    v["summary"] = json!(entry.summary());
    v
}

fn list(args: ListArgs) -> i32 {
    let mut entries = Library::default().entries();
    if let Some(s) = &args.search {
        entries.retain(|e| matches(e, s));
    }
    entries.sort_by_key(|e| (normalize(&e.track.artist_line()), normalize(&e.track.title)));
    if args.json {
        let rows: Vec<_> = entries.iter().map(entry_json).collect();
        println!("{}", serde_json::to_string_pretty(&rows).unwrap_or_default());
        return 0;
    }
    for e in &entries {
        let id: String = e.id.chars().take(SHORT_ID).collect();
        let artist = e.track.artist_line();
        let song = if artist.is_empty() { e.track.title.clone() } else { format!("{artist} — {}", e.track.title) };
        println!("{id}  {song}  ({})", e.summary());
    }
    0
}

pub async fn run(args: LibraryArgs) -> i32 {
    match args.command {
        LibraryCommand::List(a) => list(a),
    }
}
