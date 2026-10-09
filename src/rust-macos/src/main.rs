mod backend;

use backend::{AppManifest, InstallState, InstalledApp, Release};
use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontFamily, FontId, Frame, Id, Key,
    KeyboardShortcut, Label, Layout, Margin, Modifiers, Pos2, Rect, Response, RichText, Sense,
    Shape, Stroke, TextureHandle, Ui, UiBuilder, Vec2, pos2, vec2,
};
use std::cmp::Ordering;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

/// Widest the app list grows before it is centred in the window, like a Settings pane.
const CONTENT_WIDTH: f32 = 760.0;
const ROW_HEIGHT: f32 = 68.0;
const ICON_SIZE: f32 = 44.0;
const ROW_PADDING: f32 = 14.0;
const APPS_FOLDER: &str = "~/Applications/ArtCraft Suite";

// ---------------------------------------------------------------------------------------------
// Background work

enum Job {
    Install { index: usize, release: Release },
    Remove { index: usize },
}

impl Job {
    fn index(&self) -> usize {
        match self {
            Job::Install { index, .. } | Job::Remove { index } => *index,
        }
    }
}

struct Active {
    index: usize,
    removing: bool,
    fraction: f32,
    message: String,
    size: u64,
}

enum Event {
    Release {
        generation: u64,
        index: usize,
        result: Result<Release, String>,
    },
    CheckFinished {
        generation: u64,
    },
    Progress {
        fraction: f32,
        message: String,
    },
    JobFinished {
        index: usize,
        removing: bool,
        outcome: Result<String, String>,
    },
}

/// Everything the user can do. Drawing code only records actions; they are applied after the
/// frame is laid out so that no widget borrows the manager mutably.
enum Action {
    Refresh,
    Channel(bool),
    Install(usize),
    UpdateAll,
    Dequeue(usize),
    Open(usize),
    Reveal(usize),
    AskRemove(usize),
    ConfirmRemove(usize),
    CancelRemove,
    OpenAppsFolder,
}

#[derive(Clone, Copy, PartialEq)]
enum Section {
    Updates,
    Installed,
    NotInstalled,
}

struct Notice {
    text: String,
    error: bool,
}

// ---------------------------------------------------------------------------------------------
// Manager

struct Manager {
    ctx: egui::Context,
    apps: Vec<AppManifest>,
    icons: Vec<Option<TextureHandle>>,
    releases: Vec<Option<Release>>,
    check_errors: Vec<Option<String>>,
    job_errors: Vec<Option<String>>,
    state: InstallState,
    present: Vec<bool>,
    stable: bool,
    checking: bool,
    generation: u64,
    queue: VecDeque<Job>,
    active: Option<Active>,
    notice: Option<Notice>,
    pending_remove: Option<usize>,
    tx: Sender<Event>,
    rx: Receiver<Event>,
}

impl Manager {
    fn new(ctx: egui::Context, apps: Vec<AppManifest>) -> Self {
        let (tx, rx) = mpsc::channel();
        let count = apps.len();
        let icons = apps.iter().map(|app| load_icon(&ctx, &app.id)).collect();
        let mut manager = Self {
            ctx,
            apps,
            icons,
            releases: vec![None; count],
            check_errors: vec![None; count],
            job_errors: vec![None; count],
            state: InstallState::default(),
            present: vec![false; count],
            stable: true,
            checking: false,
            generation: 0,
            queue: VecDeque::new(),
            active: None,
            notice: None,
            pending_remove: None,
            tx,
            rx,
        };
        manager.reload_state();
        manager.refresh(false);
        manager
    }

    fn reload_state(&mut self) {
        match backend::load_state() {
            Ok(state) => self.state = state,
            Err(error) => {
                self.notice = Some(Notice {
                    text: format!("Couldn't read the list of installed apps: {error:#}"),
                    error: true,
                });
            }
        }
        // An app the user deleted in Finder is treated as not installed, so Get works again.
        self.present = self
            .apps
            .iter()
            .map(|app| {
                self.state.apps.contains_key(&app.id)
                    && backend::bundle_path(app).is_ok_and(|path| path.is_dir())
            })
            .collect();
    }

    fn busy_with_jobs(&self) -> bool {
        self.active.is_some() || !self.queue.is_empty()
    }

