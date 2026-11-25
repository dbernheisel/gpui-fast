# Paint scale

`Window::fast_with_paint_scale(origin, scale, f)` prepaints or paints `f` as if
every point `p` it lays out at were drawn at `origin + (p − origin)·scale`, the
way a canvas zooms its world. It lives in `crates/gpui/src/fast/paint_scale.rs`.
Upstream has nothing like it, and won't take new drawing API, so it's an
exception to rule 5 of [`upstream-sync.md`](upstream-sync.md).

A canvas lays its world out at 1×, then wraps the world's prepaint and its
paint in the same call:

```rust
self.world.layout_as_root(AvailableSpace::min_size(), window, cx);
window.fast_with_paint_scale(origin, zoom, |window| {
    self.world.prepaint_at(origin, window, cx)
});
// and in paint, with the same origin and zoom:
window.fast_with_paint_scale(origin, zoom, |window| self.world.paint(window, cx));
```

`crates/gpui_perf/examples/canvas_scale.rs` is that canvas, with 200 cards.

## What the scope does

- **Device pixels.** Inside the scope `Window::scale_factor` is the display's
  times the scale, so every conversion to device pixels (quads, radii,
  borders, shadows, paths, masks) scales with it. What's left, `origin·(1 −
  scale)` in whole device pixels, is added to each primitive as the scene takes
  it, to the layers and rounded clips it opens, and to accessibility bounds.
- **Text, SVGs and emoji** rasterize at the quarter octave at or above the
  scale and draw at the exact size from there: glyphs through their sprite's
  matrix, SVGs and emoji by the sprite's bounds. Exact sizes rasterized each
  glyph again on every frame of a pinch, and the atlas grew without bound
  (37, 98 and 191 MB over 1, 3 and 6 sweeps in the spike). A sweep from 0.25
  to 4 rasterizes at 17 scales.
- **Layout** stays at 1× and snaps to the display's scale factor, so the
  bounds an element reads in the scope are its own.
- **Everything an element reads is local**, as under a CSS transform: layout
  bounds, the content mask, the hitboxes `Window::insert_hitbox` returns, and
  `Window::mouse_position`. The window keeps each hitbox where it draws, so
  hit-testing needs no change.
- **Mouse listeners** registered in the scope get the event's positions in
  its coordinates (and a scroll's pixel delta divided by the scale), and read
  `mouse_position` there too, so a listener that compares the pointer with
  bounds it kept from prepaint or paint (a click, a drag, a selection, a
  caret) compares like with like.
- **Deferred draws** (`Window::defer_draw`: popovers, menus, tooltips) draw in
  the scope they were deferred from.
- **Nested scopes** compose: an inner scope's origin is in the outer scope's
  coordinates, and its scale multiplies the outer one.
- **Scroll containers** inside a scope paint directly rather than into scroll
  layers (`fast::layers::active` is false there). A scope inside a scroll
  layer paints into it as anything else does.
- **Retained views** are drawn again from the last frame only at the same
  scope (`RetainedContext::paint_scale`), and a view built again on its own
  inside one (`fast::splice`) is built in it.

`Window::fast_paint_scale` returns the scope being drawn, as a `PaintScale`
with `map`, `unmap`, `map_bounds`, `unmap_bounds` and `scale`, for anything
that compares a window point with layout bounds outside a mouse listener.

## What it doesn't do

- **Input method bounds.** `InputHandler::bounds_for_range` returns a text
  field's local bounds, so the platform places an input method's candidate
  window as at 1×. An element that wants it right maps them with the
  `PaintScale` it kept.
- **Code outside the scope that reads its elements' bounds**, such as
  `anchored` fitting a popover into the window (it fits it in the window's
  size as seen from the scope) and the drag preview (drawn at the pointer at
  1×, offset by the local grab point).
- **Native views and composition surfaces** (`with_composition_surface`,
  `NativeView`) are placed from their local bounds, not where the scope draws
  them. Don't put them in a scope.
- A scale of 1, or one that isn't positive, draws `f` as it is.

## Checking it

`cargo test -p gpui --features test-support --lib fast::tests::paint_scale`
covers mapping, nesting, the pointer and local events, layout, retained views
across a change of scale, splices, deferred draws, scroll containers, rounded
clips and layers, accessibility bounds and the glyph buckets.

The example measures it. Run it from the repository root, on macOS without
the Metal toolchain with `--features gpui_platform_fast/runtime_shaders`:

```sh
SPIKE_PINCH=3 SPIKE_REPEAT=6 cargo run -p gpui_perf --release --example canvas_scale
SPIKE_ZOOM=2 SPIKE_CLICK=1 cargo run -p gpui_perf --release --example canvas_scale
SPIKE_ZOOM=4 SPIKE_CAPTURE=/tmp/zoom4.png cargo run -p gpui_perf --release --example canvas_scale \
  --features gpui_platform_fast/test-support
```

The sweep prints frame intervals, the world layer's prepaint and paint time
(p50, p95, max) and the process's footprint, whose `IOAccelerator` line is the
glyph atlas.
