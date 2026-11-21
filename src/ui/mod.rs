pub mod draw;
pub mod keys;
pub mod library;
pub mod onboarding;
pub mod settings;

pub fn jump_group(starts: &[usize], selected: usize, dir: i32) -> Option<usize> {
    if starts.is_empty() {
        return None;
    }
    let current = starts.iter().rposition(|s| *s <= selected).unwrap_or(0) as i32;
    let next = (current + dir).rem_euclid(starts.len() as i32) as usize;
    Some(starts[next])
}

#[cfg(test)]
mod tests {
    use super::jump_group;

    #[test]
    fn jumps_between_groups_and_wraps() {
        let starts = [0, 2, 5];
        assert_eq!(jump_group(&starts, 0, 1), Some(2));
        assert_eq!(jump_group(&starts, 3, 1), Some(5));
        assert_eq!(jump_group(&starts, 6, 1), Some(0));
        assert_eq!(jump_group(&starts, 3, -1), Some(0));
        assert_eq!(jump_group(&starts, 0, -1), Some(5));
        assert_eq!(jump_group(&[], 0, 1), None);
    }
}
