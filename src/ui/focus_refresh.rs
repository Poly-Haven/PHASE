#[derive(Default)]
pub struct State {
    focused: bool,
}

impl State {
    pub fn update(&mut self, focused: bool) -> bool {
        let gained_focus = focused && !self.focused;
        self.focused = focused;
        gained_focus
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn refreshes_once_on_startup_and_again_after_refocus() {
        let mut state = super::State::default();

        assert!(state.update(true));
        assert!(!state.update(true));
        assert!(!state.update(false));
        assert!(state.update(true));
    }
}
