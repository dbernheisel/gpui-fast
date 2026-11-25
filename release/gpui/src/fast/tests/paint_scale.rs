// Modified by Longbridge for gpui-fast.
//! `fast::paint_scale`: a subtree laid out at 1× and drawn scaled about a
//! point, as a zoomed canvas draws its nodes.

use crate::{
    AnyElement, App, AppContext, AvailableSpace, Bounds, Context, Element, ElementId, Entity,
    GlobalElementId, Hsla, InputEvent, InspectorElementId, InteractiveElement, IntoElement,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels,
    Point, Render, ScaledPixels, ScrollDelta, ScrollEvent, Style, Styled, TestAppContext, Window,
    WindowHandle, canvas, div, point, px, size,
};
use crate::{fast::paint_scale::bucket, prelude::*, scene::PaintOperation};
use std::{cell::RefCell, rc::Rc};

/// Lays `world` out at 1× and draws it scaled by `scale` about `origin`,
/// or about its own corner when `origin` is `None`, prepainted there. The
/// world is laid out on its own, as a canvas lays out its world, or as the
/// element's child when `inline`.
struct Scaled {
    world: AnyElement,
    origin: Option<Point<Pixels>>,
    scale: f32,
    inline: bool,
}

impl Element for Scaled {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size = size(crate::relative(1.).into(), crate::relative(1.).into());
        let children = self.inline.then(|| self.world.request_layout(window, cx));
        (window.request_layout(style, children, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let origin = self.origin.unwrap_or(bounds.origin);
        if self.inline {
            window.fast_with_paint_scale(origin, self.scale, |window| {
                self.world.prepaint(window, cx)
            });
            return;
        }
        self.world
            .layout_as_root(AvailableSpace::min_size(), window, cx);
        window.fast_with_paint_scale(origin, self.scale, |window| {
            self.world.prepaint_at(origin, window, cx)
        });
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let origin = self.origin.unwrap_or(bounds.origin);
        window.fast_with_paint_scale(origin, self.scale, |window| self.world.paint(window, cx));
    }
}

type World = Rc<dyn Fn(&mut Board, &mut Context<Board>) -> AnyElement>;

/// A window whose content is `world`, scaled by `scale` about `origin`.
struct Board {
    scale: f32,
    origin: Point<Pixels>,
    world: World,
    /// Anything the world's views and listeners note down.
    log: Rc<RefCell<Vec<String>>>,
    card: Option<Entity<Card>>,
    inline: bool,
}

impl Render for Board {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let world = (self.world.clone())(self, cx);
        div().size_full().child(Scaled {
            world,
            origin: (!self.inline).then_some(self.origin),
            scale: self.scale,
            inline: self.inline,
        })
    }
}

/// A view of its own inside the world: one quad.
struct Card {
    color: Hsla,
}

impl Render for Card {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .absolute()
            .left(px(10.))
            .top(px(20.))
            .size(px(30.))
            .bg(self.color)
    }
}

const ORIGIN: Point<Pixels> = point(px(100.), px(50.));

fn board(
    cx: &mut TestAppContext,
    scale: f32,
    world: impl Fn(&mut Board, &mut Context<Board>) -> AnyElement + 'static,
) -> WindowHandle<Board> {
    cx.add_window(move |_, _| Board {
        scale,
        origin: ORIGIN,
        world: Rc::new(world),
        log: Rc::default(),
        card: None,
        inline: false,
    })
}

/// The world's root: 400 points square, prepainted at the scope's origin.
fn world() -> crate::Div {
    div().relative().size(px(400.))
}

/// A quad at `(10, 20)` in the world, 30 points square.
fn square(color: Hsla) -> crate::Div {
    div()
        .absolute()
        .left(px(10.))
        .top(px(20.))
        .size(px(30.))
        .bg(color)
}

/// Window points `(x, y, w, h)` in device pixels, at the test window's
/// display scale of 2.
fn device(x: f32, y: f32, w: f32, h: f32) -> Bounds<ScaledPixels> {
    Bounds::new(
        point(ScaledPixels(x * 2.), ScaledPixels(y * 2.)),
        size(ScaledPixels(w * 2.), ScaledPixels(h * 2.)),
    )
}

