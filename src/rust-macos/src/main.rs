mod backend;

use backend::{AppManifest, InstallState, Release};
use eframe::egui::{self, Color32, RichText};
use std::sync::mpsc::{self, Receiver, Sender};

enum Event {
    Releases(Vec<Result<Release, String>>),
    Progress(f32, String),
    Done(String),
    Failed(String),
}

struct Manager {
    apps: Vec<AppManifest>,
    releases: Vec<Option<Release>>,
    errors: Vec<String>,
    state: InstallState,
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
        let state = backend::load_state().unwrap_or_default();
        let mut manager = Self {
            apps,
            releases: vec![None; count],
            errors: vec![String::new(); count],
            state,
            stable: true,
            busy: false,
            status: "Ready to check releases.".into(),
            progress: 0.0,
            pending_remove: None,
            tx,
            rx,
        };
        manager.refresh();
        manager
    }

    fn refresh(&mut self) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.status = "Checking upstream releases…".into();
        let apps = self.apps.clone();
        let stable = self.stable;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let results = apps
                .iter()
                .map(|app| backend::resolve(app, stable).map_err(|e| e.to_string()))
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
                    self.status = format!(
                        "{available} of {} macOS releases available.",
                        self.apps.len()
                    );
                    self.busy = false;
                    self.progress = 0.0;
                }
                Event::Progress(value, status) => {
                    self.progress = value;
                    self.status = status;
                }
                Event::Done(status) => {
                    self.state = backend::load_state().unwrap_or_default();
                    self.status = status;
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
            ui.add_space(18.0);
            ui.horizontal(|ui| {
                ui.heading(RichText::new("ArtCraft Suite").size(30.0).strong());
                ui.add_space(8.0);
                ui.label(RichText::new("for Apple Silicon").color(Color32::LIGHT_GRAY));
            });
            ui.label("Install and update the seven ArtCraft creative applications.");
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.label("Release channel:");
                if ui.selectable_label(self.stable, "Stable").clicked() && !self.stable && !self.busy {
                    self.stable = true;
                    self.refresh();
                }
                if ui.selectable_label(!self.stable, "Latest").clicked() && self.stable && !self.busy {
                    self.stable = false;
                    self.refresh();
                }
                ui.add_space(12.0);
                if ui.add_enabled(!self.busy, egui::Button::new("Check releases")).clicked() { self.refresh(); }
            });
            ui.separator();
            ui.label(&self.status);
            if self.busy { ui.add(egui::ProgressBar::new(self.progress).show_percentage()); }
            ui.add_space(7.0);
            egui::ScrollArea::vertical().show(ui, |ui| {
                for index in 0..self.apps.len() {
                    let app = &self.apps[index];
                    let installed = self.state.apps.get(&app.id);
                    let installed_version = installed.map(|s| s.version.clone());
                    let available = self.releases[index].as_ref().map(|r| r.version.clone());
                    let mut install_clicked = false;
                    let mut open_clicked = false;
                    let mut remove_clicked = false;
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label(RichText::new(&app.name).size(19.0).strong());
                                ui.label(RichText::new(&app.description).color(Color32::LIGHT_GRAY));
                                if let Some(version) = &installed_version {
                                    ui.label(RichText::new(format!("Installed {version}")).color(Color32::from_rgb(115, 220, 163)));
                                }
                                if let Some(version) = &available {
                                    ui.label(format!("Available {version}"));
                                } else if !self.errors[index].is_empty() {
                                    ui.label(RichText::new(format!("Unavailable: {}", self.errors[index])).color(Color32::from_rgb(235, 166, 113)));
                                }
                            });
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if installed.is_some() {
                                    remove_clicked = ui.add_enabled(!self.busy, egui::Button::new("Remove")).clicked();
                                    open_clicked = ui.add_enabled(!self.busy, egui::Button::new("Open")).clicked();
                                }
                                let action = if installed_version.is_none() { "Install" }
                                    else if installed_version != available { "Update" } else { "Reinstall" };
                                install_clicked = ui.add_enabled(!self.busy && available.is_some(), egui::Button::new(action)).clicked();
                            });
                        });
                    });
                    ui.add_space(6.0);
                    if install_clicked { self.install(index); }
                    if open_clicked {
                        if let Err(error) = backend::launch(&self.apps[index]) {
                            self.status = format!("Could not open {}: {error:#}", self.apps[index].name);
                        }
                    }
                    if remove_clicked { self.pending_remove = Some(index); }
                }
            });
            ui.separator();
            ui.label(RichText::new("Installed apps live in ~/Applications/ArtCraft Suite. Removing an app leaves its documents and settings in place.").small().color(Color32::GRAY));
            ui.hyperlink_to("ArtCraft Suite Manager by erikerikerik", "https://github.com/erikerikerik/artcraft-suite");
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
            .with_inner_size([880.0, 710.0])
            .with_min_inner_size([700.0, 500.0]),
        ..Default::default()
    };
    eframe::run_native(
        "ArtCraft Suite",
        options,
        Box::new(move |_cc| Ok(Box::new(Manager::new(apps)))),
    )
}
