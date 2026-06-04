use std::ffi::OsString;
use std::io::{BufRead, BufReader};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use super::{colors, layout, AppState, AssetType, RowKey};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScriptKind {
    Normalize,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ScriptKey {
    pub row: RowKey,
    pub kind: ScriptKind,
}

pub struct ScriptJob {
    pub kind: ScriptKind,
    pub rx: Receiver<ScriptEvent>,
    pub started_at: Instant,
    pub output: String,
}

#[derive(Clone)]
pub struct ScriptRun {
    pub kind: ScriptKind,
    pub output: String,
    pub started_at: Instant,
    pub finished_at: Instant,
    pub exit_code: Option<i32>,
    pub succeeded: bool,
}

pub enum ScriptEvent {
    Output(String),
    Finished { exit_status: ExitStatus },
    FailedToStart(String),
}

struct ScriptSpec {
    program: OsString,
    args: Vec<OsString>,
}

pub fn draw_context_menu(
    ui: &mut egui::Ui,
    state: &mut AppState,
    key: &RowKey,
    notion_url: &str,
    open_notion_in_app: bool,
) {
    if ui.button("Open on Notion").clicked() {
        open_notion_link(notion_url, open_notion_in_app);
        ui.close_menu();
    }

    if state.is_admin() && matches!(key.asset_type, AssetType::Hdris) {
        let running = state.script_jobs.contains_key(&ScriptKey {
            row: key.clone(),
            kind: ScriptKind::Normalize,
        });
        if ui
            .add_enabled(!running, egui::Button::new("Normalize"))
            .clicked()
        {
            start(state, key, ScriptKind::Normalize);
            ui.close_menu();
        }
    }
}

pub fn draw_row_status(state: &mut AppState, ui: &mut egui::Ui, key: &RowKey) {
    let script_key = ScriptKey {
        row: key.clone(),
        kind: ScriptKind::Normalize,
    };

    if state.script_jobs.contains_key(&script_key) {
        ui.add_space(layout::ROW_INTRA_ICON_GAP);
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(layout::INLINE_ICON_SIZE, layout::INLINE_ICON_SIZE),
            egui::Sense::click(),
        );
        super::loading_indicator::draw_image_at(ui, rect, egui::Color32::WHITE);
        if response
            .on_hover_text("View Normalize output")
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            state.script_output_dialog = Some(script_key);
        }
        return;
    }

    let failed = state
        .script_results
        .get(&script_key)
        .map(|run| !run.succeeded)
        .unwrap_or(false);
    if failed {
            ui.add_space(layout::ROW_INTRA_ICON_GAP);
            ui.horizontal(|ui| {
                let tex = super::warn_icon_texture(ui.ctx());
                ui.add(
                    egui::Image::new(egui::load::SizedTexture::new(
                        tex.id(),
                        egui::vec2(layout::INLINE_ICON_SIZE, layout::INLINE_ICON_SIZE),
                    ))
                    .tint(colors::MSG_WARNING),
                );
                let resp = ui
                    .add(
                        egui::Label::new(
                            egui::RichText::new(ScriptKind::Normalize.failed_label())
                                .color(colors::MSG_WARNING),
                        )
                        .sense(egui::Sense::click()),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                if resp.clicked() {
                    state.script_output_dialog = Some(script_key);
                }
            });
    }
}

pub fn draw_output_dialog(state: &mut AppState, ctx: &egui::Context) {
    let Some(script_key) = state.script_output_dialog.clone() else {
        return;
    };

    if let Some(job) = state.script_jobs.get(&script_key) {
        let kind = job.kind;
        let output = job.output.clone();
        let duration = job.started_at.elapsed();
        draw_dialog_contents(
            ctx,
            state,
            &script_key,
            kind,
            output,
            duration,
        );
        return;
    }

    let Some(run) = state.script_results.get(&script_key).cloned() else {
        state.script_output_dialog = None;
        return;
    };
    draw_dialog_contents(
        ctx,
        state,
        &script_key,
        run.kind,
        run.output,
        run.finished_at.saturating_duration_since(run.started_at),
    );
}

pub fn pump(state: &mut AppState) {
    let keys: Vec<_> = state.script_jobs.keys().cloned().collect();
    for key in keys {
        let mut finished: Option<ScriptRun> = None;
        if let Some(job) = state.script_jobs.get_mut(&key) {
            while let Ok(event) = job.rx.try_recv() {
                match event {
                    ScriptEvent::Output(text) => job.output.push_str(&text),
                    ScriptEvent::FailedToStart(message) => {
                        let output = if job.output.is_empty() {
                            message
                        } else {
                            format!("{}\n{}", message, job.output)
                        };
                        finished = Some(ScriptRun {
                            kind: job.kind,
                            output,
                            started_at: job.started_at,
                            finished_at: Instant::now(),
                            exit_code: None,
                            succeeded: false,
                        });
                    }
                    ScriptEvent::Finished { exit_status } => {
                        finished = Some(ScriptRun {
                            kind: job.kind,
                            output: job.output.clone(),
                            started_at: job.started_at,
                            finished_at: Instant::now(),
                            exit_code: exit_status.code(),
                            succeeded: exit_status.success(),
                        });
                    }
                }
            }
        }

        if let Some(run) = finished {
            let succeeded = run.succeeded;
            let duration = format_duration(run.finished_at.saturating_duration_since(run.started_at));
            state.script_results.insert(key.clone(), run);
            state.script_jobs.remove(&key);
            if succeeded {
                state.row_toasts.insert(
                    key.row.clone(),
                    super::RowToast {
                        text: format!("{} in {duration}", ScriptKind::Normalize.success_label()),
                        created_at: Instant::now(),
                    },
                );
            }
        }
    }
}

