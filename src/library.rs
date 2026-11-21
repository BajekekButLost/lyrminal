use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::lyrics::formats::{self, Format};
use crate::lyrics::{Credits, Lyrics, SyncKind, ttml};
use crate::providers::cache::fnv;
use crate::providers::{TrackQuery, local};

pub const MAX_BYTES: usize = 8 * 1024 * 1024;
const CLIPBOARD_TIMEOUT: Duration = Duration::from_secs(4);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "location", rename_all = "lowercase")]
pub enum Source {
    File(String),
    Url(String),
    Clipboard,
}

impl Source {
    pub fn label(&self) -> &'static str {
        match self {
            Source::File(_) => "file",
            Source::Url(_) => "URL",
            Source::Clipboard => "clipboard",
        }
    }

    pub fn name(&self) -> String {
        match self {
            Source::File(p) => Path::new(p)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.clone()),
            Source::Url(u) => {
                let rest = u.split_once("://").map(|(_, r)| r).unwrap_or(u);
                rest.split(['/', '?', '#']).next().unwrap_or(rest).to_string()
            }
            Source::Clipboard => "clipboard".into(),
        }
    }

    pub fn location(&self) -> Option<&str> {
        match self {
            Source::File(p) | Source::Url(p) => Some(p),
            Source::Clipboard => None,
        }
    }

    pub fn hint(&self) -> Option<Format> {
        self.location().and_then(Format::from_name)
    }

    pub fn can_watch(&self) -> bool {
        !matches!(self, Source::Clipboard)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: String,
    pub key: String,
    pub track: TrackQuery,
    pub source: Source,
    #[serde(default)]
    pub format: Option<Format>,
    #[serde(default)]
    pub detected: Option<Format>,
    #[serde(default)]
    pub kind: Option<SyncKind>,
    #[serde(default)]
    pub watch: bool,
    pub file: String,
    pub added_at: u64,
    pub updated_at: u64,
}

impl Entry {
    pub fn summary(&self) -> String {
        let mut parts = vec![self.source.label().to_string()];
        if let Some(f) = self.detected.or(self.format) {
            parts.push(f.label().to_string());
        }
        if let Some(k) = self.kind {
            parts.push(k.label().to_string());
        }
        if self.watch && self.source.can_watch() {
            parts.push("checks for changes".into());
        }
        parts.join(" · ")
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Index {
    #[serde(default)]
    entries: Vec<Entry>,
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn credits(source: &Source) -> Credits {
    Credits {
        provider: local::NAME.into(),
        provider_url: match source {
            Source::Url(u) => Some(u.clone()),
            _ => None,
        },
        source: Some(source.name()),
        source_tag: None,
        songwriters: Vec::new(),
        contributors: Vec::new(),
    }
}

pub fn expand_path(input: &str) -> PathBuf {
    let input = input.trim();
    let path = if input == "~" {
        dirs::home_dir().unwrap_or_default()
    } else if let Some(rest) = input.strip_prefix("~/") {
        dirs::home_dir().unwrap_or_default().join(rest)
    } else {
        PathBuf::from(input)
    };
    if path.is_absolute() {
        path
    } else {
        std::env::current_dir().map(|d| d.join(&path)).unwrap_or(path)
    }
}

pub fn parse(text: &str, source: &Source, format: Option<Format>, duration: Option<f64>) -> Result<(Lyrics, Format), String> {
    parse_hinted(text, source, format, source.hint(), duration)
}

fn parse_hinted(
    text: &str,
    source: &Source,
    format: Option<Format>,
    hint: Option<Format>,
    duration: Option<f64>,
) -> Result<(Lyrics, Format), String> {
    formats::parse(text, format, hint, credits(source), duration)
}

pub fn title_warning(text: &str, format: Format, track: &TrackQuery) -> Option<String> {
    if format != Format::Ttml && format != Format::Json {
        return None;
    }
    let meta = ttml::metadata(text);
    if let (Some(a), Some(b)) = (&meta.spotify_id, &track.spotify_id) {
        return (a != b).then(|| format!("The file is tagged for another Spotify track ({a})"));
    }
    let want = crate::providers::normalize(&crate::providers::clean_title(&track.title));
    if want.is_empty() || meta.titles.is_empty() {
        return None;
    }
    let matches = meta.titles.iter().any(|t| {
        let got = crate::providers::normalize(&crate::providers::clean_title(t));
        got == want || got.contains(&want) || want.contains(&got)
    });
    (!matches).then(|| format!("The file says it is for “{}”", meta.titles[0]))
}

async fn read_capped(path: &Path) -> Result<Vec<u8>, String> {
    let meta = tokio::fs::metadata(path).await.map_err(|e| format!("{}: {e}", path.display()))?;
    if meta.is_dir() {
        return Err(format!("{} is a folder", path.display()));
    }
    if meta.len() as usize > MAX_BYTES {
        return Err("the file is larger than 8 MB".into());
    }
    tokio::fs::read(path).await.map_err(|e| format!("{}: {e}", path.display()))
}

async fn fetch_url(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, String> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("the URL must start with http:// or https://".into());
    }
    let mut resp = client.get(url).send().await.map_err(|e| e.to_string())?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("the server answered HTTP {}", status.as_u16()));
    }
    if resp.content_length().is_some_and(|l| l as usize > MAX_BYTES) {
        return Err("the download is larger than 8 MB".into());
    }
    let mut out = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
        out.extend_from_slice(&chunk);
        if out.len() > MAX_BYTES {
            return Err("the download is larger than 8 MB".into());
        }
    }
    Ok(out)
}

