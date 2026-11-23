use std::collections::HashMap;

use crate::anim::CubicBezier;
use crate::lyrics::process::{self, ItemKind, Prepared, State};

const GLIDE_SECS: f64 = 0.5;
const GLIDE_CURVE: CubicBezier = CubicBezier::new(0.22, 1.0, 0.36, 1.0);

#[derive(Debug, Clone, Copy)]
struct Glide {
    from: f64,
    to: f64,
    start: f64,
}

impl Glide {
    fn at(value: f64) -> Self {
        Self {
            from: value,
            to: value,
            start: f64::NEG_INFINITY,
        }
    }

    fn value(&self, now: f64) -> f64 {
        let p = ((now - self.start) / GLIDE_SECS).clamp(0.0, 1.0);
        self.from + (self.to - self.from) * GLIDE_CURVE.ease(p)
    }

    fn done(&self, now: f64) -> bool {
        now >= self.start + GLIDE_SECS
    }
}

#[derive(Debug, Default)]
pub struct InterludeSpace {
    glides: HashMap<usize, Glide>,
    busy: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Spacing {
    pub tops: Vec<f64>,
    spaces: Vec<(usize, f64, bool)>,
    hidden: Vec<f64>,
    shown: Vec<f64>,
}

impl Spacing {
    pub fn shift(&self, item: usize) -> f64 {
        self.shown.get(item).or(self.shown.last()).copied().unwrap_or(0.0)
    }

    pub fn hidden_total(&self) -> f64 {
        self.hidden.last().copied().unwrap_or(0.0)
    }

    pub fn is_open(&self, item: usize) -> bool {
        self.spaces.iter().find(|s| s.0 == item).is_none_or(|s| s.2)
    }
}

impl InterludeSpace {
    pub fn reset(&mut self) {
        self.glides.clear();
        self.busy = false;
    }

    pub fn is_animating(&self) -> bool {
        self.busy
    }

