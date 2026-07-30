//! The canvas, headless. No window, no textures, no typesetter -- the widget
//! takes page *sizes* and texture ids, so everything about its geometry can be
//! asserted without any of those.

use egui::{TextureId, Vec2};
use lilook_ui::{Canvas, CanvasInput, PageTexture};

fn view<'a>(pages: &'a [PageTexture], scenes: &'a [lilook_core::scene::Scene]) -> CanvasInput<'a> {
    CanvasInput {
        pages,
        scenes,
        selected: None,
        editable: &[],
    }
}

fn page(w: f64, h: f64) -> PageTexture {
    PageTexture {
        texture: TextureId::default(),
        size_pt: (w, h),
    }
}

#[test]
fn fits_the_document_on_the_first_frame() {
    let pages = [page(240.0, 160.0)];
    let mut canvas = Canvas::new();
    let mut fitted = None;
    egui::__run_test_ui(|ui| {
        let out = canvas.ui(ui, view(&pages, &[]));
        fitted = Some((out.viewport, out.pages.clone(), out.response.rect));
    });

    let (viewport, boxes, rect) = fitted.expect("the canvas ran");
    assert_eq!(boxes.len(), 1);

    // The page is centred and inside the widget, with room to spare.
    let on_screen = viewport.screen_rect(&boxes[0]);
    assert!(rect.contains_rect(on_screen), "{on_screen:?} vs {rect:?}");
    assert!(
        (on_screen.center() - rect.center()).length() < 1.0,
        "page not centred: {:?} vs {:?}",
        on_screen.center(),
        rect.center()
    );
    // Aspect ratio survives the fit -- a stretched figure would be a lie.
    let aspect = on_screen.width() / on_screen.height();
    assert!((aspect - 1.5).abs() < 1e-3, "aspect {aspect}");
}

#[test]
fn a_click_maps_back_to_a_point_on_a_page() {
    let pages = [page(240.0, 160.0), page(240.0, 160.0)];
    let mut canvas = Canvas::new();
    egui::__run_test_ui(|ui| {
        let out = canvas.ui(ui, view(&pages, &[]));
        // Round-trip a known point on the second page through the whole chain:
        // page pt -> document pt -> screen -> back.
        let b = &out.pages[1];
        let want = (30.0, 40.0);
        let screen = out.viewport.to_screen(b.to_doc(want));
        let doc = out.viewport.to_doc(screen);
        assert!(b.contains(doc));
        let got = b.to_page(doc);
        assert!(
            (got.0 - want.0).abs() < 1e-2 && (got.1 - want.1).abs() < 1e-2,
            "{got:?}"
        );
        // ...and it belongs to page 1, not to page 0.
        assert!(!out.pages[0].contains(doc));
    });
}

#[test]
fn an_empty_document_does_not_panic_or_refit_away() {
    let mut canvas = Canvas::new();
    egui::__run_test_ui(|ui| {
        let out = canvas.ui(ui, view(&[], &[]));
        assert!(out.pages.is_empty());
        assert!(out.hover.is_none());
    });
    // Still pending a fit, so the first real render is framed correctly.
    let pages = [page(100.0, 100.0)];
    let mut zoom = None;
    egui::__run_test_ui(|ui| {
        canvas.ui(ui, view(&pages, &[]));
        zoom = Some(canvas.zoom());
    });
    assert_ne!(zoom, Some(1.0), "the first page must trigger the fit");
}

#[test]
fn zooming_does_not_move_the_page_off_its_own_coordinates() {
    let pages = [page(240.0, 160.0)];
    let mut canvas = Canvas::new();
    egui::__run_test_ui(|ui| {
        canvas.ui(ui, view(&pages, &[]));
    });
    let before = canvas.zoom();
    canvas.set_zoom(before * 2.0);
    assert!((canvas.zoom() - before * 2.0).abs() < 1e-4);

    let mut ok = false;
    egui::__run_test_ui(|ui| {
        let out = canvas.ui(ui, view(&pages, &[]));
        let r = out.viewport.screen_rect(&out.pages[0]);
        // Twice the zoom, twice the size, still square with the axes.
        ok = (r.width() / r.height() - 1.5).abs() < 1e-3
            && (r.width() - 240.0 * canvas.zoom()).abs() < 1e-3;
    });
    assert!(ok);
    let _ = Vec2::ZERO;
}

