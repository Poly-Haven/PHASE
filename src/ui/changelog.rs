//! The "What's new" dialog shown the first time a newly installed PHASE runs.
//!
//! Release notes are written by hand as small Markdown fragments, so rather
//! than pulling in a Markdown crate for headings, bullets and the odd bold or
//! `code` run, they are parsed here into the handful of blocks we render.

use super::{colors, layout, AppState};

/// A run of text with the emphasis it was written with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub strong: bool,
    pub code: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Heading {
        level: u8,
        spans: Vec<Span>,
    },
    Bullet(Vec<Span>),
    Paragraph(Vec<Span>),
    /// A gap between paragraphs. Runs of empty lines collapse into one.
    Blank,
}

/// One release's notes, parsed and ready to draw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub version: String,
    pub blocks: Vec<Block>,
}

/// Every version the user skipped past, newest first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Changelog {
    pub entries: Vec<Entry>,
}

impl Changelog {
    pub fn from_releases(releases: Vec<crate::updater::ReleaseNotes>) -> Self {
        Self {
            entries: releases
                .into_iter()
                .map(|release| Entry {
                    version: release.version,
                    blocks: parse(&release.notes),
                })
                .collect(),
        }
    }
}

pub fn parse(markdown: &str) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    for line in markdown.lines() {
        let line = line.trim();
        if line.is_empty() {
            if !matches!(blocks.last(), None | Some(Block::Blank)) {
                blocks.push(Block::Blank);
            }
        } else if let Some((level, text)) = heading(line) {
            blocks.push(Block::Heading {
                level,
                spans: spans(text),
            });
        } else if let Some(text) = bullet(line) {
            blocks.push(Block::Bullet(spans(text)));
        } else {
            blocks.push(Block::Paragraph(spans(line)));
        }
    }
    while matches!(blocks.last(), Some(Block::Blank)) {
        blocks.pop();
    }
    blocks
}

/// `## Heading` — the space is required, so a `#tag` stays plain text.
fn heading(line: &str) -> Option<(u8, &str)> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let text = line[hashes..].strip_prefix(' ')?.trim();
    Some((hashes as u8, text))
}

fn bullet(line: &str) -> Option<&str> {
    line.strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .map(str::trim)
}

/// Split a line on `**bold**` and `` `code` ``. Anything else, including an
/// unclosed marker, stays literal.
fn spans(text: &str) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut plain = String::new();
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("**") {
            if let Some(end) = after.find("**") {
                push_plain(&mut spans, &mut plain);
                spans.push(emphasised(&after[..end], true, false));
                rest = &after[end + 2..];
                continue;
            }
        }
        if let Some(after) = rest.strip_prefix('`') {
            if let Some(end) = after.find('`') {
                push_plain(&mut spans, &mut plain);
                spans.push(emphasised(&after[..end], false, true));
                rest = &after[end + 1..];
                continue;
            }
        }
        let next = rest.chars().next().expect("rest is not empty");
        plain.push(next);
        rest = &rest[next.len_utf8()..];
    }
    push_plain(&mut spans, &mut plain);
    spans
}

fn emphasised(text: &str, strong: bool, code: bool) -> Span {
    Span {
        text: text.to_string(),
        strong,
        code,
    }
}

fn push_plain(spans: &mut Vec<Span>, plain: &mut String) {
    if !plain.is_empty() {
        spans.push(emphasised(plain, false, false));
        plain.clear();
    }
}

pub fn draw(state: &mut AppState, ctx: &egui::Context) {
    let Some(changelog) = state.changelog.clone() else {
        return;
    };

    let mut close = false;
    egui::Window::new(format!(
        "What's new in PHASE v{}",
        crate::updater::current_version()
    ))
    .collapsible(false)
    .resizable(true)
    .default_width(layout::CHANGELOG_DIALOG_WIDTH)
    .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
    .show(ctx, |ui| {
        egui::ScrollArea::vertical()
            .max_height(layout::CHANGELOG_DIALOG_SCROLL_HEIGHT)
            .show(ui, |ui| {
                for (index, entry) in changelog.entries.iter().enumerate() {
                    if index > 0 {
                        ui.add_space(layout::DIALOG_SECTION_SPACING_MEDIUM);
                        ui.separator();
                        ui.add_space(layout::DIALOG_SECTION_SPACING_SMALL);
                    }
                    // Only worth labelling once more than one version is covered.
                    if changelog.entries.len() > 1 {
                        ui.label(
                            egui::RichText::new(format!("v{}", entry.version))
                                .size(layout::CHANGELOG_VERSION_SIZE)
                                .strong()
                                .color(colors::TEXT_PRIMARY),
                        );
                        ui.add_space(layout::DIALOG_SECTION_SPACING_SMALL);
                    }
                    draw_blocks(ui, &entry.blocks);
                }
            });
        ui.add_space(layout::DIALOG_SECTION_SPACING_MEDIUM);
        if ui.button("Close").clicked() {
            close = true;
        }
    });

    if close {
        state.changelog = None;
    }
}

