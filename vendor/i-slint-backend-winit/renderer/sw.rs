// Copyright © SixtyFPS GmbH <info@slint.dev>
// SPDX-License-Identifier: GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0

//! Delegate the rendering to the [`i_slint_renderer_software::SoftwareRenderer`]

use core::num::NonZeroU32;
use core::ops::DerefMut;
use i_slint_core::graphics::Rgb8Pixel;
use i_slint_core::platform::PlatformError;
use i_slint_core::renderer::DrawOutcome;
pub use i_slint_renderer_software::SoftwareRenderer;
use i_slint_renderer_software::{PremultipliedRgbaColor, RepaintBufferType, TargetPixel};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use winit::event_loop::ActiveEventLoop;

use super::WinitCompatibleRenderer;

pub struct WinitSoftwareRenderer {
    renderer: SoftwareRenderer,
    _context: RefCell<Option<softbuffer::Context<Arc<winit::window::Window>>>>,
    surface: RefCell<
        Option<softbuffer::Surface<Arc<winit::window::Window>, Arc<winit::window::Window>>>,
    >,
}

#[repr(transparent)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct SoftBufferPixel(pub u32);

impl From<SoftBufferPixel> for PremultipliedRgbaColor {
    #[inline]
    fn from(pixel: SoftBufferPixel) -> Self {
        let v = pixel.0;
        PremultipliedRgbaColor {
            red: (v >> 16) as u8,
            green: (v >> 8) as u8,
            blue: v as u8,
            alpha: (v >> 24) as u8,
        }
    }
}

impl From<PremultipliedRgbaColor> for SoftBufferPixel {
    #[inline]
    fn from(pixel: PremultipliedRgbaColor) -> Self {
        Self(
            (pixel.alpha as u32) << 24
                | ((pixel.red as u32) << 16)
                | ((pixel.green as u32) << 8)
                | (pixel.blue as u32),
        )
    }
}

impl TargetPixel for SoftBufferPixel {
    fn blend(&mut self, color: PremultipliedRgbaColor) {
        let mut x = PremultipliedRgbaColor::from(*self);
        x.blend(color);
        *self = x.into();
    }

    fn from_rgb(r: u8, g: u8, b: u8) -> Self {
        Self(0xff000000 | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32))
    }

    fn background() -> Self {
        Self(0)
    }
}

impl WinitSoftwareRenderer {
    pub fn new_suspended(
        shared_backend_data: &Rc<crate::SharedBackendData>,
    ) -> Result<Box<dyn WinitCompatibleRenderer>, PlatformError> {
        if shared_backend_data.requested_graphics_api.is_some() {
            return Err("The software renderer doesn't support a graphics API selection".into());
        }
        Ok(Box::new(Self {
            renderer: SoftwareRenderer::new(),
            _context: RefCell::new(None),
            surface: RefCell::new(None),
        }))
    }
}

// Sonkore: SONKORE_FRAME_STATS=1 prints, every 2 s, how many frames were
// drawn, how long they took (the whole present, and the renderer alone),
// how many pixels the dirty regions covered and the buffer ages seen.
#[derive(Default)]
struct FrameStats {
    since: Option<std::time::Instant>,
    frames: u32,
    total: std::time::Duration,
    render: std::time::Duration,
    pixels: u64,
    ages: [u32; 3],
}

thread_local! {
    static FRAME_STATS: RefCell<FrameStats> = RefCell::new(FrameStats::default());
    static FRAME_STATS_ON: bool = std::env::var_os("SONKORE_FRAME_STATS").is_some();
}

fn frame_stats(total: std::time::Duration, render: std::time::Duration, pixels: u64, age: u8, rects: String) {
    if !FRAME_STATS_ON.with(|on| *on) {
        return;
    }
    FRAME_STATS.with(|s| {
        let mut s = s.borrow_mut();
        let now = std::time::Instant::now();
        let since = *s.since.get_or_insert(now);
        s.frames += 1;
        s.total += total;
        s.render += render;
        s.pixels += pixels;
        s.ages[(age as usize).min(2)] += 1;
        if now.duration_since(since).as_secs_f32() >= 2.0 {
            let n = s.frames.max(1);
            eprintln!(
                "frames {} in {:.1} s: present {:.2} ms, render {:.2} ms, {} px a frame, ages {:?}",
                s.frames,
                now.duration_since(since).as_secs_f32(),
                s.total.as_secs_f64() * 1000.0 / n as f64,
                s.render.as_secs_f64() * 1000.0 / n as f64,
                s.pixels / n as u64,
                s.ages
            );
            eprintln!("  last frame's regions: {rects}");
            *s = FrameStats { since: Some(now), ..Default::default() };
        }
    });
}

