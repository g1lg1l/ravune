//! The visualizer: what comes out of the speakers, as a shader light show (the default, on
//! the GPU: see visualizer.wgsl), spectrum bars or a scope. It docks at any edge of the window
//! (at the bottom, under the on-screen keys, unless moved) or floats in a window of its own.

use std::sync::Arc;

use eframe::egui::{self, CornerRadius, Id, Rect, RichText, Sense, Stroke, Ui, pos2};
use eframe::egui_wgpu::{self, CallbackResources, CallbackTrait, ScreenDescriptor, wgpu};
use ravune_engine::Engine;
use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};
use serde::{Deserialize, Serialize};

use crate::theme::Theme;
use crate::{App, icons, kit};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Dock {
    #[default]
    Bottom,
    Top,
    Left,
    Right,
    Window,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    #[default]
    Shader,
    Spectrum,
    Scope,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub show: bool,
    pub dock: Dock,
    pub mode: Mode,
}

/// Frames analysed at once (~43 ms at 48 kHz).
const SIZE: usize = 2048;
const BARS: usize = 64;
/// The bars span this, spaced evenly in octaves.
const LOW: f32 = 30.0;
const HIGH: f32 = 18_000.0;
/// dB at the foot of the bars.
const FLOOR: f32 = -66.0;

pub struct Visualizer {
    fft: Arc<dyn RealToComplex<f32>>,
    frames: Vec<f32>,
    windowed: Vec<f32>,
    bins: Vec<Complex<f32>>,
    /// Each bar's level (0..1, falling smoothly) and its peak with when it was reached.
    bars: [f32; BARS],
    peaks: [(f32, f64); BARS],
    last: f64,
    dt: f32,
    /// Bass, mids and highs (0..1, falling back smoothly), the bass's slow average and the
    /// kick: how far the bass just jumped over it.
    energy: [f32; 3],
    slow_bass: f32,
    kick: f32,
    /// The shader's pipelines are in egui's renderer (not in tests, or without a GPU).
    pub gpu: bool,
}

impl Default for Visualizer {
    fn default() -> Self {
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(SIZE);
        let bins = fft.make_output_vec();
        Self { fft, frames: vec![0.0; SIZE], windowed: vec![0.0; SIZE], bins, bars: [0.0; BARS], peaks: [(0.0, 0.0); BARS], last: 0.0, dt: 0.0, energy: [0.0; 3], slow_bass: 0.0, kick: 0.0, gpu: false }
    }
}

impl Visualizer {
    /// Reads the latest output and moves the bars toward it. `volume`: the app's output
    /// volume, taken back out so it shows the mix however loud you listen.
    fn update(&mut self, engine: Option<&Engine>, volume: f32, now: f64) {
        let dt = (now - self.last).clamp(0.0, 0.1) as f32;
        (self.last, self.dt) = (now, dt);
        let Some(engine) = engine else {
            self.frames.fill(0.0);
            self.fall(&[0.0; BARS], now, dt);
            return;
        };
        engine.stats().scope(&mut self.frames);
        let gain = 1.0 / volume.max(0.01);
        self.frames.iter_mut().for_each(|x| *x *= gain);
        for (k, (w, x)) in self.windowed.iter_mut().zip(&self.frames).enumerate() {
            *w = x * (std::f32::consts::PI * k as f32 / SIZE as f32).sin().powi(2); // Hann
        }
        if self.fft.process(&mut self.windowed, &mut self.bins).is_err() {
            return;
        }
        // a full-scale sine reads 0 dB: the window's sum is SIZE / 2
        let magnitude = |k: usize| self.bins.get(k).map_or(0.0, |c| c.norm() * 4.0 / SIZE as f32);
        let bin_hz = engine.sample_rate as f32 / SIZE as f32;
        let levels: [f32; BARS] = std::array::from_fn(|b| {
            let edge = |b: usize| LOW * (HIGH / LOW).powf(b as f32 / BARS as f32);
            let (lo, hi) = (edge(b), edge(b + 1));
            let center = (lo * hi).sqrt();
            // the loudest bin inside, or between the two around it for a bar narrower than a bin
            let (first, last) = ((lo / bin_hz).ceil() as usize, (hi / bin_hz).floor() as usize);
            let mag = if first <= last {
                (first..=last).map(magnitude).fold(0.0, f32::max)
            } else {
                let at = center / bin_hz;
                let k = at as usize;
                magnitude(k) + (magnitude(k + 1) - magnitude(k)) * at.fract()
            };
            // music falls about 3 dB an octave: tilted, a mix looks level
            let db = 20.0 * mag.max(1e-9).log10() + 3.0 * (center / 1000.0).log2();
            ((db - FLOOR) / -FLOOR).clamp(0.0, 1.0)
        });
        self.fall(&levels, now, dt);
    }

