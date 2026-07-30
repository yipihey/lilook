//! The figure canvas: rendered pages, navigation, selection, and the direct
//! manipulation gestures.
//!
//! Like the inspector, this emits events rather than touching a document, and
//! it takes textures and `Scene`s rather than compiling anything -- which is
//! what lets it run under `__run_test_ui` with no display and no typesetter.
//!
//! Selection is where the pixels and the byte ranges meet. The canvas never
//! guesses what was clicked from the drawing: it hit-tests in *data* space
//! against the points the compile backend recovered, each of which carries the
//! call site that drew it (ADR-0008).
//!
//! Gestures are anchored to the state at the moment of the press, never
//! integrated frame by frame from the live scene. The scene arrives from
//! another thread a compile behind the pointer, so integrating against it would
//! feed the lag back into the gesture and make a drag stutter or run away.

use egui::{Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use lilook_core::compile::AxisMap;
use lilook_core::scene::{Scene, SceneHit};

use crate::viewport::{stack_pages, stacked_size, PageBox, Viewport};

/// One rasterised page, already uploaded by the shell.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageTexture {
    pub texture: egui::TextureId,
    /// Page size in typographic points, *not* pixels: the canvas works in pt so
    /// that a re-render at a different resolution changes nothing here.
    pub size_pt: (f64, f64),
}

/// What the canvas asks the shell to do. Mapping these onto intents and
/// transactions is the shell's job, exactly as with `UiEvent`.
#[derive(Debug, Clone, PartialEq)]
pub enum CanvasEvent {
    /// A call site was clicked: a series, or a diagram's background.
    Select(usize),
    /// A gesture started: open a transaction.
    Begin,
    /// It finished: commit, so the whole gesture is one undo step.
    Commit,
    /// New axis limits for a diagram, from a pan or a zoom.
    SetLimits {
        figure: usize,
        x: (f64, f64),
        y: (f64, f64),
    },
    /// A data point was dragged to a new position.
    MovePoint {
        node: usize,
        index: usize,
        to: (f64, f64),
    },
    /// A rule line was dragged. Its coordinate is a whole positional argument,
    /// not an element of an array, so this is a different edit from a point move.
    MoveRule {
        node: usize,
        /// Which positional argument -- `hlines(1, 2, 3)` has three.
        slot: usize,
        to: f64,
    },
    /// The diagram was resized by its frame. Sizes are in typographic points --
    /// `width` and `height` on `lq.diagram` *are* the data area's dimensions,
    /// which is what makes dragging the axis frame mean what it looks like it
    /// means. `None` is an axis the gesture did not touch.
    SetSize {
        figure: usize,
        width_pt: Option<f64>,
        height_pt: Option<f64>,
    },
}

/// Which edge of the frame is being dragged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grip {
    Right,
    Bottom,
    Corner,
}

const ZOOM_LIMITS: (f32, f32) = (0.05, 16.0);
const PAGE_GAP_PT: f64 = 12.0;
/// Selection tolerance in *screen* pixels. Converted to page points against the
/// live zoom, so grabbing a point feels the same however far in you are.
const PICK_RADIUS_PX: f32 = 9.0;
/// Frames of no wheel input before a zoom gesture is considered finished. A
/// wheel produces a burst of events with no press or release to bracket it, and
/// one undo step per tick would make undo useless after a zoom.
const ZOOM_IDLE_FRAMES: u32 = 12;
/// How close to the axis frame counts as grabbing it, in screen pixels.
const GRIP_RADIUS_PX: f32 = 7.0;
/// A diagram smaller than this is not a figure, it is an accident.
const MIN_SIZE_PT: f64 = 24.0;

