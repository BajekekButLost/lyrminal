use std::io::{self, Write};
use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use crate::anim::Rgb;
use crate::app::{App, LyricsState, Overlay, SyncRow};
use crate::config::{Background, PlayerMode};
use crate::player::Status;
use crate::providers::{ordered_ids, provider_name};
use crate::render::{FrameInput, LinkSpan};
use crate::term::kitty::{self, Placement};
use crate::theme::Theme;
use crate::ui::keys::{Action, Binding, HELP, RENDERER_KEY};
use crate::ui::library::{self as library_ui, LibraryDialog, Row as LibRow, SourceKind, View as LibView};
use crate::ui::onboarding;
use crate::ui::settings::{self, Kind};

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub struct DrawOutput {
    pub art: Option<Placement>,
    pub links: Vec<LinkSpan>,
    pub pixel: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Target {
    Row(usize),
    Section(usize),
    Back,
    Decrease,
    Increase,
    Action(Action),
}

#[derive(Debug, Clone, Default)]
pub struct UiHits {
    pub modal: Option<Rect>,
    pub guide: Option<Rect>,
    pub targets: Vec<(Rect, Target)>,
    pub columns: usize,
}

impl UiHits {
    pub fn at(&self, x: u16, y: u16) -> Option<Target> {
        let p = Position::new(x, y);
        self.targets.iter().rev().find(|(r, _)| r.contains(p)).map(|(_, t)| *t)
    }

    pub fn in_modal(&self, x: u16, y: u16) -> bool {
        self.modal.is_some_and(|r| r.contains(Position::new(x, y)))
    }

    pub fn in_guide(&self, x: u16, y: u16) -> bool {
        self.guide.is_some_and(|r| r.contains(Position::new(x, y)))
    }

    pub(crate) fn push(&mut self, rect: Rect, target: Target, bounds: Rect) {
        let rect = rect.intersection(bounds);
        if !rect.is_empty() {
            self.targets.push((rect, target));
        }
    }
}

pub(crate) fn row_rect(x: u16, y: u16, width: u16) -> Rect {
    Rect { x, y, width, height: 1 }
}

fn ink(theme: &Theme, alpha: f32) -> Style {
    Style::default().fg(theme.base.mix(theme.text, alpha).to_color())
}

fn accent(theme: &Theme) -> Style {
    Style::default().fg(theme.accent.to_color())
}

fn fmt_time(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

pub(crate) fn truncate(s: &str, width: usize) -> String {
    if UnicodeWidthStr::width(s) <= width {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    for ch in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if w + cw + 1 > width {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out.push('…');
    out
}

fn paint_background(f: &mut Frame, theme: &Theme) {
    let area = f.area();
    if theme.paint == Background::None {
        return;
    }
    let buf = f.buffer_mut();
    for y in area.y..area.y + area.height {
        let bg = theme.bg_at(y, area.height).to_color();
        for x in area.x..area.x + area.width {
            let cell = &mut buf[(x, y)];
            cell.set_symbol(" ");
            cell.set_bg(bg);
        }
    }
}

pub(crate) fn spinner(app: &App) -> &'static str {
    SPINNER[((app.wall() * 12.0) as usize) % SPINNER.len()]
}

pub fn draw(f: &mut Frame, app: &mut App, now: Instant, dt: f64) -> DrawOutput {
    let theme = app.theme;
    let pixel = app.pixel_active();
    if !pixel {
        paint_background(f, &theme);
    }
    let (guide_area, area) = match app.guide {
        Some(_) => {
            let split = onboarding::split(f.area());
            (Some(split.panel), split.main)
        }
        None => (None, f.area()),
    };
    let has_track = app.track.is_some();
    let header_h = if app.cfg.appearance.show_header && has_track { 6 } else { 0 };
    let progress_h = if app.cfg.appearance.show_progress && has_track { 1 } else { 0 };
    let status_h = if app.cfg.appearance.show_status { 1 } else { 0 };
    let [header, lyrics_area, progress, status] = Layout::vertical([
        Constraint::Length(header_h),
        Constraint::Min(3),
        Constraint::Length(progress_h),
        Constraint::Length(status_h),
    ])
    .areas(area);

    let art = if header_h > 0 { draw_header(f, app, header) } else { None };
    let mut links = Vec::new();
    draw_lyrics(f, app, lyrics_area, now, dt, &mut links);
    app.progress_bar = if progress_h > 0 {
        Some(draw_progress(f, app, progress, now))
    } else {
        None
    };
    let mut hits = UiHits::default();
    if status_h > 0 {
        draw_status(f, app, status, &mut hits);
    }
    if let Some(panel) = guide_area {
        onboarding::draw(f, app, panel, &mut hits);
    }
    let overlay_open = !matches!(app.overlay, Overlay::None);
    if overlay_open {
        hits = UiHits::default();
    }
    match &app.overlay {
        Overlay::None => {}
        Overlay::Help => draw_help(f, &theme, app.pixel_supported(), &mut hits),
        Overlay::Settings(_) => draw_settings(f, app, &mut hits),
        Overlay::Players { .. } => draw_players(f, app, &mut hits),
        Overlay::Syncs(_) => draw_syncs(f, app, &mut hits),
        Overlay::Library(_) => draw_library(f, app, &mut hits),
    }
    app.ui = hits;
    DrawOutput {
        art,
        links: if overlay_open { Vec::new() } else { links },
        pixel,
    }
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) -> Option<Placement> {
    let theme = &app.theme;
    let track = app.track.as_ref()?;
    let inner = Rect {
        x: area.x + 2,
        y: area.y + 1,
        width: area.width.saturating_sub(4),
        height: area.height.saturating_sub(2),
    };
    let rows = 4u16.min(inner.height);
    let mut text_x = inner.x;
    let mut art = None;
    if app.cfg.appearance.album_art
        && app.caps.kitty_graphics
        && app.artwork.is_some()
        && let Some((cw, ch)) = app.caps.cell_px
    {
        let cols = ((rows as f32 * ch / cw).round() as u16).clamp(rows, 14);
        let placement = Placement {
            col: inner.x,
            row: inner.y,
            cols,
            rows,
        };
        let hole = Rect {
            x: placement.col,
            y: placement.row,
            width: cols.min((area.x + area.width).saturating_sub(placement.col)),
            height: rows,
        };
        f.buffer_mut().set_style(hole, Style::default().bg(ratatui::style::Color::Reset));
        art = Some(placement);
        text_x = inner.x + cols + 2;
    }
    let width = (inner.x + inner.width).saturating_sub(text_x) as usize;
    let source = match &app.lyrics {
        LyricsState::Ready { resolution, prepared, .. } => {
            let credits = &prepared.lyrics.credits;
            let tag = credits.source_tag.as_ref().or_else(|| {
                credits.source.as_ref().filter(|_| crate::render::cell::layout::provider_in_header(credits))
            });
            let mut s = tag.map(|tag| format!("{tag} · ")).unwrap_or_default();
            s.push_str(&format!(
                "{} · {}",
                provider_name(&resolution.candidate.provider),
                prepared.lyrics.kind.label()
            ));
            if resolution.candidate.provider == crate::providers::local::ID && resolution.candidate.id == crate::app::TEMPORARY_ID {
                s.push_str(" · temporary");
            } else if resolution.manual {
                s.push_str(" · chosen");
            }
            s
        }
        LyricsState::Loading | LyricsState::Waiting => format!("{} looking for lyrics", spinner(app)),
        LyricsState::NotFound(_) => "no lyrics found".into(),
        _ => String::new(),
    };
    let mut album = track.album.clone().unwrap_or_default();
    if album.is_empty() {
        album = track.album_artists.join(", ");
    }
    let lines = vec![
        Line::from(Span::styled(
            truncate(&track.title, width),
            ink(theme, 0.95).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(truncate(&track.artists.join(", "), width), ink(theme, 0.72))),
        Line::from(Span::styled(truncate(&album, width), ink(theme, 0.5))),
        Line::from(Span::styled(truncate(&source, width), accent(theme))),
    ];
    let text_area = Rect {
        x: text_x,
        y: inner.y,
        width: width as u16,
        height: rows,
    };
    f.render_widget(Paragraph::new(lines), text_area);
    art
}

fn centered_message(f: &mut Frame, theme: &Theme, area: Rect, lines: Vec<Line<'static>>) {
    let h = lines.len() as u16;
    let y = area.y + area.height.saturating_sub(h) * 2 / 5;
    let rect = Rect {
        x: area.x + 2,
        y,
        width: area.width.saturating_sub(4),
        height: h.min(area.height),
    };
    let _ = theme;
    f.render_widget(Paragraph::new(lines).alignment(ratatui::layout::Alignment::Center), rect);
}

fn draw_lyrics(f: &mut Frame, app: &mut App, area: Rect, now: Instant, dt: f64, links: &mut Vec<LinkSpan>) {
    let theme = app.theme;
    let title = |s: &str| Line::from(Span::styled(s.to_string(), ink(&theme, 0.85).add_modifier(Modifier::BOLD)));
    let dim = |s: String| Line::from(Span::styled(s, ink(&theme, 0.5)));
    match &app.lyrics {
        LyricsState::Ready { .. } => {
            let (prepared, id) = app.prepared().unwrap();
            let t = app.song_time(now);
            let wall = app.wall();
            let screen_height = f.area().height;
            let show_cursor = app.cfg.controls.enabled;
            if app.pixel_active()
                && let Some(cell) = app.caps.cell_px
            {
                let screen = f.area();
                let input = FrameInput {
                    prepared: &prepared,
                    t,
                    wall,
                    dt,
                    theme: &theme,
                    appearance: &app.cfg.appearance,
                    lyrics: &app.cfg.lyrics,
                    scroll: &app.cfg.scroll,
                    follow: &app.follow,
                    screen_height,
                    show_cursor,
                };
                let pixel = app.pixel.as_mut().unwrap();
                let out = pixel.render(&input, id, screen, area, cell);
                if let Some(cy) = out.credits_y {
                    draw_pixel_credits(f, &theme, &prepared.lyrics.credits, area, cell, cy, out.content_x, prepared.all_sung(t), app.cfg.appearance.show_header, links);
                }
                if show_cursor && app.follow.manual && out.info.cursor_item.is_some() {
                    let y = area.y + (area.height as f32 * 0.4) as u16;
                    let x = ((out.content_x / cell.0) as u16).saturating_sub(2);
                    f.buffer_mut().set_string(x, y, "▌", accent(&theme));
                }
                app.frame = out.info;
                return;
            }
            let input = FrameInput {
                prepared: &prepared,
                t,
                wall,
                dt,
                theme: &theme,
                appearance: &app.cfg.appearance,
                lyrics: &app.cfg.lyrics,
                scroll: &app.cfg.scroll,
                follow: &app.follow,
                screen_height,
                show_cursor,
            };
            let info = app.cell.render(&input, id, area, f.buffer_mut());
            links.extend(info.links.iter().cloned());
            app.frame = info;
        }
        LyricsState::NoPlayer => {
            let mut lines = vec![title("No media player found")];
            if app.cfg.player.mode == PlayerMode::Fixed {
                lines.push(dim(format!("Waiting for “{}” to appear on the media bus", app.cfg.player.target)));
            } else {
                lines.push(dim("Start playing something in a player that supports MPRIS".into()));
            }
            lines.push(dim("p: choose a player · Esc: settings · ?: help".into()));
            if let Some(e) = &app.player_error {
                lines.push(Line::from(Span::styled(e.clone(), ink(&theme, 0.4))));
            }
            centered_message(f, &theme, area, lines);
        }
        LyricsState::NoTrack => centered_message(f, &theme, area, vec![title("Nothing is playing")]),
        LyricsState::Unsupported(what) => centered_message(
            f,
            &theme,
            area,
            vec![title(what), dim("There are no lyrics for this".into())],
        ),
        LyricsState::Waiting | LyricsState::Loading => {
            let s = spinner(app);
            centered_message(f, &theme, area, vec![Line::from(Span::styled(format!("{s}  Looking for lyrics"), ink(&theme, 0.6)))]);
        }
        LyricsState::NotFound(attempts) => {
            let mut lines = vec![title("No lyrics found"), Line::default()];
            for a in attempts {
                lines.push(dim(format!("{}: {}", a.provider, a.outcome)));
            }
            lines.push(Line::default());
            lines.push(dim("s: search every provider · L: add your own lyrics · R: try again · Esc: settings".into()));
            centered_message(f, &theme, area, lines);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_pixel_credits(
    f: &mut Frame,
    theme: &Theme,
    credits: &crate::lyrics::Credits,
    area: Rect,
    cell: (f32, f32),
    credits_y: f32,
    content_x: f32,
    all_sung: bool,
    header: bool,
    links: &mut Vec<LinkSpan>,
) {
    let col = (content_x / cell.0).round() as u16;
    let width = (area.x + area.width).saturating_sub(col + 2);
    let rows = crate::render::cell::layout::credit_rows(credits, width, crate::render::cell::layout::Align::Left, header);
    let first = (credits_y / cell.1).ceil() as i32;
    let screen_h = f.area().height;
    for (i, row) in rows.iter().enumerate() {
        let y = first + i as i32;
        if y < area.y as i32 || y >= (area.y + area.height) as i32 {
            continue;
        }
        let y = y as u16;
        let bg = if theme.paint == Background::None { theme.base } else { theme.bg_at(y, screen_h) };
        let emphasis = matches!(row.kind, crate::render::cell::layout::RowKind::Credit { emphasis: true });
        let alpha = if all_sung { 0.8 } else { 0.5 } * if emphasis { 1.0 } else { 0.8 };
        let fg = bg.mix(theme.text, alpha);
        let style = Style::default().fg(fg.to_color());
        let mut texts = vec![String::new(); row.links.len()];
        for g in &row.glyphs {
            let link = row.link_at(g.col);
            let style = if link.is_some() { style.add_modifier(Modifier::UNDERLINED) } else { style };
            f.buffer_mut().set_string(col + g.col, y, &g.text, style);
            if let Some(i) = link {
                texts[i].push_str(&g.text);
            }
        }
        for (link, text) in row.links.iter().zip(texts) {
            links.push(LinkSpan {
                x: col + link.start,
                y,
                text,
                url: link.url.clone(),
                fg,
                bg: None,
                bold: false,
            });
        }
    }
}

fn draw_progress(f: &mut Frame, app: &App, area: Rect, now: Instant) -> (u16, u16, u16) {
    let theme = &app.theme;
    let pos = app.scrub.unwrap_or_else(|| app.position(now));
    let len = app.track.as_ref().and_then(|t| t.length).unwrap_or(0.0);
    let left = fmt_time(pos);
    let right = if len > 0.0 { fmt_time(len) } else { "--:--".into() };
    let pad = 2u16;
    let bar_w = area
        .width
        .saturating_sub(pad * 2 + left.len() as u16 + right.len() as u16 + 2) as usize;
    let filled = if len > 0.0 { ((pos / len).clamp(0.0, 1.0) * bar_w as f64).round() as usize } else { 0 };
    let (head, rest) = if app.scrub.is_some() && filled < bar_w {
        ("●", bar_w - filled - 1)
    } else {
        ("", bar_w - filled)
    };
    let line = Line::from(vec![
        Span::raw(" ".repeat(pad as usize)),
        Span::styled(left.clone(), ink(theme, 0.6)),
        Span::raw(" "),
        Span::styled("━".repeat(filled), accent(theme)),
        Span::styled(head, accent(theme)),
        Span::styled("─".repeat(rest), ink(theme, 0.22)),
        Span::raw(" "),
        Span::styled(right, ink(theme, 0.6)),
    ]);
    f.render_widget(Paragraph::new(line), area);
    (area.y, area.x + pad + left.len() as u16 + 1, bar_w as u16)
}

fn draw_status(f: &mut Frame, app: &App, area: Rect, hits: &mut UiHits) {
    let theme = &app.theme;
    let left = match &app.snapshot {
        Some(s) => {
            let icon = s.status.icon();
            let mode = if app.cfg.player.mode == PlayerMode::Auto { "auto" } else { "fixed" };
            format!("  {icon} {} ({mode})", s.player.display_name())
        }
        None => "  ■ no player".into(),
    };
    let mut right_parts: Vec<(String, Option<Action>)> = Vec::new();
    let offset = app.cfg.sync.offset_ms + app.track_offset_ms();
    if offset != 0 {
        right_parts.push((format!("offset {offset:+} ms"), None));
    }
    if app.follow.manual {
        right_parts.push(("scrolling · f to follow".into(), Some(Action::Follow)));
    }
    if let Some(hint) = app.update.hint() {
        right_parts.push((hint, Some(Action::Updates)));
    }
    right_parts.push(("? help".into(), Some(Action::Help)));
    let joined = right_parts.iter().map(|(s, _)| s.as_str()).collect::<Vec<_>>().join(" · ");
    let right = format!("{joined}  ");
    let middle = app
        .toast
        .as_ref()
        .map(|(m, _)| m.clone())
        .unwrap_or_default();
    let width = area.width as usize;
    let lw = UnicodeWidthStr::width(left.as_str());
    let (right, right_parts) = if lw + UnicodeWidthStr::width(right.as_str()) + 2 > width {
        ("? help  ".to_string(), vec![("? help".to_string(), Some(Action::Help))])
    } else {
        (right, right_parts)
    };
    let rw = UnicodeWidthStr::width(right.as_str());
    let free = width.saturating_sub(lw + rw);
    hits.push(row_rect(area.x, area.y, lw as u16), Target::Action(Action::Players), area);
    let mut x = area.x + (lw + free) as u16;
    for (text, action) in &right_parts {
        let w = UnicodeWidthStr::width(text.as_str()) as u16;
        if let Some(a) = action {
            hits.push(row_rect(x, area.y, w), Target::Action(*a), area);
        }
        x += w + 3;
    }
    let middle = if free > 4 { truncate(&middle, free - 4) } else { String::new() };
    let mw = UnicodeWidthStr::width(middle.as_str());
    let gap_left = free.saturating_sub(mw) / 2;
    let gap_right = free.saturating_sub(mw + gap_left);
    let playing = app.snapshot.as_ref().is_some_and(|s| s.status == Status::Playing);
    let line = Line::from(vec![
        Span::styled(left, if playing { accent(theme) } else { ink(theme, 0.55) }),
        Span::raw(" ".repeat(gap_left)),
        Span::styled(middle, ink(theme, 0.9).add_modifier(Modifier::BOLD)),
        Span::raw(" ".repeat(gap_right)),
        Span::styled(right, ink(theme, 0.45)),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

pub(crate) fn panel(theme: &Theme) -> (Rgb, Style) {
    let bg = if theme.paint == Background::None {
        theme.base.mix(theme.text, 0.05)
    } else {
        theme.bg_top.mix(Rgb::BLACK, 0.45)
    };
    (bg, Style::default().bg(bg.to_color()).fg(bg.mix(theme.text, 0.9).to_color()))
}

pub(crate) fn panel_ink(theme: &Theme, bg: Rgb, alpha: f32) -> Style {
    Style::default().fg(bg.mix(theme.text, alpha).to_color()).bg(bg.to_color())
}

fn modal(f: &mut Frame, theme: &Theme, title: &str, w: u16, h: u16, hits: &mut UiHits) -> (Rect, Rgb) {
    let area = f.area();
    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height.saturating_sub(2));
    let rect = Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    };
    let (bg, style) = panel(theme);
    hits.modal = Some(rect);
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent.to_color()).bg(bg.to_color()))
        .title(Span::styled(format!(" {title} "), Style::default().fg(theme.accent.to_color()).add_modifier(Modifier::BOLD)))
        .style(style);
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let padded = Rect {
        x: inner.x + 1,
        y: inner.y,
        width: inner.width.saturating_sub(2),
        height: inner.height,
    };
    (padded, bg)
}

fn draw_help(f: &mut Frame, theme: &Theme, pixel: bool, hits: &mut UiHits) {
    let shown = |b: &&Binding| pixel || b.keys != RENDERER_KEY;
    let rows: usize = HELP.iter().map(|g| g.bindings.iter().filter(shown).count() + 2).sum::<usize>() + 2;
    let (inner, bg) = modal(f, theme, "Keys", 76, rows as u16 + 2, hits);
    let mut lines = Vec::new();
    for g in HELP {
        lines.push(Line::from(Span::styled(g.title, Style::default().fg(theme.accent.to_color()).add_modifier(Modifier::BOLD))));
        for b in g.bindings.iter().filter(shown) {
            lines.push(Line::from(vec![
                Span::styled(format!("  {:<14}", b.keys), panel_ink(theme, bg, 0.95).add_modifier(Modifier::BOLD)),
                Span::styled(b.desc, panel_ink(theme, bg, 0.7)),
            ]));
        }
        lines.push(Line::default());
    }
    lines.push(Line::from(Span::styled(
        "Lyrminal is unofficial and not affiliated with Spicy Lyrics, lrc.red, Better Lyrics, lrcmux or LRCLIB.",
        panel_ink(theme, bg, 0.45),
    )));
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_settings(f: &mut Frame, app: &App, hits: &mut UiHits) {
    let Overlay::Settings(state) = &app.overlay else { return };
    let theme = &app.theme;
    let area = f.area();
    let (inner, bg) = modal(f, theme, "Settings", 96.min(area.width), area.height.saturating_sub(4).max(16), hits);
    let secs = state.sections(&app.cfg);
    let section = state.section.min(secs.len() - 1);
    let items = state.items(&app.cfg);
    let selected = state.item.min(items.len().saturating_sub(1));
    let [nav, body] = Layout::horizontal([Constraint::Length(16), Constraint::Min(20)]).areas(inner);
    let nav_lines: Vec<Line> = secs
        .iter()
        .enumerate()
        .map(|(i, s)| {
            if i == section {
                Line::from(Span::styled(
                    format!(" ▸ {}", s.title),
                    Style::default().fg(theme.accent.to_color()).add_modifier(Modifier::BOLD),
                ))
            } else {
                Line::from(Span::styled(format!("   {}", s.title), panel_ink(theme, bg, 0.55)))
            }
        })
        .collect();
    f.render_widget(Paragraph::new(nav_lines), nav);
    for i in 0..secs.len() {
        hits.push(row_rect(nav.x, nav.y + i as u16, nav.width), Target::Section(i), nav);
    }

    let [crumb, list, desc, footer] = Layout::vertical([
        Constraint::Length(if state.page.is_some() { 2 } else { 0 }),
        Constraint::Min(4),
        Constraint::Length(4),
        Constraint::Length(1),
    ])
    .areas(body);
    if let Some(id) = &state.page {
        let name = provider_name(id).to_string();
        let crumb_w = UnicodeWidthStr::width("‹ Providers / ") + UnicodeWidthStr::width(name.as_str());
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("‹ Providers / ", panel_ink(theme, bg, 0.5)),
                Span::styled(
                    name,
                    Style::default().fg(theme.accent.to_color()).bg(bg.to_color()).add_modifier(Modifier::BOLD),
                ),
            ])),
            crumb,
        );
        hits.push(row_rect(crumb.x, crumb.y, crumb_w as u16), Target::Back, crumb);
    }
    let label_w = 30usize;
    let visible = list.height as usize;
    let scroll = selected.saturating_sub(visible.saturating_sub(2));
    let sel_bg = bg.mix(theme.accent, 0.18);
    let mut lines = Vec::new();
    for (i, it) in items.iter().enumerate().skip(scroll).take(visible) {
        let is_sel = i == selected;
        let row_bg = if is_sel { sel_bg } else { bg };
        let value = if is_sel && state.editing.is_some() {
            let buf = state.editing.as_deref().unwrap_or("");
            let shown = match &it.kind {
                Kind::Text { secret: true, .. } => "•".repeat(buf.chars().count()),
                _ => buf.to_string(),
            };
            format!("{shown}▏")
        } else if matches!(it.kind, Kind::Action(settings::SettingsAction::Update)) {
            app.update.value()
        } else if is_sel && matches!(it.kind, Kind::Choice { .. } | Kind::Number { .. } | Kind::Preset { .. }) {
            format!("‹ {} ›", settings::value_text(&app.cfg, &it.kind))
        } else {
            settings::value_text(&app.cfg, &it.kind)
        };
        let value = if crate::overrides::item_lock(it).is_some() { format!("🔒 {value}") } else { value };
        let label_style = if matches!(it.kind, Kind::Info) {
            panel_ink(theme, row_bg, 0.85).add_modifier(Modifier::BOLD)
        } else {
            panel_ink(theme, row_bg, if is_sel { 1.0 } else { 0.8 })
        };
        let marker = if is_sel { "› " } else { "  " };
        let label = truncate(&it.label, label_w);
        let pad = label_w.saturating_sub(UnicodeWidthStr::width(label.as_str()));
        let width = list.width as usize;
        let value = truncate(&value, width.saturating_sub(label_w + 4));
        let vw = UnicodeWidthStr::width(value.as_str());
        let tail = width.saturating_sub(label_w + 2 + vw);
        let y = list.y + (i - scroll) as u16;
        hits.push(row_rect(list.x, y, list.width), Target::Row(i), list);
        if is_sel && value.starts_with('‹') && value.ends_with('›') {
            let vx = list.x + (label_w + 2) as u16;
            hits.push(row_rect(vx, y, 2), Target::Decrease, list);
            hits.push(row_rect(vx + vw as u16 - 2, y, 2), Target::Increase, list);
        }
        lines.push(Line::from(vec![
            Span::styled(marker, Style::default().fg(theme.accent.to_color()).bg(row_bg.to_color())),
            Span::styled(label, label_style),
            Span::styled(" ".repeat(pad), panel_ink(theme, row_bg, 0.5)),
            Span::styled(
                value,
                if is_sel { Style::default().fg(theme.accent.to_color()).bg(row_bg.to_color()).add_modifier(Modifier::BOLD) } else { panel_ink(theme, row_bg, 0.6) },
            ),
            Span::styled(" ".repeat(tail), panel_ink(theme, row_bg, 0.5)),
        ]));
    }
    f.render_widget(Paragraph::new(lines), list);
    let desc_text = if let Some(a) = state.confirm {
        let what = match a {
            settings::SettingsAction::ClearCache => "clear the lyrics cache",
            settings::SettingsAction::ResetDefaults => "reset every setting and API key",
            settings::SettingsAction::PickPlayer => "pick a player",
            settings::SettingsAction::Onboarding => "start the welcome tour",
            settings::SettingsAction::Update => "update Lyrminal",
        };
        format!("Press y or click it again to {what}, any other key to cancel.")
    } else if state.editing.is_some() && items.get(selected).is_some_and(|i| matches!(i.kind, Kind::Text { folder: true, .. })) {
        "Type or paste a folder, Tab to complete it, Enter to save, Esc to cancel, Ctrl-U to clear.".to_string()
    } else if state.editing.is_some() {
        "Type or paste, Enter to save, Esc to cancel, Ctrl-U to clear.".to_string()
    } else if items.get(selected).is_some_and(|i| matches!(i.kind, Kind::Action(settings::SettingsAction::Update))) {
        app.update.desc()
    } else {
        items
            .get(selected)
            .map(|i| match crate::overrides::item_lock(i) {
                Some(o) => format!("{}. {}", settings::lock_message(o), i.desc),
                None => i.desc.clone(),
            })
            .unwrap_or_default()
    };
    f.render_widget(
        Paragraph::new(desc_text)
            .style(panel_ink(theme, bg, 0.65))
            .wrap(Wrap { trim: true }),
        desc,
    );
    let hint = if state.page.is_some() {
        "↑↓ move · ←→ change · ↵ edit · r reset · Esc back"
    } else if section == 0 {
        "↵ configure · Space on/off · K/J reorder · r reset · Tab section · Esc close"
    } else {
        "Tab section · ↑↓ move · ←→ change · ↵ edit · r reset · Esc close"
    };
    f.render_widget(Paragraph::new(hint).style(panel_ink(theme, bg, 0.4)), footer);
}

fn draw_players(f: &mut Frame, app: &App, hits: &mut UiHits) {
    let Overlay::Players { selected } = &app.overlay else { return };
    let theme = &app.theme;
    let (inner, bg) = modal(f, theme, "Players", 90, app.players.len() as u16 + 7, hits);
    let current = app.snapshot.as_ref().map(|s| s.player.bus.clone());
    let sel_bg = bg.mix(theme.accent, 0.18);
    let mut lines = Vec::new();
    let auto_label = format!("Auto · follow the active player (prefers {})", app.cfg.player.preferred.join(", "));
    let rows: Vec<(String, String, bool)> = std::iter::once((
        auto_label,
        String::new(),
        app.cfg.player.mode == PlayerMode::Auto,
    ))
    .chain(app.players.iter().map(|p| {
        let mut detail = format!("{} {}", p.status.icon(), p.process.clone().unwrap_or_else(|| p.short_bus().to_string()));
        if let Some(pid) = p.pid {
            detail.push_str(&format!(" · pid {pid}"));
        }
        if let Some(np) = &p.now_playing {
            detail.push_str(&format!(" · {np}"));
        }
        let fixed_here = app.cfg.player.mode == PlayerMode::Fixed && p.matches(&app.cfg.player.target);
        (p.display_name(), detail, fixed_here || (current.as_deref() == Some(p.bus.as_str()) && app.cfg.player.mode == PlayerMode::Fixed))
    }))
    .collect();
    let width = inner.width as usize;
    for (i, (name, detail, chosen)) in rows.iter().enumerate() {
        let row_bg = if i == *selected { sel_bg } else { bg };
        let mark = if *chosen { "✓ " } else { "  " };
        let following = i > 0 && current.as_deref() == Some(app.players[i - 1].bus.as_str());
        let text = format!("{mark}{name}{}", if following { "  (following)" } else { "" });
        let detail = truncate(detail, width.saturating_sub(UnicodeWidthStr::width(text.as_str()) + 3));
        let fill = width.saturating_sub(UnicodeWidthStr::width(text.as_str()) + UnicodeWidthStr::width(detail.as_str()) + 2);
        hits.push(row_rect(inner.x, inner.y + i as u16, inner.width), Target::Row(i), inner);
        lines.push(Line::from(vec![
            Span::styled(text, panel_ink(theme, row_bg, 0.95).add_modifier(if *chosen { Modifier::BOLD } else { Modifier::empty() })),
            Span::styled("  ", panel_ink(theme, row_bg, 0.5)),
            Span::styled(detail, panel_ink(theme, row_bg, 0.55)),
            Span::styled(" ".repeat(fill), panel_ink(theme, row_bg, 0.5)),
        ]));
    }
    if app.players.is_empty() {
        lines.push(Line::from(Span::styled("  No MPRIS players are running.", panel_ink(theme, bg, 0.5))));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled("Enter or click twice to follow · Esc close", panel_ink(theme, bg, 0.4))));
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_syncs(f: &mut Frame, app: &App, hits: &mut UiHits) {
    let Overlay::Syncs(picker) = &app.overlay else { return };
    let theme = &app.theme;
    let area = f.area();
    let (inner, bg) = modal(f, theme, "Choose a sync", 110, area.height.saturating_sub(4), hits);
    let [list, footer] = Layout::vertical([Constraint::Min(3), Constraint::Length(2)]).areas(inner);
    let order = ordered_ids(&app.cfg);
    let rows = picker.rows(&order);
    let current = match &app.lyrics {
        LyricsState::Ready { resolution, .. } => Some((resolution.candidate.provider.clone(), resolution.candidate.id.clone(), resolution.manual)),
        _ => None,
    };
    let visible = list.height as usize;
    let scroll = picker.selected.saturating_sub(visible.saturating_sub(3));
    let sel_bg = bg.mix(theme.accent, 0.18);
    let width = list.width as usize;
    let track_len = app.track.as_ref().and_then(|t| t.length);
    let mut lines = Vec::new();
    for (i, row) in rows.iter().enumerate().skip(scroll).take(visible) {
        let is_sel = i == picker.selected;
        let row_bg = if is_sel { sel_bg } else { bg };
        if !matches!(row, SyncRow::Header { .. }) {
            hits.push(row_rect(list.x, list.y + (i - scroll) as u16, list.width), Target::Row(i), list);
        }
        let line = match row {
            SyncRow::Auto => {
                let auto_on = current.as_ref().is_none_or(|c| !c.2);
                Line::from(vec![
                    Span::styled(if auto_on { "✓ " } else { "  " }, Style::default().fg(theme.accent.to_color()).bg(row_bg.to_color())),
                    Span::styled(
                        format!("{:<w$}", "Automatic · best match from the provider priority", w = width.saturating_sub(2)),
                        panel_ink(theme, row_bg, 0.95).add_modifier(Modifier::BOLD),
                    ),
                ])
            }
            SyncRow::Header { provider, status } => {
                let mut spans = vec![Span::styled(
                    provider_name(provider).to_string(),
                    Style::default().fg(theme.accent.to_color()).bg(bg.to_color()).add_modifier(Modifier::BOLD),
                )];
                if let Some(s) = status {
                    spans.push(Span::styled(format!("  {}", truncate(s, width.saturating_sub(20))), panel_ink(theme, bg, 0.45)));
                }
                Line::from(spans)
            }
            SyncRow::Candidate(c) => {
                let chosen = current.as_ref().is_some_and(|(p, id, _)| *p == c.provider && *id == c.id);
                let mut meta = Vec::new();
                if let Some(d) = c.duration {
                    let mut s = fmt_time(d);
                    if let Some(l) = track_len {
                        let diff = d - l;
                        if diff.abs() >= 1.0 {
                            s.push_str(&format!(" ({diff:+.0}s)"));
                        }
                    }
                    meta.push(s);
                }
                if let Some(t) = c.tier {
                    meta.push(t.label().to_string());
                }
                if let Some(k) = c.kind_hint {
                    meta.push(k.label().to_string());
                }
                if let Some(n) = &c.note {
                    meta.push(n.clone());
                }
                let meta = meta.join(" · ");
                let main = format!(
                    "{} — {}{}",
                    c.title,
                    c.artists.join(", "),
                    c.album.as_ref().map(|a| format!(" · {a}")).unwrap_or_default()
                );
                let main = truncate(&main, width.saturating_sub(meta.len() + 6));
                let fill = width.saturating_sub(4 + UnicodeWidthStr::width(main.as_str()) + UnicodeWidthStr::width(meta.as_str()));
                Line::from(vec![
                    Span::styled(if chosen { "  ✓ " } else { "    " }, Style::default().fg(theme.accent.to_color()).bg(row_bg.to_color())),
                    Span::styled(main, panel_ink(theme, row_bg, if is_sel { 1.0 } else { 0.8 })),
                    Span::styled(" ".repeat(fill), panel_ink(theme, row_bg, 0.5)),
                    Span::styled(meta, panel_ink(theme, row_bg, 0.5)),
                ])
            }
        };
        lines.push(line);
    }
    f.render_widget(Paragraph::new(lines), list);
    let status = if let Some(c) = &picker.applying {
        format!("{} loading {} from {}…", spinner(app), c.title, provider_name(&c.provider))
    } else if let Some(e) = &picker.error {
        format!("Could not load that sync: {e}")
    } else if !picker.pending.is_empty() {
        format!("{} searching {}", spinner(app), picker.pending.iter().map(|p| provider_name(p)).collect::<Vec<_>>().join(", "))
    } else {
        String::new()
    };
    f.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(status, panel_ink(theme, bg, 0.7))),
            Line::from(Span::styled("Enter or click twice to use this sync · Tab next provider · r search again · Esc close", panel_ink(theme, bg, 0.4))),
        ]),
        footer,
    );
}