    /// Bars jump up and fall back; peaks hang a moment first. The bands' energy and the kick
    /// follow the same way.
    fn fall(&mut self, levels: &[f32; BARS], now: f64, dt: f32) {
        let mean = |r: std::ops::Range<usize>| levels[r.clone()].iter().sum::<f32>() / r.len() as f32;
        for (e, now) in self.energy.iter_mut().zip([mean(0..10), mean(10..36), mean(36..BARS)]) {
            *e = now.max(*e - 1.2 * dt);
        }
        self.slow_bass += (self.energy[0] - self.slow_bass) * (dt * 4.0).min(1.0);
        self.kick = ((self.energy[0] - self.slow_bass) * 5.0).clamp(0.0, 1.0).max(self.kick - 3.0 * dt);
        for ((bar, peak), level) in self.bars.iter_mut().zip(&mut self.peaks).zip(levels) {
            *bar = level.max(*bar - 1.6 * dt);
            *peak = if *level >= peak.0 {
                (*level, now)
            } else if now - peak.1 > 0.6 {
                ((peak.0 - 0.5 * dt).max(*bar), peak.1)
            } else {
                *peak
            };
        }
    }

    /// What the shader gets this frame, laid out as `U` in visualizer.wgsl.
    fn uniforms(&self, t: &Theme, size: [u32; 2], time: f64) -> Vec<f32> {
        let rgb = |c: egui::Color32| c.to_normalized_gamma_f32();
        let mut u = vec![size[0] as f32, size[1] as f32, (time % 3600.0) as f32, self.dt];
        u.extend([self.energy[0], self.energy[1], self.energy[2], self.kick]);
        for c in [t.wave_bands[0], t.wave_bands[1], t.wave_bands[2], t.accent, t.bg_field] {
            u.extend(rgb(c));
        }
        u.extend([0.0, size[0] as f32 / size[1] as f32, 0.0, 0.0]); // the sRGB flag is set in `prepare`
        u.extend(self.bars);
        u
    }

    fn moving(&self) -> bool {
        self.peaks.iter().any(|p| p.0 > 0.0)
    }

