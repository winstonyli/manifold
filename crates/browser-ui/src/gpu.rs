use std::collections::HashMap;
use std::sync::Arc;

use wgpu::util::DeviceExt;
use winit::window::Window;

/// `GpuState::render`'s result -- lets the caller tell a truly-failed
/// screenshot save apart from one that was never attempted this frame
/// (surface unusable), so a request isn't silently dropped just because it
/// landed on a transient `Outdated`/`Occluded` frame.
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
"#;

/// Graph-view thumbnails never need to exceed roughly `GRAPH_NODE_SIZE`
/// (see `main.rs`) on screen — cap well above that so it still looks sharp
/// at the enlarged peek-preview size too, without paying full content
/// resolution.
const THUMBNAIL_MAX_SIZE: (u32, u32) = (256, 192);

/// Holds the currently focused tab's frame as a GPU texture for the
/// hand-rolled content pipeline. Recreated on every uploaded frame — no
/// dirty-tracking, see `upload_frame`. Since only the focused tab ever
/// produces frames (background tabs are `SetVisible(false)`, M5), a single
/// slot (not a per-tab map) is enough here; `tab_id` records which tab it
/// belongs to so `render` can avoid briefly showing a just-defocused tab's
/// stale content in a newly-focused tab's viewport before its own first
/// frame arrives.
struct TabTexture {
    bind_group: wgpu::BindGroup,
    tab_id: String,
}