// --------------------------------------------------------------- selection

use lilook_core::compile::{AxisMap, Transform};
use lilook_core::scene::{Scene, SeriesGeom};
use lilook_ui::CanvasEvent;

/// A 200x100 pt page with a diagram whose data area is inset 20 pt, showing
/// 0..10 on x and 0..10 on y. No compiler needed: the canvas consumes scenes.
fn fixture() -> ([PageTexture; 1], Vec<Scene>) {
    let scene = Scene {
        figure: 1,
        page: 0,
        area: (20.0, 20.0, 180.0, 80.0),
        transform: Transform {
            x: AxisMap {
                origin: 20.0,
                scale: 16.0,
                min: 0.0,
                max: 10.0,
            },
            y: AxisMap {
                origin: 80.0,
                scale: -6.0,
                min: 0.0,
                max: 10.0,
            },
        },
        series: vec![
            SeriesGeom {
                node: 2,
                channels: vec![],
                points: vec![(0.0, 0.0), (5.0, 9.0), (10.0, 1.0)],
            },
            SeriesGeom {
                node: 3,
                channels: vec![],
                points: vec![(0.0, 5.0), (10.0, 5.0)],
            },
        ],
    };
    ([page(200.0, 100.0)], vec![scene])
}

/// Drive a real egui context so the click goes through pointer input rather
/// than through a hand-made `Response`.
fn click_at(
    canvas: &mut Canvas,
    pages: &[PageTexture],
    scenes: &[Scene],
    at: Option<egui::Pos2>,
) -> (Vec<CanvasEvent>, lilook_ui::Viewport) {
    let ctx = egui::Context::default();
    ctx.set_fonts(egui::FontDefinitions::empty());
    let out = std::cell::RefCell::new(None);

    let mut run = |input: egui::RawInput| {
        let _ = ctx.run_ui(input, |ui| {
            let o = canvas.ui(ui, view(pages, scenes));
            *out.borrow_mut() = Some((o.events.clone(), o.viewport));
        });
    };

    // First frame lays the canvas out and fits the page.
    run(egui::RawInput::default());
    let Some(at) = at else {
        return out.into_inner().unwrap();
    };

    let mut input = egui::RawInput::default();
    let modifiers = egui::Modifiers::default();
    input.events.push(egui::Event::PointerMoved(at));
    input.events.push(egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers,
    });
    input.events.push(egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers,
    });
    run(input);
    out.into_inner().unwrap()
}

/// The M4 property: a click at the screen position of a known data point
/// selects the call site that drew it.
#[test]
fn clicking_a_point_selects_the_call_site_that_drew_it() {
    let (pages, scenes) = fixture();
    let mut canvas = Canvas::new();

    // Where the second point of series #2 landed, all the way to screen space.
    let (_, viewport) = click_at(&mut canvas, &pages, &scenes, None);
    let boxes = lilook_ui::stack_pages(&[(200.0, 100.0)], 12.0);
    let target = viewport.to_screen(boxes[0].to_doc(scenes[0].transform.to_page((5.0, 9.0))));

    let (events, _) = click_at(&mut canvas, &pages, &scenes, Some(target));
    assert_eq!(
        events,
        vec![CanvasEvent::Select(2)],
        "the peak belongs to #2"
    );
}

