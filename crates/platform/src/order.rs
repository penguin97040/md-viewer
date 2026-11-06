//! Tab order arithmetic. `at` is always an insertion index into the strip as it stands, with the
//! dragged tab still in it, as `movement.js` `insertion` computes it.

/// The insertion index for a pointer at `x`, given each tab's left edge and width. A pointer left of
/// a tab's midpoint inserts before that tab.
pub fn insertion_index(x: f32, tabs: impl IntoIterator<Item = (f32, f32)>) -> usize {
    let mut n = 0;
    for (i, (left, width)) in tabs.into_iter().enumerate() {
        if x < left + width / 2.0 {
            return i;
        }
        n = i + 1;
    }
    n
}

/// Moves `items[from]` so it lands at insertion index `at`. Returns false, changing nothing, if the
/// move is out of range or would leave the order as it is.
pub fn reorder<T>(items: &mut Vec<T>, from: usize, at: usize) -> bool {
    if from >= items.len() || at > items.len() {
        return false;
    }
    let to = if at > from { at - 1 } else { at };
    if to == from {
        return false;
    }
    let item = items.remove(from);
    items.insert(to, item);
    true
}

/// The active index after the tab at `removed` left a strip that now has `remaining` tabs.
pub fn active_after_removal(active: usize, removed: usize, remaining: usize) -> usize {
    let active = if removed < active { active - 1 } else { active };
    active.min(remaining.saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insertion_uses_tab_midpoints() {
        let tabs = [(0.0, 100.0), (100.0, 100.0), (200.0, 100.0)];
        assert_eq!(insertion_index(-5.0, tabs), 0);
        assert_eq!(insertion_index(49.0, tabs), 0);
        assert_eq!(insertion_index(51.0, tabs), 1);
        assert_eq!(insertion_index(149.0, tabs), 1);
        assert_eq!(insertion_index(251.0, tabs), 3);
        assert_eq!(insertion_index(10.0, []), 0);
    }

    #[test]
    fn reorder_forward_and_back() {
        let mut v = vec!['a', 'b', 'c', 'd'];
        assert!(reorder(&mut v, 0, 3));
        assert_eq!(v, ['b', 'c', 'a', 'd']);
        assert!(reorder(&mut v, 3, 0));
        assert_eq!(v, ['d', 'b', 'c', 'a']);
        assert!(reorder(&mut v, 1, 4));
        assert_eq!(v, ['d', 'c', 'a', 'b']);
    }

    #[test]
    fn reorder_onto_itself_changes_nothing() {
        let mut v = vec!['a', 'b', 'c'];
        assert!(!reorder(&mut v, 1, 1));
        assert!(!reorder(&mut v, 1, 2));
        assert!(!reorder(&mut v, 9, 0));
        assert!(!reorder(&mut v, 0, 9));
        assert_eq!(v, ['a', 'b', 'c']);
    }

    #[test]
    fn active_follows_removal() {
        assert_eq!(active_after_removal(2, 0, 3), 1);
        assert_eq!(active_after_removal(1, 1, 2), 1);
        assert_eq!(active_after_removal(2, 2, 2), 1);
        assert_eq!(active_after_removal(0, 0, 0), 0);
        assert_eq!(active_after_removal(0, 2, 2), 0);
    }
}
