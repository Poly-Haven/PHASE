pub const ROTATION_PERIOD_SECONDS: f64 = 3.0;

pub fn rotation_angle(time_seconds: f64) -> f32 {
    let progress = (time_seconds / ROTATION_PERIOD_SECONDS).rem_euclid(1.0);
    (progress * std::f64::consts::TAU) as f32
}

pub fn draw_button(ui: &mut egui::Ui) {
    let response = ui.add_enabled(false, egui::Button::new("      Loading"));
    let texture = super::loading_texture(ui.ctx());
    let angle = ui.input(|i| rotation_angle(i.time));
    let size = egui::vec2(14.0, 14.0);
    let icon_rect = egui::Rect::from_center_size(
        egui::pos2(response.rect.left() + 16.0, response.rect.center().y),
        size,
    );
    egui::Image::new(egui::load::SizedTexture::new(texture.id(), size))
        .rotate(angle, egui::Vec2::splat(0.5))
        .paint_at(ui, icon_rect);
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(16));
}

#[cfg(test)]
mod tests {
    #[test]
    fn rotation_angle_completes_one_turn_every_three_seconds() {
        let epsilon = 0.0001;

        assert!((super::rotation_angle(0.0) - 0.0).abs() < epsilon);
        assert!((super::rotation_angle(1.5) - std::f32::consts::PI).abs() < epsilon);
        assert!((super::rotation_angle(3.0) - 0.0).abs() < epsilon);
    }
}
