use clap::{Args, Subcommand};
use serde_json::json;

use crate::app::LibraryDone;
use crate::ipc::LibraryChange;
use crate::library::{Entry, Library, Source};
use crate::lyrics::formats::Format;
use crate::providers::TrackQuery;
use crate::setting_keys::normalize;
use crate::ui::library::ImportRequest;

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
    #[command(about = "Save a sync from a file, a URL or stdin; it is for the playing song unless you give --title")]
    Add(AddArgs),
}

#[derive(Args, Debug, Default)]
pub struct TrackArgs {
    #[arg(long, help = "The song's title")]
    pub title: Option<String>,
    #[arg(long = "artist", value_name = "NAME", help = "An artist of the song; repeat it for more")]
    pub artists: Vec<String>,
    #[arg(long, help = "The song's album")]
    pub album: Option<String>,
    #[arg(long, value_name = "LENGTH", value_parser = parse_length, help = "The song's length, in seconds or as m:ss")]
    pub duration: Option<f64>,
    #[arg(long, help = "The song's ISRC")]
    pub isrc: Option<String>,
    #[arg(long, value_name = "ID", help = "The song's Spotify track id")]
    pub spotify_id: Option<String>,
}

#[derive(Args, Debug)]
pub struct AddArgs {
    #[arg(value_name = "FILE|URL|-", help = "A lyrics file or an http(s) URL; leave it out or use - to read the lyrics from stdin")]
    pub source: Option<String>,
    #[command(flatten)]
    pub track: TrackArgs,
    #[arg(long, value_parser = parse_format, help = "The lyrics format instead of detecting it: lrc, ttml, srt, vtt, ass, sbv, sami, json or text")]
    pub format: Option<Format>,
    #[arg(long, help = "Don't re-read the file or URL every time the sync loads")]
    pub no_watch: bool,
    #[arg(
        long,
        conflicts_with_all = ["title", "artists", "album", "duration", "isrc", "spotify_id", "no_watch"],
        help = "Don't save it: show it in the running app for the playing song until the song changes"
    )]
    pub temporary: bool,
}

pub fn parse_length(s: &str) -> Result<f64, String> {
    let parts: Vec<&str> = s.trim().split(':').collect();
    let mut total = 0.0;
    for p in &parts {
        let n: f64 = p.trim().parse().map_err(|_| format!("“{s}” is not a length like 254 or 4:14"))?;
        if n < 0.0 || !n.is_finite() {
            return Err(format!("“{s}” is not a length like 254 or 4:14"));
        }
        total = total * 60.0 + n;
    }
    if parts.len() > 3 || total <= 0.0 {
        return Err(format!("“{s}” is not a length like 254 or 4:14"));
    }
    Ok(total)
}

pub fn parse_format(s: &str) -> Result<Format, String> {
    let want = normalize(s);
    Format::ALL
        .into_iter()
        .find(|f| normalize(f.extension()) == want || normalize(f.label()) == want)
        .ok_or_else(|| {
            format!(
                "“{s}” is not a format, use one of {}",
                Format::ALL.iter().map(|f| f.extension()).collect::<Vec<_>>().join(", ")
            )
        })
}

impl TrackArgs {
    fn given(&self) -> bool {
        self.title.is_some()
    }

    pub fn apply(&self, q: &mut TrackQuery) {
        let text = |v: &Option<String>| v.as_ref().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        if let Some(t) = text(&self.title) {
            q.title = t;
        }
        if !self.artists.is_empty() {
            q.artists = self.artists.iter().map(|a| a.trim().to_string()).filter(|a| !a.is_empty()).collect();
        }
        if let Some(a) = &self.album {
            q.album = text(&Some(a.clone()));
        }
        if let Some(d) = self.duration {
            q.duration = Some(d);
        }
        if let Some(i) = &self.isrc {
            q.isrc = text(&Some(i.clone()));
        }
        if let Some(s) = &self.spotify_id {
            q.spotify_id = text(&Some(s.clone()));
        }
    }
}

pub async fn playing_track() -> Result<TrackQuery, String> {
    let data = crate::ipc::call(&crate::ipc::Request::NowPlaying)
        .await
        .map_err(|e| format!("{e}, so give the song with --title and --artist"))?;
    let title = data["title"].as_str().unwrap_or("").to_string();
    if title.is_empty() {
        return Err("nothing is playing, so give the song with --title and --artist".into());
    }
    let text = |k: &str| data[k].as_str().map(str::to_string).filter(|s| !s.is_empty());
    Ok(TrackQuery {
        title,
        artists: data["artists"]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default(),
        album: text("album"),
        duration: data["length"].as_f64().filter(|l| *l > 0.0),
        spotify_id: text("spotifyId"),
        isrc: text("isrc"),
    })
}

