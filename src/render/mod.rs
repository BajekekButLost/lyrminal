pub mod cell;
pub mod effects;
pub mod lineanim;
pub mod pixel;
pub mod scroll;

use crate::anim::Rgb;
use crate::config::{AppearanceConfig, LyricsConfig, ScrollAnimation, ScrollConfig, VisualStyle};
use crate::lyrics::process::Prepared;
use crate::theme::Theme;

#[derive(Debug, Clone, Default)]
pub struct Follow {
    pub manual: bool,
    pub offset: f64,
}

pub struct FrameInput<'a> {
    pub prepared: &'a Prepared,
    pub t: f64,
    pub wall: f64,
    pub dt: f64,
    pub theme: &'a Theme,
    pub appearance: &'a AppearanceConfig,
    pub lyrics: &'a LyricsConfig,
    pub scroll: &'a ScrollConfig,
    pub follow: &'a Follow,
    pub screen_height: u16,
    pub show_cursor: bool,
}

impl FrameInput<'_> {
    pub fn style(&self) -> VisualStyle {
        self.appearance.style
    }

    pub fn scroll_mode(&self) -> scroll::ScrollMode {
        match self.scroll.animation {
            ScrollAnimation::Staggered => scroll::ScrollMode::Ripple,
            ScrollAnimation::Smooth => scroll::ScrollMode::Rigid,
        }
    }

    pub fn scroll_target(&self, t: f64) -> (usize, f64) {
        let prepared = self.prepared;
        let target = prepared.scroll_target(t + self.scroll.lead_ms as f64 / 1000.0);
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
        (target, silence)
    }
}

#[derive(Debug, Clone, Default)]
pub struct FrameInfo {
    pub base: f64,
    pub total: f64,
    pub viewport: f64,
    pub animating: bool,
    pub cursor_item: Option<usize>,
    pub links: Vec<LinkSpan>,
    pub hits: Vec<Hit>,
}

#[derive(Debug, Clone, Copy)]
pub struct Hit {
    pub y: u16,
    pub x0: u16,
    pub x1: u16,
    pub time: f64,
}

pub fn push_px_hit(hits: &mut Vec<Hit>, rect: (f32, f32, f32, f32), cell: (f32, f32), clip_rows: (u16, u16), time: f64) {
    let (x0, y0, x1, y1) = rect;
    let (cw, ch) = cell;
    let r0 = (y0 / ch).floor().max(clip_rows.0 as f32) as u16;
    let r1 = ((y1 / ch).ceil() as u16).min(clip_rows.1);
    let c0 = (x0 / cw).floor().max(0.0) as u16;
    let c1 = (x1 / cw).ceil().max(0.0) as u16;
    for y in r0..r1 {
        hits.push(Hit { y, x0: c0, x1: c1, time });
    }
}

#[derive(Debug, Clone)]
pub struct LinkSpan {
    pub x: u16,
    pub y: u16,
    pub text: String,
    pub url: String,
    pub fg: Rgb,
    pub bg: Option<Rgb>,
    pub bold: bool,
}
