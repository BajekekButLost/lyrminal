pub mod raster;

use std::collections::HashMap;
use std::ops::Range;

use cosmic_text::{Align, Attrs, Buffer, CacheKey, Family, FontSystem, LayoutGlyph, Metrics, Shaping, SubpixelBin, SwashCache, SwashContent, Weight, Wrap};
use ratatui::layout::Rect;

use self::raster::{Canvas, Mask};
use super::effects::{self, EdgeWord, bloom, spring};
use super::lineanim::LineAnims;
use super::scroll::{ScrollInput, Scroller};
use super::{FrameInfo, FrameInput, push_px_hit};
use crate::config::ClickSeek;
use crate::anim::{Rgb, Spring, lerp};
use crate::config::{Alignment, Background, SyllableSpacing, VisualStyle};
use crate::lyrics::process::{self, ItemKind, Prepared, State};
use crate::lyrics::{LyricLine, SyncKind, VocalGroup};

const RASTER_STEP: f32 = 0.05;
const BLUR_KEY_SCALE: f32 = 100.0;
const SUBPIXEL_STEPS: f32 = 4.0;
const MASK_TTL_FRAMES: u64 = 120;

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
    background: bool,
    size: f32,
    top: f32,
    height: f32,
    x: f32,
    glyphs: Vec<PGlyph>,
    syl_boxes: Vec<(f32, f32)>,
    syl_letters: Vec<usize>,
    syl_words: Vec<usize>,
    syl_rows: Vec<f32>,
    word_letters: Vec<usize>,
}

impl Part {
    fn syllable_chain(&self, spacing: SyllableSpacing, scale: impl Fn(usize) -> f32) -> Vec<(f32, f32)> {
        let n = self.syl_boxes.len();
        let mut out: Vec<(f32, f32)> = (0..n).map(|i| (0.0, scale(i))).collect();
        if spacing == SyllableSpacing::Separate {
            for (k, o) in out.iter_mut().enumerate() {
                let (x0, x1) = self.syl_boxes[k];
                if x0 <= x1 {
                    o.0 = (x0 + x1) / 2.0 * (1.0 - o.1);
                }
            }
            return out;
        }
        let mut i = 0;
        while i < n {
            let mut j = i + 1;
            while j < n && self.syl_words.get(j) == self.syl_words.get(i) && self.syl_rows[j] == self.syl_rows[i] {
                j += 1;
            }
            let mut seg: Vec<usize> = (i..j).filter(|&k| self.syl_boxes[k].0 <= self.syl_boxes[k].1).collect();
            seg.sort_by(|a, b| self.syl_boxes[*a].0.total_cmp(&self.syl_boxes[*b].0));
            if let Some(&first) = seg.first() {
                let lo = self.syl_boxes[first].0;
                let hi = seg.iter().map(|&k| self.syl_boxes[k].1).fold(f32::MIN, f32::max);
                let extent = |pos: usize| {
                    let k = seg[pos];
                    let end = seg.get(pos + 1).map(|&nk| self.syl_boxes[nk].0).unwrap_or(hi);
                    (end - self.syl_boxes[k].0).max(0.0)
                };
                let total: f32 = (0..seg.len()).map(|pos| extent(pos) * out[seg[pos]].1).sum();
                let mut cursor = (lo + hi) / 2.0 - total / 2.0;
                for pos in 0..seg.len() {
                    let k = seg[pos];
                    let s = out[k].1;
                    out[k].0 = cursor - self.syl_boxes[k].0 * s;
                    cursor += extent(pos) * s;
                }
            }
            i = j;
        }
        out
    }
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
    spacing: SyllableSpacing,
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
        Self::at(effects::SPLINES.word_scale.at(0.0), effects::SPLINES.word_lift.at(0.0), 0.0)
    }

    fn letter() -> Self {
        Self::at(effects::SPLINES.letter_scale.at(0.0), effects::SPLINES.letter_lift.at(0.0), 0.0)
    }

    fn at(scale: f64, lift: f64, glow: f64) -> Self {
        Self {
            scale: Spring::new(scale, 0.88, 0.64),
            lift: Spring::new(lift, 1.45, 0.4),
            glow: Spring::new(glow, 1.18, 0.56),
        }
    }

    fn drive(&mut self, scale: f64, lift: f64, glow: f64, dt: f64) -> bool {
        self.scale.set_goal(scale);
        self.lift.set_goal(lift);
        self.glow.set_goal(glow);
        self.scale.step(dt);
        self.lift.step(dt);
        self.glow.step(dt);
        self.scale.is_settled() && self.lift.is_settled() && self.glow.is_settled()
    }
}

