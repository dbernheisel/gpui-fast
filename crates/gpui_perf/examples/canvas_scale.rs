//! 200 cards on a world layer that `Window::fast_with_paint_scale` zooms, as a Shade
//! `<Canvas>` zooms its nodes: the paint scale's performance check
//! (docs/paint-scale.md).
//!
//! `SPIKE_ZOOM=2` holds one zoom. `SPIKE_PINCH=3` sweeps the zoom from 0.25 to 4 over that many
//! seconds, then prints frame and paint times and the process's footprint before and after;
//! `SPIKE_REPEAT=6` sweeps up and down that many times, to see whether memory levels off.
//! `SPIKE_CLICK=1` clicks the first card's button where it draws at the zoom, then where it
//! would be at 1×, and prints which cards counted a click. `SPIKE_CAPTURE=PATH` saves the window
//! after 2 s (with the `test-support` feature of `gpui_platform`). A real pinch or ⌘-scroll
//! zooms too.

extern crate gpui_fast as gpui;
extern crate gpui_platform_fast as gpui_platform;

#[path = "../../gpui/examples/example_support/fonts.rs"]
mod example_support;

use gpui::{
    AnyElement, App, AvailableSpace, Bounds, Context, ElementId, GlobalElementId,
    InspectorElementId, LayoutId, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PinchEvent, Pixels, PlatformInput, Point, ScrollEvent, Style, Window,
    WindowBounds, WindowOptions, div, point, prelude::*, px, rgb, size,
};
use gpui_platform::application;
use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

const COLUMNS: usize = 20;
const ROWS: usize = 10;
const CARD_W: f32 = 160.;
const CARD_H: f32 = 64.;
const STEP_X: f32 = 200.;
const STEP_Y: f32 = 100.;
const MARGIN: f32 = 40.;

#[derive(Default)]
struct Stats {
    /// Time to prepaint and paint the world layer, per frame.
    paint: Vec<Duration>,
    /// Time between frames during a sweep.
    frames: Vec<Duration>,
    last_frame: Option<Instant>,
}

struct Board {
    zoom: f32,
    /// The world point at the viewport's top leading corner.
    camera: Point<Pixels>,
    clicks: Vec<usize>,
    sweep: Option<(Instant, f32)>,
    stats: Rc<RefCell<Stats>>,
}

impl Board {
    fn world(&self, cx: &mut Context<Self>) -> AnyElement {
        let world_size = size(
            px(MARGIN * 2. + STEP_X * COLUMNS as f32),
            px(MARGIN * 2. + STEP_Y * ROWS as f32),
        );
        div()
            .relative()
            .w(world_size.width)
            .h(world_size.height)
            .children((0..COLUMNS * ROWS).map(|i| {
                // SPIKE_NOTEXT draws the cards' boxes only, to tell glyphs from the rest.
                if std::env::var("SPIKE_NOTEXT").is_ok() {
                    let (column, row) = (i % COLUMNS, i / COLUMNS);
                    return div()
                        .absolute()
                        .left(px(MARGIN + STEP_X * column as f32))
                        .top(px(MARGIN + STEP_Y * row as f32))
                        .w(px(CARD_W))
                        .h(px(CARD_H))
                        .bg(rgb(0xffffff))
                        .border_1()
                        .border_color(rgb(0xc8c4ba))
                        .rounded_md()
                        .shadow_sm();
                }
                let (column, row) = (i % COLUMNS, i / COLUMNS);
                let status = ["done", "running", "failed", "blocked"][i % 4];
                div()
                    .absolute()
                    .left(px(MARGIN + STEP_X * column as f32))
                    .top(px(MARGIN + STEP_Y * row as f32))
                    .w(px(CARD_W))
                    .h(px(CARD_H))
                    .p_2()
                    .flex()
                    .flex_col()
                    .justify_between()
                    .bg(rgb(0xffffff))
                    .border_1()
                    .border_color(rgb(0xc8c4ba))
                    .rounded_md()
                    .shadow_sm()
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .text_sm()
                            .text_color(rgb(0x1d1d1b))
                            .child(format!("Stage {i}"))
                            .child(div().text_xs().text_color(rgb(0x5f5d57)).child(status)),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .items_center()
                            .text_xs()
                            .text_color(rgb(0x5f5d57))
                            .child(format!("Clicks: {}", self.clicks[i]))
                            .child(
                                div()
                                    .id(ElementId::NamedInteger("run".into(), i as u64))
                                    .px_2()
                                    .rounded_sm()
                                    .bg(rgb(0x2f5d8a))
                                    .text_color(rgb(0xffffff))
                                    .hover(|style| style.bg(rgb(0x4a7db0)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.clicks[i] += 1;
                                        cx.notify();
                                    }))
                                    .child("Run"),
                            ),
                    )
            }))
            .into_any_element()
    }
}

