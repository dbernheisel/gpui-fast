// Modified by Longbridge for gpui-fast.
//! A subtree painted at a scale about a point, as a canvas zooms its world.
//!
//! [`Window::fast_with_paint_scale`] prepaints or paints a subtree that lays
//! out at 1×, in its own local coordinates, and draws it as if every point
//! `p` it paints were at `o + (p − o)·s` in the window. Inside the scope,
//! [`Window::scale_factor`] is the display's times `s`, so every conversion to
//! device pixels (quads, radii, borders, shadows, paths, masks) scales with
//! it, and glyphs, SVGs and emoji are rasterized at the zoomed size, rounded
//! up to the next quarter octave so that a pinch rasterizes each at a few
//! sizes rather than one per frame. What is left, `o·(1 − s)` in device
//! pixels and rounded to whole ones so glyphs stay on their grid, is added to
//! each primitive as the scene takes it (`fast::layers::scene::move_primitive`,
//! which scroll layers already use), to the layers and rounded clips it
//! opens, and to its accessibility bounds.
//!
//! Inside a scope, everything an element reads is local, as under a CSS
//! transform: layout bounds, the content mask, the hitboxes
//! [`Window::insert_hitbox`] hands back and [`Window::mouse_position`]. The
//! window keeps each hitbox where it draws, so hit-testing is unchanged, and
//! a mouse listener registered in a scope gets the event's positions (and a
//! scroll's pixel delta) in the scope's local coordinates, reading
//! [`Window::mouse_position`] there too. What an element draws later from
//! the scope (`Window::defer_draw`) and a view built again on its own inside
//! it (`fast::splice`) draw in the same scope. Code that compares a window
//! point with layout bounds anywhere else keeps the scope, from
//! [`Window::fast_paint_scale`], to [`PaintScale::unmap`] the point.
//!
//! Scopes compose: one opened inside another scales about its origin in the
//! outer scope's coordinates, by both scales. Inside a scope, scroll
//! containers paint directly rather than into scroll layers, and retained
//! views are drawn again from the last frame only at the same scope.

use crate::{
    App, Bounds, ContentMask, DispatchPhase, FileDropEvent, Hitbox, LongPressEvent, MouseDownEvent,
    MouseEvent, MouseExitEvent, MouseMoveEvent, MousePressureEvent, MouseUpEvent, PinchEvent,
    Pixels, Point, Primitive, RenderGlyphParams, ScaledPixels, Scene, ScrollDelta, ScrollEvent,
    TouchDragEvent, TransformationMatrix, Window, point,
};
use std::any::Any;

/// The scope a window is painting in, if any, and the scope of the mouse
/// listener being dispatched, for [`Window::mouse_position`] to read.
#[derive(Default)]
pub(crate) struct WindowScale {
    scope: Option<PaintScale>,
    pointer: Option<PaintScale>,
}

/// Where a subtree painted with [`Window::fast_with_paint_scale`] draws:
/// a point `p` it lays out at draws at `p·scale + offset` in the window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaintScale {
    scale: f32,
    /// The whole device pixels added to each primitive.
    offset: Point<ScaledPixels>,
    /// The display's scale factor, which layout snaps to inside the scope.
    display: f32,
}

impl PaintScale {
    /// The scale the subtree draws at, every enclosing scope's included.
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Where local point `p` draws, in window pixels.
    pub fn map(&self, p: Point<Pixels>) -> Point<Pixels> {
        point(
            Pixels(p.x.0 * self.scale + self.offset.x.0 / self.display),
            Pixels(p.y.0 * self.scale + self.offset.y.0 / self.display),
        )
    }

    /// The local point that draws at window point `p`.
    pub fn unmap(&self, p: Point<Pixels>) -> Point<Pixels> {
        point(
            Pixels((p.x.0 - self.offset.x.0 / self.display) / self.scale),
            Pixels((p.y.0 - self.offset.y.0 / self.display) / self.scale),
        )
    }

    /// Where local `bounds` draw, in window pixels.
    pub fn map_bounds(&self, bounds: Bounds<Pixels>) -> Bounds<Pixels> {
        Bounds::new(self.map(bounds.origin), bounds.size.map(|d| d * self.scale))
    }