type AnimKey = (usize, PartKind, usize);
type LetterKey = (usize, PartKind, usize, usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct MaskKey {
    block: usize,
    part: usize,
    blur: u16,
    idle: bool,
    sub_x: u8,
    sub_y: u8,
}

fn split_subpixel(v: f32) -> (i32, u8) {
    let steps = (v * SUBPIXEL_STEPS).round();
    let whole = (steps / SUBPIXEL_STEPS).floor();
    (whole as i32, (steps - whole * SUBPIXEL_STEPS) as u8)
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
    masks: HashMap<MaskKey, (Mask, u64)>,
    glows: HashMap<CacheKey, Mask>,
    anims: HashMap<AnimKey, SylAnim>,
    letters: HashMap<LetterKey, SylAnim>,
    lines: LineAnims,
    springs_settled: HashMap<usize, bool>,
    prepared_id: usize,
    frame: u64,
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

fn part_group(kind: PartKind, line: &LyricLine) -> Option<&VocalGroup> {
    match kind {
        PartKind::Lead | PartKind::Roman { timed: true, .. } => Some(&line.lead),
        PartKind::Bg(i) => line.background.get(i),
        _ => None,
    }
}

fn scale_about(p: (f32, f32), pivot: (f32, f32), s: f32) -> (f32, f32) {
    (pivot.0 + (p.0 - pivot.0) * s, pivot.1 + (p.1 - pivot.1) * s)
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
            letters: HashMap::new(),
            lines: LineAnims::default(),
            springs_settled: HashMap::new(),
            prepared_id: 0,
            frame: 0,
        }
    }

    pub fn reset(&mut self) {
        self.layout = None;
        self.masks.clear();
        self.clear_springs();
        self.lines.reset();
        self.scroller.reset();
    }

    fn clear_springs(&mut self) {
        self.anims.clear();
        self.letters.clear();
        self.springs_settled.clear();
    }

    fn forget_block(&mut self, bi: usize) {
        if self.springs_settled.remove(&bi).is_some() {
            self.anims.retain(|k, _| k.0 != bi);
            self.letters.retain(|k, _| k.0 != bi);
        }
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
        let mut syl_rows = vec![f32::NAN; group.syllables.len().max(1)];
        let mut word_letters = vec![0usize; words.len().max(1)];
        for g in glyphs.iter_mut() {
            if g.blank {
                continue;
            }
            let si = g.syl.min(syl_boxes.len() - 1);
            if syl_rows[si].is_nan() {
                syl_rows[si] = g.baseline;
            }
            let b = &mut syl_boxes[si];
            b.0 = b.0.min(g.glyph.x);
            b.1 = b.1.max(g.glyph.x + g.glyph.w);
            g.letter_in_syl = syl_letters[si];
            syl_letters[si] += 1;
            let wl = g.word.min(word_letters.len() - 1);
            g.letter = word_letters[wl];
            word_letters[wl] += 1;
        }
        let mut known = syl_rows.iter().copied().find(|r| !r.is_nan()).unwrap_or(0.0);
        for r in syl_rows.iter_mut() {
            if r.is_nan() {
                *r = known;
            } else {
                known = *r;
            }
        }
        Part {
            kind,
            background: matches!(kind, PartKind::Bg(_)),
            size,
            top: 0.0,
            height,
            x,
            glyphs,
            syl_boxes,
            syl_letters,
            syl_words: words_of_syl,
            syl_rows,
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
                    let lead_size = if line.background_only { size * 0.72 } else { size };
                    let mut lead = self.build_part(PartKind::Lead, &line.lead, false, lead_size, width, x, align, &family, weight);
                    lead.background = line.background_only;
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
        self.clear_springs();
        self.lines.reset();
    }

    fn layer_for(&self, bi: usize) -> Option<Mask> {
        let layout = self.layout.as_ref()?;
        let block = layout.blocks.get(bi)?;
        let size = block.parts.first().map(|p| p.size).unwrap_or(16.0);
        let pad = (size * 0.6) as i32;
        Some(Mask::new(
            -pad,
            -pad,
            (layout.content_w as i32 + pad * 2) as usize,
            (block.height as i32 + pad * 2) as usize,
        ))
    }

    fn part_mask(&mut self, key: MaskKey, input: &FrameInput) -> Option<&mut Mask> {
        let MaskKey { block: bi, part: pi, blur, idle, sub_x, sub_y } = key;
        let (fx, fy) = (sub_x as f32 / SUBPIXEL_STEPS, sub_y as f32 / SUBPIXEL_STEPS);
        if !self.masks.contains_key(&key) {
            let part = self.layout.as_ref()?.blocks.get(bi)?.parts.get(pi)?.clone();
            let mut mask = self.layer_for(bi)?;
            let line = self.layout.as_ref().and_then(|l| {
                let item = input.prepared.items.get(l.blocks[bi].item)?;
                match item.kind {
                    ItemKind::Line(li) => input.prepared.lyrics.lines.get(li),
                    _ => None,
                }
            });
            let group = line.and_then(|line| part_group(part.kind, line));
            let spring_idle = idle
                && input.style() == VisualStyle::Spring
                && input.prepared.lyrics.kind == SyncKind::Syllable
                && group.is_some();
            let rest_scale = effects::SPLINES.word_scale.at(0.0) as f32;
            let chain = part.syllable_chain(input.appearance.syllable_spacing, |_| rest_scale);
            for g in part.glyphs.iter().filter(|g| !g.blank) {
                let baseline = part.top + g.baseline;
                let (origin, scale) = if spring_idle {
                    let group = group.unwrap();
                    let si = g.syl.min(group.syllables.len() - 1);
                    let mut origin = (part.x + g.glyph.x, baseline);
                    let mut scale = rest_scale;
                    let mut lift = effects::SPLINES.word_lift.at(0.0) as f32;
                    if input.appearance.emphasis
                        && !matches!(part.kind, PartKind::Roman { .. })
                        && process::spring_emphasis(&group.syllables[si])
                    {
                        let ls = effects::SPLINES.letter_scale.at(0.0) as f32;
                        let letter_pivot = (origin.0 + g.glyph.w / 2.0, baseline);
                        origin = scale_about(origin, letter_pivot, ls);
                        scale *= ls;
                        lift += 2.0 * effects::SPLINES.letter_lift.at(0.0) as f32;
                    }
                    let (a, s) = chain[si.min(chain.len() - 1)];
                    ((part.x + a + (origin.0 - part.x) * s, origin.1 + lift * part.size), scale)
                } else {
                    ((part.x + g.glyph.x, baseline), 1.0)
                };
                self.stamp_glyph(&mut mask, &g.glyph, (origin.0 + fx, origin.1 + fy), scale, |_| 1.0);
            }
            let blurred = if blur > 0 { mask.blurred(blur as f32 / BLUR_KEY_SCALE) } else { mask };
            self.masks.insert(key, (blurred, self.frame));
        }
        let frame = self.frame;
        self.masks.get_mut(&key).map(|(m, used)| {
            *used = frame;
            m
        })
    }

    fn stamp_glyph(&mut self, layer: &mut Mask, glyph: &LayoutGlyph, origin: (f32, f32), scale: f32, shade: impl FnMut(i32) -> f32) -> Option<(CacheKey, i32, i32)> {
        let raster = ((scale / RASTER_STEP).round() * RASTER_STEP).max(RASTER_STEP);
        let k = scale / raster;
        let pen_x = origin.0 + glyph.font_size * glyph.x_offset * scale;
        let pen_y = origin.1 + (glyph.y - glyph.font_size * glyph.y_offset) * scale;
        let (mut cache_key, _, _) = CacheKey::new(glyph.font_id, glyph.glyph_id, glyph.font_size * raster, (0.0, 0.0), glyph.font_weight, glyph.cache_key_flags);
        cache_key.x_bin = SubpixelBin::new(pen_x).1;
        cache_key.y_bin = SubpixelBin::new(-pen_y).1;
        let img = self.fonts.swash.get_image(&mut self.fonts.system, cache_key).as_ref()?;
        let (w, h) = (img.placement.width as usize, img.placement.height as usize);
        let anchor_x = pen_x - cache_key.x_bin.as_float() * k;
        let anchor_y = pen_y + cache_key.y_bin.as_float() * k;
        let left = anchor_x + img.placement.left as f32 * k;
        let top = anchor_y - img.placement.top as f32 * k;
        match img.content {
            SwashContent::Mask => layer.stamp_resampled(&img.data, w, h, left, top, k, shade),
            SwashContent::SubpixelMask => {
                let a: Vec<u8> = img.data.chunks_exact(4).map(|p| p[1]).collect();
                layer.stamp_resampled(&a, w, h, left, top, k, shade)
            }
            SwashContent::Color => {
                let a: Vec<u8> = img.data.chunks_exact(4).map(|p| p[3]).collect();
                layer.stamp_resampled(&a, w, h, left, top, k, shade)
            }
        }
        Some((cache_key, anchor_x.round() as i32, anchor_y.round() as i32))
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
        self.frame += 1;
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
            spacing: input.appearance.syllable_spacing,
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
        self.lines.begin_frame();
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
            let distance = item_idx.abs_diff(target);
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
                        let cx = bx + spacing + (i as f32 - 1.0) * spacing * d.spread as f32;
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
            let static_kind = kind == SyncKind::Static;
            let blur_target = if input.appearance.depth_dimming && !manual && !static_kind && state != State::Active {
                distance.min(5) as f64 * 0.03 * size as f64
            } else {
                0.0
            };
            let active_target = if !static_kind && state == State::Active { 1.0 } else { 0.0 };
            let look = self.lines.step(item_idx, active_target, blur_target, input.dt);
            let line = &prepared.lyrics.lines[li];
            let bloom_settling = style == VisualStyle::Bloom && state == State::Sung && t - line.full_end() < 1.5;
            let springs_busy = !self.springs_settled.get(&bi).copied().unwrap_or(true);
            let live = !static_kind && (state == State::Active || !look.settled || springs_busy || bloom_settling);
            let fade = if input.appearance.fade_previous && !manual && state == State::Sung && item_idx < target {
                1.0 - ((t - it.end) / 0.4).clamp(0.0, 1.0) as f32
            } else {
                1.0
            };
            let (ix, sub_x) = split_subpixel(x0);
            let (iy, sub_y) = split_subpixel(y);
            if live {
                animating = true;
                let block = LiveBlock {
                    bi,
                    li,
                    state,
                    t,
                    active: look.active as f32,
                    blur: look.depth as f32,
                    fade,
                    ix,
                    iy,
                    fx: sub_x as f32 / SUBPIXEL_STEPS,
                    fy: sub_y as f32 / SUBPIXEL_STEPS,
                    clip,
                    glow_color,
                };
                let settled = self.draw_live_block(input, &block);
                self.springs_settled.insert(bi, settled);
                continue;
            }
            self.forget_block(bi);
            let rest = if state == State::Active { State::NotSung } else { state };
            let blur = (look.depth.max(0.0) as f32 * BLUR_KEY_SCALE).round() as u16;
            let idle = state == State::NotSung;
            for pi in 0..part_count {
                let alpha = {
                    let part = &self.layout.as_ref().unwrap().blocks[bi].parts[pi];
                    (rest_alpha(input, rest, part, line, t) * fade * contrast).clamp(0.0, 1.0)
                };
                if alpha <= 0.01 {
                    continue;
                }
                let key = MaskKey { block: bi, part: pi, blur, idle, sub_x, sub_y };
                if let Some(mask) = self.part_mask(key, input) {
                    let (mw, mh, mx, my) = (mask.w, mask.h, mask.x, mask.y);
                    let data = std::mem::take(&mut mask.data);
                    self.canvas.draw_alpha(&data, mw, mh, ix + mx, iy + my, clip, |_| (text, alpha));
                    if let Some((mask, _)) = self.masks.get_mut(&key) {
                        mask.data = data;
                    }
                }
            }
        }
        let frame = self.frame;
        self.masks.retain(|_, (_, used)| frame - *used <= MASK_TTL_FRAMES);
        info.animating = animating;
        let credits_offset = item_tops
            .len()
            .checked_sub(1)
            .map(|last| self.scroller.item_offset(last, input.wall))
            .unwrap_or(0.0);
        PixelFrame {
            info,
            credits_y: Some(ly + (credits_top as f64 - base + credits_offset) as f32).filter(|cy| *cy < ly + lh),
            content_x: x0,
        }
    }

    fn draw_live_block(&mut self, input: &FrameInput, block: &LiveBlock) -> bool {
        let parts = self.layout.as_ref().unwrap().blocks[block.bi].parts.clone();
        let Some(mut layer) = self.layer_for(block.bi) else { return true };
        let mut glows: Vec<(CacheKey, i32, i32, f32, f32)> = Vec::new();
        let mut settled = true;
        for part in &parts {
            settled &= self.render_part(input, part, block, &mut layer, &mut glows);
        }
        let size = parts.first().map(|p| p.size).unwrap_or(16.0);
        if input.appearance.glow && !glows.is_empty() {
            let visible = (1.0 - block.blur / (0.15 * size)).clamp(0.0, 1.0);
            for (key, gx, gy, g, gsize) in glows {
                if let Some(m) = self.glow_mask(key, (gsize * 0.078).max(2.4)) {
                    let a = (g * 0.45 * visible).min(1.0);
                    self.canvas.draw_alpha(
                        &m.data,
                        m.w,
                        m.h,
                        block.ix + gx + m.x,
                        block.iy + gy + m.y,
                        block.clip,
                        |_| (block.glow_color, a),
                    );
                }
            }
        }
        let layer = if block.blur > 0.05 { layer.blurred(block.blur) } else { layer };
        let text = input.theme.text;
        self.canvas.draw_alpha(
            &layer.data,
            layer.w,
            layer.h,
            block.ix + layer.x,
            block.iy + layer.y,
            block.clip,
            |_| (text, 1.0),
        );
        settled
    }

    fn render_part(
        &mut self,
        input: &FrameInput,
        part: &Part,
        block: &LiveBlock,
        layer: &mut Mask,
        glows: &mut Vec<(CacheKey, i32, i32, f32, f32)>,
    ) -> bool {
        let prepared = input.prepared;
        let kind = prepared.lyrics.kind;
        let style = input.style();
        let contrast = input.appearance.contrast;
        let line = &prepared.lyrics.lines[block.li];
        let (bi, state, t) = (block.bi, block.state, block.t);
        let size = part.size;
        let background = part.background;
        let rest_state = if state == State::Active { State::NotSung } else { state };
        let rest = rest_alpha(input, rest_state, part, line, t);
        let roman_k = if matches!(part.kind, PartKind::Roman { timed: true, .. }) { 0.82 } else { 1.0 };
        let (active, fade) = (block.active, block.fade);
        let mix = move |act: f32| ((rest + (act * roman_k - rest) * active) * contrast * fade).clamp(0.0, 1.0);
        let px = part.x + block.fx;
        let py = part.top + block.fy;

        let Some(group) = part_group(part.kind, line).filter(|_| kind == SyncKind::Syllable) else {
            let act = match (part.kind, kind, style) {
                (PartKind::Translation | PartKind::Roman { .. }, _, _) => 0.62,
                (_, SyncKind::Line, VisualStyle::Bloom) => {
                    lerp(bloom::UNLIT as f64, bloom::LIT as f64, effects::bloom_line_tone(t, line.start, line.end)) as f32
                }
                (_, SyncKind::Line, VisualStyle::Spring) => {
                    let p = process::progress(t, line.start, line.end);
                    lerp(spring::LINE_UNLIT as f64, spring::ACTIVE_LIT as f64, effects::vertical_fill(p, 0.5)) as f32
                }
                _ => 0.6,
            };
            let glow = if kind == SyncKind::Line && style == VisualStyle::Spring {
                effects::SPLINES.line_glow.at(process::progress(t, line.start, line.end)) as f32 * 0.4 * active
            } else {
                0.0
            };
            let alpha = mix(act);
            for g in part.glyphs.iter().filter(|g| !g.blank) {
                let placed = self.stamp_glyph(layer, &g.glyph, (px + g.glyph.x, py + g.baseline), 1.0, |_| alpha);
                if glow > 0.02
                    && let Some((key, gx, gy)) = placed
                {
                    glows.push((key, gx, gy, glow, size));
                }
            }
            return true;
        };

        let (unlit, lit) = match style {
            VisualStyle::Spring if background => (spring::BG_UNLIT, spring::BG_LIT),
            VisualStyle::Spring => (spring::ACTIVE_UNLIT, spring::ACTIVE_LIT),
            VisualStyle::Bloom if background => (bloom::UNLIT, bloom::BG_LIT),
            VisualStyle::Bloom => (bloom::UNLIT, bloom::LIT),
        };
        let words = process::words(group);
        let line_done = if state == State::Sung { Some(line.full_end()) } else { None };
        let emphasis_on = input.appearance.emphasis && !matches!(part.kind, PartKind::Roman { .. });
        let mut settled = true;

        if style == VisualStyle::Spring {
            let dt = input.dt;
            let rest_scale = effects::SPLINES.letter_scale.at(0.0);
            let rest_lift = effects::SPLINES.letter_lift.at(0.0);
            for si in 0..group.syllables.len() {
                let s = &group.syllables[si];
                let p = process::progress(t, s.start, s.end);
                let goal = (effects::SPLINES.word_scale.at(p), effects::SPLINES.word_lift.at(p), effects::glow(p));
                let anim = self.anims.entry((bi, part.kind, si)).or_insert_with(|| SylAnim::at(goal.0, goal.1, goal.2));
                settled &= anim.drive(goal.0, goal.1, goal.2, dt);
                if !(emphasis_on && process::spring_emphasis(s)) {
                    continue;
                }
                let n = part.syl_letters.get(si).copied().unwrap_or(1).max(1);
                let window = |k: usize| effects::spring_letter_window(s.start, s.end, n, k);
                let active_letter = (0..n).find_map(|k| {
                    let (ws, we) = window(k);
                    (process::state(t, ws, we) == State::Active).then(|| (k, process::progress(t, ws, we)))
                });
                let syl_state = process::state(t, s.start, s.end);
                for k in 0..n {
                    let (ws, we) = window(k);
                    let letter_state = process::state(t, ws, we);
                    let (ts, tl, tg) = match syl_state {
                        State::NotSung => (rest_scale, rest_lift, 0.0),
                        State::Sung => (
                            effects::SPLINES.letter_scale.at(1.0),
                            effects::SPLINES.letter_lift.at(1.0),
                            effects::glow(1.0),
                        ),
                        State::Active => {
                            let mut v = (rest_scale, rest_lift, 0.0);
                            if let Some((a, pct)) = active_letter {
                                let d = k.abs_diff(a) as f64;
                                let falloff = 1.0 / (1.0 + d.powf(2.8));
                                let glow_falloff = 1.0 / (1.0 + 0.9 * d);
                                v = (
                                    rest_scale + (effects::SPLINES.letter_scale.at(pct) - rest_scale) * falloff,
                                    rest_lift + (effects::SPLINES.letter_lift.at(pct) - rest_lift) * falloff,
                                    effects::glow(pct) * glow_falloff,
                                );
                            }
                            if letter_state == State::NotSung {
                                v = (rest_scale, rest_lift, 0.0);
                            } else if letter_state == State::Sung && active_letter.is_none() {
                                v.2 = effects::glow(0.2);
                            }
                            v
                        }
                    };
                    let letter = self.letters.entry((bi, part.kind, si, k)).or_insert_with(|| SylAnim::at(ts, tl, tg));
                    settled &= letter.drive(ts, tl, tg, dt);
                }
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

        let chain = if style == VisualStyle::Spring {
            let anims = &self.anims;
            part.syllable_chain(input.appearance.syllable_spacing, |si| {
                anims
                    .get(&(bi, part.kind, si))
                    .map(|a| a.scale.position as f32)
                    .unwrap_or_else(|| effects::SPLINES.word_scale.at(0.0) as f32)
            })
        } else {
            Vec::new()
        };
        let glyphs = part.glyphs.clone();
        for g in glyphs.iter().filter(|g| !g.blank) {
            let si = g.syl.min(group.syllables.len() - 1);
            let s = &group.syllables[si];
            let (sx0, sx1) = part.syl_boxes[si];
            let sw = (sx1 - sx0).max(1.0);
            let baseline = py + g.baseline;
            let gx0 = px + g.glyph.x;
            let gw = g.glyph.w.max(1.0);
            let origin0 = (gx0, baseline);
            let (ul, l) = (unlit, lit);
            match style {
                VisualStyle::Spring => {
                    let syl = self.anims.get(&(bi, part.kind, si)).copied().unwrap_or_else(SylAnim::new);
                    let (ca, cs) = chain[si.min(chain.len() - 1)];
                    let place = |x: f32| px + ca + (x - px) * cs;
                    let emphasized = emphasis_on && process::spring_emphasis(s);
                    let (origin, scale, lift, glow, fill_p, fill_x0, fill_w) = if emphasized {
                        let n = part.syl_letters.get(si).copied().unwrap_or(1).max(1);
                        let k = g.letter_in_syl;
                        let letter = self.letters.get(&(bi, part.kind, si, k)).copied().unwrap_or_else(SylAnim::letter);
                        let (ws, we) = effects::spring_letter_window(s.start, s.end, n, k);
                        let letter_pivot = (gx0 + gw / 2.0, baseline);
                        let ls = letter.scale.position as f32;
                        let o = scale_about(origin0, letter_pivot, ls);
                        (
                            (place(o.0), o.1),
                            ls * cs,
                            syl.lift.position + 2.0 * letter.lift.position,
                            letter.glow.position as f32,
                            effects::spring_letter_fill(t, ws, we),
                            place(o.0),
                            gw * ls * cs,
                        )
                    } else {
                        (
                            (place(gx0), baseline),
                            cs,
                            syl.lift.position,
                            syl.glow.position as f32 * 0.5,
                            process::progress(t, s.start, s.end),
                            place(px + sx0),
                            sw * cs,
                        )
                    };
                    let origin = (origin.0, origin.1 + lift as f32 * size);
                    let placed = self.stamp_glyph(layer, &g.glyph, origin, scale, move |x| {
                        let xf = ((x as f32 - fill_x0) / fill_w) as f64;
                        mix(lerp(ul as f64, l as f64, effects::spring_fill(fill_p, xf)) as f32)
                    });
                    let glow = glow * active;
                    if glow > 0.02
                        && let Some((key, gx, gy)) = placed
                    {
                        glows.push((key, gx, gy, glow, size * scale));
                    }
                }
                VisualStyle::Bloom => {
                    let w = &words[g.word.min(words.len() - 1)];
                    let mut lift = bloom_lift(t - w.start) * 0.0588;
                    if let Some(done) = line_done {
                        lift *= 1.0 - bloom_lift(t - done);
                    }
                    let mut scale = 1.0f32;
                    let mut glow = 0.0f32;
                    if emphasis_on && process::bloom_emphasis(group, w, background) && t >= w.start {
                        let n = part.word_letters.get(g.word).copied().unwrap_or(1);
                        let amount = effects::bloom_flame(t - w.start, w.end - w.start, n, g.letter);
                        scale = 1.0 + (effects::bloom_flame_peak(w.end - w.start) * amount) as f32;
                        glow = (amount * 0.5) as f32 * active;
                    }
                    let edge = edges.get(&(g.baseline as i32)).copied();
                    let fade_w = 0.882 * size as f64;
                    let done = state == State::Sung;
                    let s_end = s.end;
                    let origin = scale_about(origin0, (gx0 + gw / 2.0, baseline), scale);
                    let origin = (origin.0, origin.1 - lift as f32 * size);
                    let placed = self.stamp_glyph(layer, &g.glyph, origin, scale, move |x| {
                        let lit_amt = match edge {
                            Some(e) => effects::bloom_edge_lit(e, (x as f32 - px) as f64, fade_w),
                            None => {
                                if done || t >= s_end { 1.0 } else { 0.0 }
                            }
                        };
                        mix(lerp(ul as f64, l as f64, lit_amt) as f32)
                    });
                    if glow > 0.02
                        && let Some((key, gx, gy)) = placed
                    {
                        glows.push((key, gx, gy, glow, size * scale));
                    }
                }
            }
        }
        settled
    }

    #[cfg(test)]
    pub fn save_png(&self, path: &str) {
        let img = image::RgbaImage::from_raw(self.canvas.w as u32, self.canvas.h as u32, self.canvas.straight_rgba()).unwrap();
        img.save(path).unwrap();
    }

}

struct LiveBlock {
    bi: usize,
    li: usize,
    state: State,
    t: f64,
    active: f32,
    blur: f32,
    fade: f32,
    ix: i32,
    iy: i32,
    fx: f32,
    fy: f32,
    clip: (i32, i32, i32, i32),
    glow_color: Rgb,
}

fn rest_alpha(input: &FrameInput, rest: State, part: &Part, line: &LyricLine, t: f64) -> f32 {
    let kind = input.prepared.lyrics.kind;
    let style = input.style();
    if kind == SyncKind::Static {
        return if matches!(part.kind, PartKind::Translation | PartKind::Roman { .. }) { 0.6 } else { 0.9 };
    }
    if matches!(part.kind, PartKind::Translation | PartKind::Roman { timed: false, .. }) {
        return if rest == State::Sung { 0.32 } else { 0.26 };
    }
    let mut a = match (style, rest) {
        (VisualStyle::Bloom, State::Sung) => {
            let lit = if part.background { bloom::BG_LIT } else { bloom::LIT };
            lerp(lit as f64, bloom::UNLIT as f64, effects::bloom_sung_dim(t - line.full_end())) as f32
        }
        (VisualStyle::Bloom, _) => bloom::UNLIT,
        (VisualStyle::Spring, State::Sung) => spring::SUNG,
        _ => spring::NOT_SUNG,
    };
    if part.background && style == VisualStyle::Spring {
        a *= 0.8;
    }
    if matches!(part.kind, PartKind::Roman { timed: true, .. }) {
        a *= 0.82;
    }
    a
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
            lines.push(LyricLine { start: t, end, opposite: i == 3, lead, background, background_only: false });
            t = end + 0.4;
        }
        Lyrics { kind: crate::lyrics::SyncKind::Syllable, lines, credits: Credits::default(), language: None, instrumental: false }
    }

    fn centroid(mask: &Mask) -> (f64, f64) {
        let (mut sx, mut sy, mut m) = (0.0, 0.0, 0.0);
        for r in 0..mask.h {
            for c in 0..mask.w {
                let a = mask.data[r * mask.w + c] as f64;
                sx += a * c as f64;
                sy += a * r as f64;
                m += a;
            }
        }
        (sx / m, sy / m)
    }

    #[test]
    fn glyphs_move_by_subpixel_steps() {
        let mut renderer = PixelRenderer::new(load_fonts());
        let (glyphs, _) = shape(
            &mut renderer.fonts,
            ShapeSpec { text: "o", ranges: &[0..1], words_of_syl: &[0], size: 44.0, width: 400.0, align: Align::Left, family: "", weight: 700 },
        );
        let glyph = glyphs[0].glyph.clone();
        let mut last: Option<(f64, f64)> = None;
        for step in 0..=20 {
            let off = step as f32 * 0.1;
            let mut mask = Mask::new(0, 0, 80, 80);
            renderer.stamp_glyph(&mut mask, &glyph, (10.0 + off, 50.0 + off), 1.0, |_| 1.0);
            let c = centroid(&mask);
            if let Some(prev) = last {
                assert!((c.0 - prev.0 - 0.1).abs() < 0.04, "x step {} at {off}", c.0 - prev.0);
                assert!((c.1 - prev.1 - 0.1).abs() < 0.04, "y step {} at {off}", c.1 - prev.1);
            }
            last = Some(c);
        }
    }

    #[test]
    fn splits_positions_into_quarter_pixels() {
        for v in [-3.3f32, -0.3, -0.05, 0.0, 0.2, 0.9, 2.62, 17.0] {
            let (whole, sub) = split_subpixel(v);
            assert!(sub < SUBPIXEL_STEPS as u8);
            assert!((whole as f32 + sub as f32 / SUBPIXEL_STEPS - v).abs() <= 0.5 / SUBPIXEL_STEPS + 1e-4, "{v}");
        }
    }

    #[test]
    fn syllables_stay_joined_while_scaled() {
        let mut renderer = PixelRenderer::new(load_fonts());
        let group = words("brigh|ter to|night", 0.0, 0.3);
        let part = renderer.build_part(PartKind::Lead, &group, false, 44.0, 800.0, 0.0, Align::Left, "", 700);
        let uniform = part.syllable_chain(SyllableSpacing::Joined, |_| 0.95);
        for (si, &(a, s)) in uniform.iter().enumerate() {
            let (x0, x1) = part.syl_boxes[si];
            let w = part.syl_words[si];
            let segment: Vec<usize> = (0..part.syl_boxes.len()).filter(|&k| part.syl_words[k] == w).collect();
            let lo = segment.iter().map(|&k| part.syl_boxes[k].0).fold(f32::MAX, f32::min);
            let hi = segment.iter().map(|&k| part.syl_boxes[k].1).fold(f32::MIN, f32::max);
            let centre = (lo + hi) / 2.0;
            assert!((a + x0 * s - (centre + (x0 - centre) * 0.95)).abs() < 1e-3);
            assert!((a + x1 * s - (centre + (x1 - centre) * 0.95)).abs() < 1e-3);
        }
        let scales = [0.95, 1.05, 1.0, 0.97];
        let mixed = part.syllable_chain(SyllableSpacing::Joined, |si| scales[si]);
        for (left, right) in [(0, 1), (2, 3)] {
            let end = mixed[left].0 + part.syl_boxes[right].0 * mixed[left].1;
            let start = mixed[right].0 + part.syl_boxes[right].0 * mixed[right].1;
            assert!((end - start).abs() < 1e-3, "gap {} between syllables {left} and {right}", start - end);
        }
        let separate = part.syllable_chain(SyllableSpacing::Separate, |si| scales[si]);
        for (si, &(a, s)) in separate.iter().enumerate() {
            let (x0, x1) = part.syl_boxes[si];
            let centre = (x0 + x1) / 2.0;
            assert!((a + centre * s - centre).abs() < 1e-3);
            assert!((s - scales[si]).abs() < 1e-6);
        }
    }

    #[test]
    #[ignore]
    fn render_preview_pngs() {
        let lyrics = sample();
        let mut renderer = PixelRenderer::new(load_fonts());
        for (name, style, t) in [("spring", VisualStyle::Spring, 4.6), ("spring-held", VisualStyle::Spring, 6.4), ("bloom", VisualStyle::Bloom, 4.6)] {
            let mut appearance = AppearanceConfig::default();
            appearance.style = style;
            let prepared = prepare(&lyrics, PrepareOptions { interludes: true, interlude_threshold: 3.0, extend_lines: style == VisualStyle::Bloom, join_split_words: false, fix_background: false });
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

    #[test]
    #[ignore]
    fn render_transition_strips() {
        let lyrics = sample();
        let strips: [(&str, VisualStyle, &[f64], (u32, u32, u32, u32)); 3] = [
            ("held", VisualStyle::Spring, &[6.25, 6.45, 6.65, 6.85, 7.05, 7.25, 7.45, 7.75, 8.3], (440, 320, 360, 90)),
            ("handover", VisualStyle::Spring, &[3.6, 3.85, 3.95, 4.05, 4.2, 4.35, 4.6, 5.2], (60, 230, 760, 250)),
            ("bloom-handover", VisualStyle::Bloom, &[3.6, 3.85, 4.0, 4.2, 4.4, 4.7, 5.2], (60, 230, 760, 250)),
        ];
        for (name, style, times, (cx, cy, cw, chh)) in strips {
            let mut renderer = PixelRenderer::new(load_fonts());
            let mut appearance = AppearanceConfig::default();
            appearance.style = style;
            let prepared = prepare(&lyrics, PrepareOptions { interludes: true, interlude_threshold: 3.0, extend_lines: style == VisualStyle::Bloom, join_split_words: false, fix_background: false });
            let theme = Theme::build(&appearance, Some(Palette { dominant: Rgb::new(0.12, 0.1, 0.25), accent: Rgb::new(0.85, 0.35, 0.55) }), None);
            let lyrics_cfg = LyricsConfig::default();
            let scroll_cfg = crate::config::ScrollConfig::default();
            let follow = Follow::default();
            let screen = Rect::new(0, 0, 100, 34);
            let area = Rect::new(0, 5, 100, 27);
            let mut out = image::RgbaImage::new(cw, chh * times.len() as u32);
            let start = times[0] - 1.0;
            let mut frame = 0u64;
            for (i, target) in times.iter().enumerate() {
                loop {
                    let t = start + frame as f64 / 60.0;
                    let input = FrameInput {
                        prepared: &prepared,
                        t,
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
                    renderer.render(&input, 7, screen, area, (10.0, 22.0));
                    frame += 1;
                    if t >= *target {
                        break;
                    }
                }
                let full = image::RgbaImage::from_raw(renderer.canvas.w as u32, renderer.canvas.h as u32, renderer.canvas.straight_rgba()).unwrap();
                let crop = image::imageops::crop_imm(&full, cx, cy, cw, chh).to_image();
                image::imageops::replace(&mut out, &crop, 0, (i as u32 * chh) as i64);
                for x in 0..cw {
                    out.put_pixel(x, i as u32 * chh, image::Rgba([255, 255, 255, 60]));
                }
            }
            out.save(format!("/tmp/lyrminal-strip-{name}.png")).unwrap();
        }
    }
}
