use std::collections::HashMap;
use std::sync::Arc;

use wgpu::util::DeviceExt;
use winit::window::Window;

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
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
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
        let thumb_texture = self.device.create_texture_with_data(
            &self.queue,
            &wgpu::TextureDescriptor {
                label: Some("tab thumbnail texture"),
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

    /// `content_top_px` reserves the top strip of the window (in physical
    /// pixels) for chrome — the content quad only draws below it — so the
    /// tab strip/toolbar egui paints there never fights the tab's own
    /// full-bleed texture for the same pixels. `egui` is `None` on the very
    /// first frames before egui has produced any output yet.
    ///
    /// `focused_tab_id` is the tab whose live content should fill the
    /// content viewport (Tab view) — `None` in Graph view, where there is
    /// no single focused tab and node thumbnails are drawn entirely via the
    /// egui pass instead. If it doesn't match whichever tab most recently
    /// produced a frame (e.g. focus just switched and the new tab hasn't
    /// painted yet), the content pass is skipped rather than briefly
    /// showing the previous tab's stale frame.
    pub fn render(
        &mut self,
        content_top_px: u32,
        focused_tab_id: Option<&str>,
        egui_output: Option<(&egui::Context, egui::FullOutput)>,
    ) {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            // Timeout/Occluded/Outdated/Lost/Validation: skip this frame and
            // try again next redraw rather than treating it as fatal.
            _ => return,
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("tab render encoder"),
            });

        let content_top = content_top_px.min(self.config.height);
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("tab render pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let showing = self
                .tab_texture
                .as_ref()
                .filter(|t| focused_tab_id == Some(t.tab_id.as_str()));
            if let Some(tab_texture) = showing {
                pass.set_viewport(
                    0.0,
                    content_top as f32,
                    self.config.width as f32,
                    (self.config.height - content_top) as f32,
                    0.0,
                    1.0,
                );
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &tab_texture.bind_group, &[]);
                pass.draw(0..3, 0..1);
            }
        }

        let mut extra_command_buffers = Vec::new();
        if let Some((ctx, mut full_output)) = egui_output {
            for (id, deltas) in &full_output.textures_delta.set {
                for delta in deltas {
                    self.egui_renderer
                        .update_texture(&self.device, &self.queue, *id, delta);
                }
            }
            let clipped_primitives =
                ctx.tessellate(full_output.shapes, full_output.pixels_per_point);
            let screen_descriptor = egui_wgpu::ScreenDescriptor {
                size_in_pixels: [self.config.width, self.config.height],
                pixels_per_point: full_output.pixels_per_point,
            };
            extra_command_buffers = self.egui_renderer.update_buffers(
                &self.device,
                &self.queue,
                &mut encoder,
                &clipped_primitives,
                &screen_descriptor,
            );
            {
                let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("egui render pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                // egui_wgpu::Renderer::render requires a 'static render pass;
                // forget_lifetime() is the documented way to get one from a
                // pass that in practice never outlives this function.
                self.egui_renderer.render(
                    &mut pass.forget_lifetime(),
                    &clipped_primitives,
                    &screen_descriptor,
                );
            }
            for id in &full_output.textures_delta.free {
                self.egui_renderer.free_texture(id);
            }
            // `TexturesDelta` asserts on `Drop` that it was actually
            // consumed (not just borrowed-and-ignored) — the loops above
            // only read `.set`/`.free` by reference, so without this the
            // very first frame with any texture delta (egui's font atlas,
            // uploaded on frame one) panics as soon as `full_output` goes
            // out of scope at the end of this block.
            full_output.textures_delta.clear();
        }

        self.queue.submit(
            extra_command_buffers
                .into_iter()
                .chain(std::iter::once(encoder.finish())),
        );
        self.queue.present(frame);
    }
}
