use super::AssetType;

pub fn labels(types: &[AssetType]) -> Vec<String> {
    types.iter().map(|t| t.label().to_string()).collect()
}

pub fn from_labels(labels: &[String]) -> Vec<AssetType> {
    let selected: Vec<_> = labels
        .iter()
        .filter_map(|label| AssetType::from_label(label))
        .collect();

    if selected.is_empty() {
        vec![AssetType::Hdris]
    } else {
        selected
    }
}

pub fn select(mut selected: Vec<AssetType>, clicked: AssetType, additive: bool) -> Vec<AssetType> {
    if !additive {
        return vec![clicked];
    }

    if selected.contains(&clicked) {
        selected.retain(|t| *t != clicked);
        if selected.is_empty() {
            selected.push(clicked);
        }
    } else {
        selected.push(clicked);
        selected.sort_by_key(|t| t.order());
    }
    selected
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_click_selects_only_clicked_type() {
        let selected = select(
            vec![AssetType::Hdris, AssetType::Textures],
            AssetType::Textures,
            false,
        );

        assert_eq!(selected, vec![AssetType::Textures]);
    }

    #[test]
    fn shift_click_adds_or_removes_without_emptying_selection() {
        let selected = select(vec![AssetType::Hdris], AssetType::Textures, true);
        assert_eq!(selected, vec![AssetType::Hdris, AssetType::Textures]);

        let selected = select(selected, AssetType::Textures, true);
        assert_eq!(selected, vec![AssetType::Hdris]);

        let selected = select(vec![AssetType::Hdris], AssetType::Hdris, true);
        assert_eq!(selected, vec![AssetType::Hdris]);
    }

}
