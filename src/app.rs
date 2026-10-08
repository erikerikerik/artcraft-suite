use crate::{
    model::{AppManifest, InstalledApp, ResolvedRelease, SuiteManifest},
    services,
};
use eframe::egui::{self, Color32, RichText, Stroke, TextureHandle, Vec2};
use std::{
    collections::{BTreeMap, VecDeque},
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    thread,
};

const BG: Color32 = Color32::from_rgb(13, 15, 22);
const PANEL: Color32 = Color32::from_rgb(18, 20, 30);
const CARD: Color32 = Color32::from_rgb(23, 25, 35);
const BORDER: Color32 = Color32::from_rgb(43, 46, 60);
const PURPLE: Color32 = Color32::from_rgb(169, 152, 255);
const PRIMARY: Color32 = Color32::from_rgb(124, 92, 252);
const MUTED: Color32 = Color32::from_rgb(151, 158, 175);
const INSTALLED_BG: Color32 = Color32::from_rgb(25, 36, 31);
const GREEN: Color32 = Color32::from_rgb(57, 201, 138);

struct AppEntry {
    manifest: AppManifest,
    selected: bool,
    installed: Option<InstalledApp>,
    release: Option<ResolvedRelease>,
    error: Option<String>,
}

enum Message {
    RefreshComplete(Vec<(String, Result<ResolvedRelease, String>)>),
    Progress(f32),
    Installed(String, Result<InstalledApp, String>),
    Removed(String, Result<(), String>),
}

pub struct ArtCraftSuite {
    apps: Vec<AppEntry>,
    icons: BTreeMap<String, TextureHandle>,
    channel: String,
    status: String,
    busy: bool,
    progress: f32,
    install_queue: VecDeque<usize>,
    sender: Sender<Message>,
    receiver: Receiver<Message>,
}