fn quads(window: &Window, color: Hsla) -> Vec<Bounds<ScaledPixels>> {
    window
        .rendered_frame
        .scene
        .quads
        .iter()
        .filter(|quad| quad.background.solid == color)
        .map(|quad| quad.bounds)
        .collect()
}

fn draw(window: &mut Window, cx: &mut App) {
    window.draw(cx).clear(cx);
}

fn click(window: &mut Window, cx: &mut App, position: Point<Pixels>) {
    let modifiers = Default::default();
    for event in [
        MouseMoveEvent {
            position,
            pressed_button: None,
            modifiers,
        }
        .to_platform_input(),
        MouseDownEvent {
            button: MouseButton::Left,
            position,
            modifiers,
            click_count: 1,
            first_mouse: false,
        }
        .to_platform_input(),
        MouseUpEvent {
            button: MouseButton::Left,
            position,
            modifiers,
            click_count: 1,
        }
        .to_platform_input(),
    ] {
        window.dispatch_event(event, cx);
    }
}

#[crate::test]
fn a_scope_draws_its_quads_scaled_about_its_origin(cx: &mut TestAppContext) {
    let handle = board(cx, 2., |_, _| {
        world().child(square(crate::red())).into_any_element()
    });
    cx.update_window(handle.into(), |_, window, cx| {
        draw(window, cx);
        // (10, 20) from the origin draws at (20, 40) from it, 60 points
        // square.
        assert_eq!(
            quads(window, crate::red()),
            vec![device(120., 90., 60., 60.)]
        );
    })
    .unwrap();
}

#[crate::test]
fn a_scale_of_one_draws_as_without_a_scope(cx: &mut TestAppContext) {
    let handle = board(cx, 1., |_, _| {
        world().child(square(crate::red())).into_any_element()
    });
    cx.update_window(handle.into(), |_, window, cx| {
        draw(window, cx);
        assert_eq!(
            quads(window, crate::red()),
            vec![device(110., 70., 30., 30.)]
        );
    })
    .unwrap();
}

#[crate::test]
fn nested_scopes_compose(cx: &mut TestAppContext) {
    let handle = board(cx, 2., |_, _| {
        world()
            .child(
                div()
                    .absolute()
                    .left(px(10.))
                    .top(px(20.))
                    .size(px(200.))
                    .child(Scaled {
                        world: div()
                            .relative()
                            .size(px(100.))
                            .child(
                                div()
                                    .absolute()
                                    .left(px(5.))
                                    .top(px(5.))
                                    .size(px(10.))
                                    .bg(crate::blue()),
                            )
                            .into_any_element(),
                        origin: None,
                        scale: 3.,
                        inline: false,
                    }),
            )
            .into_any_element()
    });
    cx.update_window(handle.into(), |_, window, cx| {
        draw(window, cx);
        // The inner origin is (110, 70) in the outer scope; (5, 5) from it
        // draws at (15, 15) there, so at (125, 85), which the outer scope
        // draws at (100, 50) + (25, 35)·2. Both scales make the size 60.
        assert_eq!(
            quads(window, crate::blue()),
            vec![device(150., 120., 60., 60.)]
        );
    })
    .unwrap();
}

#[crate::test]
fn the_pointer_hits_what_draws_and_listeners_read_local_points(cx: &mut TestAppContext) {
    let handle = board(cx, 2., |board, _| {
        let log = board.log.clone();
        let log_click = board.log.clone();
        world()
            .child(
                square(crate::red())
                    .id("button")
                    .on_mouse_down(MouseButton::Left, move |event, window, _| {
                        log.borrow_mut().push(format!(
                            "down {:?} pointer {:?}",
                            event.position,
                            window.mouse_position()
                        ));
                    })
                    .on_click(move |_, _, _| log_click.borrow_mut().push("click".into())),
            )
            .into_any_element()
    });
    cx.update_window(handle.into(), |_, window, cx| {
        draw(window, cx);
        // The square draws over (120, 90)..(180, 150). Its 1× place,
        // (110, 70)..(140, 100), takes nothing.
        click(window, cx, point(px(115.), px(75.)));
        draw(window, cx);
        click(window, cx, point(px(130.), px(100.)));
        draw(window, cx);
    })
    .unwrap();
    let log = handle
        .read_with(cx, |board, _| board.log.borrow().clone())
        .unwrap();
    // (130, 100) in the window is (100, 50) + (30, 50)/2 in the scope.
    let local = point(px(115.), px(75.));
    assert_eq!(
        log,
        vec![
            format!("down {local:?} pointer {local:?}"),
            "click".to_string()
        ]
    );
}