/// Where the first card's Run button's middle is, in world points.
fn first_button() -> Point<Pixels> {
    // The card's padding is 8 and the button sits at its trailing bottom corner.
    point(
        px(MARGIN + CARD_W - 8. - 16.),
        px(MARGIN + CARD_H - 8. - 8.),
    )
}

#[allow(
    clippy::disallowed_methods,
    reason = "the footprint is written once, after the sweep"
)]
impl Render for Board {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some((started, seconds)) = self.sweep {
            // SPIKE_REPEAT sweeps up and down that many times, to see whether memory levels off.
            let repeat: f32 = std::env::var("SPIKE_REPEAT")
                .ok()
                .and_then(|r| r.parse().ok())
                .unwrap_or(1.);
            let u = (started.elapsed().as_secs_f32() / seconds).min(repeat);
            let t = if u >= repeat { 1. } else { u.fract() };
            let along = if (u as u32).is_multiple_of(2) {
                t
            } else {
                1. - t
            };
            // SPIKE_FLAT holds the zoom at 1 through the sweep: the same redraws, no scale.
            self.zoom = if std::env::var("SPIKE_FLAT").is_ok() {
                1.
            } else {
                0.25 * 16f32.powf(along)
            };
            let t = u / repeat;
            let mut stats = self.stats.borrow_mut();
            let now = Instant::now();
            if let Some(last) = stats.last_frame.replace(now) {
                stats.frames.push(now - last);
            }
            drop(stats);
            if t < 1. {
                window.request_animation_frame();
            } else {
                self.sweep = None;
                report(&self.stats.borrow(), "after the sweep");
                if let Ok(path) = std::env::var("SPIKE_FOOTPRINT") {
                    let full = std::process::Command::new("/usr/bin/footprint")
                        .args(["-p", &std::process::id().to_string()])
                        .output()
                        .map(|o| o.stdout)
                        .unwrap_or_default();
                    std::fs::write(path, full).ok();
                }
            }
        }
        let world = self.world(cx);
        div()
            .size_full()
            .bg(rgb(0xf1eee7))
            .on_scroll(cx.listener(|this, event: &ScrollEvent, window, cx| {
                let delta = event.delta.pixel_delta(window.line_height());
                if event.modifiers.platform {
                    this.zoom = (this.zoom * (1. + f32::from(delta.y) / 200.)).clamp(0.25, 4.);
                } else {
                    this.camera = this.camera - delta.map(|d| d / this.zoom);
                }
                cx.notify();
            }))
            .on_pinch(cx.listener(|this, event: &PinchEvent, _, cx| {
                this.zoom = (this.zoom * (1. + event.delta)).clamp(0.25, 4.);
                cx.notify();
            }))
            .child(ScaledLayer {
                world,
                zoom: self.zoom,
                camera: self.camera,
                stats: self.stats.clone(),
            })
    }
}

/// The world, laid out at 1× and drawn at the camera's zoom.
struct ScaledLayer {
    world: AnyElement,
    zoom: f32,
    camera: Point<Pixels>,
    stats: Rc<RefCell<Stats>>,
}

impl ScaledLayer {
    /// Where the world's origin draws: the viewport's corner less the camera, zoomed.
    fn origin(&self, bounds: Bounds<Pixels>) -> Point<Pixels> {
        bounds.origin - self.camera.map(|c| c * self.zoom)
    }
}

impl Element for ScaledLayer {
    type RequestLayoutState = ();
    type PrepaintState = Instant;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size = size(gpui::relative(1.).into(), gpui::relative(1.).into());
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Instant {
        let started = Instant::now();
        let origin = self.origin(bounds);
        self.world
            .layout_as_root(AvailableSpace::min_size(), window, cx);
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            window.fast_with_paint_scale(origin, self.zoom, |window| {
                self.world.prepaint_at(origin, window, cx);
            })
        });
        started
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        started: &mut Instant,
        window: &mut Window,
        cx: &mut App,
    ) {
        let origin = self.origin(bounds);
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            window.fast_with_paint_scale(origin, self.zoom, |window| {
                self.world.paint(window, cx);
            })
        });
        self.stats.borrow_mut().paint.push(started.elapsed());
    }
}

