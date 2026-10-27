use crate::store::model::NodeId;
use std::collections::{HashMap, HashSet};
/// The present mode asked for: `Immediate` shows a frame without queueing it behind a vsync (key to screen 33 ms against
/// 100 for `Fifo`, README "View latency, measured"); the event loop then caps its own frame rate (`GpuState::present_blocks`).
/// The surface falls back to `Fifo`, which every backend supports, when it lacks the mode.
const PREFERRED_PRESENT_MODE: wgpu::PresentMode = wgpu::PresentMode::Immediate;

use std::sync::Arc;

use wgpu::util::DeviceExt;
use winit::window::Window;

mod image_cache;

/// `GpuState::render`'s result -- lets the caller tell a truly-failed
/// screenshot save apart from one that was never attempted this frame
/// (surface unusable), so a request isn't silently dropped just because it
/// landed on a transient `Outdated`/`Occluded` frame.
/// A pane's rectangle in physical pixels: x, y, width, height.
pub type PxRect = (f32, f32, f32, f32);

/// One content quad: the whole texture is stretched over `viewport` (which may extend past the
/// window), and only `clip` of it shows. They are equal except in a Tab-view transition, where
/// `clip` is the card's thumbnail area and `viewport` is the page scaled to cover it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanePx {
    pub viewport: PxRect,
    pub clip: PxRect,
}

impl PanePx {
    /// A pane with nothing cropped.
    pub fn whole(rect: PxRect) -> Self {
        Self { viewport: rect, clip: rect }
    }
}

/// A surface status a debug command makes the next frame see.
#[derive(Clone, Copy)]
pub(crate) enum SurfaceFault {
    Outdated,
    Lost,
}

impl SurfaceFault {
    pub(crate) fn parse(word: &str) -> Option<Self> {
        match word {
            "outdated" => Some(Self::Outdated),
            "lost" => Some(Self::Lost),
            _ => None,
        }
    }
}

pub struct RenderOutcome {
    pub surface_ok: bool,
    /// `Some` only when a screenshot was requested AND `surface_ok` --
    /// `Ok(())` once the PNG is actually written to disk, `Err` with a
    /// message otherwise.
    pub screenshot_result: Option<Result<(), String>>,
}

const SHADER: &str = r#"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOutput {
    // Fullscreen triangle, no vertex buffer needed.
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    var out: VertexOutput;
    let p = positions[idx];
    out.position = vec4<f32>(p, 0.0, 1.0);
    out.uv = vec2<f32>((p.x + 1.0) * 0.5, 1.0 - (p.y + 1.0) * 0.5);
    return out;
}

@group(0) @binding(0) var tab_texture: texture_2d<f32>;
@group(0) @binding(1) var tab_sampler: sampler;

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(tab_texture, tab_sampler, in.uv);
}

// GPU replacement for `downscale_rgba`'s own CPU box filter (see
// `GpuState::render_thumbnail`'s doc comment for the full story). A plain
// single-tap `textureSample` per output texel would alias badly at the
// downscale ratios a full-resolution tab frame needs (commonly 4-8x per
// axis) -- a true box filter has to average every source texel that maps
// into one output texel, not just interpolate the 4 nearest ones. Emulated
// here as a fixed 4x4 grid of *bilinear-filtered* taps spread across that
// output texel's own footprint in source UV space: cheap (16 taps, a
// compile-time-constant loop, no dynamic branching) and close enough to a
// true box average for a thumbnail nobody examines pixel-by-pixel.
// `box_size` is one output texel's width/height, in the *source* texture's
// own [0,1] UV space: how far `in.uv` moves per output pixel, which the
// derivatives give directly (`1.0 / thumbnail_size` for the full-screen
// triangle), so no uniform is needed. Taken before the loop, in uniform
// control flow.
const DOWNSAMPLE_TAPS: i32 = 4;

@fragment
fn fs_downsample(in: VertexOutput) -> @location(0) vec4<f32> {
    let box_size = abs(vec2<f32>(dpdx(in.uv).x, dpdy(in.uv).y));
    var sum = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    for (var iy = 0; iy < DOWNSAMPLE_TAPS; iy = iy + 1) {
        for (var ix = 0; ix < DOWNSAMPLE_TAPS; ix = ix + 1) {
            let grid = (vec2<f32>(f32(ix), f32(iy)) + vec2<f32>(0.5, 0.5)) / f32(DOWNSAMPLE_TAPS);
            let offset = (grid - vec2<f32>(0.5, 0.5)) * box_size;
            sum = sum + textureSample(tab_texture, tab_sampler, in.uv + offset);
        }
    }
    return sum / f32(DOWNSAMPLE_TAPS * DOWNSAMPLE_TAPS);
}
"#;

/// Graph-view thumbnails never need to exceed roughly `GRAPH_NODE_SIZE`
/// (see `main.rs`) on screen — cap well above that so it still looks sharp
/// at the enlarged peek-preview size too, without paying full content
/// resolution.
const THUMBNAIL_MAX_SIZE: (u32, u32) = (256, 192);

/// The non-sRGB counterpart of an sRGB texture format, for `render_
/// thumbnail`'s own reinterpreting source view (see that function's doc
/// comment for why). Identity for a format that's already non-sRGB --
/// `upload_frame` only ever receives the two sRGB formats named here (see
/// its own doc comment on `format`), but this stays total rather than
/// panicking on anything else, since a texture *view*'s own format list
/// happily includes a format equal to the texture's own.
fn non_srgb_view_format(format: wgpu::TextureFormat) -> wgpu::TextureFormat {
    match format {
        wgpu::TextureFormat::Bgra8UnormSrgb => wgpu::TextureFormat::Bgra8Unorm,
        wgpu::TextureFormat::Rgba8UnormSrgb => wgpu::TextureFormat::Rgba8Unorm,
        other => other,
    }
}

/// One visible tab's frame as a GPU texture for the hand-rolled content
/// pipeline. Keyed by tab id in `GpuState::tab_textures` -- split view needs
/// two of these live at once (the primary and secondary pane), so a single
/// slot does not suffice. `texture`/`width`/`height`/`format` are
/// kept (not just the `bind_group`) so `upload_frame` can tell whether a new
/// frame's dimensions match this one closely enough to reuse it in place via
/// `queue.write_texture` instead of allocating a brand new GPU texture --
/// see that function's own doc comment.
struct TabTexture {
    texture: wgpu::Texture,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    bind_group: wgpu::BindGroup,
    /// A frame for this tab was skipped or dropped, so the next upload must write the whole texture.
    stale: bool,
}

