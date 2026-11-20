pub mod layout;

use std::collections::HashMap;
use std::sync::Arc;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use self::layout::{Group, Layout, LayoutOptions, Row, RowKind, build, row_x};
use super::effects::{self, EdgeWord, bloom, spring};
use super::scroll::{ScrollInput, ScrollMode, Scroller};
use super::{FrameInfo, FrameInput, LinkSpan};
use crate::anim::{Rgb, lerp};
use crate::config::{Alignment, Background, VisualStyle};
use crate::lyrics::process::{self, ItemKind, Prepared, State};
use crate::lyrics::{SyncKind, VocalGroup};

const BLOOM_FADE_COLS: f64 = 2.0;

#[derive(Default)]
pub struct CellRenderer {
    layout: Option<(usize, LayoutOptions, Arc<Layout>)>,
    scroller: Scroller,
    prepared_id: usize,
}

#[derive(Clone, Copy)]
struct Ink {
    alpha: f32,
    glow: f32,
    bold: bool,
    italic: bool,
    underline: bool,
}

impl Ink {
    fn new(alpha: f32) -> Self {
        Self {
            alpha,
            glow: 0.0,
            bold: false,
            italic: false,
            underline: false,
        }
    }
}

fn group_of<'a>(prepared: &'a Prepared, line: usize, group: Group) -> Option<&'a VocalGroup> {
    let l = prepared.lyrics.lines.get(line)?;
    match group {
        Group::Lead => Some(&l.lead),
        Group::Bg(i) => l.background.get(i),
    }
}

fn row_edge_words(row: &Row, group: &VocalGroup) -> Vec<EdgeWord> {
    let mut words: Vec<EdgeWord> = Vec::new();
    let mut current: Option<(usize, EdgeWord)> = None;
    for g in row.glyphs.iter().filter(|g| !g.space) {
        let s = &group.syllables[g.syl.min(group.syllables.len() - 1)];
        match current.as_mut() {
            Some((w, ew)) if *w == g.word => {
                ew.w = (g.col + g.width) as f64 - ew.x;
                ew.start = ew.start.min(s.start);
                ew.end = ew.end.max(s.end);
                ew.letters += 1;
            }
            _ => {
                if let Some((_, ew)) = current.take() {
                    words.push(ew);
                }
                current = Some((
                    g.word,
                    EdgeWord {
                        x: g.col as f64,
                        w: g.width as f64,
                        start: s.start,
                        end: s.end,
                        letters: 1,
                    },
                ));
            }
        }
    }
    if let Some((_, ew)) = current {
        words.push(ew);
    }
    words
}

struct WordTiming {
    start: f64,
    end: f64,
    letters: usize,
    emphasized: bool,
}

fn word_timings(group: &VocalGroup, background: bool) -> HashMap<usize, WordTiming> {
    process::words(group)
        .into_iter()
        .enumerate()
        .map(|(i, w)| {
            let emphasized = process::bloom_emphasis(group, &w, background);
            (
                i,
                WordTiming {
                    start: w.start,
                    end: w.end,
                    letters: w.letters,
                    emphasized,
                },
            )
        })
        .collect()
}

impl CellRenderer {
    pub fn reset(&mut self) {
        self.layout = None;
        self.scroller.reset();
    }

    pub fn is_animating(&self) -> bool {
        self.scroller.is_animating()
    }

    fn layout_for(&mut self, prepared: &Prepared, prepared_id: usize, opts: LayoutOptions) -> Arc<Layout> {
        if prepared_id != self.prepared_id {
            self.prepared_id = prepared_id;
            self.scroller.reset();
        }
        if let Some((id, o, l)) = &self.layout
            && *id == prepared_id
            && *o == opts
        {
            return l.clone();
        }
        let l = Arc::new(build(prepared, opts));
        self.layout = Some((prepared_id, opts, l.clone()));
        l
    }

