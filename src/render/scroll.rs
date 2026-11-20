use std::collections::HashMap;

use super::effects::{SCROLL_CURVE, ripple_delay};
use crate::anim::Spring;

#[derive(Debug, Clone, Copy)]
struct Tween {
    from: f64,
    start: f64,
    duration: f64,
}

impl Tween {
    fn value(&self, now: f64) -> f64 {
        if now <= self.start {
            self.from
        } else {
            self.from * (1.0 - SCROLL_CURVE.ease((now - self.start) / self.duration))
        }
    }

    fn done(&self, now: f64) -> bool {
        now >= self.start + self.duration
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollMode {
    Rigid,
    Ripple,
}

#[derive(Debug, Clone)]
pub struct Scroller {
    spring: Spring,
    base: f64,
    tweens: HashMap<usize, Tween>,
    last_target: Option<usize>,
    initialized: bool,
}

impl Default for Scroller {
    fn default() -> Self {
        Self {
            spring: Spring::new(0.0, 1.0, 1.0),
            base: 0.0,
            tweens: HashMap::new(),
            last_target: None,
            initialized: false,
        }
    }
}

pub struct ScrollInput<'a> {
    pub now: f64,
    pub dt: f64,
    pub desired: f64,
    pub target: usize,
    pub tops: &'a [f64],
    pub mode: ScrollMode,
    pub silence: f64,
    pub viewport: f64,
    pub manual: bool,
}

impl Scroller {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn base(&self) -> f64 {
        self.base
    }

    pub fn item_offset(&self, item: usize, now: f64) -> f64 {
        self.tweens.get(&item).map(|t| t.value(now)).unwrap_or(0.0)
    }

    pub fn is_animating(&self) -> bool {
        !self.tweens.is_empty() || !self.spring.is_settled()
    }

    pub fn update(&mut self, input: ScrollInput) {
        let ScrollInput {
            now,
            dt,
            desired,
            target,
            tops,
            mode,
            silence,
            viewport,
            manual,
        } = input;
        self.tweens.retain(|_, t| !t.done(now));
        if !self.initialized {
            self.initialized = true;
            self.spring.snap(desired);
            self.base = desired;
            self.last_target = Some(target);
            return;
        }
        let jump = (desired - self.base).abs() > viewport * 1.5;
        if manual || mode == ScrollMode::Rigid {
            if !self.tweens.is_empty() {
                self.tweens.clear();
            }
            if jump {
                self.spring.snap(desired);
            } else {
                self.spring.set_goal(desired);
                self.spring.step(dt);
            }
            self.base = self.spring.position;
            self.last_target = Some(target);
            return;
        }
        let target_changed = self.last_target != Some(target);
        if (desired - self.base).abs() < 1e-9 {
            self.spring.snap(desired);
            self.last_target = Some(target);
            return;
        }
        if jump {
            self.tweens.clear();
            self.base = desired;
            self.spring.snap(desired);
            self.last_target = Some(target);
            return;
        }
        let old_base = self.base;
        let duration = super::effects::scroll_duration(silence);
        let lo = tops.partition_point(|&y| y < old_base.min(desired) - viewport);
        let hi = tops.partition_point(|&y| y < old_base.max(desired) + viewport * 2.0);
        for (item, _) in tops.iter().enumerate().take(hi).skip(lo) {
            let current = self.item_offset(item, now);
            let from = desired - old_base + current;
            if from.abs() < 1e-6 {
                self.tweens.remove(&item);
                continue;
            }
            let delay = if target_changed && item > target {
                ripple_delay(item - target)
            } else {
                0.0
            };
            self.tweens.insert(
                item,
                Tween {
                    from,
                    start: now + delay,
                    duration,
                },
            );
        }
        self.base = desired;
        self.spring.snap(desired);
        self.last_target = Some(target);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ripple_converges_to_target() {
        let tops = [0.0, 3.0, 6.0, 9.0, 12.0];
        let mut s = Scroller::default();
        let input = |now: f64, desired: f64, target: usize| ScrollInput {
            now,
            dt: 1.0 / 60.0,
            desired,
            target,
            tops: &tops,
            mode: ScrollMode::Ripple,
            silence: 0.5,
            viewport: 20.0,
            manual: false,
        };
        s.update(input(0.0, 0.0, 0));
        s.update(input(1.0, 3.0, 1));
        assert_eq!(s.base(), 3.0);
        assert!((s.item_offset(2, 1.0) - 3.0).abs() < 1e-9);
        assert!(s.item_offset(2, 3.0).abs() < 1e-9);
    }

    #[test]
    fn rigid_spring_moves_smoothly() {
        let tops = [0.0, 10.0];
        let mut s = Scroller::default();
        let mk = |desired: f64| ScrollInput {
            now: 0.0,
            dt: 1.0 / 60.0,
            desired,
            target: 0,
            tops: &tops,
            mode: ScrollMode::Rigid,
            silence: 0.5,
            viewport: 20.0,
            manual: false,
        };
        s.update(mk(0.0));
        s.update(mk(10.0));
        assert!(s.base() > 0.0 && s.base() < 10.0);
    }
}
