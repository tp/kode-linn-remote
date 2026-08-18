//! Values we set ourselves and defend, briefly, against the device.
//!
//! A remote is a screen next to a machine that lags it. Pressing pause and
//! watching the icon appear, vanish, and come back is not the network being
//! slow — it is us showing the press, then overwriting it with a reply that
//! was composed before the press arrived, then correcting ourselves.
//!
//! So a press sets the value locally and holds it. The hold ends when the
//! device agrees, or when it has gone on long enough that we are more likely
//! wrong than early. There is no third outcome: a held value always resolves,
//! which is what stops a dropped command leaving the screen lying.

/// How long a locally-set value outranks the device.
///
/// Long enough to cover a command round trip and the device's own settling
/// time, short enough that being wrong is a blip rather than a stuck screen.
pub(crate) const OPTIMISTIC_TTL_MS: u64 = 2_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Optimistic<T> {
    value: T,
    expires_at_ms: u64,
}

impl<T: Copy + PartialEq> Optimistic<T> {
    /// Claims a value on the user's behalf until the device catches up.
    pub(crate) fn hold(value: T, now_ms: u64) -> Self {
        Self {
            value,
            expires_at_ms: now_ms.saturating_add(OPTIMISTIC_TTL_MS),
        }
    }

    /// Decides what to believe, given what the device just said.
    ///
    /// Returns the value to display, and clears the hold once it has resolved
    /// — whether that is because the device agreed or because it ran out of
    /// time. Mirrors the same decision made for track skips in
    /// `app_runtime::playlist`.
    pub(crate) fn reconcile(held: &mut Option<Self>, incoming: T, now_ms: u64) -> T {
        let Some(current) = *held else {
            return incoming;
        };

        if incoming == current.value {
            *held = None;
            return incoming;
        }
        if now_ms >= current.expires_at_ms {
            *held = None;
            return incoming;
        }
        current.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_a_hold_the_device_is_simply_believed() {
        let mut held: Option<Optimistic<u8>> = None;
        assert_eq!(Optimistic::reconcile(&mut held, 7, 0), 7);
    }

    #[test]
    fn a_held_value_outranks_a_device_that_has_not_caught_up() {
        let mut held = Some(Optimistic::hold(50, 0));
        // The device is still reporting the level from before the press.
        assert_eq!(Optimistic::reconcile(&mut held, 40, 100), 50);
        assert!(held.is_some());
    }

    #[test]
    fn agreement_ends_the_hold() {
        let mut held = Some(Optimistic::hold(50, 0));
        assert_eq!(Optimistic::reconcile(&mut held, 50, 100), 50);
        assert!(held.is_none());
    }

    #[test]
    fn a_hold_that_is_never_confirmed_gives_way() {
        let mut held = Some(Optimistic::hold(50, 0));
        assert_eq!(Optimistic::reconcile(&mut held, 40, OPTIMISTIC_TTL_MS), 40);
        assert!(held.is_none(), "an expired hold must not linger");
    }

    #[test]
    fn holding_again_restarts_the_clock() {
        // Several presses in a row must not let the first one expire mid-turn
        // and let the device drag the value backwards.
        let mut held = Some(Optimistic::hold(50, 0));
        assert_eq!(Optimistic::reconcile(&mut held, 40, 1_000), 50);

        held = Some(Optimistic::hold(60, 1_500));
        // Past the *first* hold's deadline, but not the second's.
        assert_eq!(Optimistic::reconcile(&mut held, 40, OPTIMISTIC_TTL_MS), 60);
        assert!(held.is_some());
    }
}
