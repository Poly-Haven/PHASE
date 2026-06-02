pub fn names(author: &str) -> impl Iterator<Item = &str> {
    author
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
}

/// Returns a map from full author name → display name.
/// If two different authors share the same first name, both use their full name.
/// Otherwise each author uses just their first name.
pub fn abbreviate_names(authors: &[String]) -> std::collections::HashMap<String, String> {
    // Count how many distinct full names share each first name.
    let mut first_name_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for full in authors {
        let first = first_name(full).to_string();
        *first_name_counts.entry(first).or_insert(0) += 1;
    }
    authors
        .iter()
        .map(|full| {
            let first = first_name(full);
            let display = if first_name_counts.get(first).copied().unwrap_or(0) > 1 {
                full.clone()
            } else {
                first.to_string()
            };
            (full.clone(), display)
        })
        .collect()
}

fn first_name(full: &str) -> &str {
    full.split_whitespace().next().unwrap_or(full)
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

    #[test]
    fn abbreviate_uses_first_name_when_unique() {
        let authors = vec!["Alice Smith".into(), "Bob Jones".into()];
        let map = super::abbreviate_names(&authors);
        assert_eq!(map["Alice Smith"], "Alice");
        assert_eq!(map["Bob Jones"], "Bob");
    }

    #[test]
    fn abbreviate_uses_full_name_when_first_names_collide() {
        let authors = vec!["Alice Smith".into(), "Alice Brown".into(), "Bob Jones".into()];
        let map = super::abbreviate_names(&authors);
        assert_eq!(map["Alice Smith"], "Alice Smith");
        assert_eq!(map["Alice Brown"], "Alice Brown");
        assert_eq!(map["Bob Jones"], "Bob");
    }
}
