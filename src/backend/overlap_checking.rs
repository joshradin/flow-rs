//! Utils for range overlap checking

use std::cmp::Ordering;
use std::collections::Bound;
use std::fmt::Debug;
use std::ops::RangeBounds;

type FullBound<T> = (Bound<T>, Bound<T>);

/// A type for checking overlaps of ranges
#[derive(Debug)]
pub struct OverlapChecker<T> {
    bounds: Vec<FullBound<T>>,
}
impl<T> Default for OverlapChecker<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> OverlapChecker<T> {
    /// Creates a new overlap checker
    pub fn new() -> Self {
        Self {
            bounds: Default::default(),
        }
    }
}

impl<T: Ord + Clone> OverlapChecker<T> {
    /// Inserts a range bound, returning true if there is no overlap with existing bounds.
    pub fn insert<R: RangeBounds<T>>(&mut self, range: R) -> bool {
        let bound: FullBound<T> = (range.start_bound().cloned(), range.end_bound().cloned());
        let any_overlap = self
            .bounds
            .iter()
            .any(|existing| ranges_overlap(existing, &bound));
        if any_overlap {
            false
        } else {
            self.bounds.push(bound);
            true
        }
    }
}

fn ranges_overlap<T: Ord>(a: &impl RangeBounds<T>, b: &impl RangeBounds<T>) -> bool {
    let (a_low, b_low) = (a.start_bound(), b.start_bound());
    let (a_hi, b_hi) = (a.end_bound(), b.end_bound());

    let low = range_max_start_bound(a_low, b_low);
    let high = range_min_end_bound(a_hi, b_hi);

    match compare_bound(low, high) {
        Some(Ordering::Less | Ordering::Equal) => true,
        Some(Ordering::Greater) | None  => false,
    }
}

fn compare_bound<'a, T: Ord>(a: Bound<&'a T>, b: Bound<&'a T>) -> Option<Ordering> {
    match (a, b) {
        (_, Bound::Unbounded) => Some(Ordering::Less),
        (Bound::Unbounded, _) => Some(Ordering::Greater),
        (Bound::Included(a), Bound::Included(b)) | (Bound::Excluded(a), Bound::Excluded(b)) => {
            Some(a.cmp(b))
        }
        (Bound::Included(a_t), Bound::Excluded(b_t)) => {
            if a_t == b_t {
                None
            } else {
                Some(a_t.cmp(b_t))
            }
        }
        (Bound::Excluded(a_t), Bound::Included(b_t)) => {
            if a_t == b_t {
                None
            } else {
                Some(a_t.cmp(b_t))
            }
        }
    }
}

fn range_max_start_bound<'a, T: Ord>(a: Bound<&'a T>, b: Bound<&'a T>) -> Bound<&'a T> {
    match (a, b) {
        (Bound::Unbounded, b) => b,
        (a, Bound::Unbounded) => a,
        (Bound::Excluded(a_t), Bound::Excluded(b_t))
        | (Bound::Included(a_t), Bound::Included(b_t)) => {
            if a_t > b_t {
                a
            } else {
                b
            }
        }
        (Bound::Included(a_t), Bound::Excluded(b_t)) => {
            if a_t > b_t {
                a
            } else {
                b
            }
        }
        (Bound::Excluded(a_t), Bound::Included(b_t)) => {
            if a_t >= b_t {
                a
            } else {
                b
            }
        }
    }
}

fn range_min_end_bound<'a, T: Ord>(a: Bound<&'a T>, b: Bound<&'a T>) -> Bound<&'a T> {
    match (a, b) {
        (Bound::Unbounded, b) => b,
        (a, Bound::Unbounded) => a,
        (Bound::Excluded(a_t), Bound::Excluded(b_t))
        | (Bound::Included(a_t), Bound::Included(b_t)) => {
            if a_t < b_t {
                a
            } else {
                b
            }
        }
        (Bound::Included(a_t), Bound::Excluded(b_t)) => {
            if a_t < b_t {
                a
            } else {
                b
            }
        }
        (Bound::Excluded(a_t), Bound::Included(b_t)) => {
            if a_t < b_t {
                a
            } else {
                b
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ranges_overlap() {
        assert!(ranges_overlap(&.., &(0..2)));
        assert_eq!(ranges_overlap(&(-1..0), &(0..2)), false);
        assert_eq!(ranges_overlap(&(-1..=0), &(0..2)), true);
        assert_eq!(ranges_overlap(&(..=0), &(0..)), true);
        assert_eq!(ranges_overlap(&(..0), &(0..)), false);
        assert_eq!(ranges_overlap(&(..0), &(10..)), false);
    }
    #[test]
    fn test_overlap_checker() {
        let mut overlap = OverlapChecker::<usize>::new();
        assert!(overlap.insert(1..=2));
        assert!(overlap.insert(3..));
    }
}