fn library_desc(app: &App, d: &LibraryDialog, row: Option<LibRow>) -> String {
    if let Some(id) = &d.confirm_remove {
        let title = d.entries.iter().find(|e| &e.id == id).map(|e| e.track.title.clone()).unwrap_or_default();
        return format!("Press y or click it again to remove “{title}” from the library, any other key to cancel.");
    }
    if d.editing.is_some() {
        return if d.form.source == SourceKind::File {
            "Type or paste a path, Tab to complete it, Enter to save, Esc to cancel, Ctrl-U to clear.".into()
        } else {
            "Type or paste the address, Enter to save, Esc to cancel, Ctrl-U to clear.".into()
        };
    }
    match row {
        Some(LibRow::Add) => match &d.track {
            Some(_) => "Add lyrics for the song that is playing from a file, the clipboard or a URL. Shortcut: i.".into(),
            None => "Nothing is playing, so there is no song to add lyrics to. Saved lyrics can still be removed below.".into(),
        },
        Some(LibRow::Temporary) => "Yes: show the lyrics now without saving them; they are forgotten when the song changes or Lyrminal closes. No: save them in the local library for this song.".into(),
        Some(LibRow::Source) => "Where to load the lyrics from: a file on this computer, the system clipboard (or text pasted into this dialog with your terminal), or a web address.".into(),
        Some(LibRow::Location) if d.form.source == SourceKind::Url => "An http:// or https:// address that returns a lyrics file. Enter to edit.".into(),
        Some(LibRow::Location) if d.start_dir.is_some() => "Path to the lyrics file, starting in your default sync library location. Enter to edit, Tab completes while editing.".into(),
        Some(LibRow::Location) => "Path to the lyrics file; ~ is your home folder. Enter to edit, Tab completes while editing. Set a default folder in Settings → Providers → Local library.".into(),
        Some(LibRow::Watch) => format!(
            "Yes: read the {} again every time these lyrics load, so later edits show up; the saved copy is used when it can't be read. No: keep the lyrics as they are now, saved in {}.",
            if d.form.source == SourceKind::Url { "URL" } else { "file" },
            app.library.dir().display()
        ),
        Some(LibRow::Format) => "Auto detects LRC, TTML, SRT, WebVTT, ASS/SSA, SBV, SAMI, JSON and plain text. Pick one if the detection gets it wrong.".into(),
        Some(LibRow::Import) => match &d.track {
            Some(_) => "Load the lyrics with the options above for the song that is playing.".into(),
            None => "Nothing is playing, so there is no song to add lyrics to.".into(),
        },
        Some(LibRow::Entry(i)) => d
            .entries
            .get(i)
            .map(|e| {
                let from = e.source.location().map(|l| format!(" from {l}")).unwrap_or_default();
                format!("{}{from}. Enter uses these lyrics for the playing song, d removes them from the library.", e.summary())
            })
            .unwrap_or_default(),
        None => String::new(),
    }
}