    pub fn snap(&mut self) {
        for g in self.glides.values_mut() {
            *g = Glide::at(g.to);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn settle(
        &mut self,
        prepared: &Prepared,
        tops: &[f64],
        end: f64,
        t: f64,
        target: usize,
        now: f64,
        reserve: bool,
    ) -> Spacing {
        let n = tops.len();
        let mut spacing = Spacing {
            tops: Vec::with_capacity(n),
            spaces: Vec::new(),
            hidden: Vec::with_capacity(n + 1),
            shown: Vec::new(),
        };
        if reserve {
            self.reset();
            spacing.tops = tops.to_vec();
            spacing.hidden = vec![0.0; n + 1];
            return spacing;
        }
        let mut hidden = 0.0;
        for (i, &top) in tops.iter().enumerate() {
            spacing.hidden.push(hidden);
            spacing.tops.push(top - hidden);
            let Some(ItemKind::Interlude { start, end: stop }) = prepared.items.get(i).map(|it| it.kind) else {
                continue;
            };
            let space = tops.get(i + 1).copied().unwrap_or(end) - top;
            let open = i == target || process::state(t, start, stop) == State::Active;
            let goal = if open { 1.0 } else { 0.0 };
            let glide = self.glides.entry(i).or_insert_with(|| Glide::at(goal));
            if glide.to != goal {
                *glide = Glide {
                    from: glide.value(now),
                    to: goal,
                    start: now,
                };
            }
            let counted = open && i >= target;
            if !counted {
                hidden += space;
            }
            spacing.spaces.push((i, space, counted));
        }
        spacing.hidden.push(hidden);
        self.glides.retain(|i, _| *i < n);
        spacing
    }

    pub fn show(&mut self, spacing: &mut Spacing, now: f64) {
        let n = spacing.tops.len();
        spacing.shown = vec![0.0; n + 1];
        self.busy = false;
        let mut shown = 0.0;
        let mut next = spacing.spaces.iter().peekable();
        for i in 0..=n {
            spacing.shown[i] = shown;
            if let Some(&&(item, space, _)) = next.peek()
                && item == i
            {
                next.next();
                let glide = self.glides.get(&item).copied().unwrap_or(Glide::at(1.0));
                if !glide.done(now) {
                    self.busy = true;
                }
                shown += (1.0 - glide.value(now)) * space;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::process::{PrepareOptions, prepare};
    use crate::lyrics::{LyricLine, Lyrics, SyncKind, VocalGroup};

    fn prepared() -> Prepared {
        let line = |text: &str, start: f64, end: f64| LyricLine {
            start,
            end,
            opposite: false,
            lead: VocalGroup::single(text.into(), start, end),
            background: vec![],
            background_only: false,
        };
        let lyrics = Lyrics {
            kind: SyncKind::Line,
            lines: vec![line("a", 0.0, 2.0), line("b", 10.0, 12.0), line("c", 12.0, 14.0)],
            credits: Default::default(),
            language: None,
            instrumental: false,
        };
        prepare(
            &lyrics,
            PrepareOptions {
                interludes: true,
                interlude_threshold: 3.0,
                extend_lines: false,
                join_split_words: false,
                fix_background: false,
            },
        )
    }

    #[test]
    fn interludes_take_no_space_until_they_start() {
        let p = prepared();
        assert!(matches!(p.items[1].kind, ItemKind::Interlude { .. }));
        let tops = [0.0, 2.0, 4.0, 6.0];
        let mut space = InterludeSpace::default();
        let target = p.scroll_target(1.0);
        let mut s = space.settle(&p, &tops, 8.0, 1.0, target, 0.0, false);
        space.show(&mut s, 0.0);
        assert_eq!(s.tops, vec![0.0, 2.0, 2.0, 4.0]);
        assert_eq!(s.shift(2), 2.0);
        assert!(!s.is_open(1));
        assert_eq!(s.hidden_total(), 2.0);

        let t = 5.0;
        let target = p.scroll_target(t);
        assert_eq!(target, 1);
        let mut s = space.settle(&p, &tops, 8.0, t, target, 1.0, false);
        space.show(&mut s, 1.0);
        assert_eq!(s.tops, tops.to_vec());
        assert_eq!(s.shift(2), 2.0);
        assert!(space.is_animating());
        space.show(&mut s, 1.25);
        assert!(s.shift(2) > 0.0 && s.shift(2) < 2.0);
        space.show(&mut s, 2.0);
        assert_eq!(s.shift(2), 0.0);
        assert_eq!(s.shift(4), 0.0);
        assert!(!space.is_animating());
    }

    #[test]
    fn scroll_stays_put_when_leading_past_an_open_interlude() {
        let p = prepared();
        let tops = [0.0, 2.0, 4.0, 6.0];
        let mut space = InterludeSpace::default();
        let mut s = space.settle(&p, &tops, 8.0, 5.0, 1, 0.0, false);
        space.show(&mut s, 0.0);
        let interlude_top = s.tops[1];

        let mut s = space.settle(&p, &tops, 8.0, 9.8, 2, 1.0, false);
        space.show(&mut s, 1.0);
        assert_eq!(s.tops[2], interlude_top);
        assert_eq!(s.shift(2), 0.0);
        assert!(!s.is_open(1));

        let mut s = space.settle(&p, &tops, 8.0, 10.0, 2, 1.2, false);
        space.show(&mut s, 1.2);
        assert_eq!(s.tops[2], interlude_top);
        space.show(&mut s, 2.0);
        assert_eq!(s.shift(2), 2.0);
    }

    #[test]
    fn reserved_space_never_moves_lines() {
        let p = prepared();
        let tops = [0.0, 2.0, 4.0, 6.0];
        let mut space = InterludeSpace::default();
        let mut s = space.settle(&p, &tops, 8.0, 1.0, 0, 0.0, true);
        space.show(&mut s, 0.0);
        assert_eq!(s.tops, tops.to_vec());
        assert_eq!(s.shift(3), 0.0);
        assert_eq!(s.hidden_total(), 0.0);
    }

    #[test]
    fn snapping_skips_the_glide() {
        let p = prepared();
        let tops = [0.0, 2.0, 4.0, 6.0];
        let mut space = InterludeSpace::default();
        let mut s = space.settle(&p, &tops, 8.0, 1.0, 0, 0.0, false);
        space.show(&mut s, 0.0);
        let mut s = space.settle(&p, &tops, 8.0, 5.0, 1, 1.0, false);
        space.snap();
        space.show(&mut s, 1.0);
        assert_eq!(s.shift(2), 0.0);
        assert!(!space.is_animating());
    }
}