impl super::WinitCompatibleRenderer for WinitSoftwareRenderer {
    fn render(&self, window: &i_slint_core::api::Window) -> Result<DrawOutcome, PlatformError> {
        let frame_start = std::time::Instant::now();
        let size = window.size();

        let Some((width, height)) = size.width.try_into().ok().zip(size.height.try_into().ok())
        else {
            // Nothing to render
            return Ok(DrawOutcome::Success);
        };

        let mut borrowed_surface = self.surface.borrow_mut();
        let Some(surface) = borrowed_surface.as_mut() else {
            // Nothing to render
            return Ok(DrawOutcome::Success);
        };

        let winit_window = surface.window().clone();

        surface
            .resize(width, height)
            .map_err(|e| format!("Error resizing softbuffer surface: {e}"))?;

        let mut target_buffer = surface
            .buffer_mut()
            .map_err(|e| format!("Error retrieving softbuffer rendering buffer: {e}"))?;

        let age = target_buffer.age();
        self.renderer.set_repaint_buffer_type(match age {
            1 => RepaintBufferType::ReusedBuffer,
            2 => RepaintBufferType::SwappedBuffers,
            _ => RepaintBufferType::NewBuffer,
        });

        let render_start = std::time::Instant::now();
        let region = if std::env::var_os("SLINT_LINE_BY_LINE").is_none() {
            let buffer: &mut [SoftBufferPixel] =
                bytemuck::cast_slice_mut(target_buffer.deref_mut());
            self.renderer.render(buffer, width.get() as usize)
        } else {
            // SLINT_LINE_BY_LINE is set and this is a debug mode where we also render in a Rgb565Pixel
            struct FrameBuffer<'a> {
                buffer: &'a mut [u32],
                line: Vec<i_slint_renderer_software::Rgb565Pixel>,
            }
            impl i_slint_renderer_software::LineBufferProvider for FrameBuffer<'_> {
                type TargetPixel = i_slint_renderer_software::Rgb565Pixel;
                fn process_line(
                    &mut self,
                    line: usize,
                    range: core::ops::Range<usize>,
                    render_fn: impl FnOnce(&mut [Self::TargetPixel]),
                ) {
                    let line_begin = line * self.line.len();
                    let sub = &mut self.line[..range.len()];
                    render_fn(sub);
                    for (dst, src) in self.buffer[line_begin..][range].iter_mut().zip(sub) {
                        let p = Rgb8Pixel::from(*src);
                        *dst =
                            0xff000000 | ((p.r as u32) << 16) | ((p.g as u32) << 8) | (p.b as u32);
                    }
                }
            }
            self.renderer.render_by_line(FrameBuffer {
                buffer: &mut target_buffer,
                line: vec![Default::default(); width.get() as usize],
            })
        };

        let render_time = render_start.elapsed();
        let region_pixels: u64 = region.iter().map(|(_, size)| size.width as u64 * size.height as u64).sum();
        // Sonkore: the renderer draws floor(logical size × scale) pixels, which at a
        // fractional scale can be one short of the window's: that last column / row would
        // keep whatever the buffer held (visible now that the buffer has alpha). It repeats
        // the one next to it.
        let mut edges = Vec::new();
        {
            let sf = window.scale_factor();
            let (w, h) = (width.get() as usize, height.get() as usize);
            let drawn = |n: usize| (((n as f32 / sf) * sf) as usize).clamp(1, n);
            let (dw, dh) = (drawn(w), drawn(h));
            let buffer: &mut [u32] = target_buffer.deref_mut();
            if dw < w {
                for row in buffer.chunks_exact_mut(w).take(h) {
                    let last = row[dw - 1];
                    row[dw..].fill(last);
                }
                edges.push((dw, 0, w - dw, h));
            }
            if dh < h {
                let (top, bottom) = buffer.split_at_mut(dh * w);
                let last = &top[(dh - 1) * w..];
                for row in bottom.chunks_exact_mut(w).take(h - dh) {
                    row.copy_from_slice(last);
                }
                edges.push((0, dh, w, h - dh));
            }
        }

        let damage = region
            .iter()
            .filter_map(|(pos, size)| {
                Some(softbuffer::Rect {
                    x: pos.x as u32,
                    y: pos.y as u32,
                    width: NonZeroU32::new(size.width)?,
                    height: NonZeroU32::new(size.height)?,
                })
            })
            .collect::<Vec<_>>();
        let mut damage = damage;
        if !damage.is_empty() {
            for (x, y, w, h) in edges {
                if let (Some(width), Some(height)) =
                    (NonZeroU32::new(w as u32), NonZeroU32::new(h as u32))
                {
                    damage.push(softbuffer::Rect { x: x as u32, y: y as u32, width, height });
                }
            }
        }
        if !damage.is_empty() {
            winit_window.pre_present_notify();
            target_buffer
                .present_with_damage(&damage)
                .map_err(|e| format!("Error presenting softbuffer buffer: {e}"))?;
        }
        frame_stats(frame_start.elapsed(), render_time, region_pixels, age, format!("{:?}", region.iter().map(|(p, s)| (p.x, p.y, s.width, s.height)).collect::<Vec<_>>()));
        Ok(DrawOutcome::Success)
    }

    fn as_core_renderer(&self) -> &dyn i_slint_core::renderer::Renderer {
        &self.renderer
    }

    fn occluded(&self, _: bool) {
        // On X11 and Windows, the buffer is completely cleared when the window is hidden
        // and the buffer age doesn't respect that, so clean the partial rendering cache
        self.renderer.set_repaint_buffer_type(RepaintBufferType::NewBuffer);
    }

    fn resume(
        &self,
        active_event_loop: &ActiveEventLoop,
        window_attributes: winit::window::WindowAttributes,
        _window_adapter_weak: std::rc::Weak<crate::winitwindowadapter::WinitWindowAdapter>,
    ) -> Result<Arc<winit::window::Window>, PlatformError> {
        let winit_window =
            active_event_loop.create_window(window_attributes).map_err(|winit_os_error| {
                PlatformError::from(format!(
                    "Error creating native window for software rendering: {winit_os_error}"
                ))
            })?;
        let winit_window = Arc::new(winit_window);

        let context = softbuffer::Context::new(winit_window.clone())
            .map_err(|e| format!("Error creating softbuffer context: {e}"))?;

        let surface = softbuffer::Surface::new(&context, winit_window.clone()).map_err(
            |softbuffer_error| format!("Error creating softbuffer surface: {softbuffer_error}"),
        )?;

        *self._context.borrow_mut() = Some(context);
        *self.surface.borrow_mut() = Some(surface);

        Ok(winit_window)
    }

    fn suspend(&self) -> Result<(), PlatformError> {
        drop(self.surface.borrow_mut().take());
        drop(self._context.borrow_mut().take());
        Ok(())
    }
}