#[derive(Debug, Clone, PartialEq)]
enum Gesture {
    /// Move the view. Changes nothing in the document.
    ViewPan,
    /// Pan the data: rewrites the diagram's limits.
    DataPan {
        figure: usize,
        /// The axes as they were when the press happened -- limits, scale *and*
        /// which space each is linear in. Carrying the whole map rather than two
        /// scale factors is what lets a log axis pan multiplicatively, so its
        /// limits approach zero and never cross it.
        start: (AxisMap, AxisMap),
    },
    MovePoint {
        node: usize,
        index: usize,
        start: (f64, f64),
        /// The axes at the press, for the same reason as above: dragging a point
        /// on a log axis moves it by a ratio, not by an amount.
        axes: (AxisMap, AxisMap),
    },
    MoveRule {
        node: usize,
        slot: usize,
        start: f64,
        axis: lilook_core::Axis,
        /// The one axis the line moves along.
        map: AxisMap,
    },
    Resize {
        figure: usize,
        /// Data-area size in points at the press.
        start: (f64, f64),
        grip: Grip,
    },
}

#[derive(Debug, Clone, Default)]
pub struct Canvas {
    view: Option<Viewport>,
    zoom: f32,
    pan: Vec2,
    refit: bool,
    gesture: Option<Gesture>,
    /// Screen-space offset accumulated since the press.
    drag: Vec2,
    /// Frames since the last wheel event, while a zoom transaction is open.
    zooming: Option<u32>,
}

impl Canvas {
    pub fn new() -> Self {
        Canvas {
            view: None,
            zoom: 1.0,
            pan: Vec2::ZERO,
            refit: true,
            gesture: None,
            drag: Vec2::ZERO,
            zooming: None,
        }
    }
}

/// Everything the canvas draws and interacts with this frame.
#[derive(Debug, Clone, Copy)]
pub struct CanvasInput<'a> {
    pub pages: &'a [PageTexture],
    pub scenes: &'a [Scene],
    pub selected: Option<usize>,
    /// Series whose points can actually be moved -- the ones whose data is a
    /// literal array. Others draw hollow handles and refuse the drag rather
    /// than pretending and failing.
    pub editable: &'a [usize],
}

#[derive(Debug)]
pub struct CanvasOutput {
    pub response: egui::Response,
    pub viewport: Viewport,
    pub pages: Vec<PageBox>,
    /// Pointer position as (page index, point within that page).
    pub hover: Option<(usize, (f64, f64))>,
    /// The series point under the pointer, if any.
    pub hovered: Option<(usize, SceneHit)>,
    pub events: Vec<CanvasEvent>,
}

impl Canvas {
    pub fn fit(&mut self) {
        self.refit = true;
    }

    pub fn zoom(&self) -> f32 {
        self.zoom.max(ZOOM_LIMITS.0)
    }