fn clipboard_commands() -> Vec<Vec<&'static str>> {
    let mut cmds: Vec<Vec<&'static str>> = Vec::new();
    if cfg!(target_os = "macos") {
        cmds.push(vec!["pbpaste"]);
    } else if cfg!(windows) {
        cmds.push(vec!["powershell", "-NoProfile", "-Command", "Get-Clipboard -Raw"]);
    } else {
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            cmds.push(vec!["wl-paste", "--no-newline"]);
        }
        cmds.push(vec!["xclip", "-selection", "clipboard", "-o"]);
        cmds.push(vec!["xsel", "--clipboard", "--output"]);
        if std::env::var_os("WAYLAND_DISPLAY").is_none() {
            cmds.push(vec!["wl-paste", "--no-newline"]);
        }
    }
    cmds
}

fn read_clipboard_blocking() -> Result<String, String> {
    let mut missing = Vec::new();
    for cmd in clipboard_commands() {
        let out = std::process::Command::new(cmd[0])
            .args(&cmd[1..])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output();
        match out {
            Ok(o) if o.status.success() && !o.stdout.is_empty() => {
                if o.stdout.len() > MAX_BYTES {
                    return Err("the clipboard holds more than 8 MB".into());
                }
                return Ok(formats::decode(&o.stdout));
            }
            Ok(_) => return Err("the clipboard is empty".into()),
            Err(_) => missing.push(cmd[0]),
        }
    }
    Err(format!(
        "could not read the clipboard (install {}), or paste into this dialog with your terminal",
        missing.join(" or ")
    ))
}

pub async fn read_clipboard() -> Result<String, String> {
    let task = tokio::task::spawn_blocking(read_clipboard_blocking);
    match tokio::time::timeout(CLIPBOARD_TIMEOUT, task).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err("reading the clipboard timed out".into()),
    }
}

pub async fn read_source(source: &Source, client: &reqwest::Client) -> Result<String, String> {
    let bytes = match source {
        Source::File(p) => read_capped(&expand_path(p)).await?,
        Source::Url(u) => fetch_url(client, u).await?,
        Source::Clipboard => return read_clipboard().await,
    };
    Ok(formats::decode(&bytes))
}

#[derive(Debug, Clone)]
pub struct Library {
    dir: PathBuf,
}

impl Default for Library {
    fn default() -> Self {
        Self::new(crate::config::data_dir().join("library"))
    }
}

impl Library {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn index_path(&self) -> PathBuf {
        self.dir.join("index.json")
    }

    fn copy_path(&self, file: &str) -> PathBuf {
        self.dir.join("lyrics").join(file)
    }

    pub fn entries(&self) -> Vec<Entry> {
        fs::read_to_string(self.index_path())
            .ok()
            .and_then(|t| serde_json::from_str::<Index>(&t).ok())
            .map(|i| i.entries)
            .unwrap_or_default()
    }