/// A tab's cached frame re-uploaded as a plain `Rgba8Unorm` texture and
/// registered with `egui_wgpu::Renderer`, so the M5 graph view can draw it
/// as an ordinary `egui::Image`/`painter().image()` call. This is *separate*
/// from `TabTexture` above because `egui_wgpu::Renderer::register_native_
/// texture` requires exactly `Rgba8Unorm` — not the sRGB/BGRA formats the
/// hand-rolled content pipeline uses — so CEF's BGRA8 frames need a channel
/// swap on the way in (see `to_thumbnail_rgba`). Persists across focus
/// changes (unlike `TabTexture`) so a suspended tab's node keeps showing
/// its last frame. `texture`/`width`/`height` kept for the same "can this
/// be reused in place via `queue.write_texture`" check `TabTexture` does --
/// `egui_id` itself, once registered, never needs to change as long as the
/// texture it's registered against is only *updated*, not replaced.
struct ThumbTexture {
    texture: wgpu::Texture,
    width: u32,
    height: u32,
    egui_id: egui::TextureId,
}

/// The backends the instance loads unless `WGPU_BACKEND` says otherwise.
#[cfg(windows)]
const DEFAULT_BACKENDS: wgpu::Backends = wgpu::Backends::DX12;
#[cfg(not(windows))]
const DEFAULT_BACKENDS: wgpu::Backends = wgpu::Backends::PRIMARY;
pub struct GpuState {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// See `render_thumbnail`'s own doc comment.
    downsample_pipeline: wgpu::RenderPipeline,
    downsample_bind_group_layout: wgpu::BindGroupLayout,
    /// Keyed by tab id -- see `TabTexture`'s own doc comment for why this
    /// isn't a single slot. Bounded by however many tabs are currently
    /// visible (at most two, focused + split partner) rather than every tab
    /// ever focused: entries are removed via `free_tab_texture` whenever a
    /// tab stops being visible (unfocused, split closed, or closed
    /// outright).
    tab_textures: HashMap<NodeId, TabTexture>,
    thumbnails: HashMap<NodeId, ThumbTexture>,
    /// Tabs whose thumbnail is older than their frame texture. `upload_frame` adds a tab; `render` downsamples it
    /// in the frame's encoder when egui draws its thumbnail; `free_tab_texture` flushes it before the frame is
    /// freed; `free_tab_thumbnail` drops it.
    thumbs_dirty: HashSet<NodeId>,
    /// Keyed by *origin* (`scheme://host[:port]`, see `favicon_fetch::
    /// UrlParts::origin`), not tab id -- favicons are fetched independently of
    /// any specific tab (see `favicon_fetch`'s own module doc comment), so
    /// two tabs on the same site share one texture instead of each
    /// re-fetching/re-uploading their own. `favicon_order` tracks insertion
    /// order for `MAX_CACHED_FAVICONS` FIFO eviction -- not tied to tab
    /// close the way `thumbnails` is, since an origin can outlive every tab
    /// that visited it.
    favicons: HashMap<String, ThumbTexture>,
    favicon_order: std::collections::VecDeque<String>,
    /// Keyed by *node id*, unlike `favicons` -- an image node's own file is
    /// decoded at creation, restore or Refresh (see `App::insert_content`),
    /// and each node is its own independent picture even if two
    /// nodes happen to point at the same file, so there's no origin-style
    /// sharing to do and no unbounded-growth risk needing `favicons`' own
    /// FIFO eviction -- bounded by however many image nodes the user
    /// deliberately creates, freed one-for-one via `free_image` when a node
    /// is removed.
    images: HashMap<NodeId, ThumbTexture>,
    egui_renderer: egui_wgpu::Renderer,
    /// Kept to recreate `surface` after `CurrentSurfaceTexture::Lost`.
    instance: wgpu::Instance,
    window: Arc<Window>,
    /// The next `render` reconfigures before acquiring (set after `Suboptimal`).
    reconfigure: bool,
    /// The status the next `render` takes in place of the surface's (debug `SURFACE`), so the recovery branches below
    /// run without a replug or a display change.
    forced: Option<SurfaceFault>,
    /// How many times an `Outdated` surface was reconfigured or a `Lost` one recreated.
    pub(crate) recoveries: u32,
    /// Set by the device-lost callback; read once by `take_device_lost`.
    device_lost: Arc<std::sync::Mutex<Option<String>>>,
}

/// GPU thumbnail downsample -- see `GpuState::render_thumbnail`'s own
/// doc comment. A separate bind group layout/pipeline from the one
/// the tab pipeline: a fixed `Rgba8Unorm` target format instead of the
/// surface's own `config.format` (the thumbnail texture egui_wgpu
/// registers must be exactly `Rgba8Unorm`, never sRGB -- see
/// `ThumbTexture`'s own doc comment).
fn downsample_pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
) -> (wgpu::BindGroupLayout, wgpu::RenderPipeline) {
    let downsample_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("thumbnail downsample bind group layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let downsample_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("thumbnail downsample pipeline layout"),
        bind_group_layouts: &[Some(&downsample_bind_group_layout)],
        immediate_size: 0,
    });
    let downsample_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("thumbnail downsample pipeline"),
        layout: Some(&downsample_pipeline_layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_downsample"),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba8Unorm,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });
    (downsample_bind_group_layout, downsample_pipeline)
}

impl GpuState {
    /// Whether presenting blocks until the display takes the frame (`Fifo`); if not, the redraw loop paces itself.
    pub fn present_blocks(&self) -> bool {
        self.config.present_mode == wgpu::PresentMode::Fifo
    }