    pub fn set_zoom(&mut self, zoom: f32) {
        match self.view {
            Some(mut v) => {
                v.zoom = self.zoom;
                v.pan = self.pan;
                v.zoom_about(v.rect.center(), zoom / self.zoom, ZOOM_LIMITS);
                self.zoom = v.zoom;
                self.pan = v.pan;
            }
            None => self.zoom = zoom.clamp(ZOOM_LIMITS.0, ZOOM_LIMITS.1),
        }
        self.refit = false;
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, input: CanvasInput<'_>) -> CanvasOutput {
        let CanvasInput {
            pages,
            scenes,
            selected,
            editable,
        } = input;
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let mut events = vec![];

        let boxes = stack_pages(
            &pages.iter().map(|p| p.size_pt).collect::<Vec<_>>(),
            PAGE_GAP_PT,
        );

        if self.refit && !pages.is_empty() {
            let v = Viewport::fit(rect, stacked_size(&boxes), 16.0);
            self.zoom = v.zoom;
            self.pan = v.pan;
            self.refit = false;
        }
        if self.zoom <= 0.0 {
            self.zoom = 1.0;
        }

        let mut viewport = Viewport {
            rect,
            zoom: self.zoom,
            pan: self.pan,
        };
        let tolerance_pt = (PICK_RADIUS_PX / viewport.zoom) as f64;
        let grip_tol = (GRIP_RADIUS_PX / viewport.zoom) as f64;
        let mut hovered_grip = None;

        // ---------------------------------------------------------- gestures
        if response.drag_started() {
            self.drag = Vec2::ZERO;
            // Precedence, and each step is a different kind of target:
            //   1. a point of the selected series, if its data is editable
            //   2. the axis frame -- a thin target the user had to aim at, and
            //      one a press can land a hair *outside* of, so it is checked
            //      against a tolerance rather than against the data area
            //   3. inside the data area: pan the data
            //   4. anywhere else: pan the view
            // Where the press *began*, not where the pointer is now: a drag is
            // only recognised once it has moved past a threshold, and by then
            // the pointer has left a target as thin as the axis frame. This
            // matters for the other gestures too -- at low zoom those few pixels
            // are enough to grab a different data point than the one clicked.
            let origin = ui
                .input(|i| i.pointer.press_origin())
                .or_else(|| response.interact_pointer_pos());
            self.gesture = Some(
                origin
                    .and_then(|p| locate(&viewport, &boxes, p))
                    .and_then(|(page, pt)| {
                        let inside = scenes
                            .iter()
                            .find(|s| s.page == page && s.contains_page_point(pt));

                        let grabbed = inside.and_then(|scene| {
                            scene
                                .hit(pt, tolerance_pt)
                                .filter(|h| Some(h.node) == selected && editable.contains(&h.node))
                        });
                        if let (Some(scene), Some(h)) = (inside, grabbed) {
                            return Some(Gesture::MovePoint {
                                node: h.node,
                                index: h.index,
                                start: h.data,
                                axes: (scene.transform.x, scene.transform.y),
                            });
                        }

                        // A rule spans the frame, so it is grabbed anywhere along
                        // its length -- checked after points, so a marker sitting on
                        // one still wins. `editable` is the same gate the point drag
                        // uses: the editor puts a rules call there only when every
                        // one of its coordinates is a literal number it can rewrite.
                        let rule = inside.and_then(|scene| {
                            scene
                                .hit_rule(pt, tolerance_pt)
                                .filter(|h| editable.contains(&h.node))
                                .and_then(|h| {
                                    let axis =
                                        scene.series.iter().find(|g| g.node == h.node).and_then(
                                            |g| match g.shape {
                                                lilook_core::SeriesShape::Rules(a) => Some(a),
                                                _ => None,
                                            },
                                        )?;
                                    Some(Gesture::MoveRule {
                                        node: h.node,
                                        slot: h.index,
                                        start: match axis {
                                            lilook_core::Axis::X => h.data.0,
                                            lilook_core::Axis::Y => h.data.1,
                                        },
                                        axis,
                                        map: match axis {
                                            lilook_core::Axis::X => scene.transform.x,
                                            lilook_core::Axis::Y => scene.transform.y,
                                        },
                                    })
                                })
                        });
                        if let Some(g) = rule {
                            return Some(g);
                        }

                        if let Some((figure, grip)) = grip_at(scenes, page, pt, grip_tol) {
                            let scene = scenes.iter().find(|s| s.figure == figure)?;
                            return Some(Gesture::Resize {
                                figure,
                                start: (scene.area.2 - scene.area.0, scene.area.3 - scene.area.1),
                                grip,
                            });
                        }

                        inside.map(|scene| Gesture::DataPan {
                            figure: scene.figure,
                            start: (scene.transform.x, scene.transform.y),
                        })
                    })
                    .unwrap_or(Gesture::ViewPan),
            );
            if !matches!(self.gesture, Some(Gesture::ViewPan)) {
                events.push(CanvasEvent::Begin);
            }
        }

        if response.dragged() {
            self.drag += response.drag_delta();
            match &self.gesture {
                Some(Gesture::ViewPan) => viewport.pan += response.drag_delta(),
                Some(Gesture::DataPan { figure, start }) => {
                    // The shift is applied in each axis's own space, so a log
                    // axis slides by a ratio. Screen pixels are divided by the
                    // live zoom first, because `scale` is in page points.
                    let page = |px: f32| px as f64 / viewport.zoom as f64;
                    events.push(CanvasEvent::SetLimits {
                        figure: *figure,
                        x: start.0.shifted(page(self.drag.x)),
                        y: start.1.shifted(page(self.drag.y)),
                    });
                }
                Some(Gesture::Resize {
                    figure,
                    start,
                    grip,
                }) => {
                    // Screen pixels straight to points: the frame follows the
                    // pointer, which is the whole point of dragging it.
                    let d = self.drag / viewport.zoom;
                    let (w, h) = (
                        (start.0 + d.x as f64).max(MIN_SIZE_PT),
                        (start.1 + d.y as f64).max(MIN_SIZE_PT),
                    );
                    events.push(CanvasEvent::SetSize {
                        figure: *figure,
                        width_pt: matches!(grip, Grip::Right | Grip::Corner).then_some(w),
                        height_pt: matches!(grip, Grip::Bottom | Grip::Corner).then_some(h),
                    });
                }
                Some(Gesture::MoveRule {
                    node,
                    slot,
                    start,
                    axis,
                    map,
                }) => {
                    let px = match axis {
                        lilook_core::Axis::X => self.drag.x,
                        lilook_core::Axis::Y => self.drag.y,
                    };
                    events.push(CanvasEvent::MoveRule {
                        node: *node,
                        slot: *slot,
                        to: map.nudged(*start, px as f64 / viewport.zoom as f64),
                    });
                }
                Some(Gesture::MovePoint {
                    node,
                    index,
                    start,
                    axes,
                }) => {
                    // Same reasoning as the pan: on a log axis a point follows the
                    // pointer only if it moves by a ratio.
                    let page = |px: f32| px as f64 / viewport.zoom as f64;
                    events.push(CanvasEvent::MovePoint {
                        node: *node,
                        index: *index,
                        to: (
                            axes.0.nudged(start.0, page(self.drag.x)),
                            axes.1.nudged(start.1, page(self.drag.y)),
                        ),
                    });
                }
                None => {}
            }
        }

        if response.drag_stopped() {
            if !matches!(self.gesture, Some(Gesture::ViewPan)) {
                events.push(CanvasEvent::Commit);
            }
            self.gesture = None;
        }

        // ------------------------------------------------------- wheel/pinch
        let wheel = if response.hovered() {
            ui.input(|i| {
                let zoom_delta = i.zoom_delta();
                let scroll = i.smooth_scroll_delta;
                i.pointer.hover_pos().map(|p| (p, zoom_delta, scroll))
            })
        } else {
            None
        };
        if let Some((p, zoom_delta, scroll)) = wheel {
            let over_data = locate(&viewport, &boxes, p).and_then(|(page, pt)| {
                scenes
                    .iter()
                    .find(|s| s.page == page && s.contains_page_point(pt))
                    .map(|s| (s, pt))
            });
            match (zoom_delta != 1.0, over_data) {
                // Zooming with the pointer inside a diagram rescales the data,
                // not the picture of it. This is the gesture that makes the
                // figure feel like a plot rather than a PDF.
                (true, Some((scene, pt))) => {
                    let c = scene.transform.to_data(pt);
                    let f = 1.0 / zoom_delta as f64;
                    let (x, y) = (&scene.transform.x, &scene.transform.y);
                    if self.zooming.is_none() {
                        events.push(CanvasEvent::Begin);
                    }
                    self.zooming = Some(0);
                    events.push(CanvasEvent::SetLimits {
                        figure: scene.figure,
                        x: (c.0 + (x.min - c.0) * f, c.0 + (x.max - c.0) * f),
                        y: (c.1 + (y.min - c.1) * f, c.1 + (y.max - c.1) * f),
                    });
                }
                (true, None) => viewport.zoom_about(p, zoom_delta, ZOOM_LIMITS),
                (false, _) => viewport.pan += scroll,
            }
        }
        if let Some(idle) = self.zooming {
            let still = wheel.map(|(_, z, _)| z != 1.0).unwrap_or(false);
            if still {
                self.zooming = Some(0);
            } else if idle >= ZOOM_IDLE_FRAMES {
                events.push(CanvasEvent::Commit);
                self.zooming = None;
            } else {
                self.zooming = Some(idle + 1);
                ui.ctx().request_repaint();
            }
        }

        self.zoom = viewport.zoom;
        self.pan = viewport.pan;
        self.view = Some(viewport);

        // ------------------------------------------------------------ paint
        let painter = ui.painter_at(rect);
        // Deliberately not `extreme_bg_color`: in a light theme that is white,
        // and a white page on a white canvas has no edge.
        let backdrop = if ui.visuals().dark_mode {
            Color32::from_gray(24)
        } else {
            Color32::from_gray(168)
        };
        painter.rect_filled(rect, 0.0, backdrop);
        for (page, b) in pages.iter().zip(&boxes) {
            let r = viewport.screen_rect(b);
            if !r.intersects(rect) {
                continue;
            }
            painter.rect_filled(r.expand(1.0), 2.0, Color32::from_black_alpha(60));
            painter.rect_filled(r, 0.0, Color32::WHITE);
            painter.image(
                page.texture,
                r,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }

        // ------------------------------------------------- hover and picking
        let hover = response
            .hover_pos()
            .and_then(|p| locate(&viewport, &boxes, p));
        let hovered = hover.and_then(|(page, pt)| pick(scenes, page, pt, tolerance_pt));
        if self.gesture.is_none() {
            hovered_grip = hover.and_then(|(page, pt)| grip_at(scenes, page, pt, grip_tol));
        }
        // The cursor is the only affordance a thin target gets before it is
        // grabbed, so it has to be right.
        if let Some((_, grip)) = hovered_grip {
            ui.ctx().set_cursor_icon(match grip {
                Grip::Right => egui::CursorIcon::ResizeHorizontal,
                Grip::Bottom => egui::CursorIcon::ResizeVertical,
                Grip::Corner => egui::CursorIcon::ResizeNwSe,
            });
        }

        if response.clicked() {
            if let Some((page, pt)) = hover {
                // Precedence: a point or its curve first, the diagram behind it
                // second. Clicking the background of a figure selects the
                // figure, which is how you get at its axes.
                let target = hovered
                    .as_ref()
                    .map(|(_, hit)| hit.node)
                    .or_else(|| figure_at(scenes, page, pt));
                if let Some(node) = target {
                    events.push(CanvasEvent::Select(node));
                }
            }
        }

        // ----------------------------------------------------------- overlay
        let accent = ui.visuals().selection.bg_fill;
        for scene in scenes {
            let Some(b) = boxes.get(scene.page) else {
                continue;
            };
            let selected_here = selected == Some(scene.figure)
                || scene.series.iter().any(|s| Some(s.node) == selected);
            if selected_here {
                outline_page_rect(
                    &painter,
                    &viewport,
                    b,
                    scene.area,
                    accent.gamma_multiply(0.7),
                );
            }
            for series in &scene.series {
                if Some(series.node) != selected {
                    continue;
                }
                // Handles on every point would be a wall of dots on a 5k-point
                // series; they are drawn only where they can be acted on.
                if series.points.len() > 200 {
                    continue;
                }
                let movable = editable.contains(&series.node);
                for &p in &series.points {
                    let at = viewport.to_screen(b.to_doc(scene.transform.to_page(p)));
                    // A handle lands on top of lilaq's own mark, which may be
                    // any colour, so it carries its own light halo rather than
                    // relying on contrast with whatever is underneath.
                    painter.circle_stroke(
                        at,
                        4.0,
                        Stroke::new(3.0, Color32::from_white_alpha(220)),
                    );
                    if movable {
                        painter.circle_filled(at, 4.0, accent);
                    } else {
                        // Hollow: hit-testable, but its data is an expression
                        // rather than something a drag could rewrite.
                        painter.circle_stroke(at, 4.0, Stroke::new(1.6, accent));
                    }
                }
            }
        }
        if let Some((page, hit)) = &hovered {
            if let (Some(b), Some(scene)) =
                (boxes.get(*page), scenes.iter().find(|s| s.page == *page))
            {
                let at = viewport.to_screen(b.to_doc(scene.transform.to_page(hit.data)));
                painter.circle_stroke(at, 4.5, Stroke::new(1.5, accent));
            }
        }

        CanvasOutput {
            response,
            viewport,
            pages: boxes,
            hover,
            hovered,
            events,
        }
    }
}

/// Which page a screen position is over, and where on it.
fn locate(viewport: &Viewport, boxes: &[PageBox], p: Pos2) -> Option<(usize, (f64, f64))> {
    let doc = viewport.to_doc(p);
    boxes
        .iter()
        .find(|b| b.contains(doc))
        .map(|b| (b.index, b.to_page(doc)))
}

/// Is the pointer on the right edge, bottom edge or corner of a data area?
///
/// The right and bottom edges only: dragging the left or top edge would move
/// the figure on the page as well as resize it, and where a diagram sits is the
/// surrounding document's business, not the figure's.
fn grip_at(scenes: &[Scene], page: usize, pt: (f64, f64), tol: f64) -> Option<(usize, Grip)> {
    for s in scenes.iter().filter(|s| s.page == page) {
        let (x0, y0, x1, y1) = s.area;
        let on_right = (pt.0 - x1).abs() <= tol && pt.1 >= y0 - tol && pt.1 <= y1 + tol;
        let on_bottom = (pt.1 - y1).abs() <= tol && pt.0 >= x0 - tol && pt.0 <= x1 + tol;
        let grip = match (on_right, on_bottom) {
            (true, true) => Grip::Corner,
            (true, false) => Grip::Right,
            (false, true) => Grip::Bottom,
            (false, false) => continue,
        };
        return Some((s.figure, grip));
    }
    None
}

/// Nearest series point across every scene on this page.
fn pick(
    scenes: &[Scene],
    page: usize,
    pt: (f64, f64),
    tolerance_pt: f64,
) -> Option<(usize, SceneHit)> {
    scenes
        .iter()
        .filter(|s| s.page == page)
        // Vertices and line segments first: an explicit marker beats an area, so
        // a scatter drawn on top of a colormesh is still pickable.
        .filter_map(|s| {
            s.hit_segment(pt, tolerance_pt)
                .or_else(|| s.hit_mesh(pt))
                .map(|h| (page, h))
        })
        .min_by(|a, b| {
            a.1.distance_pt
                .partial_cmp(&b.1.distance_pt)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
}

/// The diagram whose data area contains this point.
fn figure_at(scenes: &[Scene], page: usize, pt: (f64, f64)) -> Option<usize> {
    scenes
        .iter()
        .find(|s| s.page == page && s.contains_page_point(pt))
        .map(|s| s.figure)
}

/// Outline a region of a page, in page points.
pub fn outline_page_rect(
    painter: &egui::Painter,
    viewport: &Viewport,
    page: &PageBox,
    rect_pt: (f64, f64, f64, f64),
    color: Color32,
) {
    let min = viewport.to_screen(page.to_doc((rect_pt.0, rect_pt.1)));
    let max = viewport.to_screen(page.to_doc((rect_pt.2, rect_pt.3)));
    painter.rect_stroke(
        Rect::from_two_pos(min, max).expand(2.0),
        2.0,
        Stroke::new(1.5, color),
        StrokeKind::Outside,
    );
}
