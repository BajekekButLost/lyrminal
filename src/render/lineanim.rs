use std::collections::HashMap;

use crate::anim::Spring;

const ACTIVE_FREQ: f64 = 2.6;
const DEPTH_FREQ: f64 = 2.2;
const EPS: f64 = 0.002;

#[derive(Debug, Clone, Copy)]
struct Entry {
    active: Spring,
    depth: Spring,
    frame: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct LineLook {
    pub active: f64,
    pub depth: f64,
    pub settled: bool,
}

#[derive(Debug, Default)]
pub struct LineAnims {
    map: HashMap<usize, Entry>,
    frame: u64,
}

impl LineAnims {
    pub fn reset(&mut self) {
        self.map.clear();
    }

    pub fn begin_frame(&mut self) {
        self.frame += 1;
        let frame = self.frame;
        self.map.retain(|_, e| frame - e.frame <= 2);
    }

    pub fn step(&mut self, item: usize, active: f64, depth: f64, dt: f64) -> LineLook {
        let frame = self.frame;
        let fresh = self.map.get(&item).is_none_or(|e| frame - e.frame > 1);
        let e = self.map.entry(item).or_insert_with(|| Entry {
            active: Spring::new(active, ACTIVE_FREQ, 1.0),
            depth: Spring::new(depth, DEPTH_FREQ, 1.0),
            frame,
        });
        if fresh {
            e.active.snap(active);
            e.depth.snap(depth);
        }
        e.frame = frame;
        e.active.set_goal(active);
        e.depth.set_goal(depth);
        e.active.step(dt);
        e.depth.step(dt);
        let a = e.active.position.clamp(0.0, 1.0);
        let settled = (e.active.position - active).abs() < EPS
            && (e.depth.position - depth).abs() < EPS
            && e.active.velocity.abs() < 0.01
            && e.depth.velocity.abs() < 0.01;
        if settled {
            e.active.snap(active);
            e.depth.snap(depth);
        }
        LineLook {
            active: if settled { active } else { a },
            depth: if settled { depth } else { e.depth.position },
            settled,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eases_between_states_and_settles() {
        let mut a = LineAnims::default();
        a.begin_frame();
        let first = a.step(3, 0.0, 1.0, 1.0 / 60.0);
        assert!(first.settled && first.active == 0.0);
        let mut last = first;
        let mut frames = 0;
        for _ in 0..120 {
            a.begin_frame();
            last = a.step(3, 1.0, 1.0, 1.0 / 60.0);
            frames += 1;
            if last.settled {
                break;
            }
            assert!(last.active > 0.0 && last.active < 1.0);
        }
        assert!(last.settled && last.active == 1.0);
        assert!(frames > 10 && frames < 60);
    }

    #[test]
    fn unseen_lines_snap_instead_of_animating() {
        let mut a = LineAnims::default();
        a.begin_frame();
        a.step(1, 0.0, 1.0, 0.016);
        for _ in 0..5 {
            a.begin_frame();
        }
        a.begin_frame();
        let look = a.step(1, 1.0, 0.5, 0.016);
        assert!(look.settled && look.active == 1.0 && look.depth == 0.5);
    }
}