    fn refresh(&mut self, force: bool) {
        if self.checking || self.busy_with_jobs() {
            return;
        }
        self.generation += 1;
        self.checking = true;
        let generation = self.generation;
        let apps = self.apps.clone();
        let stable = self.stable;
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            // Check every app at once so rows fill in as answers arrive.
            std::thread::scope(|scope| {
                for (index, app) in apps.iter().enumerate() {
                    let tx = tx.clone();
                    let ctx = ctx.clone();
                    scope.spawn(move || {
                        let result = backend::resolve_with_refresh(app, stable, force)
                            .map_err(|error| format!("{error:#}"));
                        let _ = tx.send(Event::Release {
                            generation,
                            index,
                            result,
                        });
                        ctx.request_repaint();
                    });
                }
            });
            let _ = tx.send(Event::CheckFinished { generation });
            ctx.request_repaint();
        });
    }

    fn set_channel(&mut self, stable: bool) {
        if self.stable == stable || self.checking || self.busy_with_jobs() {
            return;
        }
        self.stable = stable;
        self.releases.iter_mut().for_each(|release| *release = None);
        self.check_errors.iter_mut().for_each(|error| *error = None);
        self.refresh(false);
    }

    fn is_queued(&self, index: usize) -> bool {
        self.queue.iter().any(|job| job.index() == index)
    }

    fn is_working_on(&self, index: usize) -> bool {
        self.active
            .as_ref()
            .is_some_and(|active| active.index == index)
            || self.is_queued(index)
    }

    fn enqueue(&mut self, job: Job) {
        let index = job.index();
        if self.checking || self.is_working_on(index) {
            return;
        }
        self.job_errors[index] = None;
        self.queue.push_back(job);
        self.start_next_job();
    }

    fn enqueue_install(&mut self, index: usize) {
        if let Some(release) = self.releases[index].clone() {
            self.enqueue(Job::Install { index, release });
        }
    }

    fn start_next_job(&mut self) {
        if self.active.is_some() || self.checking {
            return;
        }
        let Some(job) = self.queue.pop_front() else {
            return;
        };
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        match job {
            Job::Install { index, release } => {
                let app = self.apps[index].clone();
                self.active = Some(Active {
                    index,
                    removing: false,
                    fraction: 0.0,
                    message: String::new(),
                    size: release.size,
                });
                std::thread::spawn(move || {
                    let result = backend::install(&app, &release, |fraction, message| {
                        let _ = tx.send(Event::Progress {
                            fraction,
                            message: message.to_string(),
                        });
                        ctx.request_repaint();
                    });
                    let _ = tx.send(Event::JobFinished {
                        index,
                        removing: false,
                        outcome: result
                            .map(|()| format!("{} {} is ready to use.", app.name, release.version))
                            .map_err(|error| format!("{error:#}")),
                    });
                    ctx.request_repaint();
                });
            }
            Job::Remove { index } => {
                let app = self.apps[index].clone();
                self.active = Some(Active {
                    index,
                    removing: true,
                    fraction: 1.0,
                    message: "Removing…".into(),
                    size: 0,
                });
                std::thread::spawn(move || {
                    let result = backend::remove(&app);
                    let _ = tx.send(Event::JobFinished {
                        index,
                        removing: true,
                        outcome: result
                            .map(|()| format!("{} was removed.", app.name))
                            .map_err(|error| format!("{error:#}")),
                    });
                    ctx.request_repaint();
                });
            }
        }
    }

    fn collect_events(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                Event::Release {
                    generation,
                    index,
                    result,
                } if generation == self.generation => match result {
                    Ok(release) => {
                        self.releases[index] = Some(release);
                        self.check_errors[index] = None;
                    }
                    Err(error) => {
                        self.releases[index] = None;
                        self.check_errors[index] = Some(error);
                    }
                },
                Event::Release { .. } => {}
                Event::CheckFinished { generation } if generation == self.generation => {
                    self.checking = false;
                    self.start_next_job();
                }
                Event::CheckFinished { .. } => {}
                Event::Progress { fraction, message } => {
                    if let Some(active) = &mut self.active {
                        active.fraction = fraction;
                        active.message = message;
                    }
                }
                Event::JobFinished {
                    index,
                    removing,
                    outcome,
                } => {
                    self.active = None;
                    self.reload_state();
                    match outcome {
                        Ok(text) => {
                            self.job_errors[index] = None;
                            self.notice = Some(Notice { text, error: false });
                        }
                        Err(error) => {
                            let verb = if removing { "remove" } else { "install" };
                            self.notice = Some(Notice {
                                text: format!(
                                    "Couldn't {verb} {}: {}",
                                    self.apps[index].name,
                                    first_line(&error)
                                ),
                                error: true,
                            });
                            self.job_errors[index] = Some(format!("Couldn't {verb}. {error}"));
                        }
                    }
                    self.start_next_job();
                }
            }
        }
    }

    fn apply(&mut self, action: Action) {
        match action {
            Action::Refresh => self.refresh(true),
            Action::Channel(stable) => self.set_channel(stable),
            Action::Install(index) => self.enqueue_install(index),
            Action::UpdateAll => {
                for index in self.indices_in(Section::Updates) {
                    self.enqueue_install(index);
                }
            }
            Action::Dequeue(index) => self.queue.retain(|job| job.index() != index),
            Action::Open(index) => {
                if let Err(error) = backend::launch(&self.apps[index]) {
                    self.notice = Some(Notice {
                        text: format!("Couldn't open {}: {error:#}", self.apps[index].name),
                        error: true,
                    });
                }
            }
            Action::Reveal(index) => {
                if let Err(error) = backend::reveal(&self.apps[index]) {
                    self.notice = Some(Notice {
                        text: format!("Couldn't show {}: {error:#}", self.apps[index].name),
                        error: true,
                    });
                }
            }
            Action::AskRemove(index) => self.pending_remove = Some(index),
            Action::ConfirmRemove(index) => {
                self.pending_remove = None;
                self.enqueue(Job::Remove { index });
            }
            Action::CancelRemove => self.pending_remove = None,
            Action::OpenAppsFolder => {
                if let Err(error) = backend::open_apps_folder() {
                    self.notice = Some(Notice {
                        text: format!("Couldn't open the apps folder: {error:#}"),
                        error: true,
                    });
                }
            }
        }
    }

    // -----------------------------------------------------------------------------------------
    // Row model

    fn installed(&self, index: usize) -> Option<&InstalledApp> {
        if self.present[index] {
            self.state.apps.get(&self.apps[index].id)
        } else {
            None
        }
    }

    /// How the release offered on the current channel compares with the installed version.
    fn offer(&self, index: usize) -> Option<Ordering> {
        let installed = self.installed(index)?;
        let release = self.releases[index].as_ref()?;
        backend::compare_versions(&release.version, &installed.version)
    }

    fn section(&self, index: usize) -> Section {
        match (self.installed(index), self.offer(index)) {
            (None, _) => Section::NotInstalled,
            (Some(_), Some(Ordering::Greater)) => Section::Updates,
            (Some(_), _) => Section::Installed,
        }
    }

    fn indices_in(&self, section: Section) -> Vec<usize> {
        (0..self.apps.len())
            .filter(|index| self.section(*index) == section)
            .collect()
    }

    fn status_line(&self, index: usize) -> (String, Tone, Option<String>) {
        if let Some(active) = self.active.as_ref().filter(|active| active.index == index) {
            let text = if active.removing {
                "Removing…".to_string()
            } else if active.fraction < 1.0 {
                if active.size > 0 {
                    format!(
                        "Downloading… {} of {}",
                        megabytes((active.fraction as f64 * active.size as f64) as u64),
                        megabytes(active.size)
                    )
                } else {
                    format!("Downloading… {:.0}%", active.fraction * 100.0)
                }
            } else {
                active.message.clone()
            };
            return (text, Tone::Secondary, None);
        }
        if let Some(job) = self.queue.iter().find(|job| job.index() == index) {
            let text = match job {
                Job::Install { .. } => "Waiting…",
                Job::Remove { .. } => "Waiting to remove…",
            };
            return (text.into(), Tone::Secondary, None);
        }
        if let Some(error) = &self.job_errors[index] {
            return (first_line(error).into(), Tone::Danger, Some(error.clone()));
        }
        let release = self.releases[index].as_ref();
        if let Some(installed) = self.installed(index) {
            let version = if installed.version == "unknown" {
                "Installed".to_string()
            } else {
                format!("Version {}", installed.version)
            };
            return match (self.offer(index), release) {
                (Some(Ordering::Greater), Some(release)) => (
                    format!(
                        "{} → {}{}",
                        installed.version,
                        release.version,
                        size_suffix(release.size)
                    ),
                    Tone::Tertiary,
                    None,
                ),
                (Some(Ordering::Less), Some(release)) => (
                    format!(
                        "{version}  ·  {} offers {}",
                        if self.stable { "Stable" } else { "Latest" },
                        release.version
                    ),
                    Tone::Tertiary,
                    None,
                ),
                _ => (version, Tone::Tertiary, None),
            };
        }
        if let Some(release) = release {
            return (
                format!("Version {}{}", release.version, size_suffix(release.size)),
                Tone::Tertiary,
                None,
            );
        }
        if let Some(error) = &self.check_errors[index] {
            return (
                friendly_check_error(error),
                Tone::Tertiary,
                Some(error.clone()),
            );
        }
        (
            if self.checking { "Checking…" } else { "" }.into(),
            Tone::Tertiary,
            None,
        )
    }
}

