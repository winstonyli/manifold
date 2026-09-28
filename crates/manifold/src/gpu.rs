use std::collections::HashMap;
use std::sync::Arc;

use wgpu::util::DeviceExt;
use winit::window::Window;

/// `GpuState::render`'s result -- lets the caller tell a truly-failed
/// screenshot save apart from one that was never attempted this frame
/// (surface unusable), so a request isn't silently dropped just because it
/// landed on a transient `Outdated`/`Occluded` frame.
/// A pane's rectangle in physical pixels: x, y, width, height.
pub type PxRect = (f32, f32, f32, f32);

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
// own [0,1] UV space -- since UV space is normalized identically for both
// the render target and the sampled texture regardless of either's actual
// pixel resolution, this is just `1.0 / thumbnail_size`, independent of
// the source's own resolution.
struct DownsampleParams {
    box_size: vec2<f32>,
};
@group(0) @binding(2) var<uniform> downsample_params: DownsampleParams;

const DOWNSAMPLE_TAPS: i32 = 4;

@fragment
fn fs_downsample(in: VertexOutput) -> @location(0) vec4<f32> {
    var sum = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    for (var iy = 0; iy < DOWNSAMPLE_TAPS; iy = iy + 1) {
        for (var ix = 0; ix < DOWNSAMPLE_TAPS; ix = ix + 1) {
            let grid = (vec2<f32>(f32(ix), f32(iy)) + vec2<f32>(0.5, 0.5)) / f32(DOWNSAMPLE_TAPS);
            let offset = (grid - vec2<f32>(0.5, 0.5)) * downsample_params.box_size;
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
/// slot no longer suffices the way it did when only ever one tab could be
/// `SetVisible(true)` at a time. `texture`/`width`/`height`/`format` are
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
    downsample_params_buffer: wgpu::Buffer,
    /// Keyed by tab id -- see `TabTexture`'s own doc comment for why this
    /// isn't a single slot. Bounded by however many tabs are currently
    /// visible (at most two, focused + split partner) rather than every tab
    /// ever focused: entries are removed via `free_tab_texture` whenever a
    /// tab stops being visible (unfocused, split closed, or closed
    /// outright).
    tab_textures: HashMap<String, TabTexture>,
    thumbnails: HashMap<String, ThumbTexture>,
    /// Keyed by *origin* (`scheme://host[:port]`, see `favicon_fetch::
    /// url_origin`), not tab id -- favicons are fetched independently of
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
    images: HashMap<String, ThumbTexture>,
    egui_renderer: egui_wgpu::Renderer,
}

/// How many distinct origins' favicons `GpuState` keeps textures for before
/// evicting the oldest (FIFO, not true LRU -- simpler, and a revisited
/// origin just re-fetches, same cost as a cache miss would have anyway).
/// Same bounded-cache posture `MAX_CLOSED_TABS`/`MAX_CLOSED_GROUPS` already
/// have elsewhere in this app, sized generously since a favicon texture is
/// tiny (typically 16-32px) and a real session could plausibly visit a
/// couple hundred distinct sites.
const MAX_CACHED_FAVICONS: usize = 200;

impl GpuState {
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
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
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

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("manifold device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::default(),
            trace: wgpu::Trace::Off,
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
        }))
        .expect("request wgpu device");

        let surface_caps = surface.get_capabilities(&adapter);
        let surface_format =
            surface_caps.formats.iter().copied().find(|f| f.is_srgb()).unwrap_or(surface_caps.formats[0]);

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
            // `Fifo` (vsync-locked) explicitly, not `surface_caps.
            // present_modes[0]` -- that was an unexamined "whatever the
            // backend happens to list first" default, and wgpu doesn't
            // guarantee that ordering puts a throttled mode first. This
            // app's winit loop runs `ControlFlow::Poll` (continuous
            // redraw -- see `App`'s own doc comment on why), so an
            // untethered `Immediate`/`Mailbox` surface would let that
            // loop present fully uncapped, burning far more GPU/CPU than
            // intended. `Fifo` is the one mode every backend is required
            // to support, so this needs no fallback.
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
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

        // GPU thumbnail downsample -- see `GpuState::render_thumbnail`'s own
        // doc comment. A separate bind group layout/pipeline from the one
        // above: a third binding (the box-size uniform) `fs_main` doesn't
        // use, and a fixed `Rgba8Unorm` target format instead of the
        // surface's own `config.format` (the thumbnail texture egui_wgpu
        // registers must be exactly `Rgba8Unorm`, never sRGB -- see
        // `ThumbTexture`'s own doc comment).
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
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
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
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
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
        // 16 bytes: `vec2<f32>` (8 bytes) padded up to WGSL's own uniform
        // address-space alignment for a struct containing one -- written
        // fresh via `queue.write_buffer` before every downsample draw
        // (see `render_thumbnail`), so this only ever needs to exist once.
        let downsample_params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("thumbnail downsample params"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

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
            downsample_params_buffer,
            tab_textures: HashMap::new(),
            thumbnails: HashMap::new(),
            favicons: HashMap::new(),
            favicon_order: std::collections::VecDeque::new(),
            images: HashMap::new(),
            egui_renderer,
        }
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
    pub fn upload_frame(&mut self, tab_id: &str, pixels: &[u8], width: u32, height: u32, format: wgpu::TextureFormat) {
        let reuse_main =
            self.tab_textures.get(tab_id).is_some_and(|t| t.width == width && t.height == height && t.format == format);
        if reuse_main {
            Self::write_full_texture(&self.queue, &self.tab_textures[tab_id].texture, pixels, width, height);
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
            self.tab_textures.insert(tab_id.to_string(), TabTexture { texture, width, height, format, bind_group });
        }

        let (thumb_width, thumb_height) = Self::thumbnail_target_size(width, height, THUMBNAIL_MAX_SIZE);
        let reuse_thumb =
            self.thumbnails.get(tab_id).is_some_and(|t| t.width == thumb_width && t.height == thumb_height);
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
            let egui_id = if let Some(existing) = self.thumbnails.get(tab_id) {
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
                tab_id.to_string(),
                ThumbTexture { texture: thumb_texture, width: thumb_width, height: thumb_height, egui_id },
            );
        }
        // Unlike the main texture's own reuse branch above (which can skip
        // straight to the next frame once written), the thumbnail's
        // *content* still needs refreshing every call regardless of
        // whether its texture object was just reused or just created --
        // `reuse_thumb` only ever answered "does a same-size destination
        // already exist," never "is it already showing this frame."
        self.render_thumbnail(tab_id, format, thumb_width, thumb_height);
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

    /// GPU-side replacement for the old CPU `to_thumbnail_rgba`/
    /// `downscale_rgba` pair -- downsamples `tab_id`'s own already-uploaded
    /// main frame texture directly into its thumbnail texture via a render
    /// pass (`fs_downsample` in `SHADER`), instead of downloading pixels to
    /// the CPU, box-filtering them there, and re-uploading the result.
    /// Found on the same "app feels slow" investigation as the persistent-
    /// texture fix above: with that fix landed, this pixel conversion/
    /// downscale was the one remaining per-frame cost of any real size,
    /// and it's exactly the kind of embarrassingly-parallel, per-pixel
    /// work a GPU does far more cheaply than a CPU loop -- doubly so in a
    /// debug build, where the CPU loop pays for zero compiler optimization
    /// but the GPU's own fixed-function/shader-core work doesn't care what
    /// profile the *driver* was compiled under.
    ///
    /// Two real correctness details this has to get right that a naive
    /// "just sample the main texture" attempt wouldn't:
    ///
    /// - **sRGB.** The main texture is `Bgra8UnormSrgb`/`Rgba8UnormSrgb`
    ///   (see `upload_frame`'s own doc comment on `format`), and sampling
    ///   an sRGB texture auto-*linearizes* the color -- writing that
    ///   straight into the thumbnail's plain (non-sRGB) `Rgba8Unorm`
    ///   target would silently store linear-light bytes where gamma-
    ///   encoded ones belong, visibly shifting brightness/contrast versus
    ///   the real tab content. Sampled instead through a *reinterpreting*
    ///   view in the non-sRGB counterpart format (`Bgra8Unorm`/
    ///   `Rgba8Unorm`, via `non_srgb_view_format`) -- returns the raw
    ///   stored bytes unconverted, exactly matching what the old CPU
    ///   path's plain byte copy already did. Requires the main texture's
    ///   own `view_formats` to declare that counterpart at creation time
    ///   (see `upload_frame`'s texture creation above).
    /// - **BGRA vs RGBA channel order.** *Not* handled here at all, on
    ///   purpose -- unlike the CPU path's manual byte swap
    ///   (`to_thumbnail_rgba`), `textureSample` already returns color in a
    ///   consistent `(r,g,b,a)` order regardless of the bound texture's
    ///   own declared channel layout; the GPU/driver handles that
    ///   remapping as part of what a texture *format* means.
    ///
    /// Deliberately reproduces, not fixes, the old CPU path's own
    /// previously-accepted cosmetic gap: bypassing sRGB decoding on the way
    /// in (rather than decoding then re-encoding, a real color-correct
    /// path) means the thumbnail still renders very slightly washed out
    /// compared to the live content pipeline, exactly as the removed
    /// `to_thumbnail_rgba`'s own doc comment already called out and
    /// accepted -- this pass changes *where* that gap's bytes get produced
    /// (GPU instead of CPU), not what they are.
    fn render_thumbnail(&mut self, tab_id: &str, format: wgpu::TextureFormat, thumb_width: u32, thumb_height: u32) {
        // One output texel's width/height in the *source* texture's own
        // [0,1] UV space -- see `fs_downsample`'s own doc comment for why
        // this needs no dependency on the source's actual resolution.
        let mut params = [0u8; 16];
        params[0..4].copy_from_slice(&(1.0f32 / thumb_width as f32).to_le_bytes());
        params[4..8].copy_from_slice(&(1.0f32 / thumb_height as f32).to_le_bytes());
        self.queue.write_buffer(&self.downsample_params_buffer, 0, &params);

        let source_view = self.tab_textures[tab_id].texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(non_srgb_view_format(format)),
            ..Default::default()
        });
        let dest_view = self.thumbnails[tab_id].texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("thumbnail downsample bind group"),
            layout: &self.downsample_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&source_view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: self.downsample_params_buffer.as_entire_binding() },
            ],
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("thumbnail downsample encoder") });
        {
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
            pass.set_pipeline(&self.downsample_pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit(std::iter::once(encoder.finish()));
    }

    /// Uploads a full frame of tightly-packed `pixels` (no row padding --
    /// the same assumption every caller of this already makes: shared-
    /// memory frame buffers and inline IPC pixels are both exactly
    /// `width * height * 4` bytes) into `texture` in place, for the
    /// "dimensions unchanged since last time" fast path both blocks in
    /// `upload_frame` share.
    fn write_full_texture(queue: &wgpu::Queue, texture: &wgpu::Texture, pixels: &[u8], width: u32, height: u32) {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            pixels,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(width * 4), rows_per_image: Some(height) },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
    }

    /// The `egui::TextureId` for a tab's cached thumbnail, if it has ever
    /// produced a frame — for the graph view to draw via `egui::Image`.
    pub fn thumbnail_id(&self, tab_id: &str) -> Option<egui::TextureId> {
        self.thumbnails.get(tab_id).map(|t| t.egui_id)
    }

    /// Must be called when a tab closes — `egui_wgpu::Renderer` doesn't
    /// know about manually-registered native textures on its own, so
    /// unlike egui's own font/image-loader textures (freed automatically
    /// via each frame's `textures_delta.free`, see `render`), this one
    /// leaks until explicitly freed here.
    pub fn free_tab_thumbnail(&mut self, tab_id: &str) {
        if let Some(thumb) = self.thumbnails.remove(tab_id) {
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
    pub fn free_tab_texture(&mut self, tab_id: &str) {
        self.tab_textures.remove(tab_id);
    }

    /// Whether `upload_frame` has ever run for this tab -- `false` right
    /// after a fresh spawn/respawn, until its engine-host's first real
    /// frame arrives. Lets Tab view show a loading spinner instead of the
    /// plain black clear `record_content_pass` draws for a pane with no
    /// texture yet (see that function's own doc comment).
    pub fn has_tab_texture(&self, tab_id: &str) -> bool {
        self.tab_textures.contains_key(tab_id)
    }

    /// `rgba` is already tightly-packed RGBA -- registered the same way as
    /// a thumbnail, in its own map since a favicon and a tab's content
    /// thumbnail are unrelated sizes and update independently. `key` is an
    /// *origin* (see `favicons`' own doc comment), not a tab id.
    pub fn upload_favicon(&mut self, key: &str, rgba: &[u8], width: u32, height: u32) {
        let texture = self.device.create_texture_with_data(
            &self.queue,
            &wgpu::TextureDescriptor {
                label: Some("favicon texture"),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            rgba,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        if let Some(existing) = self.favicons.get(key) {
            self.egui_renderer.update_egui_texture_from_wgpu_texture(
                &self.device,
                &view,
                wgpu::FilterMode::Linear,
                existing.egui_id,
            );
        } else {
            let egui_id = self.egui_renderer.register_native_texture(&self.device, &view, wgpu::FilterMode::Linear);
            self.favicons.insert(key.to_string(), ThumbTexture { texture, width, height, egui_id });
            self.favicon_order.push_back(key.to_string());
            if self.favicon_order.len() > MAX_CACHED_FAVICONS {
                if let Some(oldest) = self.favicon_order.pop_front() {
                    if let Some(favicon) = self.favicons.remove(&oldest) {
                        self.egui_renderer.free_texture(&favicon.egui_id);
                    }
                }
            }
        }
    }

    /// The `egui::TextureId` for an origin's cached favicon, if one has
    /// been fetched — for the tab strip and graph nodes to draw next to the
    /// title. `key` is an origin (see `favicons`' own doc comment).
    pub fn favicon_id(&self, key: &str) -> Option<egui::TextureId> {
        self.favicons.get(key).map(|t| t.egui_id)
    }

    /// Registers an image node's decoded picture -- same one-shot
    /// `create_texture_with_data` registration as `upload_favicon`, in its
    /// own map since an image node's picture is unrelated in size/lifetime
    /// to a favicon or a tab's content thumbnail. `key` is a *node id* (see
    /// `images`' own doc comment), called at creation, restore or after a
    /// Refresh (which frees the old one first), not every frame like a
    /// tab's thumbnail.
    pub fn upload_image(&mut self, key: &str, rgba: &[u8], width: u32, height: u32) {
        let texture = self.device.create_texture_with_data(
            &self.queue,
            &wgpu::TextureDescriptor {
                label: Some("image node texture"),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            rgba,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let egui_id = self.egui_renderer.register_native_texture(&self.device, &view, wgpu::FilterMode::Linear);
        self.images.insert(key.to_string(), ThumbTexture { texture, width, height, egui_id });
    }

    /// The `egui::TextureId` for an image node's picture, if it decoded
    /// successfully — for Graph view to draw via `egui::Image`.
    pub fn image_id(&self, key: &str) -> Option<egui::TextureId> {
        self.images.get(key).map(|t| t.egui_id)
    }

    /// Must be called when an image node is removed -- same "manually
    /// registered native textures leak until freed" reasoning as
    /// `free_tab_thumbnail`'s own doc comment.
    pub fn free_image(&mut self, key: &str) {
        if let Some(image) = self.images.remove(key) {
            self.egui_renderer.free_texture(&image.egui_id);
        }
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
        panes: &[(PxRect, Option<&TabTexture>)],
        pipeline: &wgpu::RenderPipeline,
        clear: bool,
    ) {
        if panes.is_empty() {
            Self::record_content_pass(encoder, view, (0.0, 0.0, 0.0, 0.0), None, pipeline, clear);
            return;
        }
        for (i, (rect, showing)) in panes.iter().enumerate() {
            Self::record_content_pass(encoder, view, *rect, *showing, pipeline, clear && i == 0);
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
        viewport: PxRect,
        showing: Option<&TabTexture>,
        pipeline: &wgpu::RenderPipeline,
        clear: bool,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("tab render pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: if clear { wgpu::LoadOp::Clear(wgpu::Color::BLACK) } else { wgpu::LoadOp::Load },
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        if let Some(tab_texture) = showing {
            let (x, y, w, h) = viewport;
            // `.max(1.0)`: wgpu validation rejects a zero/negative-size
            // viewport — a transition's interpolated rect should never
            // actually collapse to zero, but a mid-animation window resize
            // is cheap insurance against a transient degenerate frame.
            pass.set_viewport(x, y, w.max(1.0), h.max(1.0), 0.0, 1.0);
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
        clear: bool,
    ) {
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("egui render pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: if clear { wgpu::LoadOp::Clear(wgpu::Color::BLACK) } else { wgpu::LoadOp::Load },
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
        panes: &[(PxRect, &str)],
        content_on_top: bool,
        egui_output: Option<(&egui::Context, egui::FullOutput)>,
        screenshot_path: Option<&str>,
    ) -> RenderOutcome {
        // Sync texture-atlas deltas (font glyphs, etc.) to the GPU-side
        // renderer, and clear them, *before* checking whether this frame's
        // surface can even be acquired. `TexturesDelta` panics on `Drop` if
        // any `.set`/`.free` entry was never consumed, and this used to run
        // only after a successful `get_current_texture()` below -- so a
        // transient Occluded/Outdated/Timeout status on a frame that also
        // introduced a new glyph (e.g. an icon font's first use) dropped
        // `egui_output`'s `FullOutput` uncleared right at the early return,
        // panicking with "Dropped TexturesDelta with 1 unapplied deltas".
        // The atlas sync itself needs no surface -- only presentation does.
        let egui_output = egui_output.map(|(ctx, mut full_output)| {
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
        });

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            // Timeout/Occluded/Outdated/Lost/Validation: skip this frame and
            // try again next redraw rather than treating it as fatal. A
            // requested screenshot was never attempted here -- the caller
            // must not drop it, only retry on a later frame whose surface
            // actually comes back usable. `egui_output`'s texture delta was
            // already applied and cleared above, so dropping it here now is
            // safe -- only the shapes (tessellated below, never touched
            // yet) go unused this frame.
            _ => return RenderOutcome { surface_ok: false, screenshot_result: None },
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder =
            self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("tab render encoder") });

        let width = self.config.width;
        let height = self.config.height;
        // Resolved once, up front -- `panes` (the parameter) is just
        // `(rect, tab_id)` pairs; this pairs each with whatever texture
        // `tab_id` currently has uploaded, if any (a tab that hasn't sent
        // its first frame yet, or whose texture was already freed, simply
        // draws nothing for this frame, same tolerance a single-pane render
        // already had).
        let panes: Vec<(PxRect, Option<&TabTexture>)> =
            panes.iter().map(|(rect, id)| (*rect, self.tab_textures.get(*id))).collect();

        // Tessellated once (`ctx.tessellate` consumes `full_output.shapes`)
        // and reused for both the on-screen pass and, if requested, the
        // offscreen screenshot pass below.
        let mut extra_command_buffers = Vec::new();
        let egui_draw = if let Some((ctx, full_output)) = egui_output {
            let clipped_primitives = ctx.tessellate(full_output.shapes, full_output.pixels_per_point);
            let screen_descriptor = egui_wgpu::ScreenDescriptor {
                size_in_pixels: [width, height],
                pixels_per_point: full_output.pixels_per_point,
            };
            extra_command_buffers = self.egui_renderer.update_buffers(
                &self.device,
                &self.queue,
                &mut encoder,
                &clipped_primitives,
                &screen_descriptor,
            );
            Some((clipped_primitives, screen_descriptor))
        } else {
            None
        };

        // Normally the content quad is the base layer and egui (chrome, any
        // dropdown) draws on top of it, unclipped. During a Tab<->Graph
        // transition the order flips: the graph (egui) is the base layer
        // and the transitioning tab's content quad draws over it, since
        // that quad is what's supposed to visually shrink/expand in front
        // of the graph, not get hidden underneath its nodes/edges.
        if content_on_top {
            if let Some((clipped_primitives, screen_descriptor)) = &egui_draw {
                Self::record_egui_pass(
                    &mut encoder,
                    &view,
                    &mut self.egui_renderer,
                    clipped_primitives,
                    screen_descriptor,
                    true,
                );
                Self::record_content_layers(&mut encoder, &view, &panes, &self.pipeline, false);
            } else {
                Self::record_content_layers(&mut encoder, &view, &panes, &self.pipeline, true);
            }
        } else {
            Self::record_content_layers(&mut encoder, &view, &panes, &self.pipeline, true);
            if let Some((clipped_primitives, screen_descriptor)) = &egui_draw {
                Self::record_egui_pass(
                    &mut encoder,
                    &view,
                    &mut self.egui_renderer,
                    clipped_primitives,
                    screen_descriptor,
                    false,
                );
            }
        }

        let pending_screenshot = screenshot_path.map(|path| {
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
            if content_on_top {
                if let Some((clipped_primitives, screen_descriptor)) = &egui_draw {
                    Self::record_egui_pass(
                        &mut encoder,
                        &readback_view,
                        &mut self.egui_renderer,
                        clipped_primitives,
                        screen_descriptor,
                        true,
                    );
                    Self::record_content_layers(&mut encoder, &readback_view, &panes, &self.pipeline, false);
                } else {
                    Self::record_content_layers(&mut encoder, &readback_view, &panes, &self.pipeline, true);
                }
            } else {
                Self::record_content_layers(&mut encoder, &readback_view, &panes, &self.pipeline, true);
                if let Some((clipped_primitives, screen_descriptor)) = &egui_draw {
                    Self::record_egui_pass(
                        &mut encoder,
                        &readback_view,
                        &mut self.egui_renderer,
                        clipped_primitives,
                        screen_descriptor,
                        false,
                    );
                }
            }

            let unpadded_bytes_per_row = width * 4;
            let padded_bytes_per_row = unpadded_bytes_per_row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
                * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
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
            (path.to_string(), readback_buffer, padded_bytes_per_row)
        });

        self.queue.submit(extra_command_buffers.into_iter().chain(std::iter::once(encoder.finish())));
        self.queue.present(frame);

        let screenshot_result = pending_screenshot.map(|(path, readback_buffer, padded_bytes_per_row)| {
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
                    if matches!(
                        self.config.format,
                        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
                    ) {
                        for px in pixels.as_chunks_mut::<4>().0 {
                            px.swap(0, 2);
                        }
                    }
                    image::save_buffer(&path, &pixels, width, height, image::ColorType::Rgba8)
                        .map_err(|err| format!("failed to save {path}: {err}"))
                }
                _ => Err(format!("failed to read back GPU buffer for {path}")),
            }
        });

        RenderOutcome { surface_ok: true, screenshot_result }
    }
}