#[crate::test]
fn layout_bounds_stay_at_one_x(cx: &mut TestAppContext) {
    let handle = board(cx, 2.5, |board, _| {
        let log = board.log.clone();
        world()
            .child(
                square(crate::red()).child(
                    canvas(
                        move |bounds, window, _| {
                            log.borrow_mut().push(format!(
                                "{bounds:?} {:?} {}",
                                window.content_mask().bounds.origin,
                                window.scale_factor()
                            ))
                        },
                        |_, _, _, _| {},
                    )
                    .size_full(),
                ),
            )
            .into_any_element()
    });
    cx.update_window(handle.into(), |_, window, cx| draw(window, cx))
        .unwrap();
    let log = handle
        .read_with(cx, |board, _| board.log.borrow().clone())
        .unwrap();
    let bounds = Bounds::new(point(px(110.), px(70.)), size(px(30.), px(30.)));
    // The window's mask, the whole window, seen from the scope.
    let mask = point(px(100. - 100. / 2.5), px(50. - 50. / 2.5));
    // Inside, the scale factor is the display's 2 times the scope's 2.5.
    assert_eq!(log, vec![format!("{bounds:?} {mask:?} 5")]);
}

/// A board whose world holds a [`Card`] view, inside a container that
/// clips to its own bounds, which are the same in the scope at any scale.
fn card_board(cx: &mut TestAppContext, inline: bool) -> WindowHandle<Board> {
    cx.add_window(move |_, cx| Board {
        scale: 2.,
        origin: ORIGIN,
        world: Rc::new(|board: &mut Board, _: &mut Context<Board>| {
            world()
                .child(
                    div()
                        .size(px(200.))
                        .overflow_hidden()
                        .child(board.card.clone().unwrap()),
                )
                .into_any_element()
        }),
        log: Rc::default(),
        card: Some(cx.new(|_| Card {
            color: crate::red(),
        })),
        inline,
    })
}

#[crate::test]
fn a_view_drawn_again_from_last_frame_keeps_its_scale(cx: &mut TestAppContext) {
    let handle = card_board(cx, false);
    let reused = |window: &Window| {
        window
            .layout_engine
            .as_ref()
            .unwrap()
            .retention
            .stats
            .views_reused
    };
    cx.update_window(handle.into(), |_, window, cx| {
        draw(window, cx);
        assert_eq!(
            quads(window, crate::red()),
            vec![device(120., 90., 60., 60.)]
        );
    })
    .unwrap();

    // The board is built again at the same scale: the card is drawn from
    // last frame where it was, its paint not moved a second time.
    handle.update(cx, |_, _, cx| cx.notify()).unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        let before = reused(window);
        draw(window, cx);
        assert!(reused(window) > before, "the card is drawn from last frame");
        assert_eq!(
            quads(window, crate::red()),
            vec![device(120., 90., 60., 60.)]
        );
    })
    .unwrap();

    // At another scale, the card is built again rather than drawn from a
    // frame painted at the old one.
    handle
        .update(cx, |board, _, cx| {
            board.scale = 3.;
            cx.notify();
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        draw(window, cx);
        assert_eq!(
            quads(window, crate::red()),
            vec![device(130., 110., 90., 90.)]
        );
    })
    .unwrap();
}