#[derive(Clone, Copy)]
enum Tone {
    Secondary,
    Tertiary,
    Danger,
}

// ---------------------------------------------------------------------------------------------
// Drawing

impl eframe::App for Manager {
    fn ui(&mut self, root: &mut Ui, _frame: &mut eframe::Frame) {
        self.collect_events();
        self.start_next_job();
        let ctx = root.ctx().clone();
        if self.checking || self.active.is_some() {
            // Keeps spinners turning; progress events also wake the UI.
            ctx.request_repaint_after(Duration::from_millis(33));
        }
        let mut actions = Vec::new();
        if ctx.input_mut(|input| {
            input.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::R))
        }) {
            actions.push(Action::Refresh);
        }
        let p = Palette::of(root);

        egui::Panel::top("toolbar")
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::new().fill(p.window).inner_margin(Margin {
                left: 24,
                right: 24,
                top: 18,
                bottom: 6,
            }))
            .show(root, |ui| self.toolbar(ui, &p, &mut actions));

        egui::Panel::bottom("footer")
            .resizable(false)
            .frame(
                Frame::new()
                    .fill(p.window)
                    .inner_margin(Margin::symmetric(24, 9)),
            )
            .show(root, |ui| self.footer(ui, &p, &mut actions));

        egui::CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(p.window)
                    .inner_margin(Margin::symmetric(24, 0)),
            )
            .show(root, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        centered(ui, |ui| self.list(ui, &p, &mut actions));
                    });
            });

        self.remove_dialog(&ctx, &p, &mut actions);

        for action in actions {
            self.apply(action);
        }
    }
}

