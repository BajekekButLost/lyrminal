use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::library::{Entry, Source, expand_path};
use crate::lyrics::formats::Format;
use crate::providers::TrackQuery;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    File,
    Clipboard,
    Url,
}

impl SourceKind {
    const ALL: [SourceKind; 3] = [SourceKind::File, SourceKind::Clipboard, SourceKind::Url];

    pub fn label(self) -> &'static str {
        match self {
            SourceKind::File => "file",
            SourceKind::Clipboard => "clipboard",
            SourceKind::Url => "URL",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Form {
    pub temporary: bool,
    pub source: SourceKind,
    pub path: String,
    pub url: String,
    pub watch: bool,
    pub format: Option<Format>,
}

impl Default for Form {
    fn default() -> Self {
        Self {
            temporary: false,
            source: SourceKind::File,
            path: String::new(),
            url: String::new(),
            watch: false,
            format: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Library,
    Import,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Add,
    Entry(usize),
    Temporary,
    Source,
    Location,
    Watch,
    Format,
    Import,
}

pub const CARD_WIDTH: usize = 30;
pub const CARD_GAP: usize = 1;
pub const CARD_HEIGHT: usize = 5;

pub fn card_columns(width: usize) -> usize {
    ((width + CARD_GAP) / (CARD_WIDTH + CARD_GAP)).max(1)
}

pub struct ImportRequest {
    pub track: TrackQuery,
    pub identity: String,
    pub temporary: bool,
    pub source: Source,
    pub watch: bool,
    pub format: Option<Format>,
    pub pasted: Option<String>,
}

pub enum Outcome {
    None,
    Close,
    Import(ImportRequest),
    Apply(Entry),
    Remove(Entry),
}

pub struct LibraryDialog {
    pub track: Option<(TrackQuery, String)>,
    pub view: View,
    pub form: Form,
    pub pasted: Option<String>,
    pub editing: Option<String>,
    pub selected: usize,
    pub columns: usize,
    pub entries: Vec<Entry>,
    pub confirm_remove: Option<String>,
    pub busy: Option<String>,
    pub error: Option<String>,
    pub notice: Option<String>,
    pub seq: u64,
}

impl LibraryDialog {
    pub fn new(track: Option<(TrackQuery, String)>, mut entries: Vec<Entry>, seq: u64) -> Self {
        sort_entries(&mut entries);
        Self {
            track,
            view: View::Library,
            form: Form::default(),
            pasted: None,
            editing: None,
            selected: 0,
            columns: 1,
            entries,
            confirm_remove: None,
            busy: None,
            error: None,
            notice: None,
            seq,
        }
    }

    pub fn set_entries(&mut self, mut entries: Vec<Entry>) {
        sort_entries(&mut entries);
        self.entries = entries;
        self.selected = self.selected.min(self.rows().len().saturating_sub(1));
    }

    pub fn rows(&self) -> Vec<Row> {
        if self.view == View::Library {
            let mut rows = vec![Row::Add];
            rows.extend((0..self.entries.len()).map(Row::Entry));
            return rows;
        }
        let mut rows = vec![Row::Temporary, Row::Source];
        if self.form.source != SourceKind::Clipboard {
            rows.push(Row::Location);
            if !self.form.temporary {
                rows.push(Row::Watch);
            }
        }
        rows.push(Row::Format);
        rows.push(Row::Import);
        rows
    }

    pub fn group_starts(&self) -> Vec<usize> {
        match self.view {
            View::Library if self.entries.is_empty() => vec![0],
            View::Library => vec![0, 1],
            View::Import => vec![0, self.rows().len() - 1],
        }
    }

    pub fn open_import(&mut self) {
        self.form = Form::default();
        self.pasted = None;
        self.view = View::Import;
        self.selected = 0;
        self.editing = None;
        self.confirm_remove = None;
        self.error = None;
        self.notice = None;
    }

    pub fn open_library(&mut self) {
        self.view = View::Library;
        self.selected = 0;
        self.editing = None;
        self.error = None;
        self.notice = None;
    }

    pub fn current(&self) -> Option<Row> {
        self.rows().get(self.selected).copied()
    }

    pub fn location(&self) -> &str {
        match self.form.source {
            SourceKind::Url => &self.form.url,
            _ => &self.form.path,
        }
    }

    fn set_location(&mut self, value: String) {
        match self.form.source {
            SourceKind::Url => self.form.url = value,
            _ => self.form.path = value,
        }
    }

    pub fn track_entry(&self) -> Option<&Entry> {
        let (q, _) = self.track.as_ref()?;
        let key = q.key();
        self.entries.iter().find(|e| e.key == key)
    }

    fn select_row(&mut self, row: Row) {
        if let Some(i) = self.rows().iter().position(|r| *r == row) {
            self.selected = i;
        }
    }

    fn request(&mut self) -> Option<ImportRequest> {
        let Some((track, identity)) = self.track.clone() else {
            self.error = Some("Nothing is playing, so there is no song to add lyrics to".into());
            return None;
        };
        let location = self.location().trim().to_string();
        let source = match self.form.source {
            SourceKind::Clipboard => Source::Clipboard,
            SourceKind::File if location.is_empty() => {
                self.error = Some("Type the path of a lyrics file first".into());
                self.select_row(Row::Location);
                return None;
            }
            SourceKind::Url if location.is_empty() => {
                self.error = Some("Type the URL of a lyrics file first".into());
                self.select_row(Row::Location);
                return None;
            }
            SourceKind::File => Source::File(expand_path(&location).display().to_string()),
            SourceKind::Url => Source::Url(location),
        };
        Some(ImportRequest {
            track,
            identity,
            temporary: self.form.temporary,
            watch: !self.form.temporary && self.form.watch && source.can_watch(),
            source,
            format: self.form.format,
            pasted: self.pasted.clone().filter(|_| self.form.source == SourceKind::Clipboard),
        })
    }
}

fn sort_entries(entries: &mut [Entry]) {
    entries.sort_by(|a, b| {
        let key = |e: &Entry| (e.track.title.to_lowercase(), e.track.artists.join(", ").to_lowercase());
        key(a).cmp(&key(b))
    });
}

fn cycle<T: Copy + PartialEq>(all: &[T], current: T, dir: i32) -> T {
    let i = all.iter().position(|x| *x == current).unwrap_or(0) as i32;
    all[(i + dir).rem_euclid(all.len() as i32) as usize]
}

fn cycle_format(current: Option<Format>, dir: i32) -> Option<Format> {
    let mut all: Vec<Option<Format>> = vec![None];
    all.extend(Format::ALL.iter().copied().map(Some));
    cycle(&all, current, dir)
}

fn common_prefix(a: &str, b: &str) -> usize {
    a.char_indices()
        .zip(b.chars())
        .take_while(|((_, x), y)| x == y)
        .last()
        .map(|((i, c), _)| i + c.len_utf8())
        .unwrap_or(0)
}

pub fn complete_path(input: &str) -> Option<String> {
    let split = input.rfind('/').map(|i| i + 1).unwrap_or(0);
    let (dir_part, prefix) = input.split_at(split);
    let dir = if dir_part.is_empty() { expand_path(".") } else { expand_path(dir_part) };
    let mut matches: Vec<(String, bool)> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let hidden_ok = prefix.starts_with('.') || !name.starts_with('.');
            (name.starts_with(prefix) && hidden_ok).then(|| (name, e.path().is_dir()))
        })
        .collect();
    if matches.is_empty() {
        return None;
    }
    matches.sort();
    let (first, first_dir) = matches[0].clone();
    if matches.len() == 1 {
        return Some(format!("{dir_part}{first}{}", if first_dir { "/" } else { "" }));
    }
    let len = matches.iter().skip(1).fold(first.len(), |n, (m, _)| n.min(common_prefix(&first[..n], m)));
    (len > prefix.len()).then(|| format!("{dir_part}{}", &first[..len]))
}

fn edit_key(d: &mut LibraryDialog, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let Some(buf) = d.editing.as_mut() else { return };
    match key.code {
        KeyCode::Esc => d.editing = None,
        KeyCode::Enter => {
            let value = buf.trim().to_string();
            d.editing = None;
            d.set_location(value);
            d.error = None;
        }
        KeyCode::Tab if d.form.source == SourceKind::File => {
            if let Some(done) = complete_path(buf) {
                *buf = done;
            }
        }
        KeyCode::Backspace => {
            buf.pop();
        }
        KeyCode::Char('u') if ctrl => buf.clear(),
        KeyCode::Char('w') if ctrl => {
            let trimmed = buf.trim_end_matches('/').trim_end().len();
            buf.truncate(trimmed);
            while buf.chars().last().is_some_and(|c| !c.is_whitespace() && c != '/') {
                buf.pop();
            }
        }
        KeyCode::Char(c) if !ctrl => buf.push(c),
        _ => {}
    }
}

fn change(d: &mut LibraryDialog, row: Row, dir: i32) -> bool {
    match row {
        Row::Temporary => d.form.temporary = !d.form.temporary,
        Row::Source => {
            d.form.source = cycle(&SourceKind::ALL, d.form.source, dir);
            d.error = None;
        }
        Row::Watch => d.form.watch = !d.form.watch,
        Row::Format => d.form.format = cycle_format(d.form.format, dir),
        _ => return false,
    }
    true
}

pub fn handle_key(d: &mut LibraryDialog, key: KeyEvent) -> Outcome {
    if d.editing.is_some() {
        edit_key(d, key);
        return Outcome::None;
    }
    if let Some(id) = d.confirm_remove.take() {
        if matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter)
            && let Some(e) = d.entries.iter().find(|e| e.id == id)
        {
            return Outcome::Remove(e.clone());
        }
        return Outcome::None;
    }
    let rows = d.rows();
    d.selected = d.selected.min(rows.len().saturating_sub(1));
    let row = rows[d.selected];
    if let KeyCode::Tab | KeyCode::BackTab = key.code {
        let dir = if key.code == KeyCode::Tab { 1 } else { -1 };
        if let Some(i) = crate::ui::jump_group(&d.group_starts(), d.selected, dir) {
            d.selected = i;
        }
        return Outcome::None;
    }
    if d.view == View::Library {
        return library_key(d, key, row);
    }
    match key.code {
        KeyCode::Char('L') => return Outcome::Close,
        KeyCode::Esc | KeyCode::Char('q') => d.open_library(),
        KeyCode::Down | KeyCode::Char('j') => d.selected = (d.selected + 1).min(rows.len() - 1),
        KeyCode::Up | KeyCode::Char('k') => d.selected = d.selected.saturating_sub(1),
        KeyCode::Home | KeyCode::Char('g') => d.selected = 0,
        KeyCode::End | KeyCode::Char('G') => d.selected = rows.len() - 1,
        KeyCode::Left | KeyCode::Char('h') => {
            change(d, row, -1);
        }
        KeyCode::Right | KeyCode::Char('l') => {
            change(d, row, 1);
        }
        KeyCode::Enter | KeyCode::Char(' ') => match row {
            Row::Location => d.editing = Some(d.location().to_string()),
            Row::Import => {
                if d.busy.is_some() {
                    return Outcome::None;
                }
                d.error = None;
                d.notice = None;
                if let Some(req) = d.request() {
                    return Outcome::Import(req);
                }
            }
            other => {
                change(d, other, 1);
            }
        },
        KeyCode::Delete | KeyCode::Backspace if row == Row::Location => d.set_location(String::new()),
        _ => {}
    }
    Outcome::None
}

fn library_key(d: &mut LibraryDialog, key: KeyEvent, row: Row) -> Outcome {
    let n = d.entries.len();
    let cols = d.columns.max(1);
    let card = d.selected.checked_sub(1);
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('L') => return Outcome::Close,
        KeyCode::Char('i') => d.open_import(),
        KeyCode::Down | KeyCode::Char('j') => match card {
            None if n > 0 => d.selected = 1,
            Some(c) if c + cols < n => d.selected += cols,
            Some(c) if c / cols < (n - 1) / cols => d.selected = n,
            _ => {}
        },
        KeyCode::Up | KeyCode::Char('k') => {
            if let Some(c) = card {
                d.selected = if c >= cols { d.selected - cols } else { 0 };
            }
        }
        KeyCode::Left | KeyCode::Char('h') => d.selected = d.selected.saturating_sub(1),
        KeyCode::Right | KeyCode::Char('l') => d.selected = (d.selected + 1).min(n),
        KeyCode::Home | KeyCode::Char('g') => d.selected = 0,
        KeyCode::End | KeyCode::Char('G') => d.selected = n,
        KeyCode::Enter | KeyCode::Char(' ') => match row {
            Row::Add => d.open_import(),
            Row::Entry(i) => {
                if let Some(e) = d.entries.get(i) {
                    return Outcome::Apply(e.clone());
                }
            }
            _ => {}
        },
        KeyCode::Char('d') | KeyCode::Delete | KeyCode::Backspace => {
            if let Row::Entry(i) = row {
                d.confirm_remove = d.entries.get(i).map(|e| e.id.clone());
            }
        }
        _ => {}
    }
    Outcome::None
}