    pub fn new(window: Arc<Window>) -> Self {
        let size = window.inner_size();
        // `backends` explicitly restricted to `PRIMARY` (Vulkan/Metal/DX12/
        // WebGPU) rather than `new_without_display_handle()`'s own default
        // of `Backends::all()`, which also includes `GL` (on Windows, an
        // ANGLE-backed secondary/legacy tier). Found live: this app's own
        // `Instance::new()` was taking ~640ms on its own -- over 40% of a
        // ~1.5s total time-to-first-frame -- and is the single largest
        // contributor to a "extreme lag on startup" report, confirmed via
        // temporary per-phase timing instrumentation through the rest of
        // `GpuState::new`/`resumed()`. Loading/probing an extra backend
        // tier this app never needs (it only ever ends up on Vulkan or
        // DX12 in practice, per `adapter.get_info()`) is pure waste --
        // `PRIMARY` is wgpu's own recommended set for exactly this reason.
        // On Windows DX12 is the default: frames were 15-30% shorter than on Vulkan in every repeat at 400 cards
        // (`PARKED.md`, "Default backend", runs `2026-10-22-ab3-*`). Elsewhere `PRIMARY` is the platform's own (Metal,
        // Vulkan). `WGPU_BACKEND` (`vulkan`, `dx12`) overrides it.
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::from_env().unwrap_or(DEFAULT_BACKENDS),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let surface = instance.create_surface(window.clone()).expect("create wgpu surface");

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .expect("request wgpu adapter");
        let wgpu::AdapterInfo { name, backend, .. } = adapter.get_info();
        eprintln!("gpu: {name} ({backend:?})");

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("manifold device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::default(),
            trace: wgpu::Trace::Off,
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
        }))
        .expect("request wgpu device");
        // wgpu's default handler panics. An error here (an oversize texture
        // that slipped past `register_rgba`, a driver hiccup) should cost a
        // frame, not the app and its unsaved state.
        device.on_uncaptured_error(Arc::new(|err: wgpu::Error| eprintln!("wgpu error: {err}")));
        let device_lost = Arc::new(std::sync::Mutex::new(None));
        let lost = device_lost.clone();
        device.set_device_lost_callback(move |reason, message| {
            eprintln!("wgpu device lost ({reason:?}): {message}");
            // Also fired when the app drops its own device at exit.
            if matches!(reason, wgpu::DeviceLostReason::Destroyed) {
                return;
            }
            *lost.lock().unwrap_or_else(|e| e.into_inner()) = Some(format!("{message} ({reason:?})"));
        });

        let surface_caps = surface.get_capabilities(&adapter);
        let surface_format =
            surface_caps.formats.iter().copied().find(|f| f.is_srgb()).unwrap_or(surface_caps.formats[0]);

        let present_mode = if surface_caps.present_modes.contains(&PREFERRED_PRESENT_MODE) {
            PREFERRED_PRESENT_MODE
        } else {
            wgpu::PresentMode::Fifo
        };
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            // Deliberately *not* also `COPY_SRC` here: some backends'
            // swapchain-presentable textures don't support arbitrary extra
            // usage flags (surface capability negotiation can reject or
            // silently misbehave). Screenshot readback (`render`'s
            // `screenshot_path`) instead re-renders into a second, ordinary
            // (non-surface) texture — those have no such restriction — so
            // this stays exactly what it was before screenshots existed.
            format: surface_format,
            width: size.width.max(1),
            height: size.height.max(1),
            // Named explicitly, not `surface_caps.present_modes[0]`: wgpu doesn't
            // guarantee that ordering puts the mode we want first. Only `Fifo`
            // blocks in present, so the winit loop (`ControlFlow::Poll`, a
            // continuous redraw -- see `App`'s own doc comment) throttles itself
            // for any other mode (`FramePacing::frame_interval`); without that
            // it would present uncapped.
            present_mode,
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
            // One frame queued, not two: key-to-screen measured 99-100 ms against 100 or 116 ms (scripts/keylat.ps1, README).
            desired_maximum_frame_latency: 1,
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(&device, &config);

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("tab texture bind group layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("tab sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("tab shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("tab pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("tab pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let (downsample_bind_group_layout, downsample_pipeline) = downsample_pipeline(&device, &shader);

        let egui_renderer = egui_wgpu::Renderer::new(&device, config.format, egui_wgpu::RendererOptions::default());

        Self {
            surface,
            device,
            queue,
            config,
            pipeline,
            bind_group_layout,
            sampler,
            downsample_pipeline,
            downsample_bind_group_layout,
            tab_textures: HashMap::new(),
            thumbnails: HashMap::new(),
            thumbs_dirty: HashSet::new(),
            favicons: HashMap::new(),
            favicon_order: std::collections::VecDeque::new(),
            images: HashMap::new(),
            egui_renderer,
            instance,
            window,
            reconfigure: false,
            forced: None,
            recoveries: 0,
            device_lost,
        }
    }

    /// The device-lost message, once: the caller toasts it.
    pub fn take_device_lost(&self) -> Option<String> {
        self.device_lost.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    /// Reconfigures the surface at the window's current size. Used when wgpu
    /// reports the swapchain stale without a winit `Resized` having arrived,
    /// so `config`'s size may itself be stale; a zero size (minimized) keeps
    /// the last one, as `resize` does.
    fn reconfigure_surface(&mut self) {
        let size = self.window.inner_size();
        if size.width > 0 && size.height > 0 {
            self.config.width = size.width;
            self.config.height = size.height;
        }
        self.surface.configure(&self.device, &self.config);
    }

    /// Makes the next frame see `fault` instead of the surface's own status.
    pub(crate) fn force_surface_fault(&mut self, fault: SurfaceFault) {
        self.forced = Some(fault);
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
    }

    /// `pixels` must be exactly `width * height * 4` bytes in whatever pixel
    /// order `format` names — Servo's `read_to_image()` produces RGBA8
    /// (`Rgba8UnormSrgb`), CEF's OSR `on_paint` produces BGRA8
    /// (`Bgra8UnormSrgb`). Passing the wrong format silently swaps red/blue.
    ///
    /// Re-uploads a live tab frame plus its graph-view thumbnail.
    ///
    /// **Reuses last frame's GPU texture in place (`queue.write_texture`)
    /// whenever this frame's size/format match it, instead of allocating a
    /// brand new `wgpu::Texture` (and, for the main frame, a new bind group)
    /// every single call.** Found on a real, reported "the app just feels
    /// slow" investigation: `device.create_texture_with_data` here was the
    /// dominant per-frame cost by a wide margin (confirmed via temporary
    /// timing instrumentation) -- allocating a fresh GPU resource from
    /// scratch on *every* uploaded frame, for a live tab that's continuously
    /// producing same-sized frames the overwhelming majority of the time
    /// (a resize is the only thing that actually changes dimensions). Only
    /// the main frame's own `bind_group` still needs recreating alongside a
    /// genuinely new texture (it references the texture's view directly);
    /// the thumbnail's `egui_id`, once registered, is instead just re-
    /// pointed at a replacement texture via `update_egui_texture_from_wgpu_
    /// texture` when a new one is unavoidable, so callers holding onto that
    /// id never see it silently change.
    ///
    /// `rows` is the band of rows that changed since the previous upload (`None`: all of them). Only a texture that
    /// exists, is not `stale` and keeps its size takes just that band.
    pub fn upload_frame(
        &mut self,
        tab_id: NodeId,
        pixels: &[u8],
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
        rows: Option<std::ops::Range<u32>>,
    ) {
        // The size comes from an view-host (over TCP for WebKit): a frame that isn't
        // `width * height * 4` bytes, or is empty or past the device limit, would panic in wgpu.
        if !frame_is_valid(pixels.len(), width, height, self.device.limits().max_texture_dimension_2d) {
            eprintln!("gpu: dropped a {width}x{height} frame of {} bytes for tab {tab_id}", pixels.len());
            self.mark_stale(tab_id);
            return;
        }
        let reuse_main = self
            .tab_textures
            .get(&tab_id)
            .is_some_and(|t| t.width == width && t.height == height && t.format == format);
        if reuse_main {
            let tab = self.tab_textures.get_mut(&tab_id).expect("reuse_main found it");
            let rows = if tab.stale { None } else { rows };
            tab.stale = false;
            Self::write_rows(&self.queue, &tab.texture, pixels, width, height, rows);
        } else {
            // `view_formats` includes this format's own non-sRGB
            // counterpart -- wgpu requires every format a texture will
            // ever be *viewed* as (not just created as) to be declared up
            // front here; `render_thumbnail` creates exactly such a
            // reinterpreting view of this same texture every time it
            // downsamples this tab's thumbnail.
            let texture = self.device.create_texture_with_data(
                &self.queue,
                &wgpu::TextureDescriptor {
                    label: Some("tab frame texture"),
                    size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[non_srgb_view_format(format)],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                pixels,
            );
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("tab texture bind group"),
                layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                ],
            });
            self.tab_textures.insert(tab_id, TabTexture { texture, width, height, format, bind_group, stale: false });
        }

        let (thumb_width, thumb_height) = Self::thumbnail_target_size(width, height, THUMBNAIL_MAX_SIZE);
        let reuse_thumb =
            self.thumbnails.get(&tab_id).is_some_and(|t| t.width == thumb_width && t.height == thumb_height);
        if !reuse_thumb {
            let thumb_texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("tab thumbnail texture"),
                size: wgpu::Extent3d { width: thumb_width, height: thumb_height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            let thumb_view = thumb_texture.create_view(&wgpu::TextureViewDescriptor::default());
            let egui_id = if let Some(existing) = self.thumbnails.get(&tab_id) {
                self.egui_renderer.update_egui_texture_from_wgpu_texture(
                    &self.device,
                    &thumb_view,
                    wgpu::FilterMode::Linear,
                    existing.egui_id,
                );
                existing.egui_id
            } else {
                self.egui_renderer.register_native_texture(&self.device, &thumb_view, wgpu::FilterMode::Linear)
            };
            self.thumbnails.insert(
                tab_id,
                ThumbTexture { texture: thumb_texture, width: thumb_width, height: thumb_height, egui_id },
            );
        }
        // The thumbnail's content is older than this frame whether its texture was just reused or just created;
        // `render` downsamples it when egui draws it.
        self.thumbs_dirty.insert(tab_id);
    }

    /// The aspect-preserving target size `render_thumbnail`'s destination
    /// texture is created at -- pure arithmetic, split out from the old
    /// CPU `downscale_rgba` (which combined this with the actual pixel
    /// averaging) so `upload_frame` can decide whether a same-size
    /// thumbnail texture already exists before doing any GPU work at all.
    /// Never upscales — a tab smaller than the cap is left at its own size.
    fn thumbnail_target_size(width: u32, height: u32, max_size: (u32, u32)) -> (u32, u32) {
        let (max_w, max_h) = max_size;
        if width <= max_w && height <= max_h {
            return (width, height);
        }
        let scale = (max_w as f32 / width as f32).min(max_h as f32 / height as f32);
        let out_w = ((width as f32 * scale).round() as u32).max(1);
        let out_h = ((height as f32 * scale).round() as u32).max(1);
        (out_w, out_h)
    }

    /// Downsamples `tab_id`'s already-uploaded main frame texture directly into its thumbnail texture via a render
    /// pass (`fs_downsample` in `SHADER`), with no pixel download to the CPU.
    ///
    /// - **sRGB.** The main texture is `Bgra8UnormSrgb`/`Rgba8UnormSrgb` (see `upload_frame`'s doc on `format`),
    ///   and sampling an sRGB texture linearizes the color. Writing that into the thumbnail's plain (non-sRGB)
    ///   `Rgba8Unorm` target would store linear-light bytes where gamma-encoded ones belong. So the pass samples
    ///   through a *reinterpreting* view in the non-sRGB counterpart format (`Bgra8Unorm`/`Rgba8Unorm`, via
    ///   `non_srgb_view_format`), which returns the stored bytes unconverted. That needs the main texture's
    ///   `view_formats` to declare the counterpart at creation time (see `upload_frame`).
    /// - **BGRA vs RGBA.** Not handled here: `textureSample` returns `(r,g,b,a)` order whatever the bound
    ///   texture's channel layout, so no byte swap is needed.
    ///
    /// Skipping sRGB decoding (rather than decoding, then re-encoding) leaves the thumbnail very slightly washed
    /// out compared to the live content pipeline. That gap is accepted.
    ///
    /// Records into the caller's `encoder` (no submit of its own): `render` flushes the thumbnails egui draws that
    /// frame, `free_tab_texture` the one about to lose its source.
    fn render_thumbnail(
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        pipeline: &wgpu::RenderPipeline,
        layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        source: &TabTexture,
        dest: &ThumbTexture,
    ) {
        let source_view = source.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(non_srgb_view_format(source.format)),
            ..Default::default()
        });
        let dest_view = dest.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("thumbnail downsample bind group"),
            layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&source_view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("thumbnail downsample pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &dest_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Uploads `rows` (all `height` rows when `None`, or when the range reaches past the frame) of tightly-packed
    /// `pixels` (no row padding -- shared-memory frame buffers and inline IPC pixels are both exactly
    /// `width * height * 4` bytes) into `texture` in place, for `upload_frame`'s "dimensions unchanged since last
    /// time" path.
    fn write_rows(
        queue: &wgpu::Queue,
        texture: &wgpu::Texture,
        pixels: &[u8],
        width: u32,
        height: u32,
        rows: Option<std::ops::Range<u32>>,
    ) {
        let rows = rows.filter(|r| r.start < r.end && r.end <= height).unwrap_or(0..height);
        let row_bytes = u64::from(width) * 4;
        let count = rows.end - rows.start;
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x: 0, y: rows.start, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            pixels,
            wgpu::TexelCopyBufferLayout {
                offset: u64::from(rows.start) * row_bytes,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(count),
            },
            wgpu::Extent3d { width, height: count, depth_or_array_layers: 1 },
        );
    }

    /// Makes `tab_id`'s next upload write the whole texture: a frame for it was skipped, so the rows that frame
    /// changed never reached the texture.
    pub fn mark_stale(&mut self, tab_id: NodeId) {
        if let Some(tab) = self.tab_textures.get_mut(&tab_id) {
            tab.stale = true;
        }
    }

    /// The `egui::TextureId` for a tab's cached thumbnail, if it has ever
    /// produced a frame — for the graph view to draw via `egui::Image`.
    pub fn thumbnail_id(&self, tab_id: NodeId) -> Option<egui::TextureId> {
        self.thumbnails.get(&tab_id).map(|t| t.egui_id)
    }

    /// A tab's thumbnail size in pixels, for cropping it to a card (see `cover_uv`).
    pub fn thumbnail_size(&self, tab_id: NodeId) -> Option<(u32, u32)> {
        self.thumbnails.get(&tab_id).map(|t| (t.width, t.height))
    }

    /// Must be called when a tab closes — `egui_wgpu::Renderer` doesn't
    /// know about manually-registered native textures on its own, so
    /// unlike egui's own font/image-loader textures (freed automatically
    /// via each frame's `textures_delta.free`, see `render`), this one
    /// leaks until explicitly freed here.
    pub fn free_tab_thumbnail(&mut self, tab_id: NodeId) {
        self.thumbs_dirty.remove(&tab_id);
        if let Some(thumb) = self.thumbnails.remove(&tab_id) {
            self.egui_renderer.free_texture(&thumb.egui_id);
        }
        // Favicons are origin-keyed, not tab-keyed (see `favicons`' own doc
        // comment) -- closing this one tab says nothing about whether some
        // other tab still shows the same origin, so there's no per-tab
        // free to do here. `upload_favicon`'s own `MAX_CACHED_FAVICONS`
        // eviction is what eventually reclaims an origin nothing visits
        // anymore.
    }

    /// Drops `tab_id`'s own content-pipeline texture (see `tab_textures`'
    /// own doc comment) -- called whenever a tab stops being visible
    /// (unfocused, a split closed, or the tab closes outright) so a
    /// suspended tab's last frame doesn't sit on the GPU indefinitely for no
    /// reason (unlike `thumbnails`, which deliberately persists past
    /// suspension so a graph node keeps showing its last frame).
    ///
    /// A dirty thumbnail is downsampled first (own encoder and submit): the tab's last frame must reach its card.
    /// The close and sleep paths call `free_tab_thumbnail` before this, so they never flush one about to be freed.
    pub fn free_tab_texture(&mut self, tab_id: NodeId) {
        if self.thumbs_dirty.remove(&tab_id) {
            if let (Some(source), Some(dest)) = (self.tab_textures.get(&tab_id), self.thumbnails.get(&tab_id)) {
                let mut encoder = self
                    .device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("thumbnail flush encoder") });
                Self::render_thumbnail(
                    &mut encoder,
                    &self.device,
                    &self.downsample_pipeline,
                    &self.downsample_bind_group_layout,
                    &self.sampler,
                    source,
                    dest,
                );
                self.queue.submit(std::iter::once(encoder.finish()));
            }
        }
        self.tab_textures.remove(&tab_id);
    }

    /// Whether `upload_frame` has ever run for this tab -- `false` right
    /// after a fresh spawn/respawn, until its view-host's first real
    /// frame arrives. Lets Tab view show a loading spinner instead of the
    /// plain black clear `record_content_pass` draws for a pane with no
    /// texture yet (see that function's own doc comment).
    pub fn has_tab_texture(&self, tab_id: NodeId) -> bool {
        self.tab_textures.contains_key(&tab_id)
    }

    /// Draws every entry in `panes` (0 for Graph view/nothing focused yet,
    /// 1 for ordinary Tab view, 2 for a split) as its own viewport-clipped
    /// content-pipeline quad -- `record_content_pass`'s own fullscreen-
    /// triangle-plus-`set_viewport` trick already confines each draw to its
    /// own rect, so compositing more than one is just calling it more than
    /// once with a different rect/texture, no scissor-rect bookkeeping
    /// needed. Only the first entry actually clears the color attachment
    /// (`clear`, matching whatever the caller would have passed for a
    /// single pane); every later one loads, so it doesn't erase the pane(s)
    /// already drawn this same pass. Still issues exactly one clearing pass
    /// even with zero panes -- something has to clear the screen before
    /// egui's own chrome/graph draws on top of it.
    fn record_content_layers(
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        panes: &[(PanePx, Option<&TabTexture>)],
        target: (u32, u32),
        pipeline: &wgpu::RenderPipeline,
        clear: Option<wgpu::Color>,
    ) {
        if panes.is_empty() {
            Self::record_content_pass(
                encoder,
                view,
                PanePx::whole((0.0, 0.0, 0.0, 0.0)),
                target,
                None,
                pipeline,
                clear,
            );
            return;
        }
        for (i, (pane, showing)) in panes.iter().enumerate() {
            Self::record_content_pass(encoder, view, *pane, target, *showing, pipeline, clear.filter(|_| i == 0));
        }
    }

    /// Renders one pane's content-pipeline quad, clipped to `viewport`
    /// (physical pixels: x, y, w, h) via `set_viewport` rather than a real
    /// scissor rect — the fullscreen-triangle trick this pipeline's own
    /// vertex shader uses only ever rasterizes within whatever viewport is
    /// currently set, so this is enough on its own to confine each of
    /// `record_content_layers`' calls to its own screen region. `showing` is
    /// `None` when that pane's own tab hasn't produced a frame yet (or its
    /// texture was already freed) — the render pass still runs (for its
    /// `clear` side effect), it just draws nothing. A free function (not a
    /// `&self` method) so `render` can call it against two different
    /// targets — the real swapchain view and, when a screenshot was
    /// requested, a second offscreen view — without the borrow checker
    /// treating `encoder`/`pipeline` as aliasing `self`.
    fn record_content_pass(
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        pane: PanePx,
        target: (u32, u32),
        showing: Option<&TabTexture>,
        pipeline: &wgpu::RenderPipeline,
        clear: Option<wgpu::Color>,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("tab render pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: clear.map_or(wgpu::LoadOp::Load, wgpu::LoadOp::Clear),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        if let Some(tab_texture) = showing {
            let (x, y, w, h) = pane.viewport;
            // `.max(1.0)`: wgpu validation rejects a zero/negative-size
            // viewport — a transition's interpolated rect should never
            // actually collapse to zero, but a mid-animation window resize
            // is cheap insurance against a transient degenerate frame.
            pass.set_viewport(x, y, w.max(1.0), h.max(1.0), 0.0, 1.0);
            // The scissor must lie inside the target; a clip wholly outside it draws nothing.
            let (cx, cy, cw, ch) = pane.clip;
            let (left, top) = (cx.max(0.0).min(target.0 as f32), cy.max(0.0).min(target.1 as f32));
            let (right, bottom) = ((cx + cw).max(0.0).min(target.0 as f32), (cy + ch).max(0.0).min(target.1 as f32));
            pass.set_scissor_rect(
                left as u32,
                top as u32,
                (right - left).max(0.0) as u32,
                (bottom - top).max(0.0) as u32,
            );
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &tab_texture.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    /// Records egui's already-tessellated draw output into `view` — see
    /// `record_content_pass`'s doc comment for why this takes explicit
    /// pieces instead of `&mut self`. Safe to call twice against two
    /// different targets with the same `clipped_primitives`/
    /// `screen_descriptor` (plain draw calls reading buffers already
    /// uploaded once by the caller before either call).
    fn record_egui_pass(
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        renderer: &mut egui_wgpu::Renderer,
        clipped_primitives: &[egui::ClippedPrimitive],
        screen_descriptor: &egui_wgpu::ScreenDescriptor,
        clear: Option<wgpu::Color>,
    ) {
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("egui render pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: clear.map_or(wgpu::LoadOp::Load, wgpu::LoadOp::Clear),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        // egui_wgpu::Renderer::render requires a 'static render pass;
        // forget_lifetime() is the documented way to get one from a pass
        // that in practice never outlives this function.
        renderer.render(&mut pass.forget_lifetime(), clipped_primitives, screen_descriptor);
    }

    /// Records one frame's layers into `view`: the content quads, then egui
    /// on top; or, during a Tab<->Graph transition (`content_on_top`), egui
    /// as the base and the shrinking/expanding content quad over it. The
    /// screen and a screenshot both draw through here, so they can't differ.
    #[allow(clippy::too_many_arguments, reason = "disjoint borrows of `GpuState` fields: `self` can't be passed whole")]
    fn record_scene(
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        panes: &[(PanePx, Option<&TabTexture>)],
        target: (u32, u32),
        pipeline: &wgpu::RenderPipeline,
        renderer: &mut egui_wgpu::Renderer,
        egui_draw: Option<&(Vec<egui::ClippedPrimitive>, egui_wgpu::ScreenDescriptor)>,
        content_on_top: bool,
        canvas: wgpu::Color,
    ) {
        match (egui_draw, content_on_top) {
            (Some((primitives, screen)), true) => {
                Self::record_egui_pass(encoder, view, renderer, primitives, screen, Some(canvas));
                Self::record_content_layers(encoder, view, panes, target, pipeline, None);
            }
            (Some((primitives, screen)), false) => {
                Self::record_content_layers(encoder, view, panes, target, pipeline, Some(canvas));
                Self::record_egui_pass(encoder, view, renderer, primitives, screen, None);
            }
            (None, _) => Self::record_content_layers(encoder, view, panes, target, pipeline, Some(canvas)),
        }
    }

    /// `panes` is 0, 1, or 2 `(viewport_px, tab_id)` pairs -- empty for
    /// plain Graph view (nothing to composite, `record_content_layers`
    /// still clears the screen), one for ordinary Tab view or a `Transition`
    /// (`main.rs`'s animated shrink/expand rect, in place of the full
    /// content area), two for a split-view pane pair. Each pane draws
    /// nothing (rather than a stale previous frame) until its own tab_id
    /// has an uploaded texture -- see `record_content_pass`'s own doc
    /// comment.
    ///
    /// `content_on_top`: which of the content quad(s) / egui layer is drawn
    /// first (the base, cleared layer) vs. second (drawn over it). Normal
    /// Tab view wants content as the base and chrome/dropdowns on top
    /// (`false`); a `Transition` wants the graph as the base and the
    /// shrinking/expanding content quad on top of it, so the animated tab
    /// content is never hidden underneath graph nodes/edges (`true`).
    ///
    /// `screenshot_path`: when set, saves this frame (chrome, any open
    /// dropdown, and content — the full composited picture, not just tab
    /// content) as a PNG there — headless mode's replacement for PrintWindow-
    /// based screenshots (see project memory's M5 gotchas on `PrintWindow`/
    /// `CopyFromScreen` GDI-handle exhaustion and foreground-lock
    /// unreliability; this reads pixels straight from the GPU instead).
    ///
    /// Implemented as a *second* render of the same content into an ordinary
    /// (non-surface) texture, not a readback of the swapchain texture
    /// itself: a presentable surface texture's usage flags are backend-
    /// negotiated (`surface_caps`) and may not include `COPY_SRC` at all,
    /// while a plain texture created with `COPY_SRC` always supports it on
    /// every backend. The extra render only happens on frames where a
    /// screenshot was actually requested (rare, testing-only) — normal
    /// frames are exactly as expensive as before this existed.
    pub fn render(
        &mut self,
        panes: &[(PanePx, NodeId)],
        content_on_top: bool,
        egui_output: Option<(&egui::Context, egui::FullOutput)>,
        screenshot_path: Option<&str>,
        canvas: egui::Color32,
    ) -> RenderOutcome {
        // The surface is sRGB, so a clear value is linear.
        let canvas = {
            let c = egui::Rgba::from(canvas);
            wgpu::Color { r: c.r() as f64, g: c.g() as f64, b: c.b() as f64, a: 1.0 }
        };
        let egui_output = self.sync_atlas(egui_output);
        let Some(frame) = self.acquire_frame() else {
            return RenderOutcome { surface_ok: false, screenshot_result: None };
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder =
            self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("tab render encoder") });

        let width = self.config.width;
        let height = self.config.height;
        // Tessellated once (`ctx.tessellate` consumes `full_output.shapes`)
        // and reused for both the on-screen pass and, if requested, the
        // offscreen screenshot pass below.
        let (egui_draw, extra_command_buffers) = match egui_output {
            Some((ctx, full_output)) => {
                let (draw, buffers) = self.tessellate_and_upload(&mut encoder, ctx, full_output);
                (Some(draw), buffers)
            }
            None => (None, Vec::new()),
        };

        Self::record_scene(
            &mut encoder,
            &view,
            &Self::pane_textures(&self.tab_textures, panes),
            (width, height),
            &self.pipeline,
            &mut self.egui_renderer,
            egui_draw.as_ref(),
            content_on_top,
            canvas,
        );

        let pending_screenshot = screenshot_path.map(|path| {
            (path, self.record_screenshot(&mut encoder, panes, egui_draw.as_ref(), content_on_top, canvas))
        });

        let submit = tracing::info_span!("submit").entered();
        self.queue.submit(extra_command_buffers.into_iter().chain(std::iter::once(encoder.finish())));
        drop(submit);
        let present = tracing::info_span!("present").entered();
        self.queue.present(frame);
        drop(present);

        let screenshot_result = pending_screenshot.map(|(path, (readback_buffer, padded_bytes_per_row))| {
            self.read_back_screenshot(path, readback_buffer, padded_bytes_per_row)
        });

        RenderOutcome { surface_ok: true, screenshot_result }
    }

    /// Sync texture-atlas deltas (font glyphs, etc.) to the GPU-side
    /// renderer, and clear them, *before* checking whether this frame's
    /// surface can even be acquired. `TexturesDelta` panics on `Drop` if
    /// any `.set`/`.free` entry was never consumed, and this used to run
    /// only after a successful `get_current_texture()` -- so a
    /// transient Occluded/Outdated/Timeout status on a frame that also
    /// introduced a new glyph (e.g. an icon font's first use) dropped
    /// `egui_output`'s `FullOutput` uncleared right at the early return,
    /// panicking with "Dropped TexturesDelta with 1 unapplied deltas".
    /// The atlas sync itself needs no surface -- only presentation does.
    fn sync_atlas<'a>(
        &mut self,
        egui_output: Option<(&'a egui::Context, egui::FullOutput)>,
    ) -> Option<(&'a egui::Context, egui::FullOutput)> {
        egui_output.map(|(ctx, mut full_output)| {
            for (id, deltas) in &full_output.textures_delta.set {
                for delta in deltas {
                    self.egui_renderer.update_texture(&self.device, &self.queue, *id, delta);
                }
            }
            for id in &full_output.textures_delta.free {
                self.egui_renderer.free_texture(id);
            }
            full_output.textures_delta.clear();
            (ctx, full_output)
        })
    }

    /// The surface's texture for this frame, or `None` when it can't be drawn to: the surface is recovered (or
    /// the failure logged) for the next redraw.
    fn acquire_frame(&mut self) -> Option<wgpu::SurfaceTexture> {
        if std::mem::take(&mut self.reconfigure) {
            self.reconfigure_surface();
        }
        let _acquire = tracing::info_span!("acquire").entered();
        let acquired = match self.forced.take() {
            Some(SurfaceFault::Outdated) => wgpu::CurrentSurfaceTexture::Outdated,
            Some(SurfaceFault::Lost) => wgpu::CurrentSurfaceTexture::Lost,
            None => self.surface.get_current_texture(),
        };
        match acquired {
            wgpu::CurrentSurfaceTexture::Success(t) => Some(t),
            // Presentable now, but wgpu asks for a reconfigure; done before the next acquire.
            wgpu::CurrentSurfaceTexture::Suboptimal(t) => {
                self.reconfigure = true;
                Some(t)
            }
            // The swapchain no longer matches the window (mode change, eGPU replug
            // with no resize event): without a reconfigure every later acquire fails too.
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.reconfigure_surface();
                self.recoveries += 1;
                None
            }
            // The surface itself is gone: make a new one for the same window.
            wgpu::CurrentSurfaceTexture::Lost => {
                // Logged once per run of failures: a window that cannot get a surface retries every frame.
                static LOST_LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                match self.instance.create_surface(self.window.clone()) {
                    Ok(surface) => {
                        self.surface = surface;
                        LOST_LOGGED.store(false, std::sync::atomic::Ordering::Relaxed);
                        self.reconfigure_surface();
                        self.recoveries += 1;
                    }
                    Err(err) => {
                        if !LOST_LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                            eprintln!("wgpu: couldn't recreate the lost surface: {err}");
                        }
                    }
                }
                None
            }
            // Timeout/Occluded/Validation: skip this frame and
            // try again next redraw rather than treating it as fatal. A
            // requested screenshot was never attempted here -- the caller
            // must not drop it, only retry on a later frame whose surface
            // actually comes back usable. `render`'s texture delta was
            // already applied and cleared (`sync_atlas`), so dropping the
            // egui output now is safe -- only the shapes (never tessellated)
            // go unused this frame.
            _ => None,
        }
    }

    /// Resolved once per use -- `panes` is just `(rect, tab_id)` pairs; this pairs each with whatever texture
    /// `tab_id` currently has uploaded, if any (a tab that hasn't sent
    /// its first frame yet, or whose texture was already freed, simply
    /// draws nothing for this frame, same tolerance a single-pane render
    /// already had).
    fn pane_textures<'a>(
        tab_textures: &'a HashMap<NodeId, TabTexture>,
        panes: &[(PanePx, NodeId)],
    ) -> Vec<(PanePx, Option<&'a TabTexture>)> {
        panes.iter().map(|(pane, id)| (*pane, tab_textures.get(id))).collect()
    }

    /// Tessellates the frame's shapes and uploads their buffers, flushing the thumbnails they draw first. The
    /// primitives and screen are drawn by every `record_scene` of the frame; the command buffers go to the
    /// submit ahead of the encoder's.
    fn tessellate_and_upload(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        ctx: &egui::Context,
        full_output: egui::FullOutput,
    ) -> ((Vec<egui::ClippedPrimitive>, egui_wgpu::ScreenDescriptor), Vec<wgpu::CommandBuffer>) {
        let clipped_primitives = tracing::info_span!("tessellate")
            .in_scope(|| ctx.tessellate(full_output.shapes, full_output.pixels_per_point));
        if tracing::enabled!(target: "mesh", tracing::Level::TRACE) {
            let (mut meshes, mut verts, mut indices) = (0, 0, 0);
            for p in &clipped_primitives {
                if let egui::epaint::Primitive::Mesh(m) = &p.primitive {
                    (meshes, verts, indices) = (meshes + 1, verts + m.vertices.len(), indices + m.indices.len());
                }
            }
            tracing::trace!(target: "mesh", label = format!("meshes={meshes} verts={verts} indices={indices}").as_str());
        }
        self.flush_thumbnails(encoder, &clipped_primitives);
        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.config.width, self.config.height],
            pixels_per_point: full_output.pixels_per_point,
        };
        let upload = tracing::info_span!("upload").entered();
        let extra_command_buffers = self.egui_renderer.update_buffers(
            &self.device,
            &self.queue,
            encoder,
            &clipped_primitives,
            &screen_descriptor,
        );
        drop(upload);
        ((clipped_primitives, screen_descriptor), extra_command_buffers)
    }

    /// Downsample the dirty thumbnails egui draws this frame (the screenshot pass draws the same
    /// primitives), before any pass that samples them.
    fn flush_thumbnails(&mut self, encoder: &mut wgpu::CommandEncoder, primitives: &[egui::ClippedPrimitive]) {
        if self.thumbs_dirty.is_empty() {
            return;
        }
        let drawn: HashSet<egui::TextureId> = primitives
            .iter()
            .filter_map(|p| match &p.primitive {
                egui::epaint::Primitive::Mesh(mesh) => Some(mesh.texture_id),
                egui::epaint::Primitive::Callback(_) => None,
            })
            .collect();
        self.thumbs_dirty.retain(|tab_id| {
            let (Some(source), Some(dest)) = (self.tab_textures.get(tab_id), self.thumbnails.get(tab_id)) else {
                return true;
            };
            if !drawn.contains(&dest.egui_id) {
                return true;
            }
            Self::render_thumbnail(
                encoder,
                &self.device,
                &self.downsample_pipeline,
                &self.downsample_bind_group_layout,
                &self.sampler,
                source,
                dest,
            );
            false
        });
    }

    /// Draws the frame again into a `COPY_SRC` texture and copies it into a buffer, returned with its padded row
    /// size for `read_back_screenshot` once the frame is submitted.
    fn record_screenshot(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        panes: &[(PanePx, NodeId)],
        egui_draw: Option<&(Vec<egui::ClippedPrimitive>, egui_wgpu::ScreenDescriptor)>,
        content_on_top: bool,
        canvas: wgpu::Color,
    ) -> (wgpu::Buffer, u32) {
        let (width, height) = (self.config.width, self.config.height);
        let readback_texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("screenshot readback texture"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let readback_view = readback_texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self::record_scene(
            encoder,
            &readback_view,
            &Self::pane_textures(&self.tab_textures, panes),
            (width, height),
            &self.pipeline,
            &mut self.egui_renderer,
            egui_draw,
            content_on_top,
            canvas,
        );

        let unpadded_bytes_per_row = width * 4;
        let padded_bytes_per_row =
            unpadded_bytes_per_row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let readback_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("screenshot readback buffer"),
            size: (padded_bytes_per_row * height) as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &readback_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback_buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        (readback_buffer, padded_bytes_per_row)
    }

    /// Waits for `record_screenshot`'s copy (submitted) and writes the pixels to `path` as a PNG.
    fn read_back_screenshot(
        &self,
        path: &str,
        readback_buffer: wgpu::Buffer,
        padded_bytes_per_row: u32,
    ) -> Result<(), String> {
        let (width, height) = (self.config.width, self.config.height);
        let (tx, rx) = std::sync::mpsc::channel();
        readback_buffer.map_async(wgpu::MapMode::Read, .., move |result| {
            let _ = tx.send(result);
        });
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        match rx.recv() {
            Ok(Ok(())) => {
                let mapped = readback_buffer.get_mapped_range(..).expect("mapped screenshot buffer");
                let mut pixels = vec![0u8; (width * height * 4) as usize];
                let unpadded_bytes_per_row = width * 4;
                for row in 0..height {
                    let src_start = (row * padded_bytes_per_row) as usize;
                    let dst_start = (row * unpadded_bytes_per_row) as usize;
                    pixels[dst_start..dst_start + unpadded_bytes_per_row as usize]
                        .copy_from_slice(&mapped[src_start..src_start + unpadded_bytes_per_row as usize]);
                }
                drop(mapped);
                readback_buffer.unmap();
                if matches!(self.config.format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb) {
                    for px in pixels.as_chunks_mut::<4>().0 {
                        px.swap(0, 2);
                    }
                }
                image::save_buffer(path, &pixels, width, height, image::ColorType::Rgba8)
                    .map_err(|err| format!("failed to save {path}: {err}"))
            }
            _ => Err(format!("failed to read back GPU buffer for {path}")),
        }
    }
}