impl Manager {
    fn toolbar(&self, ui: &mut Ui, p: &Palette, actions: &mut Vec<Action>) {
        centered(ui, |ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let idle = !self.checking && !self.busy_with_jobs();
                if refresh_button(ui, p, self.checking, idle)
                    .on_hover_text(if idle {
                        "Check for updates (⌘R)"
                    } else {
                        "Available when installs finish"
                    })
                    .clicked()
                {
                    actions.push(Action::Refresh);
                }
                ui.add_space(4.0);
                let selected = if self.stable { 0 } else { 1 };
                let response = segmented(ui, p, &["Stable", "Latest"], selected, idle);
                if let Some(choice) = response.0 {
                    actions.push(Action::Channel(choice == 0));
                }
                response.1.on_hover_text(if idle {
                    "Stable shows tested releases. Latest includes pre-releases."
                } else {
                    "Available when installs finish"
                });
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 1.0;
                        ui.add(
                            Label::new(
                                RichText::new("ArtCraft Suite")
                                    .font(display(26.0))
                                    .color(p.text),
                            )
                            .truncate(),
                        );
                        ui.add(
                            Label::new(
                                RichText::new(
                                    "Creative apps for your Mac, verified before they install",
                                )
                                .font(regular(13.0))
                                .color(p.secondary),
                            )
                            .truncate(),
                        );
                    });
                });
            });
        });
    }

    fn footer(&self, ui: &mut Ui, p: &Palette, actions: &mut Vec<Action>) {
        centered(ui, |ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if link(ui, p, "Show Apps Folder")
                    .on_hover_text(APPS_FOLDER)
                    .clicked()
                {
                    actions.push(Action::OpenAppsFolder);
                }
                ui.add_space(12.0);
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    let (text, color) = match &self.notice {
                        Some(notice) => (
                            notice.text.clone(),
                            if notice.error { p.danger } else { p.secondary },
                        ),
                        None => (
                            format!(
                                "Apps install to {APPS_FOLDER}. Removing an app keeps your documents."
                            ),
                            p.tertiary,
                        ),
                    };
                    ui.add(Label::new(RichText::new(text).font(regular(12.0)).color(color)).truncate());
                });
            });
        });
    }

    fn list(&self, ui: &mut Ui, p: &Palette, actions: &mut Vec<Action>) {
        ui.add_space(6.0);
        if let Some(error) = self.global_check_error() {
            Frame::new()
                .fill(p.warning_fill)
                .corner_radius(CornerRadius::same(10))
                .inner_margin(Margin::symmetric(14, 12))
                .show(ui, |ui| {
                    let size = vec2(ui.available_width(), 34.0);
                    ui.allocate_ui_with_layout(size, Layout::right_to_left(Align::Center), |ui| {
                        if pill(ui, p, "Try Again", PillStyle::Plain, true, None).clicked() {
                            actions.push(Action::Refresh);
                        }
                        ui.add_space(8.0);
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                ui.label(
                                    RichText::new("Couldn't check for apps")
                                        .font(semibold(13.0))
                                        .color(p.text),
                                );
                                ui.add(
                                    Label::new(
                                        RichText::new(friendly_check_error(&error))
                                            .font(regular(12.0))
                                            .color(p.secondary),
                                    )
                                    .truncate(),
                                )
                                .on_hover_text(error);
                            });
                        });
                    });
                });
            ui.add_space(4.0);
        }

        for (section, title) in [
            (Section::Updates, "Updates Available"),
            (Section::Installed, "Installed"),
            (Section::NotInstalled, "Not Installed"),
        ] {
            let indices = self.indices_in(section);
            if indices.is_empty() {
                continue;
            }
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                ui.set_min_height(26.0);
                ui.add_space(4.0);
                ui.label(RichText::new(title).font(semibold(15.0)).color(p.text));
                ui.label(
                    RichText::new(indices.len().to_string())
                        .font(regular(15.0))
                        .color(p.tertiary),
                );
                if section == Section::Updates {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let waiting = indices.iter().all(|index| self.is_working_on(*index));
                        if pill(
                            ui,
                            p,
                            "Update All",
                            PillStyle::Filled,
                            !self.checking && !waiting,
                            None,
                        )
                        .clicked()
                        {
                            actions.push(Action::UpdateAll);
                        }
                    });
                }
            });
            ui.add_space(6.0);
            Frame::new()
                .fill(p.group)
                .stroke(Stroke::new(1.0, p.group_stroke))
                .corner_radius(CornerRadius::same(10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for (position, index) in indices.iter().enumerate() {
                        self.row(ui, p, *index, position == 0, actions);
                    }
                });
        }
        ui.add_space(18.0);
    }

    fn global_check_error(&self) -> Option<String> {
        if self.checking || self.releases.iter().any(Option::is_some) {
            return None;
        }
        self.check_errors.iter().flatten().next().cloned()
    }

    fn row(&self, ui: &mut Ui, p: &Palette, index: usize, first: bool, actions: &mut Vec<Action>) {
        let app = &self.apps[index];
        let (rect, _) =
            ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::hover());
        let text_left = rect.left() + ROW_PADDING + ICON_SIZE + 12.0;
        if !first {
            ui.painter().hline(
                text_left..=rect.right(),
                rect.top(),
                Stroke::new(1.0, p.separator),
            );
        }

        let icon_rect = Rect::from_center_size(
            pos2(rect.left() + ROW_PADDING + ICON_SIZE / 2.0, rect.center().y),
            Vec2::splat(ICON_SIZE),
        );
        match &self.icons[index] {
            Some(texture) => egui::Image::new(texture)
                .fit_to_exact_size(icon_rect.size())
                .paint_at(ui, icon_rect),
            None => {
                ui.painter().rect_filled(
                    icon_rect,
                    CornerRadius::same(10),
                    parse_color(&app.accent),
                );
                ui.painter().text(
                    icon_rect.center(),
                    Align2::CENTER_CENTER,
                    initials(&app.name),
                    semibold(16.0),
                    Color32::WHITE,
                );
            }
        }

        // Controls, laid out from the right edge.
        let controls_rect = Rect::from_min_max(
            pos2(text_left, rect.top()),
            pos2(rect.right() - ROW_PADDING, rect.bottom()),
        );
        let mut controls = ui.new_child(
            UiBuilder::new()
                .max_rect(controls_rect)
                .layout(Layout::right_to_left(Align::Center)),
        );
        controls.spacing_mut().item_spacing.x = 8.0;
        self.row_controls(&mut controls, p, index, actions);
        let text_right = controls.min_rect().left() - 12.0;

        // Name, description and status, vertically centred as one block.
        let text_rect = Rect::from_min_max(
            pos2(text_left, rect.center().y - 25.0),
            pos2(text_right.max(text_left + 40.0), rect.center().y + 25.0),
        );
        let mut text = ui.new_child(
            UiBuilder::new()
                .max_rect(text_rect)
                .layout(Layout::top_down(Align::Min)),
        );
        text.spacing_mut().item_spacing.y = 1.0;
        text.add(
            Label::new(RichText::new(&app.name).font(semibold(14.0)).color(p.text)).truncate(),
        );
        text.add(
            Label::new(
                RichText::new(&app.description)
                    .font(regular(12.0))
                    .color(p.secondary),
            )
            .truncate(),
        );
        let (status, tone, detail) = self.status_line(index);
        let color = match tone {
            Tone::Secondary => p.secondary,
            Tone::Tertiary => p.tertiary,
            Tone::Danger => p.danger,
        };
        let response = text.add(
            Label::new(RichText::new(status).font(regular(11.5)).color(color))
                .truncate()
                .show_tooltip_when_elided(detail.is_none()),
        );
        if let Some(detail) = detail {
            response.on_hover_text(detail);
        }
    }

    fn row_controls(&self, ui: &mut Ui, p: &Palette, index: usize, actions: &mut Vec<Action>) {
        let release = self.releases[index].as_ref();
        let shows_more = self.installed(index).is_some() && !self.is_working_on(index);
        if !shows_more {
            // Keep the action buttons in one column whether or not a row has a "More" button.
            ui.add_space(26.0 + ui.spacing().item_spacing.x);
        }
        if let Some(active) = self.active.as_ref().filter(|active| active.index == index) {
            let fraction = (!active.removing && active.fraction < 1.0).then_some(active.fraction);
            progress_ring(ui, p, fraction, false);
            return;
        }
        if self.is_queued(index) {
            if progress_ring(ui, p, Some(0.0), true)
                .on_hover_text("Waiting. Click to cancel.")
                .clicked()
            {
                actions.push(Action::Dequeue(index));
            }
            return;
        }
        if self.installed(index).is_some() {
            let offer = self.offer(index);
            let more = more_button(ui, p, !self.checking);
            egui::Popup::menu(&more).show(|ui| {
                ui.set_min_width(170.0);
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.spacing_mut().button_padding = vec2(8.0, 3.0);
                if ui.button("Open").clicked() {
                    actions.push(Action::Open(index));
                }
                if ui.button("Show in Finder").clicked() {
                    actions.push(Action::Reveal(index));
                }
                if let Some(release) = release.filter(|_| offer != Some(Ordering::Greater)) {
                    ui.add(egui::Separator::default().spacing(9.0));
                    let label = match offer {
                        Some(Ordering::Equal) => format!("Reinstall {}", release.version),
                        Some(Ordering::Less) => format!("Downgrade to {}", release.version),
                        _ => format!("Replace with {}", release.version),
                    };
                    if ui.button(label).clicked() {
                        actions.push(Action::Install(index));
                    }
                }
                ui.add(egui::Separator::default().spacing(9.0));
                if ui.button("Remove…").clicked() {
                    actions.push(Action::AskRemove(index));
                }
            });
            if offer == Some(Ordering::Greater) {
                if pill(ui, p, "Update", PillStyle::Filled, !self.checking, None).clicked() {
                    actions.push(Action::Install(index));
                }
            } else if pill(ui, p, "Open", PillStyle::Plain, true, None).clicked() {
                actions.push(Action::Open(index));
            }
            return;
        }
        if release.is_some() {
            if pill(ui, p, "Get", PillStyle::Plain, !self.checking, None).clicked() {
                actions.push(Action::Install(index));
            }
        } else if self.checking && self.check_errors[index].is_none() {
            progress_ring(ui, p, None, false);
        } else {
            let response = pill(ui, p, "Get", PillStyle::Plain, false, None);
            if let Some(error) = &self.check_errors[index] {
                response.on_hover_text(friendly_check_error(error));
            }
        }
    }

    fn remove_dialog(&self, ctx: &egui::Context, p: &Palette, actions: &mut Vec<Action>) {
        let Some(index) = self.pending_remove else {
            return;
        };
        let app = &self.apps[index];
        let modal = egui::Modal::new(Id::new("remove-app"))
            .backdrop_color(Color32::from_black_alpha(if p.dark { 110 } else { 60 }))
            .frame(
                Frame::new()
                    .fill(p.group)
                    .stroke(Stroke::new(1.0, p.group_stroke))
                    .corner_radius(CornerRadius::same(14))
                    .inner_margin(Margin::same(20))
                    .shadow(egui::Shadow {
                        offset: [0, 10],
                        blur: 30,
                        spread: 0,
                        color: Color32::from_black_alpha(if p.dark { 120 } else { 45 }),
                    }),
            )
            .show(ctx, |ui| {
                ui.set_width(268.0);
                ui.vertical_centered(|ui| {
                    if let Some(texture) = &self.icons[index] {
                        ui.add(egui::Image::new(texture).fit_to_exact_size(Vec2::splat(64.0)));
                        ui.add_space(8.0);
                    }
                    ui.label(
                        RichText::new(format!("Remove “{}”?", app.name))
                            .font(semibold(14.0))
                            .color(p.text),
                    );
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(format!(
                            "It will be deleted from {APPS_FOLDER}. Your documents and settings are not affected."
                        ))
                        .font(regular(12.0))
                        .color(p.secondary),
                    );
                    ui.add_space(16.0);
                    let width = (ui.available_width() - 8.0) / 2.0;
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        if pill(ui, p, "Cancel", PillStyle::Neutral, true, Some(width)).clicked() {
                            actions.push(Action::CancelRemove);
                        }
                        if pill(ui, p, "Remove", PillStyle::Danger, true, Some(width)).clicked() {
                            actions.push(Action::ConfirmRemove(index));
                        }
                    });
                });
            });
        if modal.should_close() {
            actions.push(Action::CancelRemove);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Controls

#[derive(Clone, Copy, PartialEq)]
enum PillStyle {
    /// Grey capsule with blue text, like App Store "Get" and "Open".
    Plain,
    /// Grey capsule with regular text, for "Cancel".
    Neutral,
    /// Blue capsule with white text.
    Filled,
    /// Red capsule with white text, for destructive confirmations.
    Danger,
}

fn pill(
    ui: &mut Ui,
    p: &Palette,
    label: &str,
    style: PillStyle,
    enabled: bool,
    width: Option<f32>,
) -> Response {
    let font = semibold(12.5);
    let text_width = ui
        .painter()
        .layout_no_wrap(label.to_string(), font.clone(), p.text)
        .size()
        .x;
    let size = vec2(width.unwrap_or((text_width + 30.0).max(70.0)), 26.0);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let pressed = response.is_pointer_button_down_on();
    let hovered = response.hovered() && enabled;
    let (fill, text) = match style {
        PillStyle::Plain | PillStyle::Neutral => (
            if pressed {
                p.pill_pressed
            } else if hovered {
                p.pill_hover
            } else {
                p.pill
            },
            match (enabled, style) {
                (false, _) => p.tertiary,
                (true, PillStyle::Plain) => p.accent,
                _ => p.text,
            },
        ),
        PillStyle::Filled | PillStyle::Danger => {
            let base = if style == PillStyle::Danger {
                p.danger
            } else {
                p.accent
            };
            if !enabled {
                (p.pill, p.tertiary)
            } else if pressed {
                (darken(base, 0.82), Color32::WHITE)
            } else if hovered {
                (darken(base, 0.9), Color32::WHITE)
            } else {
                (base, Color32::WHITE)
            }
        }
    };
    if ui.is_rect_visible(rect) {
        ui.painter().rect_filled(rect, CornerRadius::same(13), fill);
        ui.painter()
            .text(rect.center(), Align2::CENTER_CENTER, label, font, text);
    }
    response
}

/// A two-or-more segment control in the style of NSSegmentedControl.
fn segmented(
    ui: &mut Ui,
    p: &Palette,
    labels: &[&str],
    selected: usize,
    enabled: bool,
) -> (Option<usize>, Response) {
    let segment = 66.0;
    let size = vec2(segment * labels.len() as f32 + 4.0, 26.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter().clone();
    painter.rect_filled(rect, CornerRadius::same(8), p.segment);
    let mut clicked = None;
    for (position, label) in labels.iter().enumerate() {
        let inner = Rect::from_min_size(
            rect.min + vec2(2.0 + segment * position as f32, 2.0),
            vec2(segment, size.y - 4.0),
        );
        let sense = if enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        let item = ui.interact(inner, response.id.with(position), sense);
        if position == selected {
            painter.rect_filled(
                inner.translate(vec2(0.0, 0.5)),
                CornerRadius::same(6),
                p.segment_shadow,
            );
            painter.rect_filled(inner, CornerRadius::same(6), p.segment_selected);
        } else if item.hovered() && enabled {
            painter.rect_filled(inner, CornerRadius::same(6), p.segment_hover);
        }
        let font = if position == selected {
            semibold(12.0)
        } else {
            regular(12.0)
        };
        let color = if enabled { p.text } else { p.tertiary };
        painter.text(inner.center(), Align2::CENTER_CENTER, *label, font, color);
        if item.clicked() && position != selected {
            clicked = Some(position);
        }
    }
    (clicked, response)
}

fn refresh_button(ui: &mut Ui, p: &Palette, spinning: bool, enabled: bool) -> Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(28.0), sense);
    let painter = ui.painter();
    if response.hovered() && enabled {
        painter.circle_filled(rect.center(), 14.0, p.pill_hover);
    }
    let color = if enabled { p.secondary } else { p.tertiary };
    if spinning {
        let start = ui.input(|input| input.time) as f32 * 5.0;
        painter.add(arc(rect.center(), 7.0, start, 4.2, Stroke::new(1.8, color)));
    } else {
        let start = -0.9_f32;
        let sweep = 5.0_f32;
        painter.add(arc(
            rect.center(),
            7.0,
            start,
            sweep,
            Stroke::new(1.8, color),
        ));
        // Arrow head at the end of the arc, pointing along the direction of travel.
        let end = start + sweep;
        let tip = rect.center() + 7.0 * vec2(end.cos(), end.sin());
        let along = vec2(-end.sin(), end.cos());
        let out = vec2(end.cos(), end.sin());
        painter.add(Shape::convex_polygon(
            vec![
                tip + along * 3.2,
                tip - along * 1.8 + out * 3.2,
                tip - along * 1.8 - out * 3.2,
            ],
            color,
            Stroke::NONE,
        ));
    }
    response
}