async fn track_for(args: &TrackArgs) -> TrackQuery {
    let mut q = if args.given() {
        TrackQuery::default()
    } else {
        playing_track().await.unwrap_or_else(|e| super::fail(e))
    };
    args.apply(&mut q);
    q
}

pub fn source_for(arg: Option<&str>) -> Result<Source, String> {
    match arg.map(str::trim) {
        None | Some("-") => {
            if super::stdin_piped() {
                Ok(Source::Stdin)
            } else {
                Err("give a lyrics file or URL, or pipe the lyrics in on stdin".into())
            }
        }
        Some(u) if u.starts_with("http://") || u.starts_with("https://") => Ok(Source::Url(u.to_string())),
        Some(p) => {
            let path = crate::library::expand_path(p);
            if path.is_dir() {
                Err(format!("{} is a folder", path.display()))
            } else if !path.exists() {
                Err(format!("{} doesn't exist", path.display()))
            } else {
                Ok(Source::File(path.display().to_string()))
            }
        }
    }
}

fn describe(q: &TrackQuery) -> String {
    let artist = q.artist_line();
    if artist.is_empty() { format!("“{}”", q.title) } else { format!("“{}” by {artist}", q.title) }
}

async fn add_temporary(source: Source, pasted: Option<String>, format: Option<Format>) -> i32 {
    let text = match pasted {
        Some(text) => text,
        None => crate::library::read_source(&source, &crate::providers::http_client())
            .await
            .unwrap_or_else(|e| super::fail(e)),
    };
    let request = crate::ipc::Request::Temporary(crate::ipc::TemporaryLyrics { text, source, format });
    match crate::ipc::call(&request).await {
        Ok(v) => {
            println!("Showing temporary lyrics for “{}” until the song changes", v["title"].as_str().unwrap_or(""));
            if let Some(w) = v["warning"].as_str() {
                eprintln!("lyrminal: {w}");
            }
            0
        }
        Err(e) => super::fail(e),
    }
}

async fn add(args: AddArgs) -> i32 {
    let source = source_for(args.source.as_deref()).unwrap_or_else(|e| super::fail(e));
    let pasted = (source == Source::Stdin).then(super::read_stdin);
    if args.temporary {
        return add_temporary(source, pasted, args.format).await;
    }
    let track = track_for(&args.track).await;
    let req = ImportRequest {
        track: track.clone(),
        identity: String::new(),
        temporary: false,
        watch: !args.no_watch && source.can_watch(),
        source,
        format: args.format,
        pasted,
    };
    let lib = Library::default();
    let done = crate::app::import(req, crate::providers::http_client(), lib.clone()).await;
    let (key, warning) = match done {
        Ok(LibraryDone::Imported { key, warning, .. }) => (key, warning),
        Ok(_) => super::fail("the sync could not be added"),
        Err(e) => super::fail(e),
    };
    let id = Library::id_for(&key);
    let summary = lib.get(&id).map(|e| e.summary()).unwrap_or_default();
    println!("Saved {} as {} ({summary})", describe(&track), &id[..SHORT_ID.min(id.len())]);
    if let Some(w) = warning {
        eprintln!("lyrminal: {w}");
    }
    super::library_changed(LibraryChange {
        added: vec![key],
        changed: vec![id],
        removed: Vec::new(),
    });
    0
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
        LibraryCommand::Add(a) => add(a).await,
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

    #[test]
    fn reads_lengths_and_formats() {
        assert_eq!(parse_length("254"), Ok(254.0));
        assert_eq!(parse_length("4:14"), Ok(254.0));
        assert_eq!(parse_length("1:00:00.5"), Ok(3600.5));
        assert!(parse_length("abc").is_err());
        assert!(parse_length("0").is_err());
        assert_eq!(parse_format("TTML"), Ok(Format::Ttml));
        assert_eq!(parse_format("webvtt"), Ok(Format::Vtt));
        assert!(parse_format("docx").is_err());
        let mut q = TrackQuery {
            title: "Old".into(),
            artists: vec!["A".into()],
            ..Default::default()
        };
        TrackArgs {
            album: Some("Album".into()),
            artists: vec!["B".into(), "C".into()],
            ..Default::default()
        }
        .apply(&mut q);
        assert_eq!((q.title.as_str(), q.artists.len(), q.album.as_deref()), ("Old", 2, Some("Album")));
    }
}