fn draw_library(f: &mut Frame, app: &App, hits: &mut UiHits) {
    let Overlay::Library(d) = &app.overlay else { return };
    let theme = &app.theme;
    let area = f.area();
    let (inner, bg) = modal(f, theme, "Local library", 100, area.height.saturating_sub(4).max(18), hits);
    let [head, list, desc, status, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(4),
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    let width = inner.width as usize;

    let mut head_lines = Vec::new();
    match &d.track {
        Some((q, identity)) => {
            let mut s = format!("{} — {}", q.title, q.artist_line());
            if let Some(a) = &q.album {
                s.push_str(&format!(" · {a}"));
            }
            head_lines.push(Line::from(vec![
                Span::styled("For  ", panel_ink(theme, bg, 0.5)),
                Span::styled(truncate(&s, width.saturating_sub(5)), panel_ink(theme, bg, 0.95).add_modifier(Modifier::BOLD)),
            ]));
            let temp = app.temporary.as_ref().is_some_and(|(id, _)| id == identity);
            let state = if temp {
                "Temporary lyrics are loaded for this song".to_string()
            } else if let Some(e) = d.track_entry() {
                format!("Saved for this song: {}", e.summary())
            } else {
                "Nothing saved for this song yet".to_string()
            };
            head_lines.push(Line::from(Span::styled(truncate(&state, width), panel_ink(theme, bg, 0.6))));
        }
        None => {
            head_lines.push(Line::from(Span::styled("Nothing is playing", panel_ink(theme, bg, 0.95).add_modifier(Modifier::BOLD))));
            head_lines.push(Line::from(Span::styled("Saved lyrics can still be removed below.", panel_ink(theme, bg, 0.6))));
        }
    }
    f.render_widget(Paragraph::new(head_lines), head);

    match d.view {
        LibView::Library => draw_library_cards(f, app, d, list, bg, hits),
        LibView::Import => draw_library_form(f, app, d, list, bg, hits),
    }

    f.render_widget(
        Paragraph::new(library_desc(app, d, d.current()))
            .style(panel_ink(theme, bg, 0.65))
            .wrap(Wrap { trim: true }),
        desc,
    );
    let (status_text, status_style) = if let Some(b) = &d.busy {
        (format!("{} {b}…", spinner(app)), panel_ink(theme, bg, 0.75))
    } else if let Some(e) = &d.error {
        (format!("✗ {e}"), Style::default().fg(theme.accent.to_color()).bg(bg.to_color()).add_modifier(Modifier::BOLD))
    } else if let Some(n) = &d.notice {
        (n.clone(), panel_ink(theme, bg, 0.75))
    } else {
        (String::new(), panel_ink(theme, bg, 0.5))
    };
    f.render_widget(Paragraph::new(truncate(&status_text, width)).style(status_style), status);
    let hint = match d.view {
        LibView::Library => "i import · arrows move · ↵ use · d remove · Tab category · Esc close",
        LibView::Import => "↑↓ move · ←→ change · ↵ select · Tab category · paste lyrics or a path · Esc back",
    };
    f.render_widget(Paragraph::new(hint).style(panel_ink(theme, bg, 0.4)), footer);
}

fn library_heading(theme: &Theme, bg: Rgb, s: String) -> Line<'static> {
    Line::from(Span::styled(
        s,
        Style::default().fg(theme.accent.to_color()).bg(bg.to_color()).add_modifier(Modifier::BOLD),
    ))
}