fn more_button(ui: &mut Ui, p: &Palette, enabled: bool) -> Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(26.0), sense);
    let fill = if response.is_pointer_button_down_on() {
        p.pill_pressed
    } else if response.hovered() && enabled {
        p.pill_hover
    } else {
        p.pill
    };
    let color = if enabled { p.accent } else { p.tertiary };
    ui.painter().circle_filled(rect.center(), 13.0, fill);
    for offset in [-5.0, 0.0, 5.0] {
        ui.painter()
            .circle_filled(rect.center() + vec2(offset, 0.0), 1.6, color);
    }
    response.on_hover_text("More")
}

/// App Store–style progress ring: determinate while downloading, spinning while verifying.
fn progress_ring(ui: &mut Ui, p: &Palette, fraction: Option<f32>, clickable: bool) -> Response {
    let sense = if clickable {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(vec2(70.0, 26.0), sense);
    let center = pos2(rect.center().x, rect.center().y);
    let radius = 11.0;
    let painter = ui.painter();
    painter.circle_stroke(center, radius, Stroke::new(2.5, p.ring_track));
    match fraction {
        Some(value) if value > 0.0 => {
            painter.add(arc(
                center,
                radius,
                -std::f32::consts::FRAC_PI_2,
                value.clamp(0.0, 1.0) * std::f32::consts::TAU,
                Stroke::new(2.5, p.accent),
            ));
        }
        Some(_) => {}
        None => {
            let start = ui.input(|input| input.time) as f32 * 5.0;
            painter.add(arc(center, radius, start, 1.7, Stroke::new(2.5, p.accent)));
        }
    }
    if clickable && response.hovered() {
        painter.rect_filled(
            Rect::from_center_size(center, Vec2::splat(7.0)),
            CornerRadius::same(1),
            p.secondary,
        );
    }
    response
}

fn link(ui: &mut Ui, p: &Palette, text: &str) -> Response {
    let response = ui.add(
        Label::new(RichText::new(text).font(regular(12.0)).color(p.accent)).sense(Sense::click()),
    );
    if response.hovered() {
        let rect = response.rect;
        ui.painter().hline(
            rect.left()..=rect.right(),
            rect.bottom() - 1.0,
            Stroke::new(1.0, p.accent),
        );
    }
    response
}

fn arc(center: Pos2, radius: f32, start: f32, sweep: f32, stroke: Stroke) -> Shape {
    let steps = ((sweep.abs() / std::f32::consts::TAU) * 48.0)
        .ceil()
        .max(2.0) as usize;
    let points = (0..=steps)
        .map(|step| {
            let angle = start + sweep * step as f32 / steps as f32;
            center + radius * vec2(angle.cos(), angle.sin())
        })
        .collect();
    Shape::line(points, stroke)
}

/// Lays out content in a column no wider than [`CONTENT_WIDTH`], centred in the space given.
fn centered<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let available = ui.available_rect_before_wrap();
    let width = available.width().min(CONTENT_WIDTH);
    let rect = Rect::from_min_size(
        pos2(available.center().x - width / 2.0, available.top()),
        vec2(width, available.height()),
    );
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::top_down(Align::Min)),
        add,
    )
    .inner
}

