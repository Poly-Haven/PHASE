use std::collections::VecDeque;
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
    Render,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ScriptKey {
    pub row: RowKey,
    pub kind: ScriptKind,
}

pub struct ScriptJob {
    pub key: ScriptKey,
    pub rx: Receiver<ScriptEvent>,
    pub started_at: Instant,
    pub output: String,
}

pub struct QueuedScript {
    pub key: ScriptKey,
    spec: ScriptSpec,
    depends_on: Vec<ScriptKey>,
}

#[derive(Clone)]
pub struct ScriptRun {
    pub kind: ScriptKind,
    pub status: ScriptRunStatus,
    pub output: String,
    pub started_at: Instant,
    pub finished_at: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptRunStatus {
    Succeeded,
    Failed,
    Blocked,
}

impl ScriptRun {
    fn succeeded(
        kind: ScriptKind,
        output: String,
        started_at: Instant,
        finished_at: Instant,
    ) -> Self {
        Self {
            kind,
            status: ScriptRunStatus::Succeeded,
            output,
            started_at,
            finished_at,
        }
    }

    fn failed(kind: ScriptKind, output: String, started_at: Instant, finished_at: Instant) -> Self {
        Self {
            kind,
            status: ScriptRunStatus::Failed,
            output,
            started_at,
            finished_at,
        }
    }

    fn blocked(kind: ScriptKind, output: String) -> Self {
        let now = Instant::now();
        Self {
            kind,
            status: ScriptRunStatus::Blocked,
            output,
            started_at: now,
            finished_at: now,
        }
    }

    pub fn succeeded_flag(&self) -> bool {
        matches!(self.status, ScriptRunStatus::Succeeded)
    }
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
    if state.is_admin() && matches!(key.asset_type, AssetType::Hdris) {
        draw_script_button(
            ui,
            state,
            key,
            "Normalize & Render",
            ScriptAction::NormalizeAndRender,
        );
        draw_script_button(ui, state, key, "Normalize", ScriptAction::Normalize);
        draw_script_button(ui, state, key, "Render", ScriptAction::Render);
    }

    if ui.button("Open on Notion").clicked() {
        open_notion_link(notion_url, open_notion_in_app);
        ui.close_menu();
    }
}

fn draw_script_button(
    ui: &mut egui::Ui,
    state: &mut AppState,
    key: &RowKey,
    label: &str,
    action: ScriptAction,
) {
    let scheduled = action
        .kinds()
        .iter()
        .all(|kind| script_is_scheduled(state, key, *kind));
    if ui
        .add_enabled(!scheduled, egui::Button::new(label))
        .clicked()
    {
        enqueue_action(state, key, action);
        ui.close_menu();
    }
}

pub fn draw_row_status(state: &mut AppState, ui: &mut egui::Ui, key: &RowKey) {
    if let Some(active_kind) = active_script_kind_for_row(state, key) {
        ui.add_space(layout::ROW_INTRA_ICON_GAP);
        ui.horizontal(|ui| {
            let (rect, response) = ui.allocate_exact_size(
                egui::vec2(layout::INLINE_ICON_SIZE, layout::INLINE_ICON_SIZE),
                egui::Sense::click(),
            );
            super::loading_indicator::draw_image_at(ui, rect, egui::Color32::WHITE);
            if response
                .on_hover_text("View script output")
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                if let Some(key) = active_script_key(state, key) {
                    state.script_output_dialog = Some(key);
                }
            }
            ui.add_space(layout::ROW_INTRA_ICON_GAP);
            ui.label(egui::RichText::new(active_kind.label()).color(colors::TEXT_PRIMARY));
        });
        return;
    }

    if script_is_queued_for_row(state, key) {
        ui.add_space(layout::ROW_INTRA_ICON_GAP);
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(layout::INLINE_ICON_SIZE, layout::INLINE_ICON_SIZE),
                egui::Sense::hover(),
            );
            super::loading_indicator::draw_image_at(ui, rect, egui::Color32::WHITE);
            ui.add_space(layout::ROW_INTRA_ICON_GAP);
            ui.label(egui::RichText::new("Queued").color(colors::TEXT_DISABLED));
        });
        return;
    }

    if let Some(run) = latest_alert_run_for_row(state, key) {
        let failed_kind = run.kind;
        let failed_status = run.status;
        ui.add_space(layout::ROW_INTRA_ICON_GAP);
        ui.horizontal(|ui| {
            let tex = match failed_kind {
                ScriptKind::Normalize => super::warn_icon_texture(ui.ctx()),
                ScriptKind::Render => super::warn_icon_texture(ui.ctx()),
            };
            ui.add(
                egui::Image::new(egui::load::SizedTexture::new(
                    tex.id(),
                    egui::vec2(layout::INLINE_ICON_SIZE, layout::INLINE_ICON_SIZE),
                ))
                .tint(colors::MSG_WARNING),
            );
            ui.add_space(layout::ROW_INTRA_ICON_GAP);
            let resp = ui
                .add(
                    egui::Label::new(
                        egui::RichText::new(match failed_status {
                            ScriptRunStatus::Blocked => format!("{} blocked", failed_kind.label()),
                            _ => failed_kind.failed_label().to_string(),
                        })
                        .color(colors::MSG_WARNING),
                    )
                    .sense(egui::Sense::click()),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if resp.clicked() {
                state.script_output_dialog = Some(ScriptKey {
                    row: key.clone(),
                    kind: failed_kind,
                });
            }
        });
    }
}