impl ArtCraftSuite {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        configure_style(&cc.egui_ctx);
        let manifest: SuiteManifest = serde_json::from_str(include_str!("../manifest/apps.json"))
            .expect("embedded manifest must be valid");
        assert_eq!(manifest.schema_version, 1, "unsupported manifest schema");
        let state = services::load_state();
        let apps = manifest
            .apps
            .into_iter()
            .map(|manifest| AppEntry {
                installed: state.apps.get(&manifest.id).cloned(),
                manifest,
                selected: false,
                release: None,
                error: None,
            })
            .collect();
        let icons = load_icons(&cc.egui_ctx);
        let (sender, receiver) = mpsc::channel();
        let mut suite = Self {
            apps,
            icons,
            channel: "Stable".into(),
            status: "Ready. Choose apps, then install.".into(),
            busy: false,
            progress: 0.0,
            install_queue: VecDeque::new(),
            sender,
            receiver,
        };
        suite.refresh();
        suite
    }

    fn refresh(&mut self) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.progress = 0.0;
        self.status = format!("Checking {} releases…", self.channel.to_lowercase());
        let channel = self.channel.clone();
        let apps: Vec<_> = self
            .apps
            .iter()
            .map(|entry| entry.manifest.clone())
            .collect();
        let sender = self.sender.clone();
        thread::spawn(move || {
            let platform = services::platform_key();
            let results = apps
                .into_iter()
                .map(|app| {
                    let result = services::resolve_release(&app, &channel, platform);
                    (app.id, result)
                })
                .collect();
            let _ = sender.send(Message::RefreshComplete(results));
        });
    }

    fn install_one(&mut self, index: usize) {
        if self.busy {
            return;
        }
        let app = self.apps[index].manifest.clone();
        let release = self.apps[index].release.clone();
        let channel = self.channel.clone();
        let sender = self.sender.clone();
        self.busy = true;
        self.progress = 0.0;
        self.status = format!("Preparing {}…", app.name);
        thread::spawn(move || {
            let result = (|| {
                let release = match release {
                    Some(release) => release,
                    None => services::resolve_release(&app, &channel, services::platform_key())?,
                };
                let extension = if release.asset.name.ends_with(".dmg") {
                    "dmg"
                } else {
                    "zip"
                };
                let package: PathBuf = std::env::temp_dir().join(format!(
                    "artcraft-suite-{}-{}.{}",
                    app.id,
                    std::process::id(),
                    extension
                ));
                let progress_sender = sender.clone();
                let download = services::download_verified(&release, &package, move |value| {
                    let _ = progress_sender.send(Message::Progress(value));
                });
                if let Err(error) = download {
                    let _ = std::fs::remove_file(&package);
                    return Err(error);
                }
                let installed = services::install(&app, &release, &package);
                let _ = std::fs::remove_file(&package);
                installed
            })();
            let _ = sender.send(Message::Installed(app.id, result));
        });
    }

    fn install_selected(&mut self) {
        if self.busy {
            return;
        }
        self.install_queue = self
            .apps
            .iter()
            .enumerate()
            .filter_map(|(index, app)| app.selected.then_some(index))
            .collect();
        if let Some(index) = self.install_queue.pop_front() {
            self.install_one(index);
        }
    }

    fn remove_one(&mut self, index: usize) {
        if self.busy {
            return;
        }
        let id = self.apps[index].manifest.id.clone();
        let name = self.apps[index].manifest.name.clone();
        let sender = self.sender.clone();
        self.busy = true;
        self.status = format!("Removing {name}…");
        thread::spawn(move || {
            let result = services::uninstall(&id);
            let _ = sender.send(Message::Removed(id, result));
        });
    }

    fn process_messages(&mut self, ctx: &egui::Context) {
        while let Ok(message) = self.receiver.try_recv() {
            let mut continue_install_queue = false;
            match message {
                Message::RefreshComplete(results) => {
                    let mut ready = 0;
                    for (id, result) in results {
                        if let Some(entry) =
                            self.apps.iter_mut().find(|entry| entry.manifest.id == id)
                        {
                            match result {
                                Ok(release) => {
                                    entry.release = Some(release);
                                    entry.error = None;
                                    ready += 1;
                                }
                                Err(error) => {
                                    entry.release = None;
                                    entry.error = Some(error);
                                }
                            }
                        }
                    }
                    self.busy = false;
                    self.progress = 0.0;
                    self.status = if ready == self.apps.len() {
                        format!("All {ready} apps are ready.")
                    } else {
                        format!(
                            "{ready} of {} releases are available. Hover unavailable items for details.",
                            self.apps.len()
                        )
                    };
                }
                Message::Progress(value) => {
                    self.progress = value;
                    self.status = format!(
                        "Downloading and verifying package… {}%",
                        (value * 100.0).round()
                    );
                }
                Message::Installed(id, result) => {
                    self.busy = false;
                    self.progress = 0.0;
                    if let Some(entry) = self.apps.iter_mut().find(|entry| entry.manifest.id == id)
                    {
                        match result {
                            Ok(installed) => {
                                entry.selected = false;
                                self.status = format!(
                                    "{} {} is installed.",
                                    entry.manifest.name, installed.version
                                );
                                entry.installed = Some(installed);
                            }
                            Err(error) => {
                                self.status =
                                    format!("Could not install {}: {error}", entry.manifest.name)
                            }
                        }
                    }
                    continue_install_queue = true;
                }
                Message::Removed(id, result) => {
                    self.busy = false;
                    if let Some(entry) = self.apps.iter_mut().find(|entry| entry.manifest.id == id)
                    {
                        match result {
                            Ok(()) => {
                                entry.installed = None;
                                self.status = format!("{} was removed.", entry.manifest.name);
                            }
                            Err(error) => {
                                self.status =
                                    format!("Could not remove {}: {error}", entry.manifest.name)
                            }
                        }
                    }
                }
            }
            if continue_install_queue && let Some(index) = self.install_queue.pop_front() {
                self.install_one(index);
            }
            ctx.request_repaint();
        }
    }

    fn header(&mut self, root: &mut egui::Ui) {
        egui::Panel::top("header")
            .exact_size(140.0)
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(egui::Margin::symmetric(28, 22))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(41, 44, 58))),
            )
            .show(root, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("A R T C R A F T   S U I T E")
                                .size(13.0)
                                .strong()
                                .color(PURPLE),
                        );
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new("Your creative toolkit, in one place.")
                                .size(27.0)
                                .strong()
                                .color(Color32::WHITE),
                        );
                        ui.add_space(3.0);
                        ui.label(
                            RichText::new("Official upstream builds · verified before install")
                                .size(15.0)
                                .color(Color32::from_rgb(156, 163, 180)),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let refresh = ui.add_enabled(
                            !self.busy,
                            egui::Button::new(RichText::new("Refresh").size(15.0))
                                .min_size(Vec2::new(76.0, 36.0)),
                        );
                        if refresh.clicked() {
                            self.refresh();
                        }
                        let previous = self.channel.clone();
                        egui::ComboBox::from_id_salt("channel")
                            .width(120.0)
                            .selected_text(RichText::new(&self.channel).size(15.0))
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut self.channel, "Stable".into(), "Stable");
                                ui.selectable_value(&mut self.channel, "Latest".into(), "Latest");
                            });
                        ui.label(
                            RichText::new("Channel")
                                .size(15.0)
                                .color(Color32::from_rgb(170, 176, 192)),
                        );
                        if previous != self.channel {
                            self.refresh();
                        }
                    });
                });
            });
    }

    fn footer(&mut self, root: &mut egui::Ui) {
        egui::Panel::bottom("footer")
            .exact_size(76.0)
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(egui::Margin::symmetric(28, 16))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(41, 44, 58))),
            )
            .show(root, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.add_space(3.0);
                        ui.label(RichText::new(&self.status).size(14.0).color(Color32::WHITE));
                        if self.busy {
                            ui.add_space(8.0);
                            ui.add(
                                egui::ProgressBar::new(self.progress)
                                    .desired_width((ui.available_width() - 180.0).max(120.0))
                                    .desired_height(4.0),
                            );
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let selected: Vec<usize> = self
                            .apps
                            .iter()
                            .enumerate()
                            .filter_map(|(index, app)| app.selected.then_some(index))
                            .collect();
                        let enabled = !self.busy && !selected.is_empty();
                        let button = ui.add_enabled(
                            enabled,
                            primary_button("Install selected", Vec2::new(140.0, 42.0)),
                        );
                        if button.clicked() {
                            self.install_selected();
                        }
                    });
                });
            });
    }

    fn card(&mut self, ui: &mut egui::Ui, index: usize, size: Vec2) {
        let installed = services::is_installed(self.apps[index].installed.as_ref());
        let fill = if installed { INSTALLED_BG } else { CARD };
        let stroke = if installed {
            Stroke::new(1.0, GREEN)
        } else {
            Stroke::new(1.0, BORDER)
        };
        let tooltip = self.apps[index].error.clone();
        let card =
            ui.allocate_ui_with_layout(size, egui::Layout::top_down(egui::Align::Min), |ui| {
                egui::Frame::new()
                    .fill(fill)
                    .stroke(stroke)
                    .corner_radius(12.0)
                    .inner_margin(egui::Margin::same(14))
                    .show(ui, |ui| {
                        ui.set_min_size(size - Vec2::splat(28.0));
                        ui.set_max_size(size - Vec2::splat(28.0));
                        ui.horizontal(|ui| {
                            ui.add_space(2.0);
                            ui.vertical_centered(|ui| {
                                ui.add_space(((size.y - 58.0) * 0.5).max(0.0));
                                ui.add_enabled(
                                    !self.busy,
                                    egui::Checkbox::without_text(&mut self.apps[index].selected),
                                );
                            });
                            ui.add_space(8.0);
                            ui.vertical_centered(|ui| {
                                ui.add_space(((size.y - 72.0) * 0.5).max(0.0));
                                if let Some(icon) = self.icons.get(&self.apps[index].manifest.id) {
                                    ui.add(
                                        egui::Image::new((icon.id(), Vec2::splat(44.0)))
                                            .corner_radius(10.0),
                                    );
                                }
                            });
                            ui.add_space(8.0);
                            let right_width = if installed { 224.0 } else { 122.0 };
                            let middle_width =
                                (ui.available_width() - right_width - 12.0).max(100.0);
                            ui.allocate_ui_with_layout(
                                Vec2::new(middle_width, size.y - 28.0),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| {
                                    ui.add_space(6.0);
                                    ui.label(
                                        RichText::new(&self.apps[index].manifest.name)
                                            .size(17.0)
                                            .strong()
                                            .color(Color32::WHITE),
                                    );
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(&self.apps[index].manifest.description)
                                                .size(14.0)
                                                .color(MUTED),
                                        )
                                        .truncate(),
                                    );
                                    if installed {
                                        let version =
                                            &self.apps[index].installed.as_ref().unwrap().version;
                                        egui::Frame::new()
                                            .fill(Color32::from_rgb(22, 75, 56))
                                            .stroke(Stroke::new(1.0, GREEN))
                                            .corner_radius(10.0)
                                            .inner_margin(egui::Margin::symmetric(8, 3))
                                            .show(ui, |ui| {
                                                ui.label(
                                                    RichText::new(format!(
                                                        "✓ INSTALLED · {version}"
                                                    ))
                                                    .size(11.0)
                                                    .strong()
                                                    .color(Color32::from_rgb(131, 240, 190)),
                                                );
                                            });
                                    }
                                },
                            );
                            ui.with_layout(egui::Layout::top_down(egui::Align::Max), |ui| {
                                ui.add_space(3.0);
                                let release_label = self.apps[index]
                                    .release
                                    .as_ref()
                                    .map(|release| format!("Available {}", release.version))
                                    .unwrap_or_else(|| {
                                        if self.apps[index].error.is_some() {
                                            "Unavailable".into()
                                        } else {
                                            "Checking…".into()
                                        }
                                    });
                                ui.label(RichText::new(release_label).size(12.0).color(PURPLE));
                                ui.add_space(8.0);
                                ui.horizontal(|ui| {
                                    if installed {
                                        if ui
                                            .add_enabled(
                                                !self.busy,
                                                action_button(
                                                    "Open",
                                                    Color32::from_rgb(35, 122, 89),
                                                    Vec2::new(56.0, 36.0),
                                                ),
                                            )
                                            .clicked()
                                            && let Some(entry) = self.apps[index].installed.as_ref()
                                            && let Err(error) = services::launch(entry)
                                        {
                                            self.status = format!(
                                                "Could not open {}: {error}",
                                                self.apps[index].manifest.name
                                            );
                                        }
                                        if ui
                                            .add_enabled(
                                                !self.busy,
                                                egui::Button::new("Remove")
                                                    .min_size(Vec2::new(66.0, 36.0)),
                                            )
                                            .clicked()
                                        {
                                            self.remove_one(index);
                                        }
                                    }
                                    let label = if installed {
                                        let different = self.apps[index]
                                            .release
                                            .as_ref()
                                            .zip(self.apps[index].installed.as_ref())
                                            .is_some_and(|(release, current)| {
                                                release.version != current.version
                                            });
                                        if different { "Update" } else { "Reinstall" }
                                    } else {
                                        "Install"
                                    };
                                    let can_install =
                                        !self.busy && self.apps[index].release.is_some();
                                    if ui
                                        .add_enabled(
                                            can_install,
                                            primary_button(
                                                label,
                                                Vec2::new(
                                                    if installed { 86.0 } else { 82.0 },
                                                    36.0,
                                                ),
                                            ),
                                        )
                                        .clicked()
                                    {
                                        self.install_queue.clear();
                                        self.install_one(index);
                                    }
                                });
                            });
                        });
                    })
            });
        let response = card.inner.response;
        if let Some(tooltip) = tooltip {
            response.on_hover_text(tooltip);
        }
    }
}