    fn write_atomic(&self, path: &Path, data: &[u8]) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let tmp = path.with_extension(format!("{}.{nanos}.tmp", std::process::id()));
        fs::write(&tmp, data)?;
        fs::rename(&tmp, path)
    }

    fn save(&self, entries: Vec<Entry>) -> io::Result<()> {
        let text = serde_json::to_string_pretty(&Index { entries }).map_err(io::Error::other)?;
        self.write_atomic(&self.index_path(), text.as_bytes())
    }

    pub fn get(&self, id: &str) -> Option<Entry> {
        self.entries().into_iter().find(|e| e.id == id)
    }

    pub fn id_for(key: &str) -> String {
        fnv(key)
    }

    pub fn put(&self, mut entry: Entry, content: &str) -> io::Result<Entry> {
        let mut entries = self.entries();
        let old = entries.iter().position(|e| e.key == entry.key).map(|i| entries.remove(i));
        let ext = entry.detected.or(entry.format).unwrap_or(Format::Text).extension();
        entry.file = format!("{}.{ext}", entry.id);
        if let Some(old) = &old {
            entry.added_at = old.added_at;
            if old.file != entry.file {
                let _ = fs::remove_file(self.copy_path(&old.file));
            }
        }
        self.write_atomic(&self.copy_path(&entry.file), content.as_bytes())?;
        entries.push(entry.clone());
        self.save(entries)?;
        Ok(entry)
    }

    pub fn remove(&self, id: &str) -> io::Result<Option<Entry>> {
        let mut entries = self.entries();
        let Some(pos) = entries.iter().position(|e| e.id == id) else { return Ok(None) };
        let entry = entries.remove(pos);
        self.save(entries)?;
        let _ = fs::remove_file(self.copy_path(&entry.file));
        Ok(Some(entry))
    }

    pub fn read_copy(&self, entry: &Entry) -> io::Result<String> {
        fs::read(self.copy_path(&entry.file)).map(|b| formats::decode(&b))
    }

    fn refresh(&self, entry: &Entry, content: &str, lyrics: &Lyrics, detected: Format) {
        let unchanged = self.read_copy(entry).is_ok_and(|c| c == content);
        if unchanged && entry.kind == Some(lyrics.kind) && entry.detected == Some(detected) {
            return;
        }
        let mut updated = entry.clone();
        updated.kind = Some(lyrics.kind);
        updated.detected = Some(detected);
        updated.updated_at = now();
        let _ = self.put(updated, content);
    }

    pub async fn load(&self, entry: &Entry, client: &reqwest::Client, duration: Option<f64>) -> Result<Lyrics, String> {
        let duration = duration.or(entry.track.duration);
        let mut problem = None;
        if entry.watch && entry.source.can_watch() {
            match read_source(&entry.source, client).await {
                Ok(text) => match parse(&text, &entry.source, entry.format, duration) {
                    Ok((lyrics, detected)) => {
                        let lib = self.clone();
                        let entry = entry.clone();
                        let copy = lyrics.clone();
                        let _ = tokio::task::spawn_blocking(move || lib.refresh(&entry, &text, &copy, detected)).await;
                        return Ok(lyrics);
                    }
                    Err(e) => problem = Some(e),
                },
                Err(e) => problem = Some(e),
            }
        }
        let text = match self.read_copy(entry) {
            Ok(t) => t,
            Err(e) => {
                return Err(match problem {
                    Some(p) => format!("{p}, and the saved copy is missing ({e})"),
                    None => format!("the saved copy is missing ({e})"),
                });
            }
        };
        parse_hinted(&text, &entry.source, entry.format, entry.detected.or(entry.source.hint()), duration).map(|(l, _)| l)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_library() -> Library {
        let dir = std::env::temp_dir().join(format!("lyrminal-lib-{}", fnv(&format!("{:?}", SystemTime::now()))));
        Library::new(dir)
    }

    fn entry(key: &str, source: Source, watch: bool) -> Entry {
        Entry {
            id: Library::id_for(key),
            key: key.into(),
            track: TrackQuery {
                title: "Song".into(),
                ..Default::default()
            },
            source,
            format: None,
            detected: Some(Format::Lrc),
            kind: Some(SyncKind::Line),
            watch,
            file: String::new(),
            added_at: 1,
            updated_at: 1,
        }
    }

    #[tokio::test]
    async fn stores_replaces_and_removes() {
        let lib = temp_library();
        let client = crate::providers::http_client();
        let e = lib.put(entry("k", Source::Clipboard, false), "[00:01.00]one\n[00:02.00]two").unwrap();
        assert_eq!(e.file, format!("{}.lrc", e.id));
        let l = lib.load(&e, &client, None).await.unwrap();
        assert_eq!(l.lines.len(), 2);
        assert_eq!(l.credits.provider, local::NAME);
        lib.put(entry("k", Source::Clipboard, false), "[00:01.00]only").unwrap();
        assert_eq!(lib.entries().len(), 1);
        let l = lib.load(&lib.get(&e.id).unwrap(), &client, None).await.unwrap();
        assert_eq!(l.lines.len(), 1);
        assert!(lib.remove(&e.id).unwrap().is_some());
        assert!(lib.entries().is_empty());
        let _ = fs::remove_dir_all(lib.dir());
    }

    #[tokio::test]
    async fn watched_files_are_reread_and_fall_back_to_the_copy() {
        let lib = temp_library();
        let client = crate::providers::http_client();
        fs::create_dir_all(lib.dir()).unwrap();
        let src = lib.dir().join("song.srt");
        fs::write(&src, "1\n00:00:01,000 --> 00:00:02,000\nfirst\n").unwrap();
        let source = Source::File(src.display().to_string());
        let e = lib.put(entry("w", source, true), "1\n00:00:01,000 --> 00:00:02,000\nfirst\n").unwrap();
        fs::write(&src, "1\n00:00:01,000 --> 00:00:02,000\nchanged\n").unwrap();
        let l = lib.load(&e, &client, None).await.unwrap();
        assert_eq!(l.lines[0].lead.text(), "changed");
        assert!(lib.read_copy(&lib.get(&e.id).unwrap()).unwrap().contains("changed"));
        fs::remove_file(&src).unwrap();
        let l = lib.load(&lib.get(&e.id).unwrap(), &client, None).await.unwrap();
        assert_eq!(l.lines[0].lead.text(), "changed");
        let _ = fs::remove_dir_all(lib.dir());
    }

    #[test]
    fn names_sources() {
        assert_eq!(Source::Url("https://example.com/a/b.lrc?x".into()).name(), "example.com");
        assert_eq!(Source::File("/tmp/x/song.ttml".into()).name(), "song.ttml");
        assert_eq!(Source::File("/tmp/x/song.ttml".into()).hint(), Some(Format::Ttml));
    }
}