fn draw_library_cards(f: &mut Frame, app: &App, d: &LibraryDialog, area: Rect, bg: Rgb, hits: &mut UiHits) {
    let theme = &app.theme;
    let width = area.width as usize;
    let n = d.entries.len();
    let selected = d.selected.min(n);
    let sel_bg = bg.mix(theme.accent, 0.18);
    let button = " + Import a new sync ";
    let button_style = if selected == 0 {
        Style::default().fg(bg.to_color()).bg(theme.accent.to_color()).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.accent.to_color()).bg(sel_bg.to_color()).add_modifier(Modifier::BOLD)
    };
    let button_w = UnicodeWidthStr::width(button) as u16;
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(if selected == 0 { "› " } else { "  " }, Style::default().fg(theme.accent.to_color()).bg(bg.to_color())),
            Span::styled(button, button_style),
            Span::styled("  i", panel_ink(theme, bg, 0.45)),
        ])),
        area,
    );
    hits.push(row_rect(area.x, area.y, button_w + 2), Target::Row(0), area);
    if area.height < 3 {
        return;
    }
    let heading_area = Rect { y: area.y + 2, height: 1, ..area };
    let grid = Rect {
        y: area.y + 3,
        height: area.height - 3,
        ..area
    };
    if n == 0 {
        f.render_widget(Paragraph::new(library_heading(theme, bg, "Library · 0 saved".into())), heading_area);
        f.render_widget(
            Paragraph::new(Span::styled("  Lyrics you save show up here. Press i to import some.", panel_ink(theme, bg, 0.5))),
            grid,
        );
        return;
    }
    let cols = library_ui::card_columns(width);
    hits.columns = cols;
    let card_w = (width - (cols - 1) * library_ui::CARD_GAP) / cols;
    let total_rows = n.div_ceil(cols);
    let visible_rows = (grid.height as usize / library_ui::CARD_HEIGHT).max(1);
    let sel_row = selected.checked_sub(1).map(|c| c / cols).unwrap_or(0);
    let first = (sel_row + 1).saturating_sub(visible_rows);
    let last = (first + visible_rows).min(total_rows);
    let mut title = format!("Library · {n} saved");
    if first > 0 || last < total_rows {
        title.push_str(&format!("  rows {}–{} of {total_rows}", first + 1, last));
    }
    f.render_widget(Paragraph::new(library_heading(theme, bg, title)), heading_area);
    let showing = match &app.lyrics {
        LyricsState::Ready { resolution, .. } if resolution.candidate.provider == crate::providers::local::ID => Some(resolution.candidate.id.clone()),
        _ => None,
    };
    let this_song = d.track_entry().map(|e| e.id.clone());
    for (i, e) in d.entries.iter().enumerate() {
        let row = i / cols;
        if row < first || row >= last {
            continue;
        }
        let col = i % cols;
        let rect = Rect {
            x: grid.x + (col * (card_w + library_ui::CARD_GAP)) as u16,
            y: grid.y + ((row - first) * library_ui::CARD_HEIGHT) as u16,
            width: card_w as u16,
            height: library_ui::CARD_HEIGHT as u16,
        }
        .intersection(grid);
        if rect.height < 3 {
            continue;
        }
        let is_sel = selected == i + 1;
        let card_bg = if is_sel { sel_bg } else { bg };
        let border = if is_sel {
            Style::default().fg(theme.accent.to_color()).bg(card_bg.to_color())
        } else {
            panel_ink(theme, card_bg, 0.3)
        };
        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(border)
            .padding(Padding::horizontal(1))
            .style(panel_ink(theme, card_bg, 0.8));
        if this_song.as_deref() == Some(e.id.as_str()) {
            block = block.title_top(Line::from(Span::styled(" this song ", Style::default().fg(theme.accent.to_color()))).right_aligned());
        }
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        let w = inner.width as usize;
        let mark = if showing.as_deref() == Some(e.id.as_str()) { "✓ " } else { "" };
        let title = truncate(&format!("{mark}{}", e.track.title), w);
        let artists = if e.track.artists.is_empty() { "Unknown artist".to_string() } else { e.track.artist_line() };
        let detail = if d.confirm_remove.as_deref() == Some(e.id.as_str()) {
            Span::styled(
                truncate("press y to remove", w),
                Style::default().fg(theme.accent.to_color()).bg(card_bg.to_color()).add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled(truncate(&e.summary(), w), panel_ink(theme, card_bg, 0.5))
        };
        f.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    title,
                    panel_ink(theme, card_bg, if is_sel { 1.0 } else { 0.9 }).add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(truncate(&artists, w), panel_ink(theme, card_bg, 0.7))),
                Line::from(detail),
            ]),
            inner,
        );
        hits.push(rect, Target::Row(i + 1), grid);
    }
}

