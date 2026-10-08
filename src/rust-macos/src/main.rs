mod backend;

use backend::{AppManifest, InstallState, Release};
use eframe::egui::{self, Color32, RichText};
use std::sync::mpsc::{self, Receiver, Sender};

enum Event {
    Releases(Vec<Result<Release, String>>),
    Progress(f32, String),
    Done(String),
    BatchDone(Vec<usize>, String),
    Failed(String),
}

struct Manager {
    apps: Vec<AppManifest>,
    releases: Vec<Option<Release>>,
    errors: Vec<String>,
    state: InstallState,
    state_error: Option<String>,
    selected: Vec<bool>,
    stable: bool,
    busy: bool,
    status: String,
    progress: f32,
    pending_remove: Option<usize>,
    tx: Sender<Event>,
    rx: Receiver<Event>,
}

impl Manager {
    fn new(apps: Vec<AppManifest>) -> Self {
        let (tx, rx) = mpsc::channel();
        let count = apps.len();
        let (state, state_error) = match backend::load_state() {
            Ok(state) => (state, None),
            Err(error) => (
                InstallState::default(),
                Some(format!(
                    "Could not recover installation settings: {error:#}"
                )),
            ),
        };
        let mut manager = Self {
            apps,
            releases: vec![None; count],
            errors: vec![String::new(); count],
            state,
            state_error,
            selected: vec![false; count],
            stable: true,
            busy: false,
            status: "Ready to check releases.".into(),
            progress: 0.0,
            pending_remove: None,
            tx,
            rx,
        };
        manager.refresh(false);
        manager
    }

    fn refresh(&mut self, force_refresh: bool) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.status = "Checking upstream releases…".into();
        let apps = self.apps.clone();
        let stable = self.stable;
        let force_refresh = force_refresh;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let results = apps
                .iter()
                .map(|app| {
                    backend::resolve_with_refresh(app, stable, force_refresh)
                        .map_err(|e| e.to_string())
                })
                .collect();
            let _ = tx.send(Event::Releases(results));
        });
    }

    fn install(&mut self, index: usize) {
        if self.busy {
            return;
        }
        let Some(release) = self.releases[index].clone() else {
            return;
        };
        let app = self.apps[index].clone();
        self.busy = true;
        self.progress = 0.0;
        self.status = format!("Installing {} {}…", app.name, release.version);
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = backend::install(&app, &release, |fraction, message| {
                let _ = tx.send(Event::Progress(
                    fraction,
                    format!("{}: {message}", app.name),
                ));
            });
            let _ = tx.send(match result {
                Ok(()) => Event::Done(format!("{} {} is installed.", app.name, release.version)),
                Err(error) => Event::Failed(format!("Could not install {}: {error:#}", app.name)),
            });
        });
    }

    fn install_selected(&mut self) {
        if self.busy {
            return;
        }
        let installs: Vec<_> = self
            .selected
            .iter()
            .enumerate()
            .filter(|(_, selected)| **selected)
            .filter_map(|(index, _)| {
                self.releases[index]
                    .clone()
                    .map(|release| (index, self.apps[index].clone(), release))
            })
            .collect();
        if installs.is_empty() {
            return;
        }

        self.busy = true;
        self.progress = 0.0;
        self.status = format!("Installing {} selected apps…", installs.len());
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let total = installs.len();
            let mut installed = Vec::new();
            let mut failures = Vec::new();
            for (position, (index, app, release)) in installs.into_iter().enumerate() {
                let result = backend::install(&app, &release, |fraction, message| {
                    let overall = (position as f32 + fraction) / total as f32;
                    let _ = tx.send(Event::Progress(
                        overall,
                        format!("{}: {message}", app.name),
                    ));
                });
                match result {
                    Ok(()) => installed.push(index),
                    Err(error) => failures.push(format!("{}: {error:#}", app.name)),
                }
            }
            let status = if failures.is_empty() {
                format!("Installed {} selected app(s).", installed.len())
            } else {
                format!(
                    "Installed {} app(s). Could not install: {}",
                    installed.len(),
                    failures.join("; ")
                )
            };
            let _ = tx.send(Event::BatchDone(installed, status));
        });
    }

    fn remove(&mut self, index: usize) {
        if self.busy {
            return;
        }
        self.pending_remove = None;
        let app = self.apps[index].clone();
        self.busy = true;
        self.status = format!("Removing {}…", app.name);
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(match backend::remove(&app) {
                Ok(()) => Event::Done(format!("{} was removed.", app.name)),
                Err(error) => Event::Failed(format!("Could not remove {}: {error:#}", app.name)),
            });
        });
    }

    fn collect_events(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                Event::Releases(results) => {
                    let available = results.iter().filter(|r| r.is_ok()).count();
                    for (index, result) in results.into_iter().enumerate() {
                        match result {
                            Ok(release) => {
                                self.releases[index] = Some(release);
                                self.errors[index].clear();
                            }
                            Err(error) => {
                                self.releases[index] = None;
                                self.errors[index] = error;
                            }
                        }
                    }
                    self.status = self.state_error.clone().unwrap_or_else(|| {
                        format!(
                            "{available} of {} macOS releases available.",
                            self.apps.len()
                        )
                    });
                    self.busy = false;
                    self.progress = 0.0;
                }
                Event::Progress(value, status) => {
                    self.progress = value;
                    self.status = status;
                }
                Event::Done(status) => {
                    match backend::load_state() {
                        Ok(state) => {
                            self.state = state;
                            self.state_error = None;
                            self.status = status;
                        }
                        Err(error) => {
                            self.state_error =
                                Some(format!("Could not reload installation settings: {error:#}"));
                            self.status = self.state_error.clone().unwrap();
                        }
                    }
                    self.busy = false;
                    self.progress = 0.0;
                }
                Event::BatchDone(installed, status) => {
                    match backend::load_state() {
                        Ok(state) => {
                            self.state = state;
                            self.state_error = None;
                            self.status = status;
                        }
                        Err(error) => {
                            self.state_error = Some(format!(
                                "Could not reload installation settings: {error:#}"
                            ));
                            self.status = self.state_error.clone().unwrap();
                        }
                    }
                    for index in installed {
                        self.selected[index] = false;
                    }
                    self.busy = false;
                    self.progress = 0.0;
                }
                Event::Failed(status) => {
                    self.status = status;
                    self.busy = false;
                    self.progress = 0.0;
                }
            }
        }
    }
}