    fn draw(&self, ui: &mut Ui, t: &Theme, mode: Mode) -> egui::Response {
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, CornerRadius::ZERO, t.bg_field);
        let rect = rect.shrink(t.gap);
        match mode {
            Mode::Shader if self.gpu => {
                let size = [rect.width().round().max(1.0) as u32, rect.height().round().max(1.0) as u32];
                painter.add(egui_wgpu::Callback::new_paint_callback(rect, Frame { uniforms: self.uniforms(t, size, ui.input(|i| i.time)), size }));
            }
            Mode::Scope => {
                painter.hline(rect.x_range(), rect.center().y, Stroke::new(1.0, t.line));
                // starting on a rising zero crossing holds a steady tone still
                let shown = SIZE / 2;
                let start = (1..SIZE - shown).find(|&i| self.frames[i - 1] < 0.0 && self.frames[i] >= 0.0).unwrap_or(SIZE - shown);
                let width = rect.width().max(1.0) as usize;
                let points = (0..=width)
                    .map(|px| {
                        let x = self.frames[start + (px * (shown - 1) / width)];
                        pos2(rect.left() + px as f32, rect.center().y - x.clamp(-1.0, 1.0) * rect.height() * 0.5)
                    })
                    .collect();
                painter.line(points, Stroke::new(1.5, t.accent));
            }
            // the shader without a GPU to run on draws the bars too
            Mode::Spectrum | Mode::Shader => {
                // docked at a side, the low end sits at the bottom and the bars grow sideways
                let tall = rect.height() > rect.width() * 1.2;
                let n = BARS as f32;
                let gap = if tall { (rect.height() / n * 0.25).min(2.0) } else { (rect.width() / n * 0.25).min(3.0) };
                for (i, (bar, peak)) in self.bars.iter().zip(&self.peaks).enumerate() {
                    let f = i as f32 / (n - 1.0);
                    let color = if f < 0.5 { Theme::mix(t.wave_bands[0], t.wave_bands[1], f * 2.0) } else { Theme::mix(t.wave_bands[1], t.wave_bands[2], f * 2.0 - 1.0) };
                    let (body, cap) = if tall {
                        let (y1, y0) = (rect.bottom() - rect.height() * i as f32 / n, rect.bottom() - rect.height() * (i + 1) as f32 / n + gap);
                        let x = |level: f32| rect.left() + rect.width() * level;
                        (Rect::from_min_max(pos2(rect.left(), y0), pos2(x(*bar), y1)), [pos2(x(peak.0), y0), pos2(x(peak.0), y1)])
                    } else {
                        let (x0, x1) = (rect.left() + rect.width() * i as f32 / n, rect.left() + rect.width() * (i + 1) as f32 / n - gap);
                        let y = |level: f32| rect.bottom() - rect.height() * level;
                        (Rect::from_min_max(pos2(x0, y(*bar)), pos2(x1, rect.bottom())), [pos2(x0, y(peak.0)), pos2(x1, y(peak.0))])
                    };
                    painter.rect_filled(body, t.radius_sm, color.gamma_multiply(0.8));
                    if peak.0 > 0.01 {
                        painter.line_segment(cap, Stroke::new(1.5, color));
                    }
                }
            }
        }
        response
    }
}

/// Where it can go: the dock, its icon and what the menu calls it.
const DOCKS: [(Dock, &str, &str); 5] = [
    (Dock::Bottom, icons::CARET_DOWN, "Dock at the bottom"),
    (Dock::Top, icons::CARET_UP, "Dock at the top"),
    (Dock::Left, icons::CARET_LEFT, "Dock on the left"),
    (Dock::Right, icons::CARET_RIGHT, "Dock on the right"),
    (Dock::Window, icons::ARROW_SQUARE_OUT, "In a window of its own"),
];

impl App {
    /// The visualizer docked at its edge. Call it where its panel belongs among the others:
    /// before the keys' panel, so at the bottom it sits under them.
    pub fn visualizer_panel(&mut self, ui: &mut Ui) {
        let prefs = self.settings.viz;
        let panel = match prefs.dock {
            Dock::Bottom => egui::Panel::bottom("viz-bottom"),
            Dock::Top => egui::Panel::top("viz-top"),
            Dock::Left => egui::Panel::left("viz-left"),
            Dock::Right => egui::Panel::right("viz-right"),
            Dock::Window => return,
        };
        let size = if matches!(prefs.dock, Dock::Left | Dock::Right) { 260.0 } else { 150.0 };
        let mut open = prefs.show;
        let frame = egui::Frame::new().fill(self.theme.bg_panel);
        panel.frame(frame).resizable(true).default_size(size).size_range(90.0..=640.0).show_collapsible(ui, &mut open, |ui| self.visualizer(ui));
        if open != prefs.show {
            self.settings.viz.show = open;
        }
    }