pub fn draw_output_dialog(state: &mut AppState, ctx: &egui::Context) {
    let Some(script_key) = state.script_output_dialog.clone() else {
        return;
    };

    if let Some(job) = state.script_jobs.get(&script_key) {
        draw_dialog_contents(
            ctx,
            state,
            &script_key,
            DialogState::Running(job.key.kind),
            job.output.clone(),
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
        match run.status {
            ScriptRunStatus::Succeeded => DialogState::Succeeded(
                run.kind,
                run.finished_at.saturating_duration_since(run.started_at),
            ),
            ScriptRunStatus::Failed => DialogState::Failed(run.kind),
            ScriptRunStatus::Blocked => DialogState::Blocked(run.kind),
        },
        run.output,
    );
}

pub fn pump(state: &mut AppState) {
    if let Some(active_key) = state.script_jobs.keys().next().cloned() {
        let mut finished: Option<ScriptRun> = None;
        if let Some(job) = state.script_jobs.get_mut(&active_key) {
            while let Ok(event) = job.rx.try_recv() {
                match event {
                    ScriptEvent::Output(text) => job.output.push_str(&text),
                    ScriptEvent::FailedToStart(message) => {
                        finished = Some(ScriptRun::failed(
                            job.key.kind,
                            if job.output.is_empty() {
                                message
                            } else {
                                format!("{}\n{}", message, job.output)
                            },
                            job.started_at,
                            Instant::now(),
                        ));
                    }
                    ScriptEvent::Finished { exit_status } => {
                        let finished_at = Instant::now();
                        finished = Some(if exit_status.success() {
                            ScriptRun::succeeded(
                                job.key.kind,
                                job.output.clone(),
                                job.started_at,
                                finished_at,
                            )
                        } else {
                            ScriptRun::failed(
                                job.key.kind,
                                job.output.clone(),
                                job.started_at,
                                finished_at,
                            )
                        });
                    }
                }
            }
        }

        if let Some(run) = finished {
            let key = active_key.clone();
            let success = run.succeeded_flag();
            state.script_results.insert(key.clone(), run);
            state.script_jobs.remove(&key);
            if success {
                state.row_toasts.insert(
                    key.row.clone(),
                    super::RowToast {
                        text: format!(
                            "{} in {}",
                            key.kind.success_label(),
                            format_duration(
                                state
                                    .script_results
                                    .get(&key)
                                    .map(|run| run
                                        .finished_at
                                        .saturating_duration_since(run.started_at))
                                    .unwrap_or_else(|| Duration::from_secs(1))
                            )
                        ),
                        created_at: Instant::now(),
                    },
                );
            } else {
                block_dependents(state, &key);
            }
        }
    }

    resolve_blocked_tasks(state);
    start_next_ready_task(state);
}

pub fn enqueue_action(state: &mut AppState, key: &RowKey, action: ScriptAction) {
    let kinds = action.kinds();
    let mut depends_on = Vec::new();

    for &kind in kinds {
        let script_key = ScriptKey {
            row: key.clone(),
            kind,
        };

        if script_is_scheduled(state, key, kind) {
            depends_on = vec![script_key.clone()];
            continue;
        }

        let Some(spec) = script_spec(state, key, kind) else {
            state.error_banner = Some(format!(
                "Could not resolve script path for {}",
                kind.label()
            ));
            return;
        };
        state.script_results.remove(&script_key);
        state.script_queue.push_back(QueuedScript {
            key: script_key.clone(),
            spec,
            depends_on: depends_on.clone(),
        });
        depends_on = vec![script_key];
    }

    start_next_ready_task(state);
}

fn start_next_ready_task(state: &mut AppState) {
    if !state.script_jobs.is_empty() {
        return;
    }
    let Some(index) = state.script_queue.iter().position(|entry| {
        entry
            .depends_on
            .iter()
            .all(|dep| dependency_succeeded(state, dep))
    }) else {
        return;
    };
    let entry = state
        .script_queue
        .remove(index)
        .expect("queue entry exists");
    let (tx, rx) = channel();
    let started_at = Instant::now();
    thread::spawn({
        let spec = entry.spec;
        move || run_script(spec, tx)
    });
    state.script_jobs.insert(
        entry.key.clone(),
        ScriptJob {
            key: entry.key,
            rx,
            started_at,
            output: String::new(),
        },
    );
}

fn resolve_blocked_tasks(state: &mut AppState) {
    loop {
        let mut changed = false;
        let mut next_queue = VecDeque::new();
        while let Some(entry) = state.script_queue.pop_front() {
            if let Some(failed_dep) = entry
                .depends_on
                .iter()
                .find(|dep| matches!(state.script_results.get(dep), Some(run) if !run.succeeded_flag()))
                .cloned()
            {
                state.script_results.insert(
                    entry.key.clone(),
                    ScriptRun::blocked(
                        entry.key.kind,
                        format!(
                            "{} blocked because {} did not succeed",
                            entry.key.kind.label(),
                            failed_dep.kind.label()
                        ),
                    ),
                );
                changed = true;
            } else {
                next_queue.push_back(entry);
            }
        }
        state.script_queue = next_queue;
        if !changed {
            break;
        }
    }
}

fn block_dependents(state: &mut AppState, failed_key: &ScriptKey) {
    let mut changed = false;
    let mut next_queue = VecDeque::new();
    while let Some(entry) = state.script_queue.pop_front() {
        if entry.depends_on.iter().any(|dep| dep == failed_key) {
            state.script_results.insert(
                entry.key.clone(),
                ScriptRun::blocked(
                    entry.key.kind,
                    format!(
                        "{} blocked because {} failed",
                        entry.key.kind.label(),
                        failed_key.kind.label()
                    ),
                ),
            );
            changed = true;
        } else {
            next_queue.push_back(entry);
        }
    }
    state.script_queue = next_queue;
    if changed {
        resolve_blocked_tasks(state);
    }
}

fn dependency_succeeded(state: &AppState, dep: &ScriptKey) -> bool {
    state
        .script_results
        .get(dep)
        .map(|run| run.succeeded_flag())
        .unwrap_or(false)
}

fn script_is_scheduled(state: &AppState, key: &RowKey, kind: ScriptKind) -> bool {
    let script_key = ScriptKey {
        row: key.clone(),
        kind,
    };
    state.script_jobs.contains_key(&script_key)
        || state
            .script_queue
            .iter()
            .any(|entry| entry.key == script_key)
}

fn active_script_kind_for_row(state: &AppState, key: &RowKey) -> Option<ScriptKind> {
    state
        .script_jobs
        .keys()
        .find(|script_key| script_key.row == *key)
        .map(|script_key| script_key.kind)
}

fn active_script_key(state: &AppState, key: &RowKey) -> Option<ScriptKey> {
    state
        .script_jobs
        .keys()
        .find(|script_key| script_key.row == *key)
        .cloned()
}

fn script_is_queued_for_row(state: &AppState, key: &RowKey) -> bool {
    state.script_queue.iter().any(|entry| entry.key.row == *key)
        && active_script_kind_for_row(state, key).is_none()
}

fn latest_alert_run_for_row<'a>(state: &'a AppState, key: &RowKey) -> Option<&'a ScriptRun> {
    state
        .script_results
        .iter()
        .filter(|(script_key, run)| {
            script_key.row == *key && !matches!(run.status, ScriptRunStatus::Succeeded)
        })
        .max_by_key(|(_, run)| run.finished_at)
        .map(|(_, run)| run)
}