#[test]
fn clicking_the_other_series_selects_the_other_call_site() {
    let (pages, scenes) = fixture();
    let mut canvas = Canvas::new();
    let (_, viewport) = click_at(&mut canvas, &pages, &scenes, None);
    let boxes = lilook_ui::stack_pages(&[(200.0, 100.0)], 12.0);

    // A point on the flat line at y = 5, far from the other series.
    let target = viewport.to_screen(boxes[0].to_doc(scenes[0].transform.to_page((10.0, 5.0))));
    let (events, _) = click_at(&mut canvas, &pages, &scenes, Some(target));
    assert_eq!(events, vec![CanvasEvent::Select(3)]);
}

/// Clicking empty space inside the axes selects the diagram: that is how the
/// user reaches `xlim`, `width` and the rest.
#[test]
fn clicking_the_background_selects_the_diagram() {
    let (pages, scenes) = fixture();
    let mut canvas = Canvas::new();
    let (_, viewport) = click_at(&mut canvas, &pages, &scenes, None);
    let boxes = lilook_ui::stack_pages(&[(200.0, 100.0)], 12.0);

    // (1, 8) is inside the axes and nowhere near either series.
    let target = viewport.to_screen(boxes[0].to_doc(scenes[0].transform.to_page((1.0, 8.0))));
    let (events, _) = click_at(&mut canvas, &pages, &scenes, Some(target));
    assert_eq!(events, vec![CanvasEvent::Select(1)]);
}

#[test]
fn clicking_off_the_page_selects_nothing() {
    let (pages, scenes) = fixture();
    let mut canvas = Canvas::new();
    let (_, viewport) = click_at(&mut canvas, &pages, &scenes, None);
    let target = viewport.rect.left_top() + egui::vec2(2.0, 2.0);
    let (events, _) = click_at(&mut canvas, &pages, &scenes, Some(target));
    assert!(events.is_empty(), "{events:?}");
}

// ---------------------------------------------------------------- gestures

/// Press, move, release. Returns every event the canvas produced across the
/// whole gesture, in order.
fn drag(
    canvas: &mut Canvas,
    pages: &[PageTexture],
    scenes: &[Scene],
    selected: Option<usize>,
    editable: &[usize],
    from: egui::Pos2,
    delta: egui::Vec2,
) -> (Vec<CanvasEvent>, lilook_ui::Viewport) {
    let ctx = egui::Context::default();
    ctx.set_fonts(egui::FontDefinitions::empty());
    let events = std::cell::RefCell::new(vec![]);
    let viewport = std::cell::RefCell::new(None);

    let mut run = |input: egui::RawInput| {
        let _ = ctx.run_ui(input, |ui| {
            let out = canvas.ui(
                ui,
                CanvasInput {
                    pages,
                    scenes,
                    selected,
                    editable,
                },
            );
            events.borrow_mut().extend(out.events.clone());
            *viewport.borrow_mut() = Some(out.viewport);
        });
    };
    let m = egui::Modifiers::default();
    let button = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: m,
    };

    run(egui::RawInput::default());
    events.borrow_mut().clear(); // the layout frame

    let mut down = egui::RawInput::default();
    down.events.push(egui::Event::PointerMoved(from));
    down.events.push(button(from, true));
    run(down);

    // Two moves, so the accumulated offset is exercised rather than a single
    // delta that happens to equal the total.
    for step in [0.5, 1.0] {
        let to = from + delta * step;
        let mut mv = egui::RawInput::default();
        mv.events.push(egui::Event::PointerMoved(to));
        run(mv);
    }

    let mut up = egui::RawInput::default();
    up.events.push(button(from + delta, false));
    run(up);

    (events.into_inner(), viewport.into_inner().unwrap())
}

fn limits(events: &[CanvasEvent]) -> Vec<((f64, f64), (f64, f64))> {
    events
        .iter()
        .filter_map(|e| match e {
            CanvasEvent::SetLimits { x, y, .. } => Some((*x, *y)),
            _ => None,
        })
        .collect()
}