impl eframe::App for Manager {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();
        self.collect_events();
        if self.busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        egui::CentralPanel::default().show(root, |ui| {
            ui.add_space(12.0);
            egui::Frame::new()
                .fill(Color32::from_rgb(18, 20, 30))
                .inner_margin(egui::Margin::same(20))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(
                                RichText::new("ARTCRAFT SUITE")
                                    .size(12.0)
                                    .strong()
                                    .color(Color32::from_rgb(169, 152, 255)),
                            );
                            ui.heading(
                                RichText::new("Your creative toolkit, in one place.")
                                    .size(25.0)
                                    .strong(),
                            );
                            ui.label(
                                RichText::new(
                                    "Official Mac releases · verified before install",
                                )
                                .color(Color32::from_rgb(156, 163, 180)),
                            );
                        });
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                if ui
                                    .add_enabled(
                                        !self.busy,
                                        egui::Button::new("Refresh releases"),
                                    )
                                    .clicked()
                                {
                                    self.refresh(true);
                                }
                                ui.add_space(8.0);
                                ui.label("Channel");
                                if ui
                                    .selectable_label(!self.stable, "Latest")
                                    .clicked()
                                    && self.stable
                                    && !self.busy
                                {
                                    self.stable = false;
                                    self.refresh(false);
                                }
                                if ui
                                    .selectable_label(self.stable, "Stable")
                                    .clicked()
                                    && !self.stable
                                    && !self.busy
                                {
                                    self.stable = true;
                                    self.refresh(false);
                                }
                            },
                        );
                    });
                });

            ui.add_space(12.0);
            ui.label(
                RichText::new("APPLICATIONS")
                    .size(12.0)
                    .strong()
                    .color(Color32::from_rgb(114, 121, 141)),
            );
            ui.add_space(5.0);
            egui::ScrollArea::vertical().show(ui, |ui| {
                let card_width = ((ui.available_width() - 40.0) / 2.0).max(300.0);
                for start in (0..self.apps.len()).step_by(2) {
                    ui.horizontal(|ui| {
                        self.app_card(ui, start, card_width);
                        if start + 1 < self.apps.len() {
                            ui.add_space(6.0);
                            self.app_card(ui, start + 1, card_width);
                        }
                    });
                    ui.add_space(6.0);
                }
            });

            ui.add_space(8.0);
            egui::Frame::new()
                .fill(Color32::from_rgb(18, 20, 30))
                .inner_margin(egui::Margin::same(14))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(&self.status);
                            if self.busy {
                                ui.add(
                                    egui::ProgressBar::new(self.progress)
                                        .show_percentage()
                                        .desired_width(ui.available_width()),
                                );
                            }
                            ui.label(
                                RichText::new(
                                    "Apps install in ~/Applications/ArtCraft Suite. Your documents stay in place when you remove an app.",
                                )
                                .small()
                                .color(Color32::GRAY),
                            );
                        });
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                let selected_count = self
                                    .selected
                                    .iter()
                                    .enumerate()
                                    .filter(|(index, selected)| {
                                        **selected && self.releases[*index].is_some()
                                    })
                                    .count();
                                let label = if selected_count == 0 {
                                    "Install selected".to_string()
                                } else {
                                    format!("Install selected ({selected_count})")
                                };
                                if ui
                                    .add_enabled(
                                        !self.busy && selected_count > 0,
                                        egui::Button::new(label)
                                            .fill(Color32::from_rgb(112, 91, 230)),
                                    )
                                    .clicked()
                                {
                                    self.install_selected();
                                }
                            },
                        );
                    });
                });
        });

        if let Some(index) = self.pending_remove {
            egui::Window::new("Remove app?")
                .collapsible(false)
                .resizable(false)
                .show(&ctx, |ui| {
                    ui.label(format!("Remove {} from this Mac?", self.apps[index].name));
                    ui.label("Documents and app settings will remain.");
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            self.pending_remove = None;
                        }
                        if ui.button("Remove app").clicked() {
                            self.remove(index);
                        }
                    });
                });
        }
    }
}