    /// The visualizer floating in its own window.
    pub fn visualizer_window(&mut self, ctx: &egui::Context) {
        if !(self.settings.viz.show && self.settings.viz.dock == Dock::Window) {
            return;
        }
        let t = self.theme.clone();
        egui::Window::new("Visualizer")
            .id(Id::unique("viz-window"))
            .title_bar(false)
            .frame(kit::window_frame(ctx, &t))
            .resizable(true)
            .default_size([560.0, 220.0])
            .default_pos(pos2(320.0, 220.0))
            .min_size([240.0, 120.0])
            .show(ctx, |ui| self.visualizer(ui));
    }

    fn visualizer(&mut self, ui: &mut Ui) {
        let t = self.theme.clone();
        let prefs = &mut self.settings.viz;
        kit::region(ui);
        kit::bar(ui, &t, |ui| {
            if ui.available_width() > 320.0 {
                ui.label(RichText::new(format!("{}  {}", icons::WAVEFORM, tr!("Visualizer"))).strong().color(t.fg_text));
            }
            kit::right(ui, |ui| {
                if kit::icon_button(ui, &t, icons::X, tr!("Close"), false).clicked() {
                    prefs.show = false;
                }
                let r = kit::icon_button(ui, &t, icons::LAYOUT, tr!("Where it goes"), false);
                kit::menu(&t, &r).show(|ui| {
                    for (dock, icon, label) in DOCKS {
                        if ui.selectable_label(prefs.dock == dock, format!("{icon}  {}", crate::i18n::t(label))).clicked() {
                            prefs.dock = dock;
                            ui.close();
                        }
                    }
                });
                kit::vsep(ui, &t);
                for (mode, icon, label) in [(Mode::Scope, icons::WAVE_SINE, tr!("Scope")), (Mode::Spectrum, icons::CHART_LINE, tr!("Spectrum")), (Mode::Shader, icons::SPARKLE, tr!("Light show"))] {
                    if kit::icon_button(ui, &t, icon, label, prefs.mode == mode).clicked() {
                        prefs.mode = mode;
                    }
                }
            });
        });
        let now = ui.input(|i| i.time);
        let mode = self.settings.viz.mode;
        self.viz.update(self.engine.as_ref(), self.settings.volume, now);
        let r = self.viz.draw(ui, &t, mode);
        if r.clicked() {
            self.settings.viz.mode = match mode {
                Mode::Shader => Mode::Spectrum,
                Mode::Spectrum => Mode::Scope,
                Mode::Scope => Mode::Shader,
            };
        }
        self.hint(&r, tr!("The mix as it leaves the master. Click: the next look"));
        if mode == Mode::Shader && self.viz.gpu {
            // the light keeps flowing, slower while nothing plays
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(if self.viz.moving() { 16 } else { 33 }));
        } else if self.viz.moving() {
            ui.ctx().request_repaint();
        }
    }
}

// ---------------------------------------------------------------------------
// The shader on the GPU: two textures in turn (one is drawn into while the other, last
// frame, is read), a pipeline drawing into them and one drawing the result on screen.
// ---------------------------------------------------------------------------

/// Lives in egui's renderer between frames.
struct Gpu {
    feedback: wgpu::RenderPipeline,
    display: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniforms: wgpu::Buffer,
    sampler: wgpu::Sampler,
    /// The screen takes linear colour.
    srgb: bool,
    targets: Option<Targets>,
}

struct Targets {
    size: [u32; 2],
    views: [wgpu::TextureView; 2],
    /// Reading texture 0 or 1.
    reads: [wgpu::BindGroup; 2],
    /// The one drawn into last.
    shown: usize,
}

/// Builds the pipelines into egui's renderer. Call once, when the app starts.
pub fn install(state: &egui_wgpu::RenderState) {
    let device = &state.device;
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("visualizer"), source: wgpu::ShaderSource::Wgsl(include_str!("visualizer.wgsl").into()) });
    let entry = |binding, ty| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT, ty, count: None };
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("visualizer"),
        entries: &[
            entry(0, wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }),
            entry(1, wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }),
            entry(2, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("visualizer"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
    let pipeline = |fragment: &str, format| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(fragment),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some(fragment),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        })
    };
    let gpu = Gpu {
        feedback: pipeline("fs_feedback", FEEDBACK_FORMAT),
        display: pipeline("fs_display", state.target_format),
        uniforms: device.create_buffer(&wgpu::BufferDescriptor { label: Some("visualizer"), size: UNIFORM_FLOATS as u64 * 4, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }),
        sampler: device.create_sampler(&wgpu::SamplerDescriptor { mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default() }),
        srgb: state.target_format.is_srgb(),
        layout,
        targets: None,
    };
    state.renderer.write().callback_resources.insert(gpu);
}