#[test]
fn dragging_inside_the_axes_pans_the_data() {
    let (pages, scenes) = fixture();
    let mut canvas = Canvas::new();
    let (_, viewport) = click_at(&mut canvas, &pages, &scenes, None);
    let boxes = lilook_ui::stack_pages(&[(200.0, 100.0)], 12.0);
    // Start in the middle of the axes, away from either series.
    let from = viewport.to_screen(boxes[0].to_doc(scenes[0].transform.to_page((2.0, 7.5))));

    let (events, _) = drag(
        &mut canvas,
        &pages,
        &scenes,
        None,
        &[],
        from,
        egui::vec2(30.0, 12.0),
    );

    assert_eq!(events.first(), Some(&CanvasEvent::Begin));
    assert_eq!(events.last(), Some(&CanvasEvent::Commit), "{events:?}");
    let ls = limits(&events);
    assert!(!ls.is_empty(), "a pan must rewrite the limits");

    let (x, y) = *ls.last().unwrap();
    let zoom = canvas.zoom() as f64;
    // Dragging right moves the content right, so the window moves left.
    let dx = 30.0 / (zoom * 16.0);
    let dy = 12.0 / (zoom * -6.0);
    assert!((x.0 - (0.0 - dx)).abs() < 1e-3, "xmin {} vs {}", x.0, -dx);
    assert!((x.1 - (10.0 - dx)).abs() < 1e-3);
    assert!((y.0 - (0.0 - dy)).abs() < 1e-3, "ymin {} vs {}", y.0, -dy);
    // The window keeps its size: a pan is not a zoom.
    assert!(((x.1 - x.0) - 10.0).abs() < 1e-6);
    assert!(((y.1 - y.0) - 10.0).abs() < 1e-6);
}

#[test]
fn dragging_a_selected_point_moves_that_point() {
    let (pages, scenes) = fixture();
    let mut canvas = Canvas::new();
    let (_, viewport) = click_at(&mut canvas, &pages, &scenes, None);
    let boxes = lilook_ui::stack_pages(&[(200.0, 100.0)], 12.0);
    let from = viewport.to_screen(boxes[0].to_doc(scenes[0].transform.to_page((5.0, 9.0))));

    let (events, _) = drag(
        &mut canvas,
        &pages,
        &scenes,
        Some(2),
        &[2],
        from,
        egui::vec2(16.0, 6.0),
    );

    let moves: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            CanvasEvent::MovePoint { node, index, to } => Some((*node, *index, *to)),
            _ => None,
        })
        .collect();
    assert!(!moves.is_empty(), "{events:?}");
    let (node, index, to) = *moves.last().unwrap();
    assert_eq!((node, index), (2, 1), "the peak of series #2");

    let zoom = canvas.zoom() as f64;
    assert!((to.0 - (5.0 + 16.0 / (zoom * 16.0))).abs() < 1e-3, "{to:?}");
    assert!((to.1 - (9.0 + 6.0 / (zoom * -6.0))).abs() < 1e-3, "{to:?}");
    assert_eq!(events.last(), Some(&CanvasEvent::Commit));
}

/// Computed data is hit-testable but not movable. Dragging it must pan, not
/// silently do nothing and not emit an edit that the core would reject.
#[test]
fn dragging_a_point_of_a_non_editable_series_pans_instead() {
    let (pages, scenes) = fixture();
    let mut canvas = Canvas::new();
    let (_, viewport) = click_at(&mut canvas, &pages, &scenes, None);
    let boxes = lilook_ui::stack_pages(&[(200.0, 100.0)], 12.0);
    let from = viewport.to_screen(boxes[0].to_doc(scenes[0].transform.to_page((5.0, 9.0))));

    let (events, _) = drag(
        &mut canvas,
        &pages,
        &scenes,
        Some(2),
        &[], // nothing editable
        from,
        egui::vec2(16.0, 6.0),
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, CanvasEvent::MovePoint { .. })),
        "{events:?}"
    );
    assert!(!limits(&events).is_empty(), "it should pan instead");
}

