use std::sync::LazyLock;

use crate::anim::{CubicBezier, EASE, Spline, clamp01, ease_sin_out, lerp};

pub struct Splines {
    pub word_scale: Spline,
    pub word_lift: Spline,
    pub glow: Spline,
    pub letter_scale: Spline,
    pub letter_lift: Spline,
    pub line_glow: Spline,
    pub dot_scale: Spline,
    pub dot_lift: Spline,
    pub dot_glow: Spline,
    pub dot_opacity: Spline,
}

pub static SPLINES: LazyLock<Splines> = LazyLock::new(|| Splines {
    word_scale: Spline::new(&[(0.0, 0.95), (0.7, 1.0505), (1.0, 1.0)]),
    word_lift: Spline::new(&[(0.0, 0.01), (0.9, -1.0 / 60.0), (1.0, 0.0)]),
    glow: Spline::new(&[(0.0, 0.0), (0.15, 1.0), (0.6, 1.0), (1.0, 0.0)]),
    letter_scale: Spline::new(&[(0.0, 0.95), (0.7, 1.175), (1.0, 1.0)]),
    letter_lift: Spline::new(&[(0.0, 0.01), (0.9, -1.0 / 56.0), (1.0, 0.0)]),
    line_glow: Spline::new(&[(0.0, 0.0), (0.5, 1.0), (1.0, 0.0)]),
    dot_scale: Spline::new(&[(0.0, 0.75), (0.7, 1.05), (1.0, 1.0)]),
    dot_lift: Spline::new(&[(0.0, 0.0), (0.9, -0.12), (1.0, 0.0)]),
    dot_glow: Spline::new(&[(0.0, 0.0), (0.6, 1.0), (1.0, 1.0)]),
    dot_opacity: Spline::new(&[(0.0, 0.35), (0.6, 1.0), (1.0, 1.0)]),
});

pub mod spring {
    pub const NOT_SUNG: f32 = 0.30;
    pub const SUNG: f32 = 0.42;
    pub const ACTIVE_UNLIT: f32 = 0.5;
    pub const ACTIVE_LIT: f32 = 0.88;
    pub const LINE_UNLIT: f32 = 0.38;
    pub const BG_UNLIT: f32 = 0.3;
    pub const BG_LIT: f32 = 0.6;
    pub const LETTER_TAIL: f64 = 0.25;
}

pub mod bloom {
    pub const UNLIT: f32 = 0.2;
    pub const LIT: f32 = 0.94;
    pub const BG_LIT: f32 = 0.38;
    pub const SUNG_DELAY: f64 = 0.25;
    pub const SUNG_FADE: f64 = 0.35;
    pub const ROW_RUSH: f64 = 0.5;
    pub const CLOCK_LAG: f64 = 0.0335;
}

pub const SUNG_DIM_CURVE: CubicBezier = CubicBezier::new(0.39, 0.575, 0.565, 1.0);
pub const LINE_TONE_CURVE: CubicBezier = CubicBezier::new(0.33, 0.0, 0.2, 1.0);
pub const SCROLL_CURVE: CubicBezier = CubicBezier::new(0.4, 0.1, 0.0, 1.0);
pub const DOT_FILL_CURVE: CubicBezier = CubicBezier::new(0.0, 0.25, 1.0, 0.58);
pub const BREATH_CURVE: CubicBezier = CubicBezier::new(0.5, 0.02, 0.42, 0.98);
pub const DOT_EXIT_CURVE: CubicBezier = CubicBezier::new(0.25, 0.0, 1.0, 0.2);

pub fn glow(p: f64) -> f64 {
    SPLINES.glow.at(clamp01(p)).clamp(0.0, 1.0)
}

pub fn spring_fill(progress: f64, x_frac: f64) -> f64 {
    let gp = -20.0 + 120.0 * clamp01(progress);
    clamp01((gp + 20.0 - x_frac * 100.0) / 20.0)
}

pub fn vertical_fill(progress: f64, y_frac: f64) -> f64 {
    spring_fill(progress, y_frac)
}

pub fn spring_letter_window(start: f64, end: f64, n: usize, i: usize) -> (f64, f64) {
    let span_end = (end - spring::LETTER_TAIL).max(start + 0.01);
    let each = (span_end - start) / n.max(1) as f64;
    let s = start + each * i as f64;
    (s, s + each)
}