fn draw_library_form(f: &mut Frame, app: &App, d: &LibraryDialog, list: Rect, bg: Rgb, hits: &mut UiHits) {
    let theme = &app.theme;
    let width = list.width as usize;
    let rows = d.rows();
    let selected = d.selected.min(rows.len().saturating_sub(1));
    let sel_bg = bg.mix(theme.accent, 0.18);
    let label_w = 22usize;
    let crumb = Line::from(vec![
        Span::styled("‹ Library / ", panel_ink(theme, bg, 0.5)),
        Span::styled(
            "Import a new sync",
            Style::default().fg(theme.accent.to_color()).bg(bg.to_color()).add_modifier(Modifier::BOLD),
        ),
    ]);
    let mut display: Vec<(Option<usize>, Line)> = vec![(None, crumb), (None, Line::default())];
    for (i, row) in rows.iter().enumerate() {
        let is_sel = i == selected;
        let row_bg = if is_sel { sel_bg } else { bg };
        let yes_no = |v: bool| if v { "yes" } else { "no" }.to_string();
        let (label, value, choice) = match row {
            LibRow::Temporary => ("Temporary", yes_no(d.form.temporary), true),
            LibRow::Source => ("Source", d.form.source.label().to_string(), true),
            LibRow::Location => {
                let url = d.form.source == SourceKind::Url;
                let value = match (&d.editing, d.location()) {
                    (Some(buf), _) if is_sel => format!("{buf}▏"),
                    (_, "") if url => "(https://example.com/song.ttml)".to_string(),
                    (_, "") => "(~/Music/lyrics/song.lrc)".to_string(),
                    (_, loc) => loc.to_string(),
                };
                (if url { "URL" } else { "Path" }, value, false)
            }
            LibRow::Watch => ("Check for changes", yes_no(d.form.watch), true),
            LibRow::Format => ("Format", d.form.format.map(|f| f.label().to_string()).unwrap_or_else(|| "auto".into()), true),
            LibRow::Import => {
                let value = if let Some(b) = &d.busy {
                    format!("{} {b}…", spinner(app))
                } else {
                    match (&d.pasted, d.form.source) {
                        (Some(p), SourceKind::Clipboard) => format!("↵ uses the pasted text ({} lines)", p.trim().lines().count()),
                        (_, SourceKind::Clipboard) => "↵ reads the clipboard".into(),
                        _ => "↵".into(),
                    }
                };
                ("Import", value, false)
            }
            LibRow::Add | LibRow::Entry(_) => continue,
        };
        let value = if is_sel && choice { format!("‹ {value} ›") } else { value };
        let value = if value.ends_with('▏') {
            let keep = width.saturating_sub(label_w + 3);
            let chars: Vec<char> = value.chars().collect();
            chars[chars.len().saturating_sub(keep)..].iter().collect()
        } else {
            truncate(&value, width.saturating_sub(label_w + 3))
        };
        let vw = UnicodeWidthStr::width(value.as_str());
        let pad = label_w.saturating_sub(UnicodeWidthStr::width(label));
        let tail = width.saturating_sub(label_w + 2 + vw);
        let label_style = if *row == LibRow::Import {
            panel_ink(theme, row_bg, 0.95).add_modifier(Modifier::BOLD)
        } else {
            panel_ink(theme, row_bg, if is_sel { 1.0 } else { 0.8 })
        };
        display.push((
            Some(i),
            Line::from(vec![
                Span::styled(if is_sel { "› " } else { "  " }, Style::default().fg(theme.accent.to_color()).bg(row_bg.to_color())),
                Span::styled(label, label_style),
                Span::styled(" ".repeat(pad), panel_ink(theme, row_bg, 0.5)),
                Span::styled(
                    value,
                    if is_sel {
                        Style::default().fg(theme.accent.to_color()).bg(row_bg.to_color()).add_modifier(Modifier::BOLD)
                    } else {
                        panel_ink(theme, row_bg, 0.6)
                    },
                ),
                Span::styled(" ".repeat(tail), panel_ink(theme, row_bg, 0.5)),
            ]),
        ));
    }
    let visible = list.height as usize;
    let sel_pos = display.iter().position(|(r, _)| *r == Some(selected)).unwrap_or(0);
    let scroll = (sel_pos + 2).saturating_sub(visible).min(display.len().saturating_sub(visible));
    if scroll == 0 {
        hits.push(row_rect(list.x, list.y, UnicodeWidthStr::width("‹ Library / ") as u16), Target::Back, list);
    }
    let mut lines = Vec::new();
    for (n, (row, line)) in display.into_iter().enumerate().skip(scroll).take(visible) {
        let y = list.y + (n - scroll) as u16;
        if let Some(i) = row {
            hits.push(row_rect(list.x, y, list.width), Target::Row(i), list);
            let choice = matches!(rows.get(i), Some(LibRow::Temporary | LibRow::Source | LibRow::Watch | LibRow::Format));
            if i == selected && choice {
                let vx = list.x + (label_w + 2) as u16;
                let vw = line.spans.get(3).map(|s| UnicodeWidthStr::width(s.content.as_ref())).unwrap_or(0) as u16;
                hits.push(row_rect(vx, y, 2), Target::Decrease, list);
                hits.push(row_rect(vx + vw.saturating_sub(2), y, 2), Target::Increase, list);
            }
        }
        lines.push(line);
    }
    f.render_widget(Paragraph::new(lines), list);
}