fn draw_dialog_contents(
    ctx: &egui::Context,
    state: &mut AppState,
    script_key: &ScriptKey,
    dialog_state: DialogState,
    output: String,
) {
    let mut close = false;
    let mut text = output;
    egui::Window::new(format!(
        "{} output — {}",
        dialog_state.kind().label(),
        script_key.row.slug
    ))
    .collapsible(false)
    .resizable(true)
    .default_size(egui::vec2(
        layout::UPDATE_DIALOG_WIDTH,
        layout::UPDATE_DIALOG_SCROLL_HEIGHT,
    ))
    .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
    .show(ctx, |ui| {
        ui.horizontal(|ui| {
            let (icon, label) = dialog_state.banner(ui.ctx());
            ui.add(icon);
            ui.label(label);
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

enum DialogState {
    Running(ScriptKind),
    Succeeded(ScriptKind, Duration),
    Failed(ScriptKind),
    Blocked(ScriptKind),
}

impl DialogState {
    fn kind(&self) -> ScriptKind {
        match *self {
            DialogState::Running(kind)
            | DialogState::Succeeded(kind, _)
            | DialogState::Failed(kind)
            | DialogState::Blocked(kind) => kind,
        }
    }

    fn banner(&self, ctx: &egui::Context) -> (egui::Image<'_>, egui::WidgetText) {
        match *self {
            DialogState::Running(kind) => {
                let tex = super::loading_texture(ctx);
                (
                    egui::Image::new(egui::load::SizedTexture::new(
                        tex.id(),
                        egui::vec2(layout::INLINE_ICON_SIZE, layout::INLINE_ICON_SIZE),
                    )),
                    egui::WidgetText::from(
                        egui::RichText::new(kind.label()).color(colors::TEXT_PRIMARY),
                    ),
                )
            }
            DialogState::Succeeded(kind, duration) => {
                let tex = super::check_texture(ctx);
                (
                    egui::Image::new(egui::load::SizedTexture::new(
                        tex.id(),
                        egui::vec2(layout::INLINE_ICON_SIZE, layout::INLINE_ICON_SIZE),
                    ))
                    .tint(colors::MSG_INFO),
                    egui::WidgetText::from(
                        egui::RichText::new(format!(
                            "{} in {}",
                            kind.success_label(),
                            format_duration(duration)
                        ))
                        .color(colors::MSG_INFO),
                    ),
                )
            }
            DialogState::Failed(kind) => {
                let tex = super::warn_icon_texture(ctx);
                (
                    egui::Image::new(egui::load::SizedTexture::new(
                        tex.id(),
                        egui::vec2(layout::INLINE_ICON_SIZE, layout::INLINE_ICON_SIZE),
                    ))
                    .tint(colors::MSG_WARNING),
                    egui::WidgetText::from(
                        egui::RichText::new(kind.failed_label()).color(colors::MSG_WARNING),
                    ),
                )
            }
            DialogState::Blocked(kind) => {
                let tex = super::warn_icon_texture(ctx);
                (
                    egui::Image::new(egui::load::SizedTexture::new(
                        tex.id(),
                        egui::vec2(layout::INLINE_ICON_SIZE, layout::INLINE_ICON_SIZE),
                    ))
                    .tint(colors::MSG_WARNING),
                    egui::WidgetText::from(
                        egui::RichText::new(format!("{} blocked", kind.label()))
                            .color(colors::MSG_WARNING),
                    ),
                )
            }
        }
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
    let _ = tx.send(ScriptEvent::Finished {
        exit_status: status,
    });
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

fn script_spec(state: &AppState, key: &RowKey, kind: ScriptKind) -> Option<ScriptSpec> {
    match kind {
        ScriptKind::Normalize => normalize_spec(state, key),
        ScriptKind::Render => render_spec(state, key),
    }
}

fn normalize_spec(state: &AppState, key: &RowKey) -> Option<ScriptSpec> {
    if !state.is_admin() || !matches!(key.asset_type, AssetType::Hdris) {
        return None;
    }
    let script = script_root()?.join("normalize.py");
    Some(ScriptSpec {
        program: OsString::from("python"),
        args: vec![
            script.into_os_string(),
            normalize_target_path(state, key).into_os_string(),
        ],
    })
}

fn render_spec(state: &AppState, key: &RowKey) -> Option<ScriptSpec> {
    if !state.is_admin() || !matches!(key.asset_type, AssetType::Hdris) {
        return None;
    }
    let script = script_root()?.join("make_previews.py");
    Some(ScriptSpec {
        program: OsString::from("python"),
        args: vec![
            script.into_os_string(),
            normalize_target_path(state, key).into_os_string(),
            OsString::from("ground"),
            OsString::from("tonemap"),
            OsString::from("nonstudio"),
        ],
    })
}

fn script_root() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|home| {
        home.join("Poly Haven Dropbox")
            .join("Assets")
            .join("PH Utils")
            .join("Scripts")
            .join("HDRIs")
    })
}

fn normalize_target_path(state: &AppState, key: &RowKey) -> std::path::PathBuf {
    state
        .prod_root_for(key.asset_type)
        .join(&key.slug)
        .join("staging")
        .join(format!("{}.exr", key.slug))
}

#[derive(Clone, Copy)]
pub enum ScriptAction {
    Normalize,
    Render,
    NormalizeAndRender,
}

impl ScriptAction {
    fn kinds(self) -> &'static [ScriptKind] {
        match self {
            ScriptAction::Normalize => &[ScriptKind::Normalize],
            ScriptAction::Render => &[ScriptKind::Render],
            ScriptAction::NormalizeAndRender => &[ScriptKind::Normalize, ScriptKind::Render],
        }
    }
}

impl ScriptKind {
    fn label(self) -> &'static str {
        match self {
            ScriptKind::Normalize => "Normalize",
            ScriptKind::Render => "Render",
        }
    }

    fn success_label(self) -> &'static str {
        match self {
            ScriptKind::Normalize => "Normalized",
            ScriptKind::Render => "Rendered",
        }
    }

    fn failed_label(self) -> &'static str {
        match self {
            ScriptKind::Normalize => "Normalize failed",
            ScriptKind::Render => "Render failed",
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