pub fn paste(d: &mut LibraryDialog, text: &str) {
    if let Some(buf) = d.editing.as_mut() {
        buf.push_str(text.trim_matches(['\n', '\r']));
        return;
    }
    if d.view == View::Library {
        if text.trim().is_empty() {
            return;
        }
        d.open_import();
    }
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return;
    }
    let single = trimmed.lines().count() == 1;
    let url = single && (trimmed.starts_with("http://") || trimmed.starts_with("https://"));
    if !url && (!single || d.form.source == SourceKind::Clipboard) {
        d.form.source = SourceKind::Clipboard;
        d.notice = Some(format!("Pasted {} lines; choose Import to use them", trimmed.lines().count()));
        d.pasted = Some(text.to_string());
        d.select_row(Row::Import);
    } else {
        if url {
            d.form.source = SourceKind::Url;
        } else if d.form.source == SourceKind::Url {
            d.form.source = SourceKind::File;
        }
        d.set_location(trimmed.to_string());
        d.select_row(Row::Location);
    }
    d.error = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventKind;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        }
    }

    fn dialog() -> LibraryDialog {
        let q = TrackQuery {
            title: "Song".into(),
            artists: vec!["Artist".into()],
            ..Default::default()
        };
        let mut d = LibraryDialog::new(Some((q, "id".into())), Vec::new(), 1);
        d.open_import();
        d
    }

    fn saved(title: &str) -> Entry {
        Entry {
            id: title.into(),
            key: title.into(),
            track: TrackQuery {
                title: title.into(),
                ..Default::default()
            },
            source: Source::Clipboard,
            format: None,
            detected: None,
            kind: None,
            watch: false,
            file: String::new(),
            added_at: 1,
            updated_at: 1,
        }
    }

    #[test]
    fn opens_on_the_library_and_imports_on_demand() {
        let entries = ["a", "b", "c", "d", "e"].map(saved).to_vec();
        let mut d = LibraryDialog::new(None, entries, 1);
        d.columns = 3;
        assert_eq!(d.view, View::Library);
        assert_eq!(d.current(), Some(Row::Add));
        handle_key(&mut d, key(KeyCode::Down));
        assert_eq!(d.current(), Some(Row::Entry(0)));
        handle_key(&mut d, key(KeyCode::Down));
        assert_eq!(d.current(), Some(Row::Entry(3)));
        handle_key(&mut d, key(KeyCode::Right));
        handle_key(&mut d, key(KeyCode::Up));
        assert_eq!(d.current(), Some(Row::Entry(1)));
        handle_key(&mut d, key(KeyCode::Right));
        handle_key(&mut d, key(KeyCode::Down));
        assert_eq!(d.current(), Some(Row::Entry(4)));
        handle_key(&mut d, key(KeyCode::BackTab));
        assert_eq!(d.current(), Some(Row::Add));
        handle_key(&mut d, key(KeyCode::Char('i')));
        assert_eq!(d.view, View::Import);
        assert_eq!(d.current(), Some(Row::Temporary));
        handle_key(&mut d, key(KeyCode::Tab));
        assert_eq!(d.current(), Some(Row::Import));
        assert!(matches!(handle_key(&mut d, key(KeyCode::Esc)), Outcome::None));
        assert_eq!(d.view, View::Library);
        assert!(matches!(handle_key(&mut d, key(KeyCode::Esc)), Outcome::Close));
    }

    #[test]
    fn every_import_starts_with_a_fresh_form() {
        let mut d = dialog();
        handle_key(&mut d, key(KeyCode::Enter));
        paste(&mut d, "https://example.com/x.lrc");
        assert!(d.form.temporary);
        handle_key(&mut d, key(KeyCode::Esc));
        handle_key(&mut d, key(KeyCode::Char('i')));
        assert!(!d.form.temporary);
        assert_eq!(d.form.source, SourceKind::File);
        assert!(d.form.url.is_empty());
        assert!(d.pasted.is_none());
    }

    #[test]
    fn hides_options_that_do_not_apply() {
        let mut d = dialog();
        assert_eq!(d.rows(), [Row::Temporary, Row::Source, Row::Location, Row::Watch, Row::Format, Row::Import]);
        handle_key(&mut d, key(KeyCode::Enter));
        assert!(d.form.temporary);
        assert_eq!(d.rows(), [Row::Temporary, Row::Source, Row::Location, Row::Format, Row::Import]);
        d.selected = 1;
        handle_key(&mut d, key(KeyCode::Right));
        assert_eq!(d.form.source, SourceKind::Clipboard);
        assert_eq!(d.rows(), [Row::Temporary, Row::Source, Row::Format, Row::Import]);
    }

    #[test]
    fn imports_need_a_location_and_carry_the_options() {
        let mut d = dialog();
        d.selected = 5;
        assert!(matches!(handle_key(&mut d, key(KeyCode::Enter)), Outcome::None));
        assert!(d.error.is_some());
        assert_eq!(d.current(), Some(Row::Location));
        handle_key(&mut d, key(KeyCode::Enter));
        for c in "/tmp/a.srt".chars() {
            handle_key(&mut d, key(KeyCode::Char(c)));
        }
        handle_key(&mut d, key(KeyCode::Enter));
        d.selected = 3;
        handle_key(&mut d, key(KeyCode::Char(' ')));
        d.selected = 5;
        match handle_key(&mut d, key(KeyCode::Enter)) {
            Outcome::Import(r) => {
                assert_eq!(r.source, Source::File("/tmp/a.srt".into()));
                assert!(r.watch);
                assert!(!r.temporary);
            }
            _ => panic!("expected an import"),
        }
    }

    #[test]
    fn pasting_lyrics_switches_to_the_clipboard() {
        let mut d = dialog();
        paste(&mut d, "[00:01.00]a\n[00:02.00]b\n");
        assert_eq!(d.form.source, SourceKind::Clipboard);
        assert_eq!(d.current(), Some(Row::Import));
        match handle_key(&mut d, key(KeyCode::Enter)) {
            Outcome::Import(r) => assert!(r.pasted.is_some_and(|p| p.contains("[00:02.00]b"))),
            _ => panic!("expected an import"),
        }
        paste(&mut d, "https://example.com/x.lrc");
        assert_eq!(d.form.source, SourceKind::Url);
        assert_eq!(d.form.url, "https://example.com/x.lrc");
    }

    #[test]
    fn completes_paths() {
        let dir = std::env::temp_dir().join(format!("lyrminal-complete-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("songs")).unwrap();
        std::fs::write(dir.join("song-one.lrc"), "").unwrap();
        std::fs::write(dir.join("song-two.lrc"), "").unwrap();
        let base = format!("{}/", dir.display());
        assert_eq!(complete_path(&format!("{base}song-o")), Some(format!("{base}song-one.lrc")));
        assert_eq!(complete_path(&format!("{base}songs")), Some(format!("{base}songs/")));
        assert_eq!(complete_path(&format!("{base}so")), Some(format!("{base}song")));
        let _ = std::fs::remove_dir_all(dir);
    }
}