    /// The local bounds that draw at window `bounds`.
    pub fn unmap_bounds(&self, bounds: Bounds<Pixels>) -> Bounds<Pixels> {
        Bounds::new(
            self.unmap(bounds.origin),
            bounds.size.map(|d| d / self.scale),
        )
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

/// The scope being drawn in, if any.
#[inline(always)]
pub(crate) fn current(window: &Window) -> Option<PaintScale> {
    window.fast_paint_scale.scope
}

/// The quarter octave at or above `scale`: what a scope rasterizes glyphs,
/// SVGs and emoji at, drawing them at the exact size from there. Exact sizes
/// would rasterize each glyph again on every frame of a pinch and grow the
/// atlas without bound.
#[inline(always)]
pub(crate) fn bucket(scale: f32) -> f32 {
    // Slack for a scale that is a quarter octave but for rounding.
    2f32.powf((scale.log2() * 4. - 1e-3).ceil() / 4.)
}

/// The scale factor a glyph or emoji is rasterized at.
#[inline(always)]
pub(crate) fn glyph_raster_scale(window: &Window, scale_factor: f32) -> f32 {
    match window.fast_paint_scale.scope {
        Some(scope) => scope.display * bucket(scope.scale),
        None => scale_factor,
    }
}

/// The scale an SVG is rasterized at, `smooth` times its device size
/// outside a scope: inside one, rasterized at the quarter octave at or above
/// its scale and drawn at the exact size from there.
#[inline(always)]
pub(crate) fn svg_raster(window: &Window, smooth: f32) -> f32 {
    match window.fast_paint_scale.scope {
        Some(scope) => smooth * bucket(scope.scale) / scope.scale,
        None => smooth,
    }
}

/// The matrix that draws a glyph rasterized at `params.scale_factor` at
/// `scale` instead, about its origin in device pixels: `k·p + o·(1 − k)`.
#[inline(always)]
pub(crate) fn glyph_matrix(
    scale: f32,
    params: &RenderGlyphParams,
    origin: Point<ScaledPixels>,
) -> TransformationMatrix {
    let k = scale / params.scale_factor;
    if k == 1. {
        return TransformationMatrix::unit();
    }
    TransformationMatrix {
        rotation_scale: [[k, 0.], [0., k]],
        translation: [origin.x.0 * (1. - k), origin.y.0 * (1. - k)],
    }
}

/// An emoji's sprite bounds, rasterized at `params.scale_factor`, drawn at
/// `scale` instead about its origin: an emoji's sprite has no matrix, and its
/// tile stretches over its bounds.
#[inline(always)]
pub(crate) fn emoji(
    bounds: Bounds<ScaledPixels>,
    origin: Point<ScaledPixels>,
    scale: f32,
    params: &RenderGlyphParams,
) -> Bounds<ScaledPixels> {
    let k = scale / params.scale_factor;
    if k == 1. {
        return bounds;
    }
    Bounds::new(
        point(
            ScaledPixels(origin.x.0 + (bounds.origin.x.0 - origin.x.0) * k),
            ScaledPixels(origin.y.0 + (bounds.origin.y.0 - origin.y.0) * k),
        ),
        bounds.size.map(|d| ScaledPixels(d.0 * k)),
    )
}

/// A hitbox as the window keeps it: where its subtree draws.
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

/// Device bounds converted inside a scope, moved by its offset.
#[inline(always)]
pub(crate) fn place_bounds(scene: &Scene, bounds: Bounds<ScaledPixels>) -> Bounds<ScaledPixels> {
    Bounds::new(bounds.origin + scene.fast_paint_offset, bounds.size)
}

/// Sets the scene's offset aside while it replays primitives painted on an
/// earlier frame, which hold the offset they were painted at, for
/// [`resume`] to put back.
#[inline(always)]
pub(crate) fn suspend(scene: &mut Scene) -> Point<ScaledPixels> {
    std::mem::take(&mut scene.fast_paint_offset)
}

#[inline(always)]
pub(crate) fn resume(scene: &mut Scene, offset: Point<ScaledPixels>) {
    scene.fast_paint_offset = offset;
}

/// An accessibility node's bounds in window pixels and the scale factor
/// that converts them to device pixels.
#[inline(always)]
pub(crate) fn a11y_bounds(window: &Window, bounds: Bounds<Pixels>) -> (Bounds<Pixels>, f32) {
    match window.fast_paint_scale.scope {
        Some(scope) => (scope.map_bounds(bounds), scope.display),
        None => (bounds, window.scale_factor),
    }
}

/// [`Window::mouse_position`]: local inside a scope, or inside a mouse
/// listener registered in one.
#[inline(always)]
pub(crate) fn pointer(window: &Window, position: Point<Pixels>) -> Point<Pixels> {
    match window
        .fast_paint_scale
        .scope
        .or(window.fast_paint_scale.pointer)
    {
        Some(scope) => scope.unmap(position),
        None => position,
    }
}

/// What [`enter`] replaced, for [`leave`] to put back.
pub(crate) struct Entered {
    previous: Option<PaintScale>,
    pushed_mask: bool,
}

/// Draws in `scope` from here, with the content mask, which is in the
/// coordinates of the scope drawn in until now, seen from `scope`.
pub(crate) fn enter(window: &mut Window, scope: Option<PaintScale>) -> Entered {
    let previous = window.fast_paint_scale.scope;
    if scope == previous {
        return Entered {
            previous,
            pushed_mask: false,
        };
    }
    let mask = window.content_mask().bounds;
    let mask = previous.map_or(mask, |previous| previous.map_bounds(mask));
    let mask = scope.map_or(mask, |scope| scope.unmap_bounds(mask));
    window.content_mask_stack.push(ContentMask { bounds: mask });
    set(window, scope);
    Entered {
        previous,
        pushed_mask: true,
    }
}

pub(crate) fn leave(window: &mut Window, entered: Entered) {
    if entered.pushed_mask {
        window.content_mask_stack.pop();
        set(window, entered.previous);
    }
}

/// [`enter`] for the next frame's draw of a deferred element, drawn in the
/// scope it was deferred from.
pub(crate) fn enter_deferred(window: &mut Window, index: usize) -> Entered {
    let scope = window.next_frame.deferred_draws[index].fast_paint_scale;
    enter(window, scope)
}

/// Draws in `scope` for a view built again on its own where it was drawn
/// last frame, whose content mask is already the scope's, returning the
/// scope to put back.
pub(crate) fn enter_gap(window: &mut Window, scope: Option<PaintScale>) -> Option<PaintScale> {
    let previous = window.fast_paint_scale.scope;
    set(window, scope);
    previous
}

pub(crate) fn leave_gap(window: &mut Window, previous: Option<PaintScale>) {
    set(window, previous);
}

fn set(window: &mut Window, scope: Option<PaintScale>) {
    window.fast_paint_scale.scope = scope;
    window.next_frame.scene.fast_paint_offset = scope.map_or(Point::default(), |s| s.offset);
}

/// Calls a mouse listener an element registered in `scope`, if any: it gets
/// the event's positions in the scope's local coordinates, and reads
/// [`Window::mouse_position`] there too.
#[inline(always)]
pub(crate) fn listen<E: MouseEvent>(
    scope: Option<PaintScale>,
    listener: &mut impl FnMut(&E, DispatchPhase, &mut Window, &mut App),
    event: &E,
    phase: DispatchPhase,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(scope) = scope else {
        return listener(event, phase, window, cx);
    };
    let outer = window.fast_paint_scale.pointer.replace(scope);
    with_local(event, scope, |event| listener(event, phase, window, cx));
    window.fast_paint_scale.pointer = outer;
}

/// Calls `f` with `event` in `scope`'s local coordinates.
fn with_local<E: 'static>(event: &E, scope: PaintScale, f: impl FnOnce(&E)) {
    let any = event as &dyn Any;
    let local = |p: Point<Pixels>| scope.unmap(p);
    if let Some(e) = any.downcast_ref::<MouseMoveEvent>() {
        return call(
            f,
            MouseMoveEvent {
                position: local(e.position),
                ..e.clone()
            },
        );
    }
    if let Some(e) = any.downcast_ref::<MouseDownEvent>() {
        return call(
            f,
            MouseDownEvent {
                position: local(e.position),
                ..e.clone()
            },
        );
    }
    if let Some(e) = any.downcast_ref::<MouseUpEvent>() {
        return call(
            f,
            MouseUpEvent {
                position: local(e.position),
                ..e.clone()
            },
        );
    }
    if let Some(e) = any.downcast_ref::<ScrollEvent>() {
        let delta = match e.delta {
            ScrollDelta::Pixels(delta) => ScrollDelta::Pixels(delta.map(|d| d / scope.scale)),
            lines => lines,
        };
        return call(
            f,
            ScrollEvent {
                position: local(e.position),
                delta,
                ..e.clone()
            },
        );
    }
    if let Some(e) = any.downcast_ref::<MouseExitEvent>() {
        return call(
            f,
            MouseExitEvent {
                position: local(e.position),
                ..e.clone()
            },
        );
    }
    if let Some(e) = any.downcast_ref::<MousePressureEvent>() {
        return call(
            f,
            MousePressureEvent {
                position: local(e.position),
                ..e.clone()
            },
        );
    }
    if let Some(e) = any.downcast_ref::<PinchEvent>() {
        return call(
            f,
            PinchEvent {
                position: local(e.position),
                ..e.clone()
            },
        );
    }
    if let Some(e) = any.downcast_ref::<TouchDragEvent>() {
        let (start_position, position) = (local(e.start_position), local(e.position));
        return call(
            f,
            TouchDragEvent {
                start_position,
                position,
                ..e.clone()
            },
        );
    }
    if let Some(e) = any.downcast_ref::<LongPressEvent>() {
        let (start_position, position) = (local(e.start_position), local(e.position));
        return call(
            f,
            LongPressEvent {
                start_position,
                position,
                ..e.clone()
            },
        );
    }
    if let Some(e) = any.downcast_ref::<FileDropEvent>() {
        let mut e = e.clone();
        match &mut e {
            FileDropEvent::Entered { position, .. }
            | FileDropEvent::Pending { position }
            | FileDropEvent::Submit { position } => *position = local(*position),
            FileDropEvent::Exited | FileDropEvent::Ended => {}
        }
        return call(f, e);
    }
    f(event)
}

fn call<E: 'static, T: 'static>(f: impl FnOnce(&E), local: T) {
    match (&local as &dyn Any).downcast_ref::<E>() {
        Some(event) => f(event),
        None => unreachable!("an event is the type it was downcast to"),
    }
}

impl Window {
    /// Prepaints or paints `f` scaled by `scale` about `origin`: what it lays
    /// out at 1× draws, and takes the pointer, at `origin + (p − origin)·scale`.
    /// `origin` is in the coordinates `f`'s elements are prepainted in, which
    /// are an enclosing scope's inside one. Call it around both the prepaint
    /// and the paint of the same children, with the same arguments. A scale
    /// of 1, or one that isn't positive, draws `f` as it is.
    ///
    /// Inside, the elements read layout bounds, the content mask, hitboxes and
    /// the pointer in their own coordinates, and their mouse listeners get
    /// events in them; see [`Window::fast_paint_scale`] for anything else.
    pub fn fast_with_paint_scale<R>(
        &mut self,
        origin: Point<Pixels>,
        scale: f32,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        if scale.is_nan() || scale <= 0.0 || scale == 1.0 {
            return f(self);
        }
        let outer = self.fast_paint_scale.scope;
        let display = self.scale_factor;
        let outer_scale = outer.map_or(1., |outer| outer.scale);
        let outer_offset = outer.map_or(Point::default(), |outer| outer.offset);
        let device = display * outer_scale;
        let scope = PaintScale {
            scale: outer_scale * scale,
            offset: outer_offset
                + point(
                    ScaledPixels((origin.x.0 * (1.0 - scale) * device).round()),
                    ScaledPixels((origin.y.0 * (1.0 - scale) * device).round()),
                ),
            display,
        };
        let entered = enter(self, Some(scope));
        let result = f(self);
        leave(self, entered);
        result
    }

    /// The scope of the [`Window::fast_with_paint_scale`] being prepainted or
    /// painted, if any. An element that compares a window point with its
    /// layout bounds outside its mouse listeners, such as an input method's
    /// position, keeps it to [`PaintScale::unmap`] the point.
    pub fn fast_paint_scale(&self) -> Option<PaintScale> {
        self.fast_paint_scale.scope
    }
}
