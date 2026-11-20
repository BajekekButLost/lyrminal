pub mod raster;

use std::collections::HashMap;
use std::ops::Range;

use cosmic_text::{Align, Attrs, Buffer, CacheKey, Family, FontSystem, LayoutGlyph, Metrics, Shaping, SwashCache, SwashContent, Weight, Wrap};
use ratatui::layout::Rect;

use self::raster::{Canvas, Mask};
use super::effects::{self, EdgeWord, bloom, spring};
use super::scroll::{ScrollInput, Scroller};
use super::{FrameInfo, FrameInput, push_px_hit};
use crate::config::ClickSeek;
use crate::anim::{Rgb, Spring, lerp};
use crate::config::{Alignment, Background, VisualStyle};
use crate::lyrics::process::{self, ItemKind, Prepared, State};
use crate::lyrics::{SyncKind, VocalGroup};

pub struct Fonts {
    pub system: FontSystem,
    pub swash: SwashCache,
}

pub fn load_fonts() -> Fonts {
    Fonts {
        system: FontSystem::new(),
        swash: SwashCache::new(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum PartKind {
    Lead,
    Bg(usize),
    Roman { bg: Option<usize>, timed: bool },
    Translation,
}

#[derive(Debug, Clone)]
struct PGlyph {
    glyph: LayoutGlyph,
    baseline: f32,
    syl: usize,
    word: usize,
    letter: usize,
    letter_in_syl: usize,
    blank: bool,
}

#[derive(Debug, Clone)]
struct Part {
    kind: PartKind,
    size: f32,
    top: f32,
    height: f32,
    x: f32,
    glyphs: Vec<PGlyph>,
    syl_boxes: Vec<(f32, f32)>,
    syl_letters: Vec<usize>,
    word_letters: Vec<usize>,
}

#[derive(Debug, Clone)]
struct PBlock {
    item: usize,
    top: f32,
    height: f32,
    parts: Vec<Part>,
}

#[derive(Debug, Clone, PartialEq)]
struct LayoutKey {
    prepared: usize,
    width: u32,
    size: u32,
    family: String,
    weight: u16,
    romanization: bool,
    translation: bool,
    background: bool,
    center: bool,
    bloom: bool,
}

struct PLayout {
    key: LayoutKey,
    blocks: Vec<PBlock>,
    item_tops: Vec<f64>,
    total: f32,
    credits_top: f32,
    content_w: f32,
}

#[derive(Clone, Copy)]
struct SylAnim {
    scale: Spring,
    lift: Spring,
    glow: Spring,
}

impl SylAnim {
    fn new() -> Self {
        Self {
            scale: Spring::new(0.95, 0.88, 0.64),
            lift: Spring::new(0.01, 1.45, 0.4),
            glow: Spring::new(0.0, 1.18, 0.56),
        }
    }
}

pub struct PixelFrame {
    pub info: FrameInfo,
    pub credits_y: Option<f32>,
    pub content_x: f32,
}

pub struct PixelRenderer {
    pub fonts: Fonts,
    pub canvas: Canvas,
    layout: Option<PLayout>,
    scroller: Scroller,
    masks: HashMap<(usize, usize, u16), Mask>,
    glows: HashMap<CacheKey, Mask>,
    anims: HashMap<(usize, PartKind, usize), SylAnim>,
    prepared_id: usize,
}

fn group_text(group: &VocalGroup, roman: bool) -> (String, Vec<Range<usize>>) {
    let mut text = String::new();
    let mut ranges = Vec::new();
    let n = group.syllables.len();
    for (i, s) in group.syllables.iter().enumerate() {
        let (t, part) = if roman {
            match &s.roman {
                Some(r) => (r.as_str(), s.roman_part_of_word),
                None => (s.text.as_str(), s.part_of_word),
            }
        } else {
            (s.text.as_str(), s.part_of_word)
        };
        let start = text.len();
        text.push_str(t);
        ranges.push(start..text.len());
        if !part && i + 1 < n {
            text.push(' ');
        }
    }
    (text, ranges)
}

fn bloom_lift(tau: f64) -> f64 {
    if tau <= 0.0 {
        return 0.0;
    }
    let w = 5.0;
    let z = 0.93;
    let wd = w * (1.0f64 - z * z).sqrt();
    1.0 - (-z * w * tau).exp() * ((wd * tau).cos() + (z * w / wd) * (wd * tau).sin())
}

struct ShapeSpec<'a> {
    text: &'a str,
    ranges: &'a [Range<usize>],
    words_of_syl: &'a [usize],
    size: f32,
    width: f32,
    align: Align,
    family: &'a str,
    weight: u16,
}

fn shape(fonts: &mut Fonts, spec: ShapeSpec) -> (Vec<PGlyph>, f32) {
    let mut buffer = Buffer::new(&mut fonts.system, Metrics::new(spec.size, spec.size * 1.22));
    buffer.set_size(Some(spec.width.max(spec.size * 2.0)), None);
    buffer.set_wrap(Wrap::WordOrGlyph);
    let family = if spec.family.trim().is_empty() {
        Family::SansSerif
    } else {
        Family::Name(spec.family.trim())
    };
    let attrs = Attrs::new().family(family).weight(Weight(spec.weight));
    buffer.set_text(spec.text, &attrs, Shaping::Advanced, Some(spec.align));
    buffer.shape_until_scroll(&mut fonts.system, false);
    let mut glyphs = Vec::new();
    let mut height: f32 = 0.0;
    for run in buffer.layout_runs() {
        height = height.max(run.line_top + run.line_height);
        for g in run.glyphs {
            let syl = spec
                .ranges
                .iter()
                .position(|r| g.start >= r.start && g.start < r.end)
                .or_else(|| spec.ranges.iter().rposition(|r| r.start <= g.start))
                .unwrap_or(0);
            let blank = spec.text[g.start..g.end.min(spec.text.len())].chars().all(char::is_whitespace);
            glyphs.push(PGlyph {
                glyph: g.clone(),
                baseline: run.line_y,
                syl,
                word: spec.words_of_syl.get(syl).copied().unwrap_or(0),
                letter: 0,
                letter_in_syl: 0,
                blank,
            });
        }
    }
    (glyphs, height.max(spec.size * 1.22))
}

impl PixelRenderer {
    pub fn new(fonts: Fonts) -> Self {
        Self {
            fonts,
            canvas: Canvas::new(1, 1),
            layout: None,
            scroller: Scroller::default(),
            masks: HashMap::new(),
            glows: HashMap::new(),
            anims: HashMap::new(),
            prepared_id: 0,
        }
    }

    pub fn reset(&mut self) {
        self.layout = None;
        self.masks.clear();
        self.anims.clear();
        self.scroller.reset();
    }

    #[allow(clippy::too_many_arguments)]
    fn build_part(
        &mut self,
        kind: PartKind,
        group: &VocalGroup,
        roman: bool,
        size: f32,
        width: f32,
        x: f32,
        align: Align,
        family: &str,
        weight: u16,
    ) -> Part {
        let (text, ranges) = group_text(group, roman);
        let words = process::words(group);
        let mut words_of_syl = vec![0usize; group.syllables.len()];
        for (wi, w) in words.iter().enumerate() {
            for s in w.syllables.clone() {
                words_of_syl[s] = wi;
            }
        }
        let (mut glyphs, height) = shape(
            &mut self.fonts,
            ShapeSpec {
                text: &text,
                ranges: &ranges,
                words_of_syl: &words_of_syl,
                size,
                width,
                align,
                family,
                weight,
            },
        );
        let mut syl_boxes = vec![(f32::MAX, f32::MIN); group.syllables.len().max(1)];
        let mut syl_letters = vec![0usize; group.syllables.len().max(1)];
        let mut word_letters = vec![0usize; words.len().max(1)];
        for g in glyphs.iter_mut() {
            if g.blank {
                continue;
            }
            let si = g.syl.min(syl_boxes.len() - 1);
            let b = &mut syl_boxes[si];
            b.0 = b.0.min(g.glyph.x);
            b.1 = b.1.max(g.glyph.x + g.glyph.w);
            g.letter_in_syl = syl_letters[si];
            syl_letters[si] += 1;
            let wl = g.word.min(word_letters.len() - 1);
            g.letter = word_letters[wl];
            word_letters[wl] += 1;
        }
        Part {
            kind,
            size,
            top: 0.0,
            height,
            x,
            glyphs,
            syl_boxes,
            syl_letters,
            word_letters,
        }
    }

    fn build_layout(&mut self, prepared: &Prepared, key: LayoutKey, content_w: f32, input: &FrameInput) {
        let size = key.size as f32;
        let has_duets = prepared.lyrics.lines.iter().any(|l| l.opposite);
        let inset = if has_duets && !key.center { content_w * 0.15 } else { 0.0 };
        let width = content_w - inset;
        let gap = if key.bloom { size * 0.8 } else { size * 0.6 };
        let family = key.family.clone();
        let weight = key.weight;
        let mut blocks = Vec::new();
        let mut item_tops = Vec::new();
        let mut top = 0.0f32;
        for (idx, item) in prepared.items.iter().enumerate() {
            let (align, x) = if key.center {
                (Align::Center, 0.0)
            } else if item.opposite {
                (Align::Right, inset)
            } else {
                (Align::Left, 0.0)
            };
            let mut parts = Vec::new();
            let mut height;
            match item.kind {
                ItemKind::Interlude { .. } => {
                    height = size * 0.9;
                }
                ItemKind::Line(li) => {
                    let line = prepared.lyrics.lines[li].clone();
                    let mut y = 0.0;
                    let mut push = |p: Part, parts: &mut Vec<Part>, gap_after: f32| {
                        let mut p = p;
                        p.top = y;
                        y += p.height + gap_after;
                        parts.push(p);
                    };
                    let lead = self.build_part(PartKind::Lead, &line.lead, false, size, width, x, align, &family, weight);
                    push(lead, &mut parts, size * 0.08);
                    if key.romanization && (line.lead.syllables.iter().any(|s| s.roman.is_some()) || line.lead.roman.is_some()) {
                        let timed = line.lead.syllables.iter().any(|s| s.roman.is_some()) && prepared.lyrics.kind == SyncKind::Syllable;
                        let group = if timed {
                            line.lead.clone()
                        } else {
                            VocalGroup::single(line.lead.roman_text().unwrap_or_default(), line.start, line.end)
                        };
                        let p = self.build_part(PartKind::Roman { bg: None, timed }, &group, timed, size * 0.62, width, x, align, &family, weight);
                        push(p, &mut parts, size * 0.08);
                    }
                    if key.background {
                        for (bi, bg) in line.background.iter().enumerate() {
                            let p = self.build_part(PartKind::Bg(bi), bg, false, size * 0.72, width, x, align, &family, weight);
                            push(p, &mut parts, size * 0.06);
                        }
                    }
                    if key.translation
                        && let Some(tr) = &line.lead.translation
                    {
                        let g = VocalGroup::single(tr.clone(), line.start, line.end);
                        let p = self.build_part(PartKind::Translation, &g, false, size * 0.58, width, x, align, &family, 400);
                        push(p, &mut parts, 0.0);
                    }
                    height = y;
                }
            }
            if height <= 0.0 {
                height = size;
            }
            item_tops.push(top as f64);
            blocks.push(PBlock {
                item: idx,
                top,
                height,
                parts,
            });
            top += height + gap;
        }
        let _ = input;
        self.layout = Some(PLayout {
            key,
            blocks,
            item_tops,
            credits_top: top + size * 0.4,
            total: top + size * 3.0,
            content_w,
        });
        self.masks.clear();
        self.anims.clear();
    }

    fn part_mask(&mut self, bi: usize, pi: usize, blur: u16) -> Option<&Mask> {
        if !self.masks.contains_key(&(bi, pi, blur)) {
            let layout = self.layout.as_ref()?;
            let part = layout.blocks.get(bi)?.parts.get(pi)?.clone();
            let content_w = layout.content_w;
            let pad = (part.size * 0.4) as i32;
            let mut mask = Mask::new(-pad, (part.top) as i32 - pad, (content_w as i32 + pad * 2) as usize, (part.height as i32 + pad * 2) as usize);
            for g in part.glyphs.iter().filter(|g| !g.blank) {
                let phys = g.glyph.physical((part.x, 0.0), 1.0);
                if let Some(img) = self.fonts.swash.get_image(&mut self.fonts.system, phys.cache_key).as_ref() {
                    let x = phys.x + img.placement.left;
                    let y = (part.top + g.baseline).round() as i32 + phys.y - img.placement.top;
                    let (w, h) = (img.placement.width as usize, img.placement.height as usize);
                    match img.content {
                        SwashContent::Mask => mask.stamp(&img.data, w, h, x, y),
                        SwashContent::SubpixelMask => {
                            let a: Vec<u8> = img.data.chunks_exact(4).map(|p| p[1]).collect();
                            mask.stamp(&a, w, h, x, y)
                        }
                        SwashContent::Color => {
                            let a: Vec<u8> = img.data.chunks_exact(4).map(|p| p[3]).collect();
                            mask.stamp(&a, w, h, x, y)
                        }
                    }
                }
            }
            let blurred = if blur > 0 { mask.blurred(blur as f32) } else { mask };
            self.masks.insert((bi, pi, blur), blurred);
        }
        self.masks.get(&(bi, pi, blur))
    }

    fn glow_mask(&mut self, key: CacheKey, radius: f32) -> Option<Mask> {
        if let Some(m) = self.glows.get(&key) {
            return Some(m.clone());
        }
        let img = self.fonts.swash.get_image(&mut self.fonts.system, key).as_ref()?;
        let (w, h) = (img.placement.width as usize, img.placement.height as usize);
        let alpha: Vec<u8> = match img.content {
            SwashContent::Mask => img.data.clone(),
            _ => img.data.chunks_exact(4).map(|p| p[3].max(p[1])).collect(),
        };
        let m = Mask {
            w,
            h,
            x: img.placement.left,
            y: -img.placement.top,
            data: alpha,
        }
        .blurred(radius);
        if self.glows.len() > 4096 {
            self.glows.clear();
        }
        self.glows.insert(key, m.clone());
        Some(m)
    }

    pub fn render(&mut self, input: &FrameInput, prepared_id: usize, screen: Rect, lyrics: Rect, cell: (f32, f32)) -> PixelFrame {
        let (cw, ch) = cell;
        let w = (screen.width as f32 * cw).round().max(1.0) as usize;
        let h = (screen.height as f32 * ch).round().max(1.0) as usize;
        self.canvas.resize(w, h);
        if input.theme.paint != Background::None {
            self.canvas.fill_gradient(input.theme.bg_top, input.theme.bg_bottom);
        } else {
            self.canvas.clear();
        }
        let mut info = FrameInfo::default();
        if prepared_id != self.prepared_id {
            self.prepared_id = prepared_id;
            self.reset();
        }
        let lx = lyrics.x as f32 * cw;
        let ly = lyrics.y as f32 * ch;
        let lw = lyrics.width as f32 * cw;
        let lh = lyrics.height as f32 * ch;
        let margin_cells = (lyrics.width / 12).clamp(2, 8);
        let content_cells = lyrics.width.saturating_sub(margin_cells * 2).min(100);
        let content_w = content_cells as f32 * cw;
        let x0 = lx + ((lyrics.width - content_cells) / 2) as f32 * cw;
        let clip = (lx as i32, ly as i32, (lx + lw) as i32, (ly + lh) as i32);
        let style = input.style();
        let size = (input.appearance.font_scale * ch).round().max(8.0);
        let key = LayoutKey {
            prepared: prepared_id,
            width: content_w as u32,
            size: size as u32,
            family: input.appearance.font_family.clone(),
            weight: input.appearance.font_weight,
            romanization: input.lyrics.romanization,
            translation: input.lyrics.translation,
            background: input.lyrics.background_vocals,
            center: input.appearance.alignment == Alignment::Center,
            bloom: style == VisualStyle::Bloom,
        };
        if self.layout.as_ref().is_none_or(|l| l.key != key) {
            self.build_layout(input.prepared, key, content_w, input);
            self.scroller.reset();
        }
        let prepared = input.prepared;
        let kind = prepared.lyrics.kind;
        let t = if style == VisualStyle::Bloom { input.t - bloom::CLOCK_LAG } else { input.t };
        let anchor = (lh * 0.4) as f64;
        let (target, silence) = input.scroll_target(t);
        let (item_tops, total, credits_top) = {
            let l = self.layout.as_ref().unwrap();
            (l.item_tops.clone(), l.total, l.credits_top)
        };
        let auto_desired = if kind == SyncKind::Static {
            -(ch as f64)
        } else {
            item_tops.get(target).copied().unwrap_or(0.0) - anchor
        };
        let manual = input.follow.manual || kind == SyncKind::Static;
        let desired = if manual { input.follow.offset * ch as f64 } else { auto_desired };
        self.scroller.update(ScrollInput {
            now: input.wall,
            dt: input.dt,
            desired,
            target,
            tops: &item_tops,
            mode: input.scroll_mode(),
            silence,
            viewport: lh as f64,
            manual,
            speed: input.scroll.speed,
        });
        let base = self.scroller.base();
        info.base = if manual { input.follow.offset } else { auto_desired / ch as f64 };
        info.total = total as f64 / ch as f64;
        info.viewport = lh as f64 / ch as f64;
        if input.follow.manual {
            let probe = base + anchor;
            info.cursor_item = item_tops
                .iter()
                .enumerate()
                .min_by(|a, b| (a.1 - probe).abs().partial_cmp(&(b.1 - probe).abs()).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(i, _)| i);
        }

        let contrast = input.appearance.contrast;
        let text = input.theme.text;
        let glow_color = input.theme.accent.mix(Rgb::WHITE, 0.45);
        let mut animating = self.scroller.is_animating();
        let block_count = self.layout.as_ref().map(|l| l.blocks.len()).unwrap_or(0);
        for bi in 0..block_count {
            let (item_idx, btop, bheight) = {
                let b = &self.layout.as_ref().unwrap().blocks[bi];
                (b.item, b.top, b.height)
            };
            let y = ly + (btop as f64 - base + self.scroller.item_offset(item_idx, input.wall)) as f32;
            if y > ly + lh + size || y + bheight < ly - size {
                continue;
            }
            let it = prepared.items[item_idx];
            let state = if kind == SyncKind::Static { State::Active } else { process::state(t, it.start, it.end) };
            let recently = state == State::Sung && t - it.end < 1.0;
            let distance = item_idx.abs_diff(target);
            let live = kind != SyncKind::Static && (state == State::Active || recently);
            if live {
                animating = true;
            }
            if let ItemKind::Interlude { start, end } = it.kind {
                if state == State::Active {
                    let dots = if style == VisualStyle::Bloom { effects::bloom_dots(t, start, end) } else { effects::spring_dots(t, start, end) };
                    let spacing = size * 0.62;
                    let r0 = size * 0.17;
                    let total_w = spacing * 2.0;
                    let bx = if input.appearance.alignment == Alignment::Center {
                        x0 + content_w / 2.0 - total_w / 2.0
                    } else if it.opposite {
                        x0 + content_w - total_w - r0 * 2.0
                    } else {
                        x0 + r0
                    };
                    let cy = y + size * 0.45;
                    for (i, d) in dots.iter().enumerate() {
                        let cx = bx + i as f32 * spacing;
                        let r = r0 * d.scale as f32;
                        let dy = d.lift as f32 * size;
                        let alpha = if style == VisualStyle::Bloom { d.opacity as f32 } else { (d.opacity * (0.55 + 0.4 * d.fill)) as f32 };
                        if input.appearance.glow && d.glow > 0.01 {
                            self.canvas.disc(cx, cy + dy, r, r * 1.6, glow_color, (d.glow * 0.35) as f32 * alpha, clip);
                        }
                        self.canvas.disc(cx, cy + dy, r, 0.0, text, alpha * 0.9, clip);
                    }
                }
                continue;
            }
            let ItemKind::Line(li) = it.kind else { continue };
            let part_count = self.layout.as_ref().unwrap().blocks[bi].parts.len();
            if input.scroll.click_seek != ClickSeek::Off && kind != SyncKind::Static {
                let line = &prepared.lyrics.lines[li];
                let parts = &self.layout.as_ref().unwrap().blocks[bi].parts;
                let clip_rows = (lyrics.y, lyrics.y + lyrics.height);
                for part in parts {
                    let group = match part.kind {
                        PartKind::Lead | PartKind::Roman { timed: true, .. } => Some(&line.lead),
                        PartKind::Bg(i) => line.background.get(i),
                        _ => None,
                    };
                    let words_mode = input.scroll.click_seek == ClickSeek::Words && kind == SyncKind::Syllable && group.is_some();
                    let mut boxes: HashMap<(i32, usize), (f32, f32, f32, f32, f64)> = HashMap::new();
                    for g in part.glyphs.iter().filter(|g| !g.blank) {
                        let key = (g.baseline as i32, if words_mode { g.word } else { 0 });
                        let gx0 = x0 + part.x + g.glyph.x;
                        let gx1 = gx0 + g.glyph.w;
                        let by = y + part.top + g.baseline;
                        let time = if words_mode {
                            group.and_then(|gr| gr.syllables.get(g.syl)).map(|s| s.start).unwrap_or(it.start)
                        } else {
                            it.start
                        };
                        let e = boxes.entry(key).or_insert((gx0, by - part.size * 0.85, gx1, by + part.size * 0.25, time));
                        e.0 = e.0.min(gx0);
                        e.2 = e.2.max(gx1);
                        e.4 = e.4.min(time);
                    }
                    for (_, (a, b, c, d, time)) in boxes {
                        push_px_hit(&mut info.hits, (a, b, c, d), (cw, ch), clip_rows, time);
                    }
                }
            }
            if !live {
                let depth_blur = if input.appearance.depth_dimming && !manual && kind != SyncKind::Static {
                    (distance.min(5) as f32 * 0.03 * size).round() as u16
                } else {
                    0
                };
                for pi in 0..part_count {
                    let pkind = self.layout.as_ref().unwrap().blocks[bi].parts[pi].kind;
                    let background = matches!(pkind, PartKind::Bg(_));
                    let mut alpha = match (kind, style, state) {
                        (SyncKind::Static, _, _) => 0.9,
                        (_, VisualStyle::Bloom, State::Sung) => {
                            let since = t - prepared.lyrics.lines[li].full_end();
                            lerp(if background { bloom::BG_LIT as f64 } else { bloom::LIT as f64 }, bloom::UNLIT as f64, effects::bloom_sung_dim(since)) as f32
                        }
                        (_, VisualStyle::Bloom, _) => bloom::UNLIT,
                        (_, VisualStyle::Spring, State::Sung) => spring::SUNG,
                        _ => spring::NOT_SUNG,
                    };
                    if background && style == VisualStyle::Spring {
                        alpha *= 0.8;
                    }
                    if matches!(pkind, PartKind::Translation | PartKind::Roman { .. }) {
                        alpha *= 0.8;
                    }
                    if input.appearance.fade_previous && !manual && state == State::Sung && item_idx < target {
                        alpha *= 1.0 - ((t - it.end) / 0.4).clamp(0.0, 1.0) as f32;
                    }
                    let alpha = (alpha * contrast).clamp(0.0, 1.0);
                    if alpha <= 0.01 {
                        continue;
                    }
                    if let Some(mask) = self.part_mask(bi, pi, depth_blur) {
                        let (mw, mh, mx, my) = (mask.w, mask.h, mask.x, mask.y);
                        let data = std::mem::take(&mut self.masks.get_mut(&(bi, pi, depth_blur)).unwrap().data);
                        self.canvas.draw_alpha(&data, mw, mh, x0 as i32 + mx, y as i32 + my, clip, |_| (text, alpha));
                        self.masks.get_mut(&(bi, pi, depth_blur)).unwrap().data = data;
                    }
                }
                continue;
            }
            for pi in 0..part_count {
                let part = self.layout.as_ref().unwrap().blocks[bi].parts[pi].clone();
                self.draw_live_part(input, &part, bi, li, state, t, x0, y, clip, glow_color);
            }
        }
        info.animating = animating;
        PixelFrame {
            info,
            credits_y: Some(ly + (credits_top as f64 - base) as f32).filter(|cy| *cy < ly + lh),
            content_x: x0,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_live_part(
        &mut self,
        input: &FrameInput,
        part: &Part,
        bi: usize,
        li: usize,
        state: State,
        t: f64,
        x0: f32,
        y: f32,
        clip: (i32, i32, i32, i32),
        glow_color: Rgb,
    ) {
        let prepared = input.prepared;
        let kind = prepared.lyrics.kind;
        let style = input.style();
        let contrast = input.appearance.contrast;
        let text = input.theme.text;
        let line = &prepared.lyrics.lines[li];
        let size = part.size;
        let background = matches!(part.kind, PartKind::Bg(_));
        let group: Option<&VocalGroup> = match part.kind {
            PartKind::Lead | PartKind::Roman { timed: true, .. } => Some(&line.lead),
            PartKind::Bg(i) => line.background.get(i),
            _ => None,
        };
        let px = x0 + part.x;
        let py = y + part.top;
        let static_alpha = |a: f32| (a * contrast).clamp(0.0, 1.0);

        let Some(group) = group.filter(|_| kind == SyncKind::Syllable) else {
            let alpha = match (part.kind, kind, style) {
                (PartKind::Translation | PartKind::Roman { .. }, _, _) => {
                    if state == State::Active { 0.62 } else { 0.32 }
                }
                (_, SyncKind::Line, VisualStyle::Bloom) => {
                    let tone = effects::bloom_line_tone(t, line.start, line.end);
                    lerp(bloom::UNLIT as f64, bloom::LIT as f64, tone) as f32
                }
                (_, SyncKind::Line, VisualStyle::Spring) => {
                    if state == State::Active {
                        let p = process::progress(t, line.start, line.end);
                        lerp(spring::LINE_UNLIT as f64, spring::ACTIVE_LIT as f64, effects::vertical_fill(p, 0.5)) as f32
                    } else {
                        spring::SUNG
                    }
                }
                _ => 0.6,
            };
            let glow = if kind == SyncKind::Line && style == VisualStyle::Spring && state == State::Active && input.appearance.glow {
                effects::SPLINES.line_glow.at(process::progress(t, line.start, line.end)) as f32 * 0.4
            } else {
                0.0
            };
            for g in part.glyphs.iter().filter(|g| !g.blank) {
                self.draw_glyph(&g.glyph, px, py + g.baseline, 1.0, (0.0, 0.0), clip, glow, glow_color, size, |_| (text, static_alpha(alpha)));
            }
            return;
        };

        let (unlit, lit) = match style {
            VisualStyle::Spring if background => (spring::BG_UNLIT, spring::BG_LIT),
            VisualStyle::Spring => (spring::ACTIVE_UNLIT, spring::ACTIVE_LIT),
            VisualStyle::Bloom if background => (bloom::UNLIT, bloom::BG_LIT),
            VisualStyle::Bloom => (bloom::UNLIT, bloom::LIT),
        };
        let sung_dim = if state == State::Sung {
            effects::bloom_sung_dim(t - line.full_end()) as f32
        } else {
            0.0
        };
        let spring_settle = if state == State::Sung {
            (((t - line.full_end()) / 0.25).clamp(0.0, 1.0)) as f32
        } else {
            0.0
        };
        let spring_rest = if background { spring::SUNG * 0.8 } else { spring::SUNG };
        let words = process::words(group);
        let line_done = if state == State::Sung { Some(line.full_end()) } else { None };

        if style == VisualStyle::Spring {
            let dt = input.dt;
            for si in 0..group.syllables.len() {
                let s = &group.syllables[si];
                let p = process::progress(t, s.start, s.end);
                let anim = self.anims.entry((bi, part.kind, si)).or_insert_with(SylAnim::new);
                anim.scale.set_goal(effects::SPLINES.word_scale.at(p));
                anim.lift.set_goal(effects::SPLINES.word_lift.at(p));
                anim.glow.set_goal(effects::glow(p));
                anim.scale.step(dt);
                anim.lift.step(dt);
                anim.glow.step(dt);
            }
        }

        let mut edges: HashMap<i32, f64> = HashMap::new();
        if style == VisualStyle::Bloom && state == State::Active {
            let mut rows: HashMap<i32, Vec<EdgeWord>> = HashMap::new();
            for (wi, w) in words.iter().enumerate() {
                let Some(first) = part.glyphs.iter().find(|g| g.word == wi && !g.blank) else { continue };
                let row = first.baseline as i32;
                let x_lo = w.syllables.clone().filter_map(|s| part.syl_boxes.get(s)).map(|b| b.0).fold(f32::MAX, f32::min);
                let x_hi = w.syllables.clone().filter_map(|s| part.syl_boxes.get(s)).map(|b| b.1).fold(f32::MIN, f32::max);
                rows.entry(row).or_default().push(EdgeWord {
                    x: x_lo as f64,
                    w: (x_hi - x_lo) as f64,
                    start: w.start,
                    end: w.end,
                    letters: w.letters,
                });
            }
            let fade = 0.882 * size as f64;
            for (row, mut ws) in rows {
                ws.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal));
                edges.insert(row, effects::bloom_row_edge(&ws, t, fade));
            }
        }

        let glyphs = part.glyphs.clone();
        for g in glyphs.iter().filter(|g| !g.blank) {
            let si = g.syl.min(group.syllables.len() - 1);
            let s = &group.syllables[si];
            let (sx0, sx1) = part.syl_boxes[si];
            let sw = (sx1 - sx0).max(1.0);
            let baseline = py + g.baseline;
            let gx0 = px + g.glyph.x;
            let gw = g.glyph.w.max(1.0);
            match style {
                VisualStyle::Spring => {
                    let n = part.syl_letters.get(si).copied().unwrap_or(1).max(1);
                    let emphasized = input.appearance.emphasis && !matches!(part.kind, PartKind::Roman { .. }) && process::spring_emphasis(s);
                    let (scale, lift, glow, fill_p, fill_x0, fill_w) = if emphasized {
                        let (ws, we) = effects::spring_letter_window(s.start, s.end, n, g.letter_in_syl);
                        let pl = process::progress(t, ws, we);
                        let fill = effects::spring_letter_fill(t, ws, we);
                        let sc = effects::SPLINES.letter_scale.at(pl);
                        let lf = effects::SPLINES.letter_lift.at(pl) * 2.0;
                        let gl = if state == State::Active { effects::glow(pl) } else { 0.0 };
                        (sc, lf, gl, fill, gx0, gw)
                    } else {
                        let a = self.anims.get(&(bi, part.kind, si)).copied().unwrap_or_else(SylAnim::new);
                        let p = process::progress(t, s.start, s.end);
                        (a.scale.position, a.lift.position, a.glow.position * 0.6, p, px + sx0, sw)
                    };
                    let scale = scale as f32;
                    let lift_px = lift as f32 * size;
                    let pivot = if emphasized {
                        (gx0 + gw / 2.0, baseline)
                    } else {
                        (px + sx0 + sw / 2.0, baseline)
                    };
                    let glow = if input.appearance.glow { glow as f32 } else { 0.0 };
                    let (ul, l) = (unlit, lit);
                    self.draw_glyph(&g.glyph, px, baseline + lift_px, scale, pivot, clip, glow * 0.8, glow_color, size, move |x| {
                        let xf = ((x as f32 - fill_x0) / fill_w) as f64;
                        let a = lerp(ul as f64, l as f64, effects::spring_fill(fill_p, xf)) as f32;
                        let a = a + (spring_rest - a) * spring_settle;
                        (text, (a * contrast).clamp(0.0, 1.0))
                    });
                }
                VisualStyle::Bloom => {
                    let w = &words[g.word.min(words.len() - 1)];
                    let mut lift = bloom_lift(t - w.start) * 0.0588;
                    if let Some(done) = line_done {
                        lift *= 1.0 - bloom_lift(t - done);
                    }
                    let mut scale = 1.0f32;
                    let mut glow = 0.0f32;
                    if input.appearance.emphasis && process::bloom_emphasis(group, w, background) && t >= w.start {
                        let n = part.word_letters.get(g.word).copied().unwrap_or(1);
                        let amount = effects::bloom_flame(t - w.start, w.end - w.start, n, g.letter);
                        scale = 1.0 + (effects::bloom_flame_peak(w.end - w.start) * amount) as f32;
                        glow = if input.appearance.glow { (amount * 0.5) as f32 } else { 0.0 };
                    }
                    let edge = edges.get(&(g.baseline as i32)).copied();
                    let fade = 0.882 * size as f64;
                    let done = state == State::Sung;
                    let px_c = px;
                    let s_end = s.end;
                    let (ul, l) = (unlit, lit);
                    self.draw_glyph(&g.glyph, px, baseline - lift as f32 * size, scale, (gx0 + gw / 2.0, baseline), clip, glow, glow_color, size, move |x| {
                        let lit_amt = match edge {
                            Some(e) => effects::bloom_edge_lit(e, (x as f32 - px_c) as f64, fade),
                            None => {
                                if done || t >= s_end { 1.0 } else { 0.0 }
                            }
                        };
                        let a = lerp(ul as f64, l as f64, lit_amt) as f32;
                        let a = if done { lerp(l as f64, ul as f64, sung_dim as f64) as f32 } else { a };
                        (text, (a * contrast).clamp(0.0, 1.0))
                    });
                }
            }
        }
    }

    #[cfg(test)]
    pub fn save_png(&self, path: &str) {
        let img = image::RgbaImage::from_raw(self.canvas.w as u32, self.canvas.h as u32, self.canvas.straight_rgba()).unwrap();
        img.save(path).unwrap();
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_glyph(
        &mut self,
        glyph: &LayoutGlyph,
        part_x: f32,
        baseline: f32,
        scale: f32,
        pivot: (f32, f32),
        clip: (i32, i32, i32, i32),
        glow: f32,
        glow_color: Rgb,
        size: f32,
        shade: impl FnMut(i32) -> (Rgb, f32),
    ) {
        let scale = (scale * 100.0).round() / 100.0;
        let origin_x = part_x + glyph.x;
        let (sx, sy) = if (scale - 1.0).abs() > 1e-3 {
            (pivot.0 + (origin_x - pivot.0) * scale, pivot.1 + (baseline - pivot.1) * scale)
        } else {
            (origin_x, baseline)
        };
        let phys = glyph.physical((sx - glyph.x * scale, 0.0), scale);
        let draw_y = sy.round() as i32 + phys.y;
        if glow > 0.02 {
            let radius = (size * 0.12 * scale).max(2.0);
            if let Some(m) = self.glow_mask(phys.cache_key, radius) {
                let a = (glow * 0.45).min(1.0);
                self.canvas.draw_alpha(&m.data, m.w, m.h, phys.x + m.x, draw_y + m.y, clip, |_| (glow_color, a));
            }
        }
        let Some(img) = self.fonts.swash.get_image(&mut self.fonts.system, phys.cache_key).as_ref() else { return };
        let (w, h) = (img.placement.width as usize, img.placement.height as usize);
        let x = phys.x + img.placement.left;
        let y = draw_y - img.placement.top;
        match img.content {
            SwashContent::Mask => self.canvas.draw_alpha(&img.data, w, h, x, y, clip, shade),
            SwashContent::SubpixelMask => {
                let a: Vec<u8> = img.data.chunks_exact(4).map(|p| p[1]).collect();
                self.canvas.draw_alpha(&a, w, h, x, y, clip, shade)
            }
            SwashContent::Color => {
                let mut shade = shade;
                let alpha = shade(x + w as i32 / 2).1;
                self.canvas.draw_rgba(&img.data, w, h, x, y, alpha, clip)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AppearanceConfig, LyricsConfig};
    use crate::lyrics::process::{PrepareOptions, prepare};
    use crate::lyrics::{Credits, LyricLine, Lyrics, Syllable};
    use crate::render::Follow;
    use crate::theme::{Palette, Theme};

    fn words(text: &str, start: f64, each: f64) -> VocalGroup {
        let mut syllables = Vec::new();
        let mut t = start;
        for w in text.split(' ') {
            let parts: Vec<&str> = w.split('|').collect();
            for (i, p) in parts.iter().enumerate() {
                let dur = if p.ends_with('~') { each * 4.0 } else { each };
                syllables.push(Syllable {
                    text: p.trim_end_matches('~').to_string(),
                    start: t,
                    end: t + dur,
                    part_of_word: i + 1 < parts.len(),
                    roman: None,
                    roman_part_of_word: false,
                });
                t += dur;
            }
        }
        VocalGroup {
            start,
            end: t,
            syllables,
            roman: None,
            translation: None,
        }
    }

    fn sample() -> Lyrics {
        let mut lines = Vec::new();
        let texts = [
            "Shine a lit|tle brigh|ter to|night",
            "Hold on to the morn|ing light~",
            "Ev|ery cit|y sings a|long",
            "We are danc|ing in the rain",
            "Nev|er let the mu|sic fade",
        ];
        let mut t = 1.0;
        for (i, s) in texts.iter().enumerate() {
            let lead = words(s, t, 0.35);
            let end = lead.end;
            let mut background = Vec::new();
            if i == 1 {
                background.push(words("(oh|oh oh)", end - 0.6, 0.2));
            }
            lines.push(LyricLine { start: t, end, opposite: i == 3, lead, background });
            t = end + 0.4;
        }
        Lyrics { kind: crate::lyrics::SyncKind::Syllable, lines, credits: Credits::default(), language: None, instrumental: false }
    }

    #[test]
    #[ignore]
    fn render_preview_pngs() {
        let lyrics = sample();
        let mut renderer = PixelRenderer::new(load_fonts());
        for (name, style, t) in [("spring", VisualStyle::Spring, 4.6), ("spring-held", VisualStyle::Spring, 6.4), ("bloom", VisualStyle::Bloom, 4.6)] {
            let mut appearance = AppearanceConfig::default();
            appearance.style = style;
            let prepared = prepare(&lyrics, PrepareOptions { interludes: true, interlude_threshold: 3.0, extend_lines: style == VisualStyle::Bloom, join_split_words: false });
            let theme = Theme::build(&appearance, Some(Palette { dominant: Rgb::new(0.12, 0.1, 0.25), accent: Rgb::new(0.85, 0.35, 0.55) }), None);
            let lyrics_cfg = LyricsConfig::default();
            let scroll_cfg = crate::config::ScrollConfig::default();
            let follow = Follow::default();
            let screen = Rect::new(0, 0, 100, 34);
            let area = Rect::new(0, 5, 100, 27);
            let started = std::time::Instant::now();
            for frame in 0..30 {
                let input = FrameInput {
                    prepared: &prepared,
                    t: t - 0.5 + frame as f64 / 60.0,
                    wall: frame as f64 / 60.0,
                    dt: 1.0 / 60.0,
                    theme: &theme,
                    appearance: &appearance,
                    lyrics: &lyrics_cfg,
                    scroll: &scroll_cfg,
                    follow: &follow,
                    screen_height: 34,
                    show_cursor: false,
                };
                renderer.render(&input, 1 + name.len(), screen, area, (10.0, 22.0));
            }
            println!("{name}: {:.1} ms/frame", started.elapsed().as_secs_f64() * 1000.0 / 30.0);
            renderer.save_png(&format!("/tmp/lyrminal-{name}.png"));
        }
    }
}
