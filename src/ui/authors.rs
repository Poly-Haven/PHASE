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

pub fn contains(author: &str, filter: &str) -> bool {
    filter.is_empty() || names(author).any(|name| name == filter)
}