pub fn start(state: &mut AppState, key: &RowKey, kind: ScriptKind) {
    let script_key = ScriptKey {
        row: key.clone(),
        kind,
    };
    if state.script_jobs.contains_key(&script_key) {
        return;
    }
    let Some(spec) = script_spec(state, key, kind) else {
        return;
    };
    state.script_results.remove(&script_key);

    let (tx, rx) = channel();
    let started_at = Instant::now();
    thread::spawn(move || run_script(spec, tx));
    state.script_jobs.insert(
        script_key,
        ScriptJob {
            kind,
            rx,
            started_at,
            output: String::new(),
        },
    );
}

fn script_spec(state: &AppState, key: &RowKey, kind: ScriptKind) -> Option<ScriptSpec> {
    match kind {
        ScriptKind::Normalize => normalize_spec(state, key),
    }
}

fn normalize_spec(state: &AppState, key: &RowKey) -> Option<ScriptSpec> {
    if !state.is_admin() || !matches!(key.asset_type, AssetType::Hdris) {
        return None;
    }

    let script = r"C:\Users\gregz\Poly Haven Dropbox\Assets\PH Utils\Scripts\HDRIs\normalize.py";
    let prod_folder = state.prod_root_for(key.asset_type).join(&key.slug);
    let exr = prod_folder.join("staging").join(format!("{}.exr", key.slug));

    Some(ScriptSpec {
        program: OsString::from("python"),
        args: vec![OsString::from(script), exr.into_os_string()],
    })
}

fn run_script(spec: ScriptSpec, tx: Sender<ScriptEvent>) {
    let mut command = Command::new(&spec.program);
    command
        .args(&spec.args)
        .env("PYTHONUNBUFFERED", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            let _ = tx.send(ScriptEvent::FailedToStart(err.to_string()));
            return;
        }
    };

    let mut readers = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        readers.push(spawn_reader(stdout, tx.clone()));
    }
    if let Some(stderr) = child.stderr.take() {
        readers.push(spawn_reader(stderr, tx.clone()));
    }

    let status = match child.wait() {
        Ok(status) => status,
        Err(err) => {
            let _ = tx.send(ScriptEvent::FailedToStart(err.to_string()));
            return;
        }
    };
    drop(child);
    for reader in readers {
        let _ = reader.join();
    }
    let _ = tx.send(ScriptEvent::Finished { exit_status: status });
}

fn spawn_reader<T>(stream: T, tx: Sender<ScriptEvent>) -> thread::JoinHandle<()>
where
    T: std::io::Read + Send + 'static,
{
    thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        let mut buffer = String::new();
        loop {
            buffer.clear();
            match reader.read_line(&mut buffer) {
                Ok(0) => break,
                Ok(_) => {
                    let _ = tx.send(ScriptEvent::Output(buffer.clone()));
                }
                Err(err) => {
                    let _ = tx.send(ScriptEvent::Output(format!("[stream error] {err}\n")));
                    break;
                }
            }
        }
    })
}

fn draw_dialog_contents(
    ctx: &egui::Context,
    state: &mut AppState,
    script_key: &ScriptKey,
    kind: ScriptKind,
    output: String,
    duration: Duration,
) {
    let mut close = false;
    let mut text = output;
    egui::Window::new(format!("{} output — {}", kind.label(), script_key.row.slug))
        .collapsible(false)
        .resizable(true)
        .default_size(egui::vec2(
            layout::UPDATE_DIALOG_WIDTH,
            layout::UPDATE_DIALOG_SCROLL_HEIGHT,
        ))
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.strong(kind.label());
                ui.label(format!("elapsed {}", format_duration(duration)));
            });
            ui.add_space(layout::DIALOG_SECTION_SPACING_MEDIUM);
            egui::ScrollArea::vertical()
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut text)
                            .font(egui::TextStyle::Monospace)
                            .desired_width(f32::INFINITY)
                            .interactive(false),
                    );
                });
            ui.add_space(layout::DIALOG_SECTION_SPACING_MEDIUM);
            if ui.button("Close").clicked() {
                close = true;
            }
        });
    if close {
        state.script_output_dialog = None;
    }
}

fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs().max(1);
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m {}s", secs / 60, secs % 60)
    }
}

impl ScriptKind {
    fn label(self) -> &'static str {
        match self {
            ScriptKind::Normalize => "Normalize",
        }
    }

    fn success_label(self) -> &'static str {
        match self {
            ScriptKind::Normalize => "Normalized",
        }
    }

    fn failed_label(self) -> &'static str {
        match self {
            ScriptKind::Normalize => "Normalize failed",
        }
    }
}

fn open_notion_link(url: &str, open_in_app: bool) {
    if url.is_empty() {
        return;
    }
    let target = if open_in_app {
        notion_app_url(url)
    } else {
        url.to_string()
    };
    let _ = open::that(target);
}

fn notion_app_url(url: &str) -> String {
    if url.starts_with("notion://") {
        return url.to_string();
    }
    if let Some(rest) = url.strip_prefix("https://") {
        return format!("notion://{rest}");
    }
    if let Some(rest) = url.strip_prefix("http://") {
        return format!("notion://{rest}");
    }
    format!("notion://{url}")
}