pub fn spring_letter_fill(t: f64, start: f64, end: f64) -> f64 {
    if t <= start {
        0.0
    } else if t >= end {
        1.0
    } else {
        ease_sin_out((t - start) / (end - start))
    }
}

pub fn neighbour_glow(distance: f64) -> f64 {
    1.0 / (1.0 + 0.9 * distance)
}

pub fn bloom_flame(elapsed: f64, duration: f64, letters: usize, index: usize) -> f64 {
    let d = duration * 1000.0;
    let n = letters.max(1) as f64;
    let phase = d.min(3000.0);
    let stagger = (0.4 * d / n).min(400.0);
    let hold = 2.0 * d / n;
    let at = (elapsed * 1000.0 - 21.0) - index as f64 * stagger;
    if at <= 0.0 || phase <= 0.0 {
        return 0.0;
    }
    if at < hold {
        EASE.ease(at / phase)
    } else {
        EASE.ease(hold / phase) * (1.0 - EASE.ease((at - hold) / phase))
    }
}

pub fn bloom_flame_peak(duration: f64) -> f64 {
    0.14 * ((duration).clamp(1.0, 2.0) - 1.0)
}

pub fn bloom_sung_dim(since_end: f64) -> f64 {
    if since_end <= bloom::SUNG_DELAY {
        0.0
    } else {
        SUNG_DIM_CURVE.ease((since_end - bloom::SUNG_DELAY) / bloom::SUNG_FADE)
    }
}

