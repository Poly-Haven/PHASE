use crate::notion::StatusGroup;

pub fn selected_color(group: StatusGroup) -> egui::Color32 {
    match group {
        StatusGroup::ToDo => super::colors::STATUS_TODO,
        StatusGroup::InProgress => super::colors::STATUS_IN_PROGRESS,
        StatusGroup::Complete => super::colors::STATUS_COMPLETE,
    }
}

pub fn select(
    mut selected: Vec<StatusGroup>,
    clicked: StatusGroup,
    additive: bool,
) -> Vec<StatusGroup> {
    if !additive {
        return vec![clicked];
    }

    if selected.contains(&clicked) {
        selected.retain(|g| *g != clicked);
        if selected.is_empty() {
            selected.push(clicked);
        }
    } else {
        selected.push(clicked);
        selected.sort_by_key(|g| g.order());
    }
    selected
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_in_progress_without_persistence() {
        assert_eq!(StatusGroup::default_filter(), vec![StatusGroup::InProgress]);
    }

    #[test]
    fn shift_click_adds_or_removes_without_emptying_selection() {
        let selected = select(vec![StatusGroup::InProgress], StatusGroup::Complete, true);
        assert_eq!(
            selected,
            vec![StatusGroup::InProgress, StatusGroup::Complete]
        );

        let selected = select(selected, StatusGroup::Complete, true);
        assert_eq!(selected, vec![StatusGroup::InProgress]);

        let selected = select(vec![StatusGroup::InProgress], StatusGroup::InProgress, true);
        assert_eq!(selected, vec![StatusGroup::InProgress]);
    }

    #[test]
    fn status_group_colors_match_requested_defaults() {
        assert_eq!(
            selected_color(StatusGroup::ToDo),
            super::super::colors::ACCENT
        );
        assert_eq!(
            selected_color(StatusGroup::InProgress),
            super::super::colors::STATUS_IN_PROGRESS
        );
        assert_eq!(
            selected_color(StatusGroup::Complete),
            super::super::colors::STATUS_COMPLETE
        );
    }
}