impl eframe::App for ArtCraftSuite {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();
        self.process_messages(&ctx);
        if self.busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.header(root);
        self.footer(root);
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(BG)
                    .inner_margin(egui::Margin::symmetric(23, 12)),
            )
            .show(root, |ui| {
                ui.label(
                    RichText::new("A P P L I C A T I O N S")
                        .size(12.0)
                        .strong()
                        .color(Color32::from_rgb(114, 121, 141)),
                );
                ui.add_space(7.0);
                let gap = 10.0;
                let width = (ui.available_width() - gap) / 2.0;
                let height = ((ui.available_height() - gap * 3.0) / 4.0).clamp(100.0, 116.0);
                for row in 0..4 {
                    ui.horizontal(|ui| {
                        let left = row * 2;
                        if left < self.apps.len() {
                            self.card(ui, left, Vec2::new(width, height));
                        }
                        ui.add_space(gap);
                        let right = left + 1;
                        if right < self.apps.len() {
                            self.card(ui, right, Vec2::new(width, height));
                        }
                    });
                    if row < 3 {
                        ui.add_space(gap);
                    }
                }
            });
    }
}

fn configure_style(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = BG;
    visuals.window_fill = PANEL;
    visuals.widgets.inactive.bg_fill = Color32::from_rgb(62, 65, 76);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(78, 81, 94);
    visuals.widgets.active.bg_fill = PRIMARY;
    visuals.selection.bg_fill = PRIMARY;
    ctx.set_visuals(visuals);
    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style.spacing.button_padding = Vec2::new(12.0, 8.0);
    style.spacing.item_spacing = Vec2::new(6.0, 6.0);
    ctx.set_style_of(egui::Theme::Dark, style);
}