// ---------------------------------------------------------------------------------------------
// Look and feel

#[derive(Clone, Copy)]
struct Palette {
    dark: bool,
    window: Color32,
    group: Color32,
    group_stroke: Color32,
    separator: Color32,
    text: Color32,
    secondary: Color32,
    tertiary: Color32,
    accent: Color32,
    danger: Color32,
    warning_fill: Color32,
    pill: Color32,
    pill_hover: Color32,
    pill_pressed: Color32,
    ring_track: Color32,
    segment: Color32,
    segment_hover: Color32,
    segment_selected: Color32,
    segment_shadow: Color32,
}

impl Palette {
    /// Colours follow Apple's system palette for macOS light and dark appearances.
    fn light() -> Self {
        Self {
            dark: false,
            window: rgb(0xF5F5F7),
            group: Color32::WHITE,
            group_stroke: rgb(0xE3E3E8),
            separator: rgb(0xE8E8ED),
            text: rgb(0x1D1D1F),
            secondary: rgb(0x6E6E73),
            tertiary: rgb(0x8E8E93),
            accent: rgb(0x007AFF),
            danger: rgb(0xE5372C),
            warning_fill: rgb(0xFFF5D9),
            pill: rgb(0xEEEEF2),
            pill_hover: rgb(0xE4E4EA),
            pill_pressed: rgb(0xD8D8DF),
            ring_track: rgb(0xE3E3E8),
            segment: rgb(0xE6E6EB),
            segment_hover: rgb(0xDCDCE2),
            segment_selected: Color32::WHITE,
            segment_shadow: Color32::from_black_alpha(28),
        }
    }

