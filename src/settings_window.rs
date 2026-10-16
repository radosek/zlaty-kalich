//! A small native settings window (F1, Cmd/Ctrl+comma, or Back on a
//! controller), drawn with egui on its own wgpu surface.

use crate::settings::{Hotspots, Quality, Settings};
use ruffle_render_wgpu::wgpu;
use std::sync::Arc;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

/// What changed, for the app to apply.
#[derive(Default)]
pub struct Changes {
    pub volume: bool,
    pub quality: bool,
    pub fullscreen: bool,
    pub slot: bool,
    /// Takes effect on the next start (re-patching or reloading assets).
    pub needs_restart: bool,
}

pub struct SettingsWindow {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    egui: egui_winit::State,
    renderer: egui_wgpu::Renderer,
    restart_pending: bool,
}

impl SettingsWindow {
    pub fn open(event_loop: &ActiveEventLoop, english: bool) -> anyhow::Result<Self> {
        let window = Arc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_title(if english { "Settings" } else { "Nastavení" })
                    .with_inner_size(winit::dpi::LogicalSize::new(460.0, 330.0))
                    .with_resizable(false),
            )?,
        );
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window.clone())?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        }))?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))?;
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or_else(|| anyhow::anyhow!("surface unsupported"))?;
        // egui blends in gamma space and expects a non-sRGB target.
        config.format = config.format.remove_srgb_suffix();
        surface.configure(&device, &config);

        let ctx = egui::Context::default();
        ctx.set_visuals(egui::Visuals::dark());
        let egui = egui_winit::State::new(
            ctx,
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        let renderer = egui_wgpu::Renderer::new(&device, config.format, Default::default());
        Ok(Self {
            window,
            surface,
            device,
            queue,
            config,
            egui,
            renderer,
            restart_pending: false,
        })
    }

    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    pub fn focus(&self) {
        self.window.focus_window();
    }

    /// Returns `None` once the window should close.
    pub fn handle(
        &mut self,
        event: &WindowEvent,
        settings: &mut Settings,
        english: bool,
    ) -> Option<Changes> {
        let response = self.egui.on_window_event(&self.window, event);
        if response.repaint {
            self.window.request_redraw();
        }
        match event {
            WindowEvent::CloseRequested => None,
            WindowEvent::Resized(size) => {
                self.config.width = size.width.max(1);
                self.config.height = size.height.max(1);
                self.surface.configure(&self.device, &self.config);
                Some(Changes::default())
            }
            WindowEvent::RedrawRequested => self.redraw(settings, english),
            _ => Some(Changes::default()),
        }
    }

    fn redraw(&mut self, settings: &mut Settings, english: bool) -> Option<Changes> {
        let input = self.egui.take_egui_input(&self.window);
        let mut changes = Changes::default();
        let mut close = false;
        let ctx = self.egui.egui_ctx().clone();
        let output = ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                close = self.ui(ui, settings, english, &mut changes);
            });
        });
        self.egui
            .handle_platform_output(&self.window, output.platform_output);
        let jobs = ctx.tessellate(output.shapes, output.pixels_per_point);

        let mut textures = output.textures_delta;
        for (id, deltas) in textures.set.drain() {
            for delta in &deltas {
                self.renderer
                    .update_texture(&self.device, &self.queue, id, delta);
            }
        }
        for id in textures.free.drain() {
            self.renderer.free_texture(&id);
        }
        textures.clear();

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f)
            | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            _ => return Some(changes),
        };
        let view = frame.texture.create_view(&Default::default());
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.config.width, self.config.height],
            pixels_per_point: output.pixels_per_point,
        };
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let commands =
            self.renderer
                .update_buffers(&self.device, &self.queue, &mut encoder, &jobs, &screen);
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                        depth_slice: None,
                    })],
                    label: Some("settings"),
                    ..Default::default()
                })
                .forget_lifetime();
            self.renderer.render(&mut pass, &jobs, &screen);
        }
        self.queue
            .submit(commands.into_iter().chain([encoder.finish()]));
        self.queue.present(frame);
        if output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .is_some_and(|v| v.repaint_delay.is_zero())
        {
            self.window.request_redraw();
        }
        (!close).then_some(changes)
    }

    /// Draws the form; returns true when the user closes it.
    fn ui(&mut self, ui: &mut egui::Ui, s: &mut Settings, en: bool, changes: &mut Changes) -> bool {
        let t = |cs: &'static str, english: &'static str| if en { english } else { cs };
        ui.spacing_mut().item_spacing.y = 10.0;
        egui::Grid::new("settings")
            .num_columns(2)
            .spacing([16.0, 12.0])
            .show(ui, |ui| {
                ui.label(t("Hudba", "Music"));
                changes.volume |= ui
                    .add(egui::Slider::new(&mut s.music_volume, 0..=100).suffix(" %"))
                    .changed();
                ui.end_row();

                ui.label(t("Kvalita obrazu", "Graphics"));
                ui.horizontal(|ui| {
                    changes.quality |= ui
                        .radio_value(&mut s.quality, Quality::Best, t("nejlepší", "best"))
                        .changed();
                    changes.quality |= ui
                        .radio_value(&mut s.quality, Quality::Game, t("podle hry", "as in game"))
                        .changed();
                });
                ui.end_row();

                ui.label(t("Celá obrazovka", "Fullscreen"));
                ui.horizontal(|ui| {
                    for (value, label) in [
                        (None, t("podle hry", "as in game")),
                        (Some(true), t("ano", "on")),
                        (Some(false), t("ne", "off")),
                    ] {
                        changes.fullscreen |=
                            ui.radio_value(&mut s.fullscreen, value, label).changed();
                    }
                });
                ui.end_row();

                ui.label(t("Pozice pro uložení", "Save slot"));
                egui::ComboBox::from_id_salt("slot")
                    .selected_text(slot_name(s.slot, en))
                    .show_ui(ui, |ui| {
                        for slot in 0..=9 {
                            changes.slot |= ui
                                .selectable_value(&mut s.slot, slot, slot_name(slot, en))
                                .changed();
                        }
                    });
                ui.end_row();

                ui.label(t("Aktivní místa", "Hotspots"));
                ui.horizontal(|ui| {
                    for (value, label) in [
                        (Hotspots::HoldSpace, t("mezerník", "hold Space")),
                        (Hotspots::Always, t("vždy", "always")),
                        (Hotspots::Off, t("vypnuto", "off")),
                    ] {
                        if ui.radio_value(&mut s.hotspots, value, label).changed() {
                            self.restart_pending = true;
                            changes.needs_restart = true;
                        }
                    }
                });
                ui.end_row();

                ui.label("SoundFont");
                ui.horizontal(|ui| {
                    let name = s
                        .soundfont
                        .as_deref()
                        .and_then(|p| p.file_name())
                        .map_or(t("vestavěný", "built-in").to_owned(), |n| {
                            n.to_string_lossy().into_owned()
                        });
                    ui.label(name);
                    if ui.button(t("Vybrat…", "Choose…")).clicked()
                        && let Some(path) = rfd::FileDialog::new()
                            .add_filter("SoundFont", &["sf2", "SF2"])
                            .pick_file()
                    {
                        s.soundfont = Some(path);
                        self.restart_pending = true;
                        changes.needs_restart = true;
                    }
                    if s.soundfont.is_some() && ui.button(t("Výchozí", "Default")).clicked() {
                        s.soundfont = None;
                        self.restart_pending = true;
                        changes.needs_restart = true;
                    }
                });
                ui.end_row();
            });

        if self.restart_pending {
            ui.colored_label(
                egui::Color32::from_rgb(0xFF, 0xD0, 0x30),
                t(
                    "Některé změny se projeví po restartu hry.",
                    "Some changes apply after restarting the game.",
                ),
            );
        }
        ui.separator();
        ui.small(t(
            "Mezerník: aktivní místa · Ctrl/Cmd+0–9: pozice · Ctrl/Cmd+šipky: hudba · F11: celá obrazovka",
            "Space: hotspots · Ctrl/Cmd+0–9: slot · Ctrl/Cmd+arrows: music · F11: fullscreen",
        ));
        ui.add_space(8.0);
        ui.button(t("Zavřít", "Close")).clicked()
    }
}

fn slot_name(slot: u8, en: bool) -> String {
    match (slot, en) {
        (0, false) => "automatická".to_owned(),
        (0, true) => "autosave".to_owned(),
        (n, false) => format!("pozice {n}"),
        (n, true) => format!("slot {n}"),
    }
}