/// A drag that starts off the page moves the view and touches no document.
#[test]
fn dragging_outside_a_figure_pans_the_view_only() {
    let (pages, scenes) = fixture();
    let mut canvas = Canvas::new();
    let (_, viewport) = click_at(&mut canvas, &pages, &scenes, None);
    let from = viewport.rect.left_top() + egui::vec2(3.0, 3.0);

    let (events, after) = drag(
        &mut canvas,
        &pages,
        &scenes,
        None,
        &[],
        from,
        egui::vec2(25.0, 10.0),
    );
    assert!(events.is_empty(), "{events:?}");
    assert!(
        (after.pan - viewport.pan - egui::vec2(25.0, 10.0)).length() < 0.5,
        "the view should have moved instead"
    );
}

/// Dragging the right edge of the axis frame changes `width` and nothing else.
/// `width` on `lq.diagram` *is* the data area's width, so the frame follows the
/// pointer one-to-one.
#[test]
fn dragging_the_frame_resizes_the_diagram() {
    let (pages, scenes) = fixture();
    let mut canvas = Canvas::new();
    let (_, viewport) = click_at(&mut canvas, &pages, &scenes, None);
    let boxes = lilook_ui::stack_pages(&[(200.0, 100.0)], 12.0);
    let area = scenes[0].area; // (20, 20, 180, 80) in page points

    // Middle of the right edge.
    let from = viewport.to_screen(boxes[0].to_doc((area.2, (area.1 + area.3) / 2.0)));
    let (events, _) = drag(
        &mut canvas,
        &pages,
        &scenes,
        None,
        &[],
        from,
        egui::vec2(20.0, 0.0),
    );

    let sizes: Vec<(Option<f64>, Option<f64>)> = events
        .iter()
        .filter_map(|e| match e {
            CanvasEvent::SetSize {
                width_pt,
                height_pt,
                ..
            } => Some((*width_pt, *height_pt)),
            _ => None,
        })
        .collect();
    assert!(!sizes.is_empty(), "{events:?}");
    let (w, h) = *sizes.last().unwrap();
    assert_eq!(h, None, "a right-edge drag must not touch the height");
    let expected = (area.2 - area.0) + 20.0 / canvas.zoom() as f64;
    assert!((w.unwrap() - expected).abs() < 1e-3, "{w:?} vs {expected}");
    assert_eq!(events.first(), Some(&CanvasEvent::Begin));
    assert_eq!(events.last(), Some(&CanvasEvent::Commit));
}

/// The corner drives both, and neither can be dragged to nothing.
#[test]
fn the_corner_resizes_both_axes_and_stops_at_a_minimum() {
    let (pages, scenes) = fixture();
    let mut canvas = Canvas::new();
    let (_, viewport) = click_at(&mut canvas, &pages, &scenes, None);
    let boxes = lilook_ui::stack_pages(&[(200.0, 100.0)], 12.0);
    let area = scenes[0].area;
    let from = viewport.to_screen(boxes[0].to_doc((area.2, area.3)));

    let (events, _) = drag(
        &mut canvas,
        &pages,
        &scenes,
        None,
        &[],
        from,
        egui::vec2(15.0, 9.0),
    );
    let (w, h) = events
        .iter()
        .rev()
        .find_map(|e| match e {
            CanvasEvent::SetSize {
                width_pt,
                height_pt,
                ..
            } => Some((*width_pt, *height_pt)),
            _ => None,
        })
        .expect("a corner drag sets both");
    assert!(w.is_some() && h.is_some());

    // Dragging far inwards clamps rather than inverting the figure.
    let (events, _) = drag(
        &mut canvas,
        &pages,
        &scenes,
        None,
        &[],
        from,
        egui::vec2(-4000.0, -4000.0),
    );
    let (w, h) = events
        .iter()
        .rev()
        .find_map(|e| match e {
            CanvasEvent::SetSize {
                width_pt,
                height_pt,
                ..
            } => Some((*width_pt, *height_pt)),
            _ => None,
        })
        .unwrap();
    assert!(w.unwrap() >= 24.0 && h.unwrap() >= 24.0, "{w:?} {h:?}");
}