    fn dark() -> Self {
        Self {
            dark: true,
            window: rgb(0x1C1C1E),
            group: rgb(0x2A2A2C),
            group_stroke: rgb(0x343437),
            separator: rgb(0x3A3A3D),
            text: rgb(0xF5F5F7),
            secondary: rgb(0xA1A1A6),
            tertiary: rgb(0x75757A),
            accent: rgb(0x0A84FF),
            danger: rgb(0xFF453A),
            warning_fill: rgb(0x3A3220),
            pill: rgb(0x3A3A3C),
            pill_hover: rgb(0x444447),
            pill_pressed: rgb(0x505053),
            ring_track: rgb(0x3E3E41),
            segment: rgb(0x2E2E31),
            segment_hover: rgb(0x38383B),
            segment_selected: rgb(0x5C5C60),
            segment_shadow: Color32::from_black_alpha(60),
        }
    }

    fn of(ui: &Ui) -> Self {
        if ui.visuals().dark_mode {
            Self::dark()
        } else {
            Self::light()
        }
    }
}

fn visuals(dark: bool) -> egui::Visuals {
    let p = if dark {
        Palette::dark()
    } else {
        Palette::light()
    };
    let mut v = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    v.panel_fill = p.window;
    v.window_fill = p.group;
    v.extreme_bg_color = p.group;
    v.faint_bg_color = p.pill;
    v.hyperlink_color = p.accent;
    v.window_stroke = Stroke::new(1.0, p.group_stroke);
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = CornerRadius::same(8);
    v.popup_shadow = egui::Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: Color32::from_black_alpha(if dark { 110 } else { 40 }),
    };
    v.selection.bg_fill = p.accent;
    v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.separator);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    for (widget, fill) in [
        (&mut v.widgets.inactive, p.pill),
        (&mut v.widgets.hovered, p.pill_hover),
        (&mut v.widgets.active, p.pill_pressed),
        (&mut v.widgets.open, p.pill_hover),
    ] {
        widget.corner_radius = CornerRadius::same(5);
        widget.bg_fill = fill;
        widget.weak_bg_fill = fill;
        widget.bg_stroke = Stroke::NONE;
        widget.fg_stroke = Stroke::new(1.0, p.text);
        widget.expansion = 0.0;
    }
    // Menu items highlight in the accent colour with white text, as in macOS menus.
    v.widgets.hovered.weak_bg_fill = p.accent;
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    v.widgets.active.weak_bg_fill = darken(p.accent, 0.88);
    v.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    v
}

fn regular(size: f32) -> FontId {
    FontId::proportional(size)
}

fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

fn display(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("display".into()))
}

/// Uses the macOS system font (San Francisco) when it can be read, at the weights and optical
/// sizes AppKit uses; falls back to egui's bundled fonts otherwise.
fn install_fonts(ctx: &egui::Context) {
    use egui::epaint::text::{FontData, VariationCoords};

    let mut fonts = egui::FontDefinitions::default();
    let fallback = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let system = std::fs::read("/System/Library/Fonts/SFNS.ttf")
        .ok()
        .map(|bytes| &*Box::leak(bytes.into_boxed_slice()));
    for (key, family, weight, optical_size) in [
        ("system-regular", FontFamily::Proportional, 400.0, 13.0),
        (
            "system-semibold",
            FontFamily::Name("semibold".into()),
            590.0,
            13.0,
        ),
        (
            "system-display",
            FontFamily::Name("display".into()),
            700.0,
            28.0,
        ),
    ] {
        let mut names = fallback.clone();
        if let Some(bytes) = system {
            let mut data = FontData::from_static(bytes);
            data.tweak.coords = VariationCoords::new([(b"wght", weight), (b"opsz", optical_size)]);
            fonts.font_data.insert(key.to_string(), Arc::new(data));
            names.insert(0, key.to_string());
        }
        fonts.families.insert(family, names);
    }
    ctx.set_fonts(fonts);
}

fn apply_style(ctx: &egui::Context) {
    install_fonts(ctx);
    ctx.set_visuals_of(egui::Theme::Light, visuals(false));
    ctx.set_visuals_of(egui::Theme::Dark, visuals(true));
    ctx.set_theme(egui::ThemePreference::System);
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = vec2(8.0, 6.0);
        style.spacing.button_padding = vec2(10.0, 4.0);
        style.spacing.menu_margin = Margin::same(5);
        style.spacing.interact_size.y = 22.0;
        style.interaction.selectable_labels = false;
        style
            .text_styles
            .insert(egui::TextStyle::Body, regular(13.0));
        style
            .text_styles
            .insert(egui::TextStyle::Button, regular(13.0));
        style
            .text_styles
            .insert(egui::TextStyle::Small, regular(11.0));
    });
}