fn primary_button(text: &str, size: Vec2) -> egui::Button<'_> {
    action_button(text, PRIMARY, size)
}

fn action_button(text: &str, fill: Color32, size: Vec2) -> egui::Button<'_> {
    egui::Button::new(RichText::new(text).size(14.0).color(Color32::WHITE))
        .fill(fill)
        .corner_radius(8.0)
        .min_size(size)
}

fn load_icons(ctx: &egui::Context) -> BTreeMap<String, TextureHandle> {
    let icons: [(&str, &[u8]); 7] = [
        (
            "photocraft",
            include_bytes!("../assets/icons/photocraft.png"),
        ),
        (
            "vectorcraft",
            include_bytes!("../assets/icons/vectorcraft.png"),
        ),
        (
            "designcraft",
            include_bytes!("../assets/icons/designcraft.png"),
        ),
        ("filmcraft", include_bytes!("../assets/icons/filmcraft.png")),
        (
            "effectcraft",
            include_bytes!("../assets/icons/effectcraft.png"),
        ),
        (
            "lightcraft",
            include_bytes!("../assets/icons/lightcraft.png"),
        ),
        (
            "printcraft",
            include_bytes!("../assets/icons/printcraft.png"),
        ),
    ];
    icons
        .into_iter()
        .filter_map(|(id, bytes)| {
            let image = image::load_from_memory(bytes).ok()?.into_rgba8();
            let size = [image.width() as usize, image.height() as usize];
            let pixels = image.into_raw();
            let color_image = egui::ColorImage::from_rgba_unmultiplied(size, &pixels);
            Some((
                id.to_owned(),
                ctx.load_texture(
                    format!("{id}-icon"),
                    color_image,
                    egui::TextureOptions::LINEAR,
                ),
            ))
        })
        .collect()
}
