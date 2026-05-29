#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlugProblem {
    Uppercase(char),
    InvalidChar(char),
}

pub fn validate(slug: &str) -> Vec<SlugProblem> {
    slug.chars()
        .filter_map(|ch| {
            if ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' {
                None
            } else if ch.is_ascii_uppercase() {
                Some(SlugProblem::Uppercase(ch))
            } else {
                Some(SlugProblem::InvalidChar(ch))
            }
        })
        .collect()
}

pub fn is_valid(slug: &str) -> bool {
    !slug.is_empty() && validate(slug).is_empty()
}

pub fn message(slug: &str) -> Option<String> {
    let problems = validate(slug);
    if problems.is_empty() {
        return None;
    }
    let details = problems
        .iter()
        .map(|problem| match problem {
            SlugProblem::Uppercase(ch) | SlugProblem::InvalidChar(ch) => ch.to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ");
    Some(format!("Invalid slug: {details}"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn slug_validation_reports_specific_invalid_characters() {
        assert!(super::is_valid("valid_slug_123"));
        assert_eq!(super::message("Bad-Slug").unwrap(), "Invalid slug: B - S");
    }
}
