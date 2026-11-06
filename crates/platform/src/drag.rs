//! The tab drag state machine, free of windowing types. Input is whatever the source window's
//! mouse listeners saw. Every path out of a drag is a release or a cancel, and nothing here changes
//! which window owns a tab, so a cancelled or failed move cannot lose one.

/// Pixels the pointer must travel before a press becomes a drag.
pub const DRAG_THRESHOLD: f32 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CancelReason {
    /// The user pressed Escape.
    Escape,
    /// A move arrived with the button already up, so the release was lost (a broken grab).
    LostRelease,
    /// The source window went away.
    SourceClosed,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum State<K: Copy> {
    Idle,
    /// Pressed on a tab, under the threshold. A release here is an ordinary click.
    Armed { tab: K, start: (f32, f32) },
    Dragging { tab: K },
    /// Cancelled while the button is still down. Stays here until the release so that the rest of
    /// the gesture cannot start a new drag or click anything.
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Step<K: Copy> {
    None,
    Started { tab: K },
    Moved { tab: K },
    /// Dropped: the caller decides what the release point means.
    Released { tab: K },
    /// A press and release without a drag: leave it to the click handler.
    Clicked,
    Cancelled { reason: CancelReason },
}

#[derive(Clone, Copy, Debug)]
pub struct Machine<K: Copy> {
    state: State<K>,
}

impl<K: Copy> Default for Machine<K> {
    fn default() -> Self {
        Self { state: State::Idle }
    }
}

impl<K: Copy> Machine<K> {
    pub fn state(&self) -> State<K> {
        self.state
    }

    pub fn is_dragging(&self) -> bool {
        matches!(self.state, State::Dragging { .. })
    }

    /// A press on a tab. Any earlier gesture is over: its release can no longer be trusted, so a
    /// new press never inherits an old one.
    pub fn press(&mut self, tab: K, at: (f32, f32)) {
        self.state = State::Armed { tab, start: at };
    }

    pub fn moved(&mut self, at: (f32, f32), button_down: bool) -> Step<K> {
        match self.state {
            State::Idle => Step::None,
            State::Cancelled => {
                if !button_down {
                    self.state = State::Idle;
                }
                Step::None
            }
            State::Armed { tab, start } => {
                if !button_down {
                    self.state = State::Idle;
                    return Step::None;
                }
                let (dx, dy) = (at.0 - start.0, at.1 - start.1);
                if dx * dx + dy * dy < DRAG_THRESHOLD * DRAG_THRESHOLD {
                    return Step::None;
                }
                self.state = State::Dragging { tab };
                Step::Started { tab }
            }
            State::Dragging { tab } => {
                if !button_down {
                    self.state = State::Idle;
                    return Step::Cancelled { reason: CancelReason::LostRelease };
                }
                Step::Moved { tab }
            }
        }
    }

    pub fn released(&mut self) -> Step<K> {
        let state = std::mem::replace(&mut self.state, State::Idle);
        match state {
            State::Idle | State::Cancelled => Step::None,
            State::Armed { .. } => Step::Clicked,
            State::Dragging { tab } => Step::Released { tab },
        }
    }

    /// Cancels a live drag. Returns `None` if nothing was in progress, so that Escape can still do
    /// its usual job when no drag is under way.
    pub fn cancel(&mut self, reason: CancelReason) -> Option<Step<K>> {
        match self.state {
            State::Dragging { .. } => {
                self.state = State::Cancelled;
                Some(Step::Cancelled { reason })
            }
            State::Armed { .. } => {
                self.state = State::Cancelled;
                None
            }
            State::Idle | State::Cancelled => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dragging() -> Machine<u32> {
        let mut m = Machine::default();
        m.press(7, (10.0, 10.0));
        assert_eq!(m.moved((30.0, 10.0), true), Step::Started { tab: 7 });
        m
    }

    #[test]
    fn small_motion_is_a_click() {
        let mut m = Machine::default();
        m.press(7, (10.0, 10.0));
        assert_eq!(m.moved((12.0, 11.0), true), Step::None);
        assert_eq!(m.released(), Step::Clicked);
        assert_eq!(m.state(), State::Idle);
    }

    #[test]
    fn drag_then_release() {
        let mut m = dragging();
        assert_eq!(m.moved((40.0, 10.0), true), Step::Moved { tab: 7 });
        assert!(m.is_dragging());
        assert_eq!(m.released(), Step::Released { tab: 7 });
        assert_eq!(m.released(), Step::None);
    }

    #[test]
    fn escape_cancels_and_the_release_does_nothing() {
        let mut m = dragging();
        assert!(matches!(m.cancel(CancelReason::Escape), Some(Step::Cancelled { .. })));
        assert_eq!(m.moved((50.0, 10.0), true), Step::None);
        assert_eq!(m.released(), Step::None);
        assert_eq!(m.state(), State::Idle);
    }

    #[test]
    fn escape_without_a_drag_is_not_consumed() {
        let mut m: Machine<u32> = Machine::default();
        assert_eq!(m.cancel(CancelReason::Escape), None);
    }

    #[test]
    fn a_lost_release_cancels_rather_than_drops() {
        let mut m = dragging();
        assert_eq!(m.moved((50.0, 10.0), false), Step::Cancelled { reason: CancelReason::LostRelease });
        assert_eq!(m.state(), State::Idle);
    }

    #[test]
    fn a_new_press_discards_an_unfinished_gesture() {
        let mut m = dragging();
        m.press(8, (0.0, 0.0));
        assert_eq!(m.released(), Step::Clicked);
    }
}
