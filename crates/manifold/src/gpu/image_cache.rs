//! The favicon and node-image texture cache: one-shot RGBA uploads registered with egui, keyed by origin
//! (favicons, FIFO-bounded) or node id (images).

use super::*;

/// How many distinct origins' favicons `GpuState` keeps textures for before
/// evicting the oldest (FIFO, not true LRU -- simpler, and a revisited
/// origin just re-fetches, same cost as a cache miss would have anyway).
/// Same bounded-cache posture `MAX_CLOSED_TABS`/`MAX_CLOSED_GROUPS` already
/// have elsewhere in this app, sized generously since a favicon texture is
/// tiny (typically 16-32px) and a real session could plausibly visit a
/// couple hundred distinct sites.
const MAX_CACHED_FAVICONS: usize = 200;

impl GpuState {
    /// One texture for tightly packed RGBA8, or `None` (logged) when it can't
    /// be one: a wrong byte count, or a side past the device limit. Callers
    /// downscale with `rgba::fit_image` first; this is the last guard, since
    /// wgpu's default error handler panics on an oversize texture.
    fn register_rgba(
        &mut self,
        label: &str,
        rgba: &[u8],
        width: u32,
        height: u32,
    ) -> Option<(wgpu::Texture, wgpu::TextureView)> {
        if !frame_is_valid(rgba.len(), width, height, self.device.limits().max_texture_dimension_2d) {
            eprintln!("{label}: refusing a {width}x{height} texture ({} bytes)", rgba.len());
            return None;
        }
        let texture = self.device.create_texture_with_data(
            &self.queue,
            &wgpu::TextureDescriptor {
                label: Some(label),
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
        Some((texture, view))
    }

    /// `rgba` is already tightly-packed RGBA -- registered the same way as
    /// a thumbnail, in its own map since a favicon and a tab's content
    /// thumbnail are unrelated sizes and update independently. `key` is an
    /// *origin* (see `favicons`' own doc comment), not a tab id.
    pub fn upload_favicon(&mut self, key: &str, rgba: &[u8], width: u32, height: u32) {
        let Some((texture, view)) = self.register_rgba("favicon texture", rgba, width, height) else { return };
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
    /// `images`' own doc comment), called at creation, restore or Refresh,
    /// not every frame like a tab's thumbnail. Uploading over an existing
    /// key frees the old registration itself.
    pub fn upload_image(&mut self, key: &str, rgba: &[u8], width: u32, height: u32) {
        let Some((texture, view)) = self.register_rgba("image node texture", rgba, width, height) else { return };
        // A second upload for one key (a PDF re-rendered before its first
        // render landed) must not leak the first registration.
        self.free_image(key);
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
}