fn percentile(values: &[Duration], p: f64) -> Duration {
    if values.is_empty() {
        return Duration::ZERO;
    }
    let mut sorted = values.to_vec();
    sorted.sort();
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

#[allow(
    clippy::disallowed_methods,
    reason = "measured once, outside the frames it measures"
)]
fn footprint() -> String {
    let output = std::process::Command::new("/usr/bin/footprint")
        .args(["-p", &std::process::id().to_string()])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    output
        .lines()
        .filter(|line| {
            line.contains("Footprint:")
                || line.contains("IOAccelerator")
                || line.contains("IOSurface")
        })
        .map(str::trim)
        .collect::<Vec<_>>()
        .join(" | ")
}

fn report(stats: &Stats, when: &str) {
    println!(
        "spike {when}: frames {} interval p50 {:?} p95 {:?} max {:?}; layer prepaint+paint p50 {:?} p95 {:?} max {:?}",
        stats.frames.len(),
        percentile(&stats.frames, 0.5),
        percentile(&stats.frames, 0.95),
        stats.frames.iter().max().copied().unwrap_or_default(),
        percentile(&stats.paint, 0.5),
        percentile(&stats.paint, 0.95),
        stats.paint.iter().max().copied().unwrap_or_default(),
    );
    println!("spike footprint {when}: {}", footprint());
}

fn click(window: &mut Window, cx: &mut App, position: Point<Pixels>) {
    let modifiers = Modifiers::default();
    window.dispatch_event(
        PlatformInput::MouseMove(MouseMoveEvent {
            position,
            pressed_button: None,
            modifiers,
        }),
        cx,
    );
    window.dispatch_event(
        PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position,
            modifiers,
            click_count: 1,
            first_mouse: false,
        }),
        cx,
    );
    window.dispatch_event(
        PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position,
            modifiers,
            click_count: 1,
        }),
        cx,
    );
}

fn run_example() {
    application().run(|cx: &mut App| {
        example_support::load_fonts(cx);
        let bounds = Bounds::new(point(px(80.), px(80.)), size(px(1000.), px(640.)));
        let zoom = std::env::var("SPIKE_ZOOM")
            .ok()
            .and_then(|z| z.parse().ok())
            .unwrap_or(1.);
        let sweep = std::env::var("SPIKE_PINCH").ok().and_then(|s| s.parse::<f32>().ok());
        let check_clicks = std::env::var("SPIKE_CLICK").is_ok();
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            move |window, cx| {
                let stats = Rc::new(RefCell::new(Stats::default()));
                let board = cx.new(|_| Board {
                    zoom,
                    camera: point(px(0.), px(0.)),
                    clicks: vec![0; COLUMNS * ROWS],
                    sweep: None,
                    stats: stats.clone(),
                });
                println!("spike footprint at launch: {}", footprint());
                let handle = board.clone();
                window
                    .spawn(cx, async move |cx| {
                        cx.background_executor().timer(Duration::from_secs(2)).await;
                        handle
                            .update_in(cx, |board, window, cx| {
                                report(&board.stats.borrow(), "after 2 s idle");
                                if let Some(seconds) = sweep {
                                    board.stats.replace(Stats::default());
                                    board.sweep = Some((Instant::now(), seconds));
                                    window.refresh();
                                }
                                cx.notify();
                            })
                            .ok();
                        if let Ok(path) = std::env::var("SPIKE_CAPTURE") {
                            let saved = cx
                                .update(|window, _| window.render_to_image().map(|image| image.save(&path)))
                                .map(|r| format!("{r:?}"));
                            println!("spike capture {path}: {saved:?}");
                        }
                        if check_clicks {
                            let zoom = handle.read_with(cx, |board, _| board.zoom);
                            let button = first_button();
                            // The window's content starts at its top leading corner.
                            let zoomed = button.map(|c| c * zoom);
                            cx.update(|window, cx| click(window, cx, zoomed)).ok();
                            let first = handle.read_with(cx, |board, _| board.clicks[0]);
                            cx.update(|window, cx| click(window, cx, button)).ok();
                            let counted = handle
                                .read_with(cx, |board, _| {
                                    board
                                        .clicks
                                        .iter()
                                        .enumerate()
                                        .filter(|(_, n)| **n > 0)
                                        .map(|(i, n)| format!("card {i}: {n}"))
                                        .collect::<Vec<_>>()
                                })
                                ;
                            println!(
                                "spike clicks at zoom {zoom}: at {zoomed:?} card 0 counted {first}; then at {button:?} (1x); counted: {counted:?}"
                            );
                        }
                    })
                    .detach();
                board
            },
        )
        .unwrap();
        cx.activate(true);
    });
}

fn main() {
    run_example();
}