/// Whether `len` bytes are a whole RGBA/BGRA frame of `width` x `height`
/// that a texture of at most `max_dim` per side can hold.
fn frame_is_valid(len: usize, width: u32, height: u32, max_dim: u32) -> bool {
    width > 0
        && height > 0
        && width <= max_dim
        && height <= max_dim
        && len as u64 == u64::from(width) * u64::from(height) * 4
}

#[cfg(test)]
mod frame_tests {
    use super::frame_is_valid;

    #[test]
    fn only_whole_frames_within_the_device_limit_are_valid() {
        assert!(frame_is_valid(2 * 3 * 4, 2, 3, 8192));
        assert!(!frame_is_valid(2 * 3 * 4 - 1, 2, 3, 8192), "short");
        assert!(!frame_is_valid(2 * 3 * 4 + 1, 2, 3, 8192), "long");
        assert!(!frame_is_valid(0, 0, 3, 8192), "empty");
        assert!(!frame_is_valid(16384 * 4, 16384, 1, 8192), "past the limit");
    }
}

/// What a frame upload and a thumbnail pass cost on the real adapter.
/// `cargo test --release -p manifold --bins frame_upload_timing -- --ignored --nocapture`.
#[cfg(test)]
mod upload_timing {
    use super::{downsample_pipeline, non_srgb_view_format, SHADER};
    use crate::bench::{best_median, report, timed};
    use std::time::Instant;