fn draw_blocks(ui: &mut egui::Ui, blocks: &[Block]) {
    for block in blocks {
        match block {
            Block::Blank => ui.add_space(layout::CHANGELOG_BLANK_LINE_HEIGHT),
            Block::Heading { level, spans } => {
                let size = if *level <= 2 {
                    layout::CHANGELOG_HEADING_SIZE
                } else {
                    layout::CHANGELOG_SUBHEADING_SIZE
                };
                ui.add_space(layout::CHANGELOG_BLANK_LINE_HEIGHT);
                draw_line(ui, spans, 0.0, |text| {
                    text.size(size).strong().color(colors::TEXT_PRIMARY)
                });
            }
            Block::Bullet(spans) => {
                draw_line(ui, spans, layout::CHANGELOG_BULLET_INDENT, |text| text);
            }
            Block::Paragraph(spans) => draw_line(ui, spans, 0.0, |text| text),
        }
    }
}

fn draw_line(
    ui: &mut egui::Ui,
    spans: &[Span],
    indent: f32,
    style: impl Fn(egui::RichText) -> egui::RichText,
) {
    ui.horizontal_wrapped(|ui| {
        // The spans already carry their own spacing, so egui must not add more
        // between them.
        ui.spacing_mut().item_spacing.x = 0.0;
        if indent > 0.0 {
            ui.add_space(indent);
            ui.label(style(
                egui::RichText::new("• ").color(colors::TEXT_DISABLED),
            ));
        }
        for span in spans {
            let mut text = egui::RichText::new(&span.text);
            if span.strong {
                text = text.strong();
            }
            if span.code {
                text = text.monospace();
            }
            ui.label(style(text));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{parse, Block, Span};

    fn plain(text: &str) -> Span {
        Span {
            text: text.into(),
            strong: false,
            code: false,
        }
    }

    #[test]
    fn headings_bullets_and_paragraphs_are_told_apart() {
        let blocks = parse("### Fixes\n- Thumbnails refresh again\nPlain line");

        assert_eq!(
            blocks,
            vec![
                Block::Heading {
                    level: 3,
                    spans: vec![plain("Fixes")],
                },
                Block::Bullet(vec![plain("Thumbnails refresh again")]),
                Block::Paragraph(vec![plain("Plain line")]),
            ]
        );
    }

    #[test]
    fn star_bullets_are_bullets_too() {
        assert_eq!(
            parse("* Starred"),
            vec![Block::Bullet(vec![plain("Starred")])]
        );
    }

    #[test]
    fn a_hash_without_a_space_stays_plain_text() {
        assert_eq!(
            parse("#1234 is not a heading"),
            vec![Block::Paragraph(vec![plain("#1234 is not a heading")])]
        );
    }

    #[test]
    fn runs_of_blank_lines_collapse_and_the_edges_are_trimmed() {
        let blocks = parse("\n\nOne\n\n\nTwo\n\n");

        assert_eq!(
            blocks,
            vec![
                Block::Paragraph(vec![plain("One")]),
                Block::Blank,
                Block::Paragraph(vec![plain("Two")]),
            ]
        );
    }

    #[test]
    fn bold_and_code_runs_become_their_own_spans() {
        let blocks = parse("- **Fast** pulls from `staging`.");

        assert_eq!(
            blocks,
            vec![Block::Bullet(vec![
                Span {
                    text: "Fast".into(),
                    strong: true,
                    code: false,
                },
                plain(" pulls from "),
                Span {
                    text: "staging".into(),
                    strong: false,
                    code: true,
                },
                plain("."),
            ])]
        );
    }

    #[test]
    fn an_unclosed_marker_is_left_alone() {
        assert_eq!(
            parse("2 ** 8 = 256"),
            vec![Block::Paragraph(vec![plain("2 ** 8 = 256")])]
        );
    }

    #[test]
    fn notes_that_are_only_whitespace_produce_nothing() {
        assert!(parse("   \n\n  ").is_empty());
    }
}