/// A tab's cached frame re-uploaded as a plain `Rgba8Unorm` texture and
/// registered with `egui_wgpu::Renderer`, so the M5 graph view can draw it
/// as an ordinary `egui::Image`/`painter().image()` call. This is *separate*
/// from `TabTexture` above because `egui_wgpu::Renderer::register_native_
/// texture` requires exactly `Rgba8Unorm` — not the sRGB/BGRA formats the
/// hand-rolled content pipeline uses — so CEF's BGRA8 frames need a channel
/// swap on the way in (see `to_thumbnail_rgba`). Persists across focus
/// changes (unlike `TabTexture`) so a suspended tab's node keeps showing
/// its last frame.
struct ThumbTexture {
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
    tab_texture: Option<TabTexture>,
    thumbnails: HashMap<String, ThumbTexture>,
    favicons: HashMap<String, ThumbTexture>,
    egui_renderer: egui_wgpu::Renderer,
}

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
        let surface = instance
            .create_surface(window.clone())
            .expect("create wgpu surface");

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .expect("request wgpu adapter");

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("multiengine-browser device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::default(),
            trace: wgpu::Trace::Off,
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
        }))
        .expect("request wgpu device");

        let surface_caps = surface.get_capabilities(&adapter);
        let surface_format = surface_caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(surface_caps.formats[0]);

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
            present_mode: surface_caps.present_modes[0],
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

        let egui_renderer = egui_wgpu::Renderer::new(
            &device,
            config.format,
            egui_wgpu::RendererOptions::default(),
        );

        Self {
            surface,
            device,
            queue,
            config,
            pipeline,
            bind_group_layout,
            sampler,
            tab_texture: None,
            thumbnails: HashMap::new(),
            favicons: HashMap::new(),
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

    /// CEF's BGRA8 capture needs a channel swap to become the plain RGBA
    /// `egui_wgpu::Renderer::register_native_texture` requires — Servo/
    /// WebKit's RGBA8 needs none. `Unorm` vs `UnormSrgb` is a decode-curve
    /// difference, not a channel-order one, so feeding sRGB-authored bytes
    /// through as `Unorm` here just means graph-view thumbnails render
    /// very slightly washed out compared to the live content pipeline —
    /// an accepted, minor cosmetic gap for a small preview thumbnail, not
    /// worth a full color-correct path in M5.
    fn to_thumbnail_rgba(pixels: &[u8], format: wgpu::TextureFormat) -> Vec<u8> {
        if format == wgpu::TextureFormat::Bgra8UnormSrgb {
            let mut out = pixels.to_vec();
            for px in out.chunks_exact_mut(4) {
                px.swap(0, 2);
            }
            out
        } else {
            pixels.to_vec()
        }
    }

    /// Box-downsamples an RGBA buffer to at most `max_size` on each axis
    /// (never upscales — a tab smaller than the cap is left as-is), so a
    /// background tab's graph-view thumbnail costs a few hundred KB of GPU
    /// memory instead of a few MB at full content resolution. Only the
    /// thumbnail texture goes through this — `upload_frame`'s hand-rolled
    /// content-pipeline texture (Tab view) stays full resolution.
    fn downscale_rgba(
        pixels: &[u8],
        width: u32,
        height: u32,
        max_size: (u32, u32),
    ) -> (Vec<u8>, u32, u32) {
        let (max_w, max_h) = max_size;
        if width <= max_w && height <= max_h {
            return (pixels.to_vec(), width, height);
        }
        let scale = (max_w as f32 / width as f32).min(max_h as f32 / height as f32);
        let out_w = ((width as f32 * scale).round() as u32).max(1);
        let out_h = ((height as f32 * scale).round() as u32).max(1);

        let mut out = vec![0u8; (out_w * out_h * 4) as usize];
        for oy in 0..out_h {
            // Source row range this output row box-averages over — computed
            // from output-row boundaries (not just a single sample point)
            // so every source row contributes to some output row.
            let src_y0 = (oy * height / out_h).min(height - 1);
            let src_y1 = (((oy + 1) * height).div_ceil(out_h)).clamp(src_y0 + 1, height);
            for ox in 0..out_w {
                let src_x0 = (ox * width / out_w).min(width - 1);
                let src_x1 = (((ox + 1) * width).div_ceil(out_w)).clamp(src_x0 + 1, width);

                let mut sum = [0u32; 4];
                let mut count = 0u32;
                for sy in src_y0..src_y1 {
                    for sx in src_x0..src_x1 {
                        let idx = ((sy * width + sx) * 4) as usize;
                        for c in 0..4 {
                            sum[c] += pixels[idx + c] as u32;
                        }
                        count += 1;
                    }
                }
                let dst_idx = ((oy * out_w + ox) * 4) as usize;
                for c in 0..4 {
                    out[dst_idx + c] = (sum[c] / count.max(1)) as u8;
                }
            }
        }
        (out, out_w, out_h)
    }

    /// `pixels` must be exactly `width * height * 4` bytes in whatever pixel
    /// order `format` names — Servo's `read_to_image()` produces RGBA8
    /// (`Rgba8UnormSrgb`), CEF's OSR `on_paint` produces BGRA8
    /// (`Bgra8UnormSrgb`). Passing the wrong format silently swaps red/blue.
    ///
    /// Updates two things: the single hand-rolled-pipeline content texture
    /// (used only for whichever tab is currently focused in Tab view — see
    /// `TabTexture`'s doc comment for why one slot suffices) and that tab's
    /// persistent graph-view thumbnail (see `ThumbTexture`).
    pub fn upload_frame(
        &mut self,
        tab_id: &str,
        pixels: &[u8],
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) {
        let texture = self.device.create_texture_with_data(
            &self.queue,
            &wgpu::TextureDescriptor {
                label: Some("tab frame texture"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            pixels,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("tab texture bind group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        self.tab_texture = Some(TabTexture {
            bind_group,
            tab_id: tab_id.to_string(),
        });

        let thumb_pixels = Self::to_thumbnail_rgba(pixels, format);
        let (thumb_pixels, thumb_width, thumb_height) =
            Self::downscale_rgba(&thumb_pixels, width, height, THUMBNAIL_MAX_SIZE);
        let thumb_texture = self.device.create_texture_with_data(
            &self.queue,
            &wgpu::TextureDescriptor {
                label: Some("tab thumbnail texture"),
                size: wgpu::Extent3d {
                    width: thumb_width,
                    height: thumb_height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &thumb_pixels,
        );
        let thumb_view = thumb_texture.create_view(&wgpu::TextureViewDescriptor::default());
        if let Some(existing) = self.thumbnails.get(tab_id) {
            self.egui_renderer.update_egui_texture_from_wgpu_texture(
                &self.device,
                &thumb_view,
                wgpu::FilterMode::Linear,
                existing.egui_id,
            );
        } else {
            let egui_id = self.egui_renderer.register_native_texture(
                &self.device,
                &thumb_view,
                wgpu::FilterMode::Linear,
            );
            self.thumbnails
                .insert(tab_id.to_string(), ThumbTexture { egui_id });
        }
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
        self.free_tab_favicon(tab_id);
    }

    /// `rgba` is already tightly-packed RGBA (both Servo's and WebKit's
    /// favicon paths hand over decoded pixels, no channel swap needed
    /// unlike `upload_frame`'s CEF case) — registered the same way as a
    /// thumbnail, in its own map since a tab's favicon and its content
    /// thumbnail are unrelated sizes and update independently.
    pub fn upload_favicon(&mut self, tab_id: &str, rgba: &[u8], width: u32, height: u32) {
        let texture = self.device.create_texture_with_data(
            &self.queue,
            &wgpu::TextureDescriptor {
                label: Some("tab favicon texture"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
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
        if let Some(existing) = self.favicons.get(tab_id) {
            self.egui_renderer.update_egui_texture_from_wgpu_texture(
                &self.device,
                &view,
                wgpu::FilterMode::Linear,
                existing.egui_id,
            );
        } else {
            let egui_id =
                self.egui_renderer
                    .register_native_texture(&self.device, &view, wgpu::FilterMode::Linear);
            self.favicons
                .insert(tab_id.to_string(), ThumbTexture { egui_id });
        }
    }

    /// The `egui::TextureId` for a tab's cached favicon, if it has sent one
    /// — for the tab strip and graph nodes to draw next to the title.
    pub fn favicon_id(&self, tab_id: &str) -> Option<egui::TextureId> {
        self.favicons.get(tab_id).map(|t| t.egui_id)
    }

    /// Same leak concern as `free_tab_thumbnail` — called from there on tab
    /// close, kept separate since a favicon can also change size (new
    /// texture registered in place is fine; only close needs an explicit
    /// free).
    pub fn free_tab_favicon(&mut self, tab_id: &str) {
        if let Some(favicon) = self.favicons.remove(tab_id) {
            self.egui_renderer.free_texture(&favicon.egui_id);
        }
    }

    /// `viewport` is the exact rect (physical pixels: x, y, w, h) the
    /// content quad draws at — normally the full window below chrome, but
    /// during a Tab↔Graph shrink/expand animation (`main.rs`'s
    /// `Transition`) the caller passes an interpolated, animated rect
    /// instead, sized/positioned partway between that and the target
    /// graph-node's thumbnail spot. `egui` is `None` on the very first
    /// frames before egui has produced any output yet.
    ///
    /// `focused_tab_id` is the tab whose live content should fill the
    /// content viewport (Tab view, or the tab a `Transition` is animating)
    /// — `None` when there is no single tab whose content belongs on
    /// screen (plain Graph view, no animation). If it doesn't match
    /// whichever tab most recently produced a frame (e.g. focus just
    /// switched and the new tab hasn't painted yet), the content pass is
    /// skipped rather than briefly showing the previous tab's stale frame.
    ///
    /// `content_on_top`: which of the content quad / egui layer is drawn
    /// first (the base, cleared layer) vs. second (drawn over it). Normal
    /// Tab view wants content as the base and chrome/dropdowns on top
    /// (`false`); a `Transition` wants the graph as the base and the
    /// shrinking/expanding content quad on top of it, so the animated
    /// tab content is never hidden underneath graph nodes/edges (`true`).
    /// Records the hand-rolled content-pipeline pass (the focused tab's live
    /// frame, if any) into `view`. A free function (not a `&self` method) so
    /// `render` can call it against two different targets — the real
    /// swapchain view and, when a screenshot was requested, a second
    /// offscreen view — without the borrow checker treating `encoder`/
    /// `pipeline` as aliasing `self` (see `render`'s own comment on why the
    /// screenshot path re-renders rather than reads back the swapchain
    /// texture directly).
    fn record_content_pass(
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        viewport: (f32, f32, f32, f32),
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
                    load: if clear {
                        wgpu::LoadOp::Clear(wgpu::Color::BLACK)
                    } else {
                        wgpu::LoadOp::Load
                    },
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
                    load: if clear {
                        wgpu::LoadOp::Clear(wgpu::Color::BLACK)
                    } else {
                        wgpu::LoadOp::Load
                    },
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
        content_rect_px: (f32, f32, f32, f32),
        focused_tab_id: Option<&str>,
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
                    self.egui_renderer
                        .update_texture(&self.device, &self.queue, *id, delta);
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
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("tab render encoder"),
            });

        let width = self.config.width;
        let height = self.config.height;
        let showing = self
            .tab_texture
            .as_ref()
            .filter(|t| focused_tab_id == Some(t.tab_id.as_str()));

        // Tessellated once (`ctx.tessellate` consumes `full_output.shapes`)
        // and reused for both the on-screen pass and, if requested, the
        // offscreen screenshot pass below.
        let mut extra_command_buffers = Vec::new();
        let egui_draw = if let Some((ctx, full_output)) = egui_output {
            let clipped_primitives =
                ctx.tessellate(full_output.shapes, full_output.pixels_per_point);
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
                Self::record_content_pass(&mut encoder, &view, content_rect_px, showing, &self.pipeline, false);
            } else {
                Self::record_content_pass(&mut encoder, &view, content_rect_px, showing, &self.pipeline, true);
            }
        } else {
            Self::record_content_pass(&mut encoder, &view, content_rect_px, showing, &self.pipeline, true);
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
                    Self::record_content_pass(&mut encoder, &readback_view, content_rect_px, showing, &self.pipeline, false);
                } else {
                    Self::record_content_pass(&mut encoder, &readback_view, content_rect_px, showing, &self.pipeline, true);
                }
            } else {
                Self::record_content_pass(&mut encoder, &readback_view, content_rect_px, showing, &self.pipeline, true);
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
            let padded_bytes_per_row = unpadded_bytes_per_row
                .div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
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

        self.queue.submit(
            extra_command_buffers
                .into_iter()
                .chain(std::iter::once(encoder.finish())),
        );
        self.queue.present(frame);

        let screenshot_result = pending_screenshot.map(|(path, readback_buffer, padded_bytes_per_row)| {
            let (tx, rx) = std::sync::mpsc::channel();
            readback_buffer.map_async(wgpu::MapMode::Read, .., move |result| {
                let _ = tx.send(result);
            });
            let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
            match rx.recv() {
                Ok(Ok(())) => {
                    let mapped = readback_buffer
                        .get_mapped_range(..)
                        .expect("mapped screenshot buffer");
                    let mut pixels = vec![0u8; (width * height * 4) as usize];
                    let unpadded_bytes_per_row = width * 4;
                    for row in 0..height {
                        let src_start = (row * padded_bytes_per_row) as usize;
                        let dst_start = (row * unpadded_bytes_per_row) as usize;
                        pixels[dst_start..dst_start + unpadded_bytes_per_row as usize].copy_from_slice(
                            &mapped[src_start..src_start + unpadded_bytes_per_row as usize],
                        );
                    }
                    drop(mapped);
                    readback_buffer.unmap();
                    if matches!(
                        self.config.format,
                        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
                    ) {
                        for px in pixels.chunks_exact_mut(4) {
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
