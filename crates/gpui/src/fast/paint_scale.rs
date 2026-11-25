//! A subtree painted at a scale about a point, as a canvas zooms its world.
//!
//! SPIKE (SHD-6063): the subtree lays out at 1×, in window coordinates, and
//! is drawn as if every point `p` it paints were at `o + (p − o)·s`. Inside
//! the scope [`Window::scale_factor`] is the display's times `s`, so every
//! conversion to device pixels (quads, radii, borders, shadows, paths, masks)
//! scales with it, and glyphs and SVGs are rasterized at the zoomed size.
//! What is left, `o·(1 − s)` in device pixels and rounded to whole ones so
//! glyphs stay on their grid, is added to each primitive as the scene takes
//! it (`fast::layers::scene::move_primitive`, which scroll layers already
//! use). Hitboxes are stored where they draw, so hit-testing is unchanged.
//!
//! Not covered: nested scopes, scroll layers and rounded clips inside the
//! scope, retained views drawn again across a change of scale, and element
//! code that compares the pointer with its layout bounds.

use crate::{Bounds, ContentMask, Hitbox, Pixels, Point, Primitive, ScaledPixels, Window, point};

/// The scope a window is painting in, if any.
#[derive(Default)]
pub(crate) struct WindowScale {
    scope: Option<Scope>,
}

#[derive(Clone, Copy)]
struct Scope {
    scale: f32,
    /// The device-pixel offset added to each primitive.
    offset: Point<ScaledPixels>,
    /// The display's scale factor when the scope began.
    display: f32,
}

impl Scope {
    /// Where local point `p` draws, in window pixels.
    fn map(&self, p: Point<Pixels>) -> Point<Pixels> {
        point(
            Pixels(p.x.0 * self.scale + self.offset.x.0 / self.display),
            Pixels(p.y.0 * self.scale + self.offset.y.0 / self.display),
        )
    }

    /// The local point that draws at window point `p`.
    fn unmap(&self, p: Point<Pixels>) -> Point<Pixels> {
        point(
            Pixels((p.x.0 - self.offset.x.0 / self.display) / self.scale),
            Pixels((p.y.0 - self.offset.y.0 / self.display) / self.scale),
        )
    }

    fn map_bounds(&self, bounds: Bounds<Pixels>) -> Bounds<Pixels> {
        Bounds::from_corners(self.map(bounds.origin), self.map(bounds.bottom_right()))
    }

    fn unmap_bounds(&self, bounds: Bounds<Pixels>) -> Bounds<Pixels> {
        Bounds::from_corners(self.unmap(bounds.origin), self.unmap(bounds.bottom_right()))
    }
}

/// [`Window::scale_factor`]: the display's, times the scope's scale.
#[inline(always)]
pub(crate) fn scale_factor(window: &Window, display: f32) -> f32 {
    match window.fast_paint_scale.scope {
        Some(scope) => display * scope.scale,
        None => display,
    }
}

/// The display's scale factor, which layout snaps to inside a scope too:
/// the subtree lays out at 1×.
#[inline(always)]
pub(crate) fn layout_scale_factor(window: &Window) -> f32 {
    window.scale_factor
}

/// The scale factor a glyph is rasterized at. SPIKE: with `SPIKE_BUCKETS`
/// set, a scope rasterizes at the next quarter octave of its scale up, so a
/// pinch rasterizes each glyph at a few sizes instead of one per frame;
/// [`glyph_transformation`] draws it at the exact size.
#[inline(always)]
pub(crate) fn glyph_raster_scale(window: &Window, scale_factor: f32) -> f32 {
    static BUCKETS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    match window.fast_paint_scale.scope {
        Some(scope) if *BUCKETS.get_or_init(|| std::env::var("SPIKE_BUCKETS").is_ok()) => {
            let bucket = 2f32.powf((scope.scale.log2() * 4.).ceil() / 4.);
            scope.display * bucket
        }
        _ => scale_factor,
    }
}

/// The matrix that draws a glyph rasterized at `raster` at `scale` instead,
/// about its origin in device pixels: `k·p + o·(1 − k)`.
#[inline(always)]
pub(crate) fn glyph_transformation(
    scale: f32,
    raster: f32,
    origin: Point<ScaledPixels>,
) -> crate::TransformationMatrix {
    let k = scale / raster;
    if k == 1. {
        return crate::TransformationMatrix::unit();
    }
    crate::TransformationMatrix {
        rotation_scale: [[k, 0.], [0., k]],
        translation: [origin.x.0 * (1. - k), origin.y.0 * (1. - k)],
    }
}

/// A hitbox moved to where its subtree draws.
#[inline(always)]
pub(crate) fn hitbox(window: &Window, mut hitbox: Hitbox) -> Hitbox {
    if let Some(scope) = window.fast_paint_scale.scope {
        hitbox.bounds = scope.map_bounds(hitbox.bounds);
        hitbox.content_mask.bounds = scope.map_bounds(hitbox.content_mask.bounds);
    }
    hitbox
}

/// A primitive moved by the scope's offset, as the scene takes it.
#[inline(always)]
pub(crate) fn place(offset: Point<ScaledPixels>, mut primitive: Primitive) -> Primitive {
    if offset != Point::default() {
        crate::fast::layers::scene::move_primitive(&mut primitive, offset);
    }
    primitive
}

impl Window {
    /// Prepaints or paints `f` scaled by `scale` about `origin`, in window
    /// pixels: what it lays out at 1× draws and takes the pointer at
    /// `origin + (p − origin)·scale`. Call it around both the prepaint and
    /// the paint of the same children, with the same arguments.
    pub fn fast_with_paint_scale<R>(
        &mut self,
        origin: Point<Pixels>,
        scale: f32,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        assert!(
            self.fast_paint_scale.scope.is_none(),
            "SPIKE: nested paint scales aren't supported"
        );
        if !(scale > 0.0) || scale == 1.0 {
            return f(self);
        }
        let display = self.scale_factor();
        let offset = point(
            ScaledPixels((origin.x.0 * (1.0 - scale) * display).round()),
            ScaledPixels((origin.y.0 * (1.0 - scale) * display).round()),
        );
        let scope = Scope {
            scale,
            offset,
            display,
        };
        // The masks around the scope are in window pixels: the scope's own
        // starts as theirs mapped back, so that mapped forward it clips the same.
        let local = ContentMask {
            bounds: scope.unmap_bounds(self.content_mask().bounds),
        };
        self.fast_paint_scale.scope = Some(scope);
        self.next_frame.scene.fast_paint_offset = offset;
        self.content_mask_stack.push(local);
        let result = f(self);
        self.content_mask_stack.pop();
        self.next_frame.scene.fast_paint_offset = Point::default();
        self.fast_paint_scale.scope = None;
        result
    }
}