const PIXEL_IDS: [u32; 2] = [9001, 9002];
const ART_Z: i32 = -1_073_741_825;
const PIXEL_Z: i32 = ART_Z - 1;

#[derive(Default)]
pub struct Graphics {
    art: Option<(u32, String)>,
    placed: Option<Placement>,
    next_id: u32,
    pixel_current: Option<u32>,
    pixel_seq: u64,
}

impl Graphics {
    pub fn send_pixel(&mut self, out: &mut impl Write, canvas: &crate::render::pixel::raster::Canvas, cols: u16, rows: u16, remote: bool) -> io::Result<()> {
        let id = if self.pixel_current == Some(PIXEL_IDS[0]) { PIXEL_IDS[1] } else { PIXEL_IDS[0] };
        self.pixel_seq += 1;
        let straight;
        let data: &[u8] = if canvas.is_opaque() {
            &canvas.px
        } else {
            straight = canvas.straight_rgba();
            &straight
        };
        if remote {
            kitty::transmit_direct(out, id, canvas.w as u32, canvas.h as u32, data)?;
        } else {
            kitty::transmit_temp_file(out, id, canvas.w as u32, canvas.h as u32, data, self.pixel_seq)?;
            kitty::forget_old_temp_files(self.pixel_seq, 4);
        }
        kitty::place(out, id, 1, Placement { col: 0, row: 0, cols, rows }, PIXEL_Z)?;
        if let Some(old) = self.pixel_current.replace(id)
            && old != id
        {
            kitty::delete_image(out, old)?;
        }
        Ok(())
    }