/// Light adds up past 1 between frames: half floats keep it.
const FEEDBACK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const UNIFORM_FLOATS: usize = 32 + BARS;

impl Targets {
    /// Textures the size of the picture in `slot` (new ones, black, when it changes).
    fn sized<'a>(slot: &'a mut Option<Targets>, device: &wgpu::Device, layout: &wgpu::BindGroupLayout, uniforms: &wgpu::Buffer, sampler: &wgpu::Sampler, size: [u32; 2]) -> &'a mut Targets {
        if slot.as_ref().is_none_or(|t| t.size != size) {
            let view = || {
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("visualizer"),
                    size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: FEEDBACK_FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                });
                texture.create_view(&wgpu::TextureViewDescriptor::default())
            };
            let views = [view(), view()];
            let read = |view: &wgpu::TextureView| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("visualizer"),
                    layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(view) },
                        wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(sampler) },
                    ],
                })
            };
            let reads = [read(&views[0]), read(&views[1])];
            *slot = Some(Targets { size, views, reads, shown: 0 });
        }
        slot.as_mut().expect("just made")
    }
}

/// One frame of the shader: its uniforms and the size to draw at.
struct Frame {
    uniforms: Vec<f32>,
    size: [u32; 2],
}

impl CallbackTrait for Frame {
    fn prepare(&self, device: &wgpu::Device, queue: &wgpu::Queue, _screen: &ScreenDescriptor, encoder: &mut wgpu::CommandEncoder, resources: &mut CallbackResources) -> Vec<wgpu::CommandBuffer> {
        let Some(Gpu { feedback, layout, uniforms, sampler, srgb, targets, .. }) = resources.get_mut::<Gpu>() else { return Vec::new() };
        let mut values = self.uniforms.clone();
        values[28] = if *srgb { 1.0 } else { 0.0 };
        let bytes: Vec<u8> = values.iter().flat_map(|x| x.to_ne_bytes()).collect();
        queue.write_buffer(uniforms, 0, &bytes);
        let targets = Targets::sized(targets, device, layout, uniforms, sampler, self.size);
        // draw into the one not shown last frame, reading that one
        let (into, from) = (1 - targets.shown, targets.shown);
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("visualizer feedback"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &targets.views[into],
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(feedback);
            pass.set_bind_group(0, &targets.reads[from], &[]);
            pass.draw(0..3, 0..1);
        }
        targets.shown = into;
        Vec::new()
    }

    fn paint(&self, _info: egui::PaintCallbackInfo, pass: &mut wgpu::RenderPass<'static>, resources: &CallbackResources) {
        let Some(gpu) = resources.get::<Gpu>() else { return };
        let Some(targets) = &gpu.targets else { return };
        pass.set_pipeline(&gpu.display);
        pass.set_bind_group(0, &targets.reads[targets.shown], &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    /// The shader compiles: a mistake in it would otherwise only show when the app starts.
    #[test]
    fn shader_is_valid_wgsl() {
        let module = naga::front::wgsl::parse_str(include_str!("visualizer.wgsl")).unwrap_or_else(|e| panic!("{}", e.emit_to_string(include_str!("visualizer.wgsl"))));
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty()).validate(&module).expect("valid");
        let u = module.types.iter().find(|(_, t)| t.name.as_deref() == Some("U")).expect("struct U").1;
        assert_eq!(u.inner.size(module.to_ctx()) as usize, super::UNIFORM_FLOATS * 4, "the uniforms Rust writes fill struct U");
    }
}