    const W: u32 = 1920;
    const H: u32 = 1080;
    const N: usize = 100;

    #[test]
    #[ignore = "timing bench; needs a GPU; run by hand"]
    fn frame_upload_timing() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .expect("request wgpu adapter");
        let info = adapter.get_info();
        println!("frame_upload_timing: adapter {} ({:?}), {W}x{H}, N = {N}", info.name, info.backend);
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("upload timing device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::default(),
            trace: wgpu::Trace::Off,
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
        }))
        .expect("request wgpu device");

        let format = wgpu::TextureFormat::Bgra8UnormSrgb;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("tab frame texture"),
            size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[non_srgb_view_format(format)],
        });
        let pixels: Vec<u8> = (0..W as usize * H as usize * 4).map(|i| (i as u8).wrapping_mul(7)).collect();
        let write_rows = |rows: u32| {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &pixels[..(W * rows * 4) as usize],
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(W * 4), rows_per_image: Some(rows) },
                wgpu::Extent3d { width: W, height: rows, depth_or_array_layers: 1 },
            );
        };
        let drain = || {
            queue.submit([]);
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        };
        drain();

        // 1: the CPU side only; each write is drained outside the timed span so staging memory stays bounded.
        write_rows(H);
        drain();
        let cpu: Vec<_> = (0..N)
            .map(|_| {
                let start = Instant::now();
                write_rows(H);
                let t = start.elapsed();
                drain();
                t
            })
            .collect();
        report("1 write_texture, whole frame, CPU time only", best_median(cpu));
        report(
            "2 whole frame: write + submit + poll",
            timed(N, || {
                write_rows(H);
                drain();
            }),
        );
        report(
            "3 64-row band: write + submit + poll",
            timed(N, || {
                write_rows(64);
                drain();
            }),
        );
        report("4 empty submit + poll", timed(N, drain));

        // 5: the current thumbnail pass, 1920x1080 source into a 256x144 target.
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let (layout, pipeline) = downsample_pipeline(&device, &shader);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let (tw, th) = (256u32, 144u32);
        let thumb = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: tw, height: th, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let thumb_pass = || {
            let source_view = texture.create_view(&wgpu::TextureViewDescriptor {
                format: Some(non_srgb_view_format(format)),
                ..Default::default()
            });
            let dest_view = thumb.create_view(&wgpu::TextureViewDescriptor::default());
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&source_view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
                ],
            });
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None,
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &dest_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &bind_group, &[]);
                pass.draw(0..3, 0..1);
            }
            queue.submit(std::iter::once(encoder.finish()));
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        };
        write_rows(H);
        drain();
        thumb_pass();
        report("5 thumbnail pass: bind group + encoder + pass + submit + poll", timed(N, thumb_pass));
    }
}