impl Manager {
    fn app_card(&mut self, ui: &mut egui::Ui, index: usize, width: f32) {
        let app = self.apps[index].clone();
        let installed = self.state.apps.get(&app.id).cloned();
        let release = self.releases[index].clone();
        let error = self.errors[index].clone();
        let accent = parse_color(&app.accent);
        let is_installed = installed.is_some();
        let installed_version = installed.as_ref().map(|item| item.version.as_str());
        let available_version = release.as_ref().map(|item| item.version.as_str());
        let action = match (installed_version, available_version) {
            (None, _) => "Install",
            (Some(_), None) => "Install",
            (Some(installed), Some(available)) => match backend::compare_versions(available, installed) {
                Some(std::cmp::Ordering::Greater) => "Update",
                Some(std::cmp::Ordering::Less) => "Downgrade",
                Some(std::cmp::Ordering::Equal) => "Reinstall",
                None => "Replace",
            },
        };
        let mut selected = self.selected[index];
        let mut install_clicked = false;
        let mut open_clicked = false;
        let mut remove_clicked = false;

        egui::Frame::group(ui.style())
            .fill(if is_installed {
                Color32::from_rgb(25, 36, 31)
            } else {
                Color32::from_rgb(23, 25, 35)
            })
            .stroke(egui::Stroke::new(
                1.0,
                if is_installed {
                    Color32::from_rgb(57, 201, 138)
                } else {
                    Color32::from_rgb(43, 46, 60)
                },
            ))
            .show(ui, |ui| {
                ui.set_width(width);
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(!self.busy && release.is_some(), |ui| {
                        ui.checkbox(&mut selected, "");
                    });
                    egui::Frame::new()
                        .fill(accent)
                        .inner_margin(egui::Margin::same(10))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(initials(&app.name))
                                    .strong()
                                    .color(Color32::WHITE),
                            );
                        });
                    ui.add_space(4.0);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&app.name).size(17.0).strong());
                        ui.label(
                            RichText::new(&app.description)
                                .color(Color32::from_rgb(151, 158, 175)),
                        );
                        if let Some(installed) = &installed {
                            ui.label(
                                RichText::new(format!("✓ INSTALLED · {}", installed.version))
                                    .size(11.0)
                                    .strong()
                                    .color(Color32::from_rgb(131, 240, 190)),
                            );
                        }
                        if let Some(release) = &release {
                            ui.label(
                                RichText::new(format!("Available {}", release.version))
                                    .size(12.0)
                                    .color(Color32::from_rgb(169, 152, 255)),
                            );
                        } else if !error.is_empty() {
                            ui.label(
                                RichText::new(format!("Unavailable: {error}"))
                                    .size(11.0)
                                    .color(Color32::from_rgb(235, 166, 113)),
                            );
                        } else {
                            ui.label(
                                RichText::new("Checking release…")
                                    .size(11.0)
                                    .color(Color32::GRAY),
                            );
                        }
                    });
                });
                ui.add_space(8.0);
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        install_clicked = ui
                            .add_enabled(
                                !self.busy && release.is_some(),
                                egui::Button::new(action)
                                    .fill(Color32::from_rgb(112, 91, 230)),
                            )
                            .clicked();
                        if is_installed {
                            remove_clicked = ui
                                .add_enabled(!self.busy, egui::Button::new("Remove"))
                                .clicked();
                            open_clicked = ui
                                .add_enabled(!self.busy, egui::Button::new("Open"))
                                .clicked();
                        }
                    },
                );
            });
        self.selected[index] = selected;

        if install_clicked {
            self.install(index);
        }
        if open_clicked {
            if let Err(error) = backend::launch(&app) {
                self.status = format!("Could not open {}: {error:#}", app.name);
            }
        }
        if remove_clicked {
            self.pending_remove = Some(index);
        }
    }
}

fn parse_color(hex: &str) -> Color32 {
    let value = hex.trim_start_matches('#');
    if value.len() != 6 {
        return Color32::from_rgb(112, 91, 230);
    }
    let channel = |start| u8::from_str_radix(&value[start..start + 2], 16).unwrap_or(112);
    Color32::from_rgb(channel(0), channel(2), channel(4))
}

fn initials(name: &str) -> String {
    let letters: String = name.chars().filter(|character| character.is_uppercase()).take(2).collect();
    if letters.is_empty() {
        name.chars().take(2).collect()
    } else {
        letters
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
            .with_inner_size([1040.0, 760.0])
            .with_min_inner_size([900.0, 660.0]),
        ..Default::default()
    };
    eframe::run_native(
        "ArtCraft Suite",
        options,
        Box::new(move |cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            Ok(Box::new(Manager::new(apps)))
        }),
    )
}