pub fn bloom_line_tone(t: f64, start: f64, end: f64) -> f64 {
    if t < start {
        0.0
    } else if t < end {
        LINE_TONE_CURVE.ease((t - start) / 0.3)
    } else {
        1.0 - LINE_TONE_CURVE.ease((t - end) / 0.4)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct EdgeWord {
    pub x: f64,
    pub w: f64,
    pub start: f64,
    pub end: f64,
    pub letters: usize,
}

pub fn bloom_row_edge(words: &[EdgeWord], t: f64, fade: f64) -> f64 {
    let Some(first) = words.first() else { return f64::MIN };
    let mut edge = first.x - fade;
    let mut left_at = f64::MIN;
    for (i, w) in words.iter().enumerate() {
        let reach = if i + 1 == words.len() {
            1.0
        } else if w.letters <= 2 {
            0.5
        } else {
            0.25
        };
        let from = (w.x - fade / 2.0).max(edge);
        let to = w.x + w.w + fade * reach - fade / 2.0;
        if t < w.start {
            return edge;
        }
        if t < w.end {
            let p = (t - w.start) / (w.end - w.start).max(1e-6);
            return lerp(from, to, p);
        }
        edge = to.max(edge);
        left_at = w.end;
    }
    let last = words.last().unwrap();
    let row_end = last.x + last.w + fade;
    let p = clamp01((t - left_at) / bloom::ROW_RUSH);
    lerp(edge, row_end + fade, p)
}

pub fn bloom_edge_lit(edge: f64, center: f64, fade: f64) -> f64 {
    clamp01((edge + fade / 2.0 - center) / fade)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dot {
    pub fill: f64,
    pub scale: f64,
    pub opacity: f64,
    pub glow: f64,
    pub lift: f64,
}

pub fn spring_dots(t: f64, start: f64, end: f64) -> [Dot; 3] {
    let dur = (end - start).max(0.01);
    let usable = (dur - 0.55).max(dur * 0.5);
    let each = usable / 3.0;
    let local = t - start;
    let hide = clamp01((local - (dur - 0.5)) / 0.4);
    let shrink = 1.0 - hide * hide;
    let mut out = [Dot {
        fill: 0.0,
        scale: 0.75,
        opacity: 0.35,
        glow: 0.0,
        lift: 0.0,
    }; 3];
    for (i, d) in out.iter_mut().enumerate() {
        let p = clamp01((local - each * i as f64) / each);
        d.fill = p;
        d.scale = SPLINES.dot_scale.at(p) * shrink;
        d.opacity = SPLINES.dot_opacity.at(p) * shrink;
        d.glow = SPLINES.dot_glow.at(p) * shrink;
        d.lift = SPLINES.dot_lift.at(p);
    }
    out
}

pub fn bloom_dots(t: f64, start: f64, end: f64) -> [Dot; 3] {
    let dur = ((end - start) * 1000.0).max(10.0);
    let local = (t - start) * 1000.0;
    let exit_at = (dur - 500.0 - 750.0).max(0.0);
    let share = (exit_at + 750.0) / 3.0;
    let cycles = (exit_at / 4000.0).floor();
    let breath = if cycles >= 1.0 && local < cycles * 4000.0 {
        let cycle_len = exit_at / cycles;
        let phase = (local % cycle_len) / cycle_len;
        if phase < 0.5 {
            1.0 + 0.2 * BREATH_CURVE.ease(phase * 2.0)
        } else {
            1.2 - 0.2 * BREATH_CURVE.ease((phase - 0.5) * 2.0)
        }
    } else {
        1.0
    };
    let (exit_scale, exit_opacity) = if local < exit_at {
        (1.0, 1.0)
    } else if local < exit_at + 750.0 {
        (lerp(1.0, 1.2, EASE.ease((local - exit_at) / 750.0)), 1.0)
    } else {
        let p = DOT_EXIT_CURVE.ease((local - exit_at - 750.0) / 250.0);
        (lerp(1.2, 0.5, p), 1.0 - p)
    };
    let mut out = [Dot {
        fill: 0.0,
        scale: 1.0,
        opacity: 0.0,
        glow: 0.0,
        lift: 0.0,
    }; 3];
    for (i, d) in out.iter_mut().enumerate() {
        let fill_start = share * i as f64;
        let fill_len = if i == 2 { (dur - 250.0 - fill_start).max(1.0) } else { share };
        let fill = DOT_FILL_CURVE.ease(clamp01((local - fill_start) / fill_len));
        let enter = clamp01((local - 50.0 * i as f64) / 750.0).powi(2);
        d.fill = fill;
        d.scale = if local < exit_at { breath } else { exit_scale };
        d.opacity = (0.27 + 0.73 * fill) * enter * exit_opacity;
        d.glow = fill * 0.6 * exit_opacity;
    }
    out
}

pub fn scroll_duration(silence: f64) -> f64 {
    let p = clamp01((silence - 0.2) / 0.55);
    lerp(0.48, 0.75, p)
}

pub fn ripple_delay(rows_below: usize) -> f64 {
    match rows_below {
        0 => 0.0,
        1 => 0.025,
        2 => 0.044,
        3 => 0.056,
        4 => 0.063,
        n => 0.063 + 0.007 * (n - 4) as f64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spring_fill_sweeps() {
        assert_eq!(spring_fill(0.0, 0.5), 0.0);
        assert_eq!(spring_fill(1.0, 0.99), 1.0);
        assert!(spring_fill(0.5, 0.2) > 0.9);
        assert!(spring_fill(0.5, 0.8) < 0.1);
    }

    #[test]
    fn bloom_edge_advances() {
        let words = [
            EdgeWord { x: 0.0, w: 4.0, start: 0.0, end: 1.0, letters: 4 },
            EdgeWord { x: 5.0, w: 3.0, start: 1.0, end: 2.0, letters: 3 },
        ];
        let a = bloom_row_edge(&words, 0.5, 2.0);
        let b = bloom_row_edge(&words, 1.5, 2.0);
        let c = bloom_row_edge(&words, 3.0, 2.0);
        assert!(a < b && b < c);
        assert_eq!(bloom_edge_lit(c, 7.5, 2.0), 1.0);
    }

    #[test]
    fn flame_rises_and_falls() {
        let early = bloom_flame(0.3, 2.0, 4, 0);
        let mid = bloom_flame(1.0, 2.0, 4, 0);
        let late = bloom_flame(5.0, 2.0, 4, 0);
        assert!(early < mid);
        assert!(late < mid);
    }

    #[test]
    fn dots_fill_in_order() {
        let d = spring_dots(2.0, 0.0, 6.0);
        assert!(d[0].fill > d[1].fill);
        let b = bloom_dots(3.0, 0.0, 10.0);
        assert!(b[0].fill >= b[1].fill && b[1].fill >= b[2].fill);
    }
}
