pub mod cell;
pub mod effects;
pub mod pixel;
pub mod scroll;

use crate::anim::Rgb;
use crate::config::{AppearanceConfig, LyricsConfig, VisualStyle};
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
    pub follow: &'a Follow,
    pub screen_height: u16,
    pub show_cursor: bool,
}

impl FrameInput<'_> {
    pub fn style(&self) -> VisualStyle {
        self.appearance.style
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