#[crate::test]
fn a_view_built_again_on_its_own_draws_in_its_scope(cx: &mut TestAppContext) {
    // Laid out as the scope's child, so that the board can be drawn from
    // last frame around the card (a world laid out on its own is built
    // again with its views).
    let handle = card_board(cx, true);
    let spliced = |window: &Window| {
        window
            .layout_engine
            .as_ref()
            .unwrap()
            .retention
            .stats
            .views_reused
    };
    cx.update_window(handle.into(), |_, window, cx| draw(window, cx))
        .unwrap();
    let card = handle
        .read_with(cx, |board, _| board.card.clone().unwrap())
        .unwrap();
    // Only the card changes: the board is drawn from last frame around it.
    card.update(cx, |card, cx| {
        card.color = crate::green();
        cx.notify();
    });
    cx.update_window(handle.into(), |_, window, cx| {
        let before = spliced(window);
        draw(window, cx);
        assert!(
            spliced(window) > before,
            "the board is drawn from last frame"
        );
        // The world's corner, the scope's origin, is the window's: (10, 20)
        // draws at (20, 40).
        assert_eq!(
            quads(window, crate::green()),
            vec![device(20., 40., 60., 60.)]
        );
        assert!(quads(window, crate::red()).is_empty());
    })
    .unwrap();
}

#[crate::test]
fn a_deferred_element_draws_in_the_scope_it_was_deferred_from(cx: &mut TestAppContext) {
    let handle = board(cx, 2., |board, _| {
        let log = board.log.clone();
        world()
            .child(
                div()
                    .absolute()
                    .left(px(10.))
                    .top(px(20.))
                    .child(crate::deferred(
                        div()
                            .id("popover")
                            .size(px(30.))
                            .bg(crate::blue())
                            .on_click(move |_, _, _| log.borrow_mut().push("click".into())),
                    )),
            )
            .into_any_element()
    });
    cx.update_window(handle.into(), |_, window, cx| {
        draw(window, cx);
        assert_eq!(
            quads(window, crate::blue()),
            vec![device(120., 90., 60., 60.)]
        );
        // Its hitbox is where it draws too.
        click(window, cx, point(px(170.), px(140.)));
        draw(window, cx);
    })
    .unwrap();
    let log = handle
        .read_with(cx, |board, _| board.log.borrow().clone())
        .unwrap();
    assert_eq!(log, vec!["click".to_string()]);
}

