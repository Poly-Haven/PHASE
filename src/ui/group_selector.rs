use super::colors;

pub struct OptionItem<T> {
    pub value: T,
    pub label: &'static str,
    pub selected_bg: Option<egui::Color32>,
}

#[derive(Default)]
pub struct Response<T> {
    pub clicked: Option<T>,
    pub additive: bool,
}

pub fn draw<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash,
    options: &[OptionItem<T>],
    selected: &[T],
) -> Response<T> {
    let text_style = egui::TextStyle::Button;
    let font_id = text_style.resolve(ui.style());
    let text_color = ui.visuals().text_color();
    let padding = egui::vec2(8.4, 3.5);
    let height = ui.spacing().interact_size.y.max(28.0);
    let separator_width = 1.0;
    let rounding = height / 2.0;

    let text_widths: Vec<f32> = options
        .iter()
        .map(|option| {
            ui.fonts(|fonts| {
                fonts
                    .layout_no_wrap(option.label.to_owned(), font_id.clone(), text_color)
                    .rect
                    .width()
            })
        })
        .collect();
    let width: f32 = text_widths.iter().map(|w| w + padding.x * 2.0).sum::<f32>()
        + separator_width * options.len().saturating_sub(1) as f32;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());

    let visuals = ui.visuals();
    let border = egui::Stroke::new(1.0, visuals.widgets.inactive.bg_stroke.color);
    ui.painter().rect_stroke(rect, rounding, border);

    let mut clicked = None;
    let mut x = rect.left();
    for (index, option) in options.iter().enumerate() {
        let option_width = text_widths[index] + padding.x * 2.0;
        let option_rect =
            egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(option_width, height));
        let response = ui.interact(
            option_rect,
            ui.id().with((&id, index)),
            egui::Sense::click(),
        );
        let is_selected = selected.contains(&option.value);
        let selected_color = option.selected_bg.unwrap_or(colors::ACCENT);
        let fill = if is_selected {
            colors::colored_background(selected_color)
        } else if response.hovered() {
            colors::PILL_OPTION_BG_HOVER
        } else {
            colors::PILL_OPTION_BG
        };
        let option_rounding = if options.len() == 1 {
            egui::Rounding::same(rounding)
        } else if index == 0 {
            egui::Rounding {
                nw: rounding,
                sw: rounding,
                ..Default::default()
            }
        } else if index == options.len() - 1 {
            egui::Rounding {
                ne: rounding,
                se: rounding,
                ..Default::default()
            }
        } else {
            egui::Rounding::ZERO
        };
        ui.painter().rect_filled(
            option_rect.shrink2(egui::vec2(0.0, 1.0)),
            option_rounding,
            fill,
        );
        if is_selected {
            ui.painter().rect_stroke(
                option_rect.shrink2(egui::vec2(0.5, 1.5)),
                option_rounding,
                egui::Stroke::new(1.0, selected_color),
            );
        }
        let label_color = if response.hovered() {
            egui::Color32::WHITE
        } else if is_selected {
            colors::TEXT_PRIMARY
        } else {
            text_color
        };
        ui.painter().text(
            option_rect.center(),
            egui::Align2::CENTER_CENTER,
            option.label,
            font_id.clone(),
            label_color,
        );
        if is_selected {
            ui.painter().text(
                option_rect.center() + egui::vec2(0.45, 0.0),
                egui::Align2::CENTER_CENTER,
                option.label,
                font_id.clone(),
                label_color,
            );
        }
        if response.clicked() {
            clicked = Some(option.value);
        }
        response.on_hover_cursor(egui::CursorIcon::PointingHand);

        x += option_width;
        if index + 1 < options.len() {
            let separator_rect = egui::Rect::from_min_size(
                egui::pos2(x, rect.top() + 1.0),
                egui::vec2(separator_width, height - 2.0),
            );
            ui.painter().rect_filled(
                separator_rect,
                0.0,
                visuals.widgets.inactive.bg_stroke.color,
            );
            x += separator_width;
        }
    }

    Response {
        clicked,
        additive: ui.input(|i| i.modifiers.shift),
    }
}
