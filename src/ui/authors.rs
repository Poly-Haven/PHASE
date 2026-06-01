pub fn names(author: &str) -> impl Iterator<Item = &str> {
    author
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
}

pub fn filter_options<'a>(authors: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let set: std::collections::BTreeSet<String> = authors
        .into_iter()
        .flat_map(names)
        .map(str::to_string)
        .collect();
    set.into_iter().collect()
}

pub fn contains_any(author: &str, filters: &[String]) -> bool {
    filters.is_empty()
        || names(author).any(|name| filters.iter().any(|filter| filter.as_str() == name))
}

pub fn select_author_filters(
    mut selected: Vec<String>,
    authors: &[String],
    clicked_index: usize,
    shift: bool,
) -> Vec<String> {
    if clicked_index >= authors.len() {
        return selected;
    }

    let clicked = &authors[clicked_index];
    if shift {
        if !selected.iter().any(|selected| selected == clicked) {
            selected.push(clicked.clone());
            selected.sort();
        }
        return selected;
    }

    vec![clicked.clone()]
}

#[cfg(test)]
mod tests {
    #[test]
    fn contains_any_selected_author() {
        assert!(super::contains_any(
            "Alice, Bob",
            &["Carol".into(), "Bob".into()]
        ));
        assert!(!super::contains_any("Alice, Bob", &["Carol".into()]));
        assert!(super::contains_any("Alice, Bob", &[]));
    }

    #[test]
    fn normal_click_selects_only_clicked_author() {
        let authors = vec![
            "Alice".into(),
            "Bob".into(),
            "Carol".into(),
            "Didier".into(),
        ];
        let selection =
            super::select_author_filters(vec!["Alice".into(), "Carol".into()], &authors, 1, false);

        assert_eq!(selection, vec!["Bob".to_string()]);
    }

    #[test]
    fn shift_click_adds_only_clicked_author() {
        let authors = vec![
            "Alice".into(),
            "Bob".into(),
            "Carol".into(),
            "Didier".into(),
        ];
        let selection = super::select_author_filters(vec!["Bob".into()], &authors, 3, true);

        assert_eq!(selection, vec!["Bob".to_string(), "Didier".to_string()]);
    }
}