#[crate::test]
fn a_scroll_container_in_a_scope_scrolls_by_what_draws(cx: &mut TestAppContext) {
    let handle = board(cx, 2., |_, _| {
        world()
            .child(
                div()
                    .id("scroller")
                    .absolute()
                    .left(px(10.))
                    .top(px(20.))
                    .size(px(100.))
                    .overflow_y_scroll()
                    .child(div().h(px(40.)))
                    .child(div().h(px(30.)).w_full().bg(crate::red()))
                    .child(div().h(px(400.))),
            )
            .into_any_element()
    });
    cx.update_window(handle.into(), |_, window, cx| {
        // Even a container the layer policy would paint into its layer.
        window.fast_layers.forced_decision = Some(crate::fast::layers::policy::Decision::Repaint);
        draw(window, cx);
        // The red row is 40 points down the container: (110, 110) in the
        // scope, (120, 170) in the window.
        assert_eq!(
            quads(window, crate::red()),
            vec![device(120., 170., 200., 60.)]
        );
        // Each 4 points of wheel over what draws scroll its content 2 of its
        // own.
        for _ in 0..10 {
            window.dispatch_event(
                ScrollEvent {
                    position: point(px(150.), px(150.)),
                    delta: ScrollDelta::Pixels(point(px(0.), px(-4.))),
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
            draw(window, cx);
        }
        assert_eq!(
            quads(window, crate::red()),
            vec![device(120., 130., 200., 60.)]
        );
        // It painted directly, not into a scroll layer.
        assert!(window.fast_layers.layers.is_empty());
    })
    .unwrap();
}

#[crate::test]
fn rounded_clips_and_layers_open_where_the_scope_draws(cx: &mut TestAppContext) {
    let handle = board(cx, 2., |_, _| {
        world()
            .child(
                square(crate::red())
                    .overflow_hidden()
                    .rounded(px(4.))
                    .child(div().size(px(30.)).bg(crate::blue())),
            )
            .child(div().absolute().left(px(10.)).top(px(100.)).child("Hi"))
            .into_any_element()
    });
    cx.update_window(handle.into(), |_, window, cx| {
        draw(window, cx);
        let operations = &window.rendered_frame.scene.paint_operations;
        let clips: Vec<_> = operations
            .iter()
            .filter_map(|operation| match operation {
                PaintOperation::FastRoundedClip(
                    crate::fast::rounded_clip::ClipOperation::Begin(shape),
                ) => Some((shape.bounds, shape.radii.top_left.width)),
                _ => None,
            })
            .collect();
        assert_eq!(
            clips,
            vec![(device(120., 90., 60., 60.), ScaledPixels(16.))]
        );
        // The text's line opens a layer at (110, 150) in the scope, which
        // draws at (120, 250), (240, 500) in device pixels.
        let layers: Vec<_> = operations
            .iter()
            .filter_map(|operation| match operation {
                PaintOperation::StartLayer(bounds) => Some(bounds.origin),
                _ => None,
            })
            .collect();
        assert!(
            layers.contains(&point(ScaledPixels(240.), ScaledPixels(500.))),
            "{layers:?}"
        );
    })
    .unwrap();
}

#[crate::test]
fn accessibility_bounds_are_where_nodes_draw(cx: &mut TestAppContext) {
    let handle = board(cx, 2., |_, _| {
        world()
            .child(
                square(crate::red())
                    .id("button")
                    .role(accesskit::Role::Button),
            )
            .into_any_element()
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.set_scale_factor(2.);
        window.set_a11y_active_for_tests(true);
        draw(window, cx);
        let tree = crate::fast::a11y::last_tree(window).expect("accessibility is active");
        let rects: Vec<_> = tree
            .nodes
            .iter()
            .filter(|(_, node)| node.role() == accesskit::Role::Button)
            .filter_map(|(_, node)| node.bounds())
            .collect();
        assert_eq!(
            rects,
            vec![accesskit::Rect {
                x0: 240.,
                y0: 180.,
                x1: 360.,
                y1: 300.
            }]
        );
    })
    .unwrap();
}

#[test]
fn glyphs_rasterize_at_quarter_octaves() {
    assert_eq!(bucket(1.), 1.);
    assert_eq!(bucket(2.), 2.);
    assert_eq!(bucket(0.25), 0.25);
    assert_eq!(bucket(1.3), 2f32.powf(0.5));
    assert_eq!(bucket(1.15), 2f32.powf(0.25));
    assert_eq!(bucket(3.9), 4.);
    // A sweep from 0.25 to 4 rasterizes at 17 scales.
    let mut scales: Vec<u32> = (0..=1000)
        .map(|i| bucket(0.25 * 16f32.powf(i as f32 / 1000.)).to_bits())
        .collect();
    scales.dedup();
    assert_eq!(scales.len(), 17);
}

#[test]
fn text_in_a_scope_is_drawn_from_its_bucket_at_the_exact_size() {
    let mut cx = TestAppContext::with_text_system(std::sync::Arc::new(
        super::oracle::GlyphBoxTextSystem(crate::NoopTextSystem),
    ));
    let cx = &mut cx;
    let handle = board(cx, 1.3, |_, _| world().child("Hi").into_any_element());
    cx.update_window(handle.into(), |_, window, cx| {
        draw(window, cx);
        let scene = &window.rendered_frame.scene;
        let matrices: Vec<_> = scene
            .monochrome_sprites
            .iter()
            .map(|sprite| sprite.transformation.rotation_scale)
            .chain(
                scene
                    .subpixel_sprites
                    .iter()
                    .map(|sprite| sprite.transformation.rotation_scale),
            )
            .collect();
        assert!(!matrices.is_empty(), "the text draws glyphs");
        let k = 1.3 / 2f32.powf(0.5);
        for matrix in matrices {
            assert_eq!(matrix, [[k, 0.], [0., k]]);
        }
    })
    .unwrap();
}