    pub fn clear_pixel(&mut self, out: &mut impl Write) -> io::Result<()> {
        if let Some(id) = self.pixel_current.take() {
            kitty::delete_image(out, id)?;
        }
        Ok(())
    }

    pub fn sync_art(&mut self, out: &mut impl Write, app: &App, want: Option<Placement>) -> io::Result<()> {
        let artwork = app.artwork.as_ref().filter(|_| app.caps.kitty_graphics);
        let want = want.filter(|_| artwork.is_some());
        if let Some(art) = artwork
            && want.is_some()
            && self.art.as_ref().map(|(_, u)| u.as_str()) != Some(art.url.as_str())
        {
            if let Some((old, _)) = self.art.take() {
                kitty::delete_image(out, old)?;
            }
            self.next_id = self.next_id.wrapping_add(1).max(1);
            let id = 7000 + (self.next_id % 1000);
            kitty::transmit_direct(out, id, art.image.width(), art.image.height(), art.image.as_raw())?;
            self.art = Some((id, art.url.clone()));
            self.placed = None;
        }
        match (want, self.art.as_ref()) {
            (Some(p), Some((id, _))) => {
                if self.placed != Some(p) {
                    kitty::place(out, *id, 1, p, ART_Z)?;
                    self.placed = Some(p);
                }
            }
            (_, Some((id, _))) => {
                if self.placed.take().is_some() {
                    kitty::delete_placements(out, *id)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub fn screen_cleared(&mut self, out: &mut impl Write) -> io::Result<()> {
        if let Some((id, _)) = self.art.take() {
            kitty::delete_image(out, id)?;
        }
        self.placed = None;
        Ok(())
    }

    pub fn clear(&mut self, out: &mut impl Write) -> io::Result<()> {
        if let Some((id, _)) = self.art.take() {
            kitty::delete_image(out, id)?;
        }
        self.placed = None;
        self.clear_pixel(out)
    }
}

pub fn write_links(out: &mut impl Write, links: &[LinkSpan]) -> io::Result<()> {
    for l in links {
        let [r, g, b] = l.fg.to_u8();
        write!(out, "\x1b7\x1b[{};{}H\x1b[0m\x1b[38;2;{r};{g};{b}m", l.y + 1, l.x + 1)?;
        if let Some(bg) = l.bg {
            let [r, g, b] = bg.to_u8();
            write!(out, "\x1b[48;2;{r};{g};{b}m")?;
        }
        if l.bold {
            write!(out, "\x1b[1m")?;
        }
        let url: String = l.url.chars().filter(|c| !c.is_control()).collect();
        write!(out, "\x1b[4m\x1b]8;;{url}\x1b\\{}\x1b]8;;\x1b\\\x1b[0m\x1b8", l.text)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn draws_the_library_dialog() {
        let (player_tx, _player_rx) = tokio::sync::mpsc::unbounded_channel();
        let (msg_tx, _msg_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(crate::config::Config::default(), crate::term::TermCaps::default(), player_tx, msg_tx);
        let dir = std::env::temp_dir().join(format!("lyrminal-draw-test-{}", std::process::id()));
        app.library = crate::library::Library::new(dir.clone());
        let track = crate::player::TrackMeta {
            title: "Call Back".into(),
            artists: vec!["Don Toliver".into()],
            album: Some("OCTANE".into()),
            length: Some(123.8),
            ..Default::default()
        };
        for title in ["Call Back", "Second Song", "Third Song", "Fourth Song"] {
            let mut q = track.query();
            q.title = title.into();
            let key = q.key();
            let entry = crate::library::Entry {
                id: crate::library::Library::id_for(&key),
                key,
                track: q,
                source: crate::library::Source::Clipboard,
                format: None,
                detected: Some(crate::lyrics::formats::Format::Lrc),
                kind: Some(crate::lyrics::SyncKind::Line),
                watch: false,
                file: String::new(),
                added_at: 1,
                updated_at: 1,
            };
            app.library.put(entry, "[00:01.00]line").unwrap();
        }
        app.track = Some(track);
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        let mut screen = |app: &mut App, keys: &[crossterm::event::KeyCode]| {
            for code in keys {
                app.on_event(crossterm::event::Event::Key(crossterm::event::KeyEvent::new(*code, crossterm::event::KeyModifiers::NONE)));
            }
            term.draw(|f| {
                draw(f, app, Instant::now(), 0.0);
            })
            .unwrap();
            let buf = term.backend().buffer().clone();
            let text: Vec<String> = (0..buf.area.height)
                .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect())
                .collect();
            text.join("\n")
        };
        use crossterm::event::KeyCode;
        let joined = screen(&mut app, &[KeyCode::Char('L'), KeyCode::Down]);
        assert!(joined.contains("Local library"));
        assert!(joined.contains("Import a new sync"));
        assert!(joined.contains("Library · 4 saved"));
        assert!(joined.contains("this song"));
        assert!(joined.contains("Fourth Song"));
        assert!(!joined.contains("Check for changes"));
        assert_eq!(app.ui.columns, 3);
        assert!(app.ui.targets.iter().any(|(_, t)| *t == Target::Row(4)));
        let joined = screen(&mut app, &[KeyCode::Char('i'), KeyCode::Down, KeyCode::Down]);
        assert!(joined.contains("Check for changes"));
        assert!(joined.contains("(~/Music/lyrics/song.lrc)"));
        assert!(app.ui.targets.iter().any(|(_, t)| *t == Target::Row(5)));
        assert!(app.ui.targets.iter().any(|(_, t)| *t == Target::Back));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn imports_saved_and_temporary_lyrics_end_to_end() {
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
        let press = |app: &mut App, code: KeyCode| app.on_event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
        let (player_tx, _player_rx) = tokio::sync::mpsc::unbounded_channel();
        let (msg_tx, mut msg_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(crate::config::Config::default(), crate::term::TermCaps::default(), player_tx, msg_tx);
        let dir = std::env::temp_dir().join(format!("lyrminal-e2e-{}", std::process::id()));
        app.library = crate::library::Library::new(dir.clone());
        let track = crate::player::TrackMeta {
            title: "Lyrminal Test Song".into(),
            artists: vec!["Nobody".into()],
            length: Some(30.0),
            ..Default::default()
        };
        let snapshot = |t: crate::player::TrackMeta| {
            crate::player::PlayerEvent::Active(Some(crate::player::Snapshot {
                player: crate::player::PlayerInfo {
                    bus: "org.mpris.MediaPlayer2.test".into(),
                    identity: "Test".into(),
                    process: None,
                    pid: None,
                    desktop_entry: None,
                    status: crate::player::Status::Paused,
                    now_playing: None,
                },
                track: Some(t),
                status: crate::player::Status::Paused,
                rate: 1.0,
                position: 0.0,
                sampled_at: Instant::now(),
                caps: Default::default(),
            }))
        };
        app.on_player(snapshot(track.clone()));

        press(&mut app, KeyCode::Char('L'));
        assert!(matches!(&app.overlay, Overlay::Library(d) if d.view == library_ui::View::Library));
        app.on_event(Event::Paste("1\n00:00:01,000 --> 00:00:02,000\nsaved line\n".into()));
        press(&mut app, KeyCode::Enter);
        let msg = tokio::time::timeout(std::time::Duration::from_secs(10), msg_rx.recv())
            .await
            .expect("the import never finished")
            .unwrap();
        app.on_msg(msg);
        assert!(matches!(app.overlay, Overlay::None));
        match &app.lyrics {
            LyricsState::Ready { prepared, resolution, .. } => {
                assert_eq!(resolution.candidate.provider, crate::providers::local::ID);
                assert_eq!(prepared.lyrics.lines[0].lead.text(), "saved line");
            }
            _ => panic!("lyrics were not installed"),
        }
        let entries = app.library.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].detected, Some(crate::lyrics::formats::Format::Srt));
        assert_eq!(entries[0].key, track.query().key());

        press(&mut app, KeyCode::Char('L'));
        press(&mut app, KeyCode::Char('i'));
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Right);
        app.on_event(Event::Paste("[00:01.00]temporary line".into()));
        press(&mut app, KeyCode::Enter);
        let msg = tokio::time::timeout(std::time::Duration::from_secs(10), msg_rx.recv())
            .await
            .expect("the import never finished")
            .unwrap();
        app.on_msg(msg);
        assert!(app.temporary.is_some());
        assert_eq!(app.library.entries().len(), 1);
        match &app.lyrics {
            LyricsState::Ready { prepared, .. } => assert_eq!(prepared.lyrics.lines[0].lead.text(), "temporary line"),
            _ => panic!("temporary lyrics were not installed"),
        }

        let mut other = track.clone();
        other.title = "Another Song".into();
        app.on_player(snapshot(other));
        assert!(app.temporary.is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn later_targets_win_and_are_clipped() {
        let bounds = Rect::new(0, 0, 20, 5);
        let mut hits = UiHits::default();
        hits.push(row_rect(0, 1, 20), Target::Row(3), bounds);
        hits.push(row_rect(10, 1, 2), Target::Increase, bounds);
        hits.push(row_rect(0, 9, 20), Target::Row(4), bounds);
        assert_eq!(hits.at(5, 1), Some(Target::Row(3)));
        assert_eq!(hits.at(11, 1), Some(Target::Increase));
        assert_eq!(hits.at(5, 9), None);
        assert_eq!(hits.targets.len(), 2);
    }
}
