use std::time::{Duration, Instant};

#[derive(Default)]
pub struct State {
    focused: bool,
    gained_focus_at: Option<Instant>,
}

impl State {
    pub fn update(&mut self, focused: bool, now: Instant) -> bool {
        if !focused {
            self.focused = false;
            self.gained_focus_at = None;
            return false;
        }

        if !self.focused {
            self.focused = true;
            self.gained_focus_at = Some(now);
            return false;
        }

        let Some(gained_focus_at) = self.gained_focus_at else {
            return false;
        };
        if now.duration_since(gained_focus_at) >= Duration::from_millis(200) {
            self.gained_focus_at = None;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn refreshes_once_on_startup_and_again_after_refocus() {
        let mut state = super::State::default();
        let t0 = std::time::Instant::now();

        assert!(!state.update(true, t0));
        assert!(!state.update(true, t0 + std::time::Duration::from_millis(199)));
        assert!(state.update(true, t0 + std::time::Duration::from_millis(200)));
        assert!(!state.update(true, t0 + std::time::Duration::from_millis(250)));
        assert!(!state.update(false, t0 + std::time::Duration::from_millis(260)));
        assert!(!state.update(true, t0 + std::time::Duration::from_millis(261)));
        assert!(state.update(true, t0 + std::time::Duration::from_millis(461)));
    }
}