fn load_icon(ctx: &egui::Context, id: &str) -> Option<TextureHandle> {
    let bytes = icon_bytes(id)?;
    let mut image = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .ok()?
        .to_rgba8();
    round_corners(&mut image);
    let size = [image.width() as usize, image.height() as usize];
    let color = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
    let options = egui::TextureOptions {
        mipmap_mode: Some(egui::TextureFilter::Linear),
        ..egui::TextureOptions::LINEAR
    };
    Some(ctx.load_texture(format!("icon-{id}"), color, options))
}

/// Clips a square icon to the rounded-square outline macOS uses, with anti-aliased corners.
/// Icons that are already rounded are unchanged, because pixels are only ever made more
/// transparent.
fn round_corners(image: &mut image::RgbaImage) {
    let size = image.width().min(image.height()) as f32;
    let radius = size * 0.225;
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let px = x as f32 + 0.5;
        let py = y as f32 + 0.5;
        let cx = px.clamp(radius, size - radius);
        let cy = py.clamp(radius, size - radius);
        let distance = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
        let coverage = (radius - distance + 0.5).clamp(0.0, 1.0);
        if coverage < 1.0 {
            pixel[3] = (pixel[3] as f32 * coverage).round() as u8;
        }
    }
}

/// Official upstream icons; see assets/icons/README.md for their sources and licences.
fn icon_bytes(id: &str) -> Option<&'static [u8]> {
    Some(match id {
        "photocraft" => include_bytes!("../../../assets/icons/photocraft.png"),
        "vectorcraft" => include_bytes!("../../../assets/icons/vectorcraft.png"),
        "designcraft" => include_bytes!("../../../assets/icons/designcraft.png"),
        "filmcraft" => include_bytes!("../../../assets/icons/filmcraft.png"),
        "effectcraft" => include_bytes!("../../../assets/icons/effectcraft.png"),
        "lightcraft" => include_bytes!("../../../assets/icons/lightcraft.png"),
        "printcraft" => include_bytes!("../../../assets/icons/printcraft.png"),
        "wordcraft" => include_bytes!("../../../assets/icons/wordcraft.png"),
        "gridcraft" => include_bytes!("../../../assets/icons/gridcraft.png"),
        "deckcraft" => include_bytes!("../../../assets/icons/deckcraft.png"),
        "soundcraft" => include_bytes!("../../../assets/icons/soundcraft.png"),
        "cadcraft" => include_bytes!("../../../assets/icons/cadcraft.png"),
        _ => return None,
    })
}

// ---------------------------------------------------------------------------------------------
// Helpers

fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

fn darken(color: Color32, factor: f32) -> Color32 {
    let scale = |channel: u8| (channel as f32 * factor).round() as u8;
    Color32::from_rgb(scale(color.r()), scale(color.g()), scale(color.b()))
}

fn parse_color(hex: &str) -> Color32 {
    let value = hex.trim_start_matches('#');
    match u32::from_str_radix(value, 16) {
        Ok(number) if value.len() == 6 => rgb(number),
        _ => rgb(0x6D5DFB),
    }
}

fn initials(name: &str) -> String {
    let letters: String = name.chars().filter(|c| c.is_uppercase()).take(2).collect();
    if letters.is_empty() {
        name.chars().take(2).collect()
    } else {
        letters
    }
}

/// Sizes in decimal megabytes, as Finder and the App Store show them.
fn megabytes(bytes: u64) -> String {
    let value = bytes as f64 / 1_000_000.0;
    if value >= 100.0 {
        format!("{value:.0} MB")
    } else {
        format!("{value:.1} MB")
    }
}

fn size_suffix(bytes: u64) -> String {
    if bytes == 0 {
        String::new()
    } else {
        format!("  ·  {}", megabytes(bytes))
    }
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or(text).trim()
}

fn friendly_check_error(error: &str) -> String {
    if error.contains("No matching macOS DMG") {
        "Not available for Mac on this channel yet".into()
    } else if error.contains("hourly release-check limit") {
        "GitHub's hourly check limit was reached. Try again in a few minutes.".into()
    } else if error.contains("error sending request") || error.contains("dns error") {
        "Couldn't reach GitHub. Check your internet connection.".into()
    } else {
        format!("Couldn't check for updates. {}", first_line(error))
    }
}

fn main() -> eframe::Result {
    let apps = match backend::manifest() {
        Ok(manifest) => manifest.apps,
        Err(error) => {
            eprintln!("Invalid application manifest: {error:#}");
            std::process::exit(1);
        }
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("ArtCraft Suite")
            .with_inner_size([820.0, 780.0])
            .with_min_inner_size([600.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native(
        "ArtCraft Suite",
        options,
        Box::new(move |cc| {
            apply_style(&cc.egui_ctx);
            Ok(Box::new(Manager::new(cc.egui_ctx.clone(), apps)))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_manifest_app_has_an_official_icon() {
        for app in backend::manifest().unwrap().apps {
            let bytes =
                icon_bytes(&app.id).unwrap_or_else(|| panic!("missing icon for {}", app.id));
            let image = image::load_from_memory(bytes).unwrap();
            assert_eq!((image.width(), image.height()), (256, 256), "{}", app.id);
        }
    }

    #[test]
    fn sizes_use_decimal_megabytes() {
        assert_eq!(megabytes(182_400_000), "182 MB");
        assert_eq!(megabytes(42_150_000), "42.1 MB");
        assert_eq!(size_suffix(0), "");
    }

    #[test]
    fn check_errors_are_explained_in_plain_language() {
        assert_eq!(
            friendly_check_error("No matching macOS DMG in the selected release channel"),
            "Not available for Mac on this channel yet"
        );
        assert!(
            friendly_check_error("GitHub's hourly release-check limit was reached.")
                .starts_with("GitHub's hourly")
        );
    }
}