    pub fn render(&mut self, input: &FrameInput, prepared_id: usize, area: Rect, buf: &mut Buffer) -> FrameInfo {
        let mut info = FrameInfo::default();
        if area.width < 8 || area.height < 3 {
            return info;
        }
        let margin = (area.width / 12).clamp(2, 8);
        let content_w = area.width.saturating_sub(margin * 2).min(100);
        let x0 = area.x + (area.width - content_w) / 2;
        let opts = LayoutOptions {
            width: content_w,
            romanization: input.lyrics.romanization,
            translation: input.lyrics.translation,
            background: input.lyrics.background_vocals,
            gap: input.appearance.line_gap,
            center: input.appearance.alignment == Alignment::Center,
        };
        let layout = self.layout_for(input.prepared, prepared_id, opts);
        let prepared = input.prepared;
        let kind = prepared.lyrics.kind;
        let style = input.style();
        let t = if style == VisualStyle::Bloom { input.t - bloom::CLOCK_LAG } else { input.t };
        let anchor = (area.height as f64 * 0.4).floor();
        let target = prepared.scroll_target(t);
        let auto_desired = if kind == SyncKind::Static {
            -1.0
        } else {
            layout.item_tops.get(target).copied().unwrap_or(0.0) - anchor
        };
        let manual = input.follow.manual || kind == SyncKind::Static;
        let desired = if manual { input.follow.offset } else { auto_desired };
        let silence = prepared
            .items
            .get(target)
            .map(|it| {
                let prev_end = target
                    .checked_sub(1)
                    .and_then(|p| prepared.items.get(p))
                    .map(|p| p.end)
                    .unwrap_or(0.0);
                (it.start - prev_end).max(0.0)
            })
            .unwrap_or(0.5);
        self.scroller.update(ScrollInput {
            now: input.wall,
            dt: input.dt,
            desired,
            target,
            tops: &layout.item_tops,
            mode: if style == VisualStyle::Bloom {
                ScrollMode::Ripple
            } else {
                ScrollMode::Rigid
            },
            silence,
            viewport: area.height as f64,
            manual,
        });
        let base = self.scroller.base();
        info.base = if manual { input.follow.offset } else { auto_desired };
        info.total = layout.total as f64;
        info.viewport = area.height as f64;

        let cursor_item = if input.follow.manual {
            let probe = base + anchor;
            layout
                .item_tops
                .iter()
                .enumerate()
                .min_by(|a, b| {
                    (a.1 - probe)
                        .abs()
                        .partial_cmp(&(b.1 - probe).abs())
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(i, _)| i)
        } else {
            None
        };
        info.cursor_item = cursor_item;

        let all_sung = prepared.all_sung(t);
        let mut timing_cache: HashMap<(usize, Group), HashMap<usize, WordTiming>> = HashMap::new();
        let mut any_active = false;

        for block in &layout.blocks {
            let item_offset = block.item.map(|i| self.scroller.item_offset(i, input.wall)).unwrap_or(0.0);
            let block_y = block.top as f64 - base + item_offset;
            if block_y > area.height as f64 + 1.0 || block_y + (block.rows.len() as f64) < -1.0 {
                continue;
            }
            let item = block.item.and_then(|i| prepared.items.get(i).map(|it| (i, *it)));
            let item_state = item
                .map(|(_, it)| {
                    if kind == SyncKind::Static {
                        State::Active
                    } else {
                        process::state(t, it.start, it.end)
                    }
                })
                .unwrap_or(State::NotSung);
            if item_state == State::Active && kind != SyncKind::Static {
                any_active = true;
            }
            let distance = item.map(|(i, _)| i.abs_diff(target)).unwrap_or(0);
            let depth = if input.appearance.depth_dimming && !manual && item_state != State::Active && kind != SyncKind::Static {
                1.0 - 0.09 * distance.min(5) as f32
            } else {
                1.0
            };
            let fade = if input.appearance.fade_previous && !manual {
                match item {
                    Some((i, it)) if item_state == State::Sung && i < target => {
                        1.0 - ((t - it.end) / 0.4).clamp(0.0, 1.0) as f32
                    }
                    _ => 1.0,
                }
            } else {
                1.0
            };

            for (ri, row) in block.rows.iter().enumerate() {
                let y = (block_y + ri as f64).round();
                if y < 0.0 || y >= area.height as f64 {
                    continue;
                }
                let sy = area.y + y as u16;
                let bg = if input.theme.paint != Background::None {
                    Some(input.theme.bg_at(sy, input.screen_height))
                } else {
                    None
                };
                let base_bg = bg.unwrap_or(input.theme.base);
                let rx = x0 + row_x(row, content_w);

                if row.kind == RowKind::Interlude {
                    if let Some((_, it)) = item
                        && item_state == State::Active
                        && let ItemKind::Interlude { start, end } = it.kind
                    {
                        let dots = if style == VisualStyle::Bloom {
                            effects::bloom_dots(t, start, end)
                        } else {
                            effects::spring_dots(t, start, end)
                        };
                        for (di, d) in dots.iter().enumerate() {
                            let symbol = if d.scale < 0.6 {
                                if d.opacity < 0.05 { " " } else { "·" }
                            } else if d.scale < 0.88 {
                                "·"
                            } else if d.scale < 1.08 {
                                "•"
                            } else {
                                "●"
                            };
                            let alpha = if style == VisualStyle::Bloom {
                                d.opacity as f32
                            } else {
                                (d.opacity * (0.55 + 0.4 * d.fill)) as f32
                            };
                            let mut ink = Ink::new(alpha);
                            ink.glow = d.glow as f32 * 0.6;
                            ink.bold = d.glow > 0.7;
                            let x = rx + di as u16 * 2;
                            put(buf, x, sy, symbol, ink, input, base_bg, bg);
                        }
                    }
                    continue;
                }

                let line_idx = match item.map(|(_, it)| it.kind) {
                    Some(ItemKind::Line(li)) => Some(li),
                    _ => None,
                };
                let link_row = matches!(row.kind, RowKind::Credit { .. }) && row.link.is_some();
                let mut link_text = String::new();
                let mut link_ink: Option<Ink> = None;

                let bloom_edge = if style == VisualStyle::Bloom
                    && item_state == State::Active
                    && kind == SyncKind::Syllable
                    && let (Some(li), RowKind::Lyric(g) | RowKind::Roman { group: g, timed: true }) = (line_idx, &row.kind)
                    && let Some(group) = group_of(prepared, li, *g)
                {
                    Some(effects::bloom_row_edge(&row_edge_words(row, group), t, BLOOM_FADE_COLS))
                } else {
                    None
                };

                for glyph in &row.glyphs {
                    let mut ink = self.glyph_ink(
                        input,
                        prepared,
                        &mut timing_cache,
                        row,
                        glyph,
                        line_idx,
                        item,
                        item_state,
                        t,
                        bloom_edge,
                        all_sung,
                    );
                    ink.alpha *= depth * fade;
                    let x = rx + glyph.col;
                    if x >= area.x + area.width {
                        continue;
                    }
                    if link_row {
                        link_text.push_str(&glyph.text);
                        ink.underline = true;
                        link_ink.get_or_insert(ink);
                    }
                    put(buf, x, sy, &glyph.text, ink, input, base_bg, bg);
                }
                if link_row
                    && let (Some(url), Some(ink)) = (row.link.clone(), link_ink)
                {
                    let fg = shade(ink, input, base_bg);
                    info.links.push(LinkSpan {
                        x: rx,
                        y: sy,
                        text: link_text,
                        url,
                        fg,
                        bg,
                        bold: ink.bold,
                    });
                }
            }

            if input.show_cursor
                && cursor_item.is_some()
                && cursor_item == block.item
            {
                let y = block_y.round();
                if y >= 0.0 && y < area.height as f64 {
                    let sy = area.y + y as u16;
                    let bg = (input.theme.paint != Background::None).then(|| input.theme.bg_at(sy, input.screen_height));
                    let mut ink = Ink::new(1.0);
                    ink.glow = 1.0;
                    put(buf, x0.saturating_sub(2), sy, "▌", ink, input, bg.unwrap_or(input.theme.base), bg);
                }
            }
        }
        info.animating = any_active || self.scroller.is_animating();
        info
    }

    #[allow(clippy::too_many_arguments)]
    fn glyph_ink(
        &self,
        input: &FrameInput,
        prepared: &Prepared,
        timing_cache: &mut HashMap<(usize, Group), HashMap<usize, WordTiming>>,
        row: &Row,
        glyph: &layout::Glyph,
        line_idx: Option<usize>,
        item: Option<(usize, process::Item)>,
        item_state: State,
        t: f64,
        bloom_edge: Option<f64>,
        all_sung: bool,
    ) -> Ink {
        let style = input.style();
        let kind = prepared.lyrics.kind;
        let contrast = input.appearance.contrast;
        let finish = |mut ink: Ink| {
            ink.alpha = (ink.alpha * contrast).clamp(0.0, 1.0);
            ink
        };
        match &row.kind {
            RowKind::Credit { emphasis } => {
                let a = if all_sung { 0.8 } else { 0.5 };
                let mut ink = Ink::new(if *emphasis { a } else { a * 0.8 });
                ink.bold = *emphasis && row.link.is_some();
                return finish(ink);
            }
            RowKind::Notice => return finish(Ink::new(0.7)),
            RowKind::Translation | RowKind::Roman { timed: false, .. } => {
                let a = match (kind, item_state) {
                    (SyncKind::Static, _) => 0.6,
                    (_, State::Active) => 0.62,
                    (_, State::Sung) => 0.32,
                    _ => 0.26,
                };
                let mut ink = Ink::new(a);
                ink.italic = true;
                return finish(ink);
            }
            RowKind::Interlude => return Ink::new(0.0),
            _ => {}
        }
        let (group_ref, roman) = match &row.kind {
            RowKind::Lyric(g) => (*g, false),
            RowKind::Roman { group, .. } => (*group, true),
            _ => (Group::Lead, false),
        };
        let background = matches!(group_ref, Group::Bg(_));
        let scale_for = |a: f32| if roman { a * 0.82 } else { a };
        let mut ink = Ink::new(0.0);
        ink.italic = background || roman;

        if kind == SyncKind::Static {
            ink.alpha = scale_for(0.9);
            return finish(ink);
        }
        let Some(li) = line_idx else { return finish(Ink::new(0.4)) };
        let Some(group) = group_of(prepared, li, group_ref) else { return finish(Ink::new(0.4)) };
        let (_, it) = item.unwrap();

        match style {
            VisualStyle::Spring => {
                let (unlit, lit) = if background {
                    (spring::BG_UNLIT, spring::BG_LIT)
                } else {
                    (spring::ACTIVE_UNLIT, spring::ACTIVE_LIT)
                };
                match item_state {
                    State::NotSung => ink.alpha = if background { spring::NOT_SUNG * 0.8 } else { spring::NOT_SUNG },
                    State::Sung => ink.alpha = if background { spring::SUNG * 0.8 } else { spring::SUNG },
                    State::Active => {
                        if kind == SyncKind::Line {
                            let p = process::progress(t, it.start, prepared.lyrics.lines[li].end);
                            let y = (row.row_in_group as f64 + 0.5) / row.rows_in_group.max(1) as f64;
                            let fill = effects::vertical_fill(p, y);
                            ink.alpha = lerp(spring::LINE_UNLIT as f64, lit as f64, fill) as f32;
                            if input.appearance.glow {
                                ink.glow = (effects::SPLINES.line_glow.at(p) * 0.45) as f32;
                            }
                        } else {
                            let s = &group.syllables[glyph.syl.min(group.syllables.len() - 1)];
                            let emphasized = input.appearance.emphasis && !roman && process::spring_emphasis(s);
                            if emphasized {
                                let n = glyph.syl_len.max(1);
                                let (ws, we) = effects::spring_letter_window(s.start, s.end, n, glyph.index_in_syl);
                                let fill = effects::spring_letter_fill(t, ws, we);
                                ink.alpha = lerp(unlit as f64, lit as f64, fill) as f32;
                                if input.appearance.glow {
                                    if t >= s.start && t < s.end {
                                        let span = (s.end - spring::LETTER_TAIL - s.start).max(0.01) / n as f64;
                                        let active = (((t - s.start) / span).floor() as usize).min(n - 1);
                                        let (as_, ae) = effects::spring_letter_window(s.start, s.end, n, active);
                                        let p_active = process::progress(t, as_, ae);
                                        let d = active.abs_diff(glyph.index_in_syl) as f64;
                                        ink.glow = (effects::glow(p_active) * effects::neighbour_glow(d)) as f32;
                                    } else if t >= s.end {
                                        ink.glow = 0.25;
                                    }
                                    ink.bold = ink.glow > 0.45;
                                }
                            } else if t < s.start {
                                ink.alpha = unlit;
                            } else if t >= s.end {
                                ink.alpha = lit;
                            } else {
                                let p = process::progress(t, s.start, s.end);
                                let x = (glyph.index_in_syl as f64 + 0.5) / glyph.syl_len.max(1) as f64;
                                ink.alpha = lerp(unlit as f64, lit as f64, effects::spring_fill(p, x)) as f32;
                                if input.appearance.glow {
                                    ink.glow = (effects::glow(p) * 0.4) as f32;
                                }
                            }
                        }
                    }
                }
            }
            VisualStyle::Bloom => {
                let lit_tone = if background { bloom::BG_LIT } else { bloom::LIT };
                let unlit = bloom::UNLIT;
                match item_state {
                    State::NotSung => ink.alpha = unlit,
                    State::Sung => {
                        let since = t - prepared.lyrics.lines[li].full_end();
                        ink.alpha = lerp(lit_tone as f64, unlit as f64, effects::bloom_sung_dim(since)) as f32;
                    }
                    State::Active => {
                        if kind == SyncKind::Line {
                            let line = &prepared.lyrics.lines[li];
                            let tone = effects::bloom_line_tone(t, line.start, line.end);
                            ink.alpha = lerp(unlit as f64, lit_tone as f64, tone) as f32;
                        } else {
                            let lit = match bloom_edge {
                                Some(edge) => {
                                    effects::bloom_edge_lit(edge, glyph.col as f64 + glyph.width as f64 / 2.0, BLOOM_FADE_COLS)
                                }
                                None => {
                                    let s = &group.syllables[glyph.syl.min(group.syllables.len() - 1)];
                                    if t >= s.end { 1.0 } else { 0.0 }
                                }
                            };
                            ink.alpha = lerp(unlit as f64, lit_tone as f64, lit) as f32;
                            if input.appearance.emphasis && input.appearance.glow && !roman && !glyph.space {
                                let timings = timing_cache
                                    .entry((li, group_ref))
                                    .or_insert_with(|| word_timings(group, background));
                                if let Some(w) = timings.get(&glyph.word)
                                    && w.emphasized
                                    && t >= w.start
                                {
                                    let amount = effects::bloom_flame(t - w.start, w.end - w.start, w.letters, glyph.letter);
                                    ink.glow = (amount * 0.75 * lit.max(0.4)) as f32;
                                    ink.bold = amount > 0.35;
                                }
                            }
                        }
                    }
                }
            }
        }
        ink.alpha = scale_for(ink.alpha);
        finish(ink)
    }
}

fn shade(ink: Ink, input: &FrameInput, base_bg: Rgb) -> Rgb {
    let fg = base_bg.mix(input.theme.text, ink.alpha);
    if ink.glow > 0.0 {
        let glow_color = input.theme.accent.mix(Rgb::WHITE, 0.45);
        fg.mix(glow_color, ink.glow * 0.55)
    } else {
        fg
    }
}

#[allow(clippy::too_many_arguments)]
fn put(buf: &mut Buffer, x: u16, y: u16, symbol: &str, ink: Ink, input: &FrameInput, base_bg: Rgb, bg: Option<Rgb>) {
    let area = buf.area;
    if x < area.x || x >= area.x + area.width || y < area.y || y >= area.y + area.height {
        return;
    }
    let fg = shade(ink, input, base_bg);
    let mut style = Style::default().fg(fg.to_color());
    if let Some(bg) = bg {
        style = style.bg(bg.to_color());
    }
    let mut m = Modifier::empty();
    if ink.bold {
        m |= Modifier::BOLD;
    }
    if ink.italic {
        m |= Modifier::ITALIC;
    }
    if ink.underline {
        m |= Modifier::UNDERLINED;
    }
    style = style.add_modifier(m);
    buf.set_string(x, y, symbol, style);
}
