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
    #[command(about = "Show everything about a saved sync, or print its saved copy")]
    Show(ShowArgs),
}

#[derive(Args, Debug)]
pub struct ShowArgs {
    #[arg(help = "The sync's id, or the start of it (see library list)")]
    pub id: String,
    #[arg(long, conflicts_with = "json", help = "Print the saved copy of the lyrics file instead")]
    pub content: bool,
    #[arg(long, help = "Print the details as JSON")]
    pub json: bool,
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

pub fn find_in(entries: &[Entry], id: &str) -> Result<Entry, String> {
    let want = id.trim().to_lowercase();
    if want.is_empty() {
        return Err("give the id of a sync (see lyrminal library list)".into());
    }
    if let Some(e) = entries.iter().find(|e| e.id == want) {
        return Ok(e.clone());
    }
    let found: Vec<&Entry> = entries.iter().filter(|e| e.id.starts_with(&want)).collect();
    match found.as_slice() {
        [e] => Ok((*e).clone()),
        [] => Err(format!("there is no sync with the id {id} in the library")),
        _ => Err(format!("{id} matches {} syncs, give more of the id", found.len())),
    }
}

pub fn find(lib: &Library, id: &str) -> Entry {
    find_in(&lib.entries(), id).unwrap_or_else(|e| super::fail(e))
}

pub fn date(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let rest = secs % 86_400;
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02} UTC", rest / 3600, rest / 60 % 60)
}

fn show(args: ShowArgs) -> i32 {
    let lib = Library::default();
    let e = find(&lib, &args.id);
    if args.content {
        match lib.read_copy(&e) {
            Ok(text) => {
                print!("{text}");
                if !text.ends_with('\n') {
                    println!();
                }
                return 0;
            }
            Err(err) => super::fail(format!("the saved copy is missing ({err})")),
        }
    }
    if args.json {
        println!("{}", serde_json::to_string_pretty(&entry_json(&e)).unwrap_or_default());
        return 0;
    }
    let t = &e.track;
    let or_none = |v: Option<String>| v.filter(|s| !s.is_empty()).unwrap_or_else(|| "-".into());
    let format = match (e.format, e.detected) {
        (Some(f), _) => format!("{} (chosen)", f.label()),
        (None, Some(f)) => format!("{} (detected)", f.label()),
        (None, None) => "-".into(),
    };
    let rows = [
        ("Title", t.title.clone()),
        ("Artist", or_none(Some(t.artist_line()))),
        ("Album", or_none(t.album.clone())),
        ("Length", or_none(t.duration.map(super::now_playing::clock))),
        ("Spotify id", or_none(t.spotify_id.clone())),
        ("ISRC", or_none(t.isrc.clone())),
        ("Source", format!("{} {}", e.source.label(), e.source.location().unwrap_or("")).trim().to_string()),
        ("Format", format),
        ("Sync", or_none(e.kind.map(|k| k.label().to_string()))),
        ("Checks for changes", if e.watch && e.source.can_watch() { "yes" } else { "no" }.into()),
        ("Saved copy", lib.dir().join("lyrics").join(&e.file).display().to_string()),
        ("Added", date(e.added_at)),
        ("Updated", date(e.updated_at)),
    ];
    println!("{}", e.id);
    for (name, value) in rows {
        println!("  {name:<18}  {value}");
    }
    0
}

pub async fn run(args: LibraryArgs) -> i32 {
    match args.command {
        LibraryCommand::List(a) => list(a),
        LibraryCommand::Show(a) => show(a),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_dates_and_finds_ids_by_prefix() {
        assert_eq!(date(0), "1970-01-01 00:00 UTC");
        assert_eq!(date(1_795_224_906), "2026-11-21 01:35 UTC");
        assert_eq!(date(951_782_400), "2000-02-29 00:00 UTC");
        let entry = |id: &str| Entry {
            id: id.into(),
            key: id.into(),
            track: Default::default(),
            source: crate::library::Source::Clipboard,
            format: None,
            detected: None,
            kind: None,
            watch: false,
            file: String::new(),
            added_at: 0,
            updated_at: 0,
        };
        let all = [entry("abc123"), entry("abd456")];
        assert_eq!(find_in(&all, "abc").unwrap().id, "abc123");
        assert!(find_in(&all, "ab").unwrap_err().contains("matches 2"));
        assert!(find_in(&all, "zz").is_err());
    }
}
