//! The coordinate chain, in one place.
//!
//! A click travels screen px -> document pt -> page pt -> data units, and an
//! overlay travels the other way. Every step is a scale and a translation, and
//! every one of them is easy to get subtly wrong in a paint closure, so they
//! live here as plain arithmetic with tests rather than inline in the canvas.
//!
//! Typst page coordinates grow right and *down*, exactly like screen
//! coordinates, so this half of the chain has no flip in it. The flip lives in
//! the data<->page transform, where `AxisMap::scale` is negative on y.

use egui::{Pos2, Rect, Vec2};

/// Where a page sits in document space, in typographic points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageBox {
    pub index: usize,
    pub origin: (f64, f64),
    pub size: (f64, f64),
}

impl PageBox {
    pub fn contains(&self, doc: (f64, f64)) -> bool {
        doc.0 >= self.origin.0
            && doc.1 >= self.origin.1
            && doc.0 <= self.origin.0 + self.size.0
            && doc.1 <= self.origin.1 + self.size.1
    }

    /// Document point -> point relative to this page's top-left, which is what
    /// probe positions are expressed in.
    pub fn to_page(&self, doc: (f64, f64)) -> (f64, f64) {
        (doc.0 - self.origin.0, doc.1 - self.origin.1)
    }

    pub fn to_doc(&self, page: (f64, f64)) -> (f64, f64) {
        (page.0 + self.origin.0, page.1 + self.origin.1)
    }
}

/// Stack pages vertically, centred, the way a PDF viewer does.
pub fn stack_pages(sizes: &[(f64, f64)], gap: f64) -> Vec<PageBox> {
    let widest = sizes.iter().map(|s| s.0).fold(0.0, f64::max);
    let mut y = 0.0;
    sizes
        .iter()
        .enumerate()
        .map(|(index, &size)| {
            let b = PageBox {
                index,
                origin: ((widest - size.0) / 2.0, y),
                size,
            };
            y += size.1 + gap;
            b
        })
        .collect()
}

pub fn stacked_size(boxes: &[PageBox]) -> (f64, f64) {
    let w = boxes
        .iter()
        .map(|b| b.origin.0 + b.size.0)
        .fold(0.0, f64::max);
    let h = boxes
        .iter()
        .map(|b| b.origin.1 + b.size.1)
        .fold(0.0, f64::max);
    (w, h)
}

/// Document space (pt) <-> screen space (px), for one frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    /// Widget area on screen.
    pub rect: Rect,
    /// Screen pixels per typographic point.
    pub zoom: f32,
    /// Screen offset of the document origin, relative to `rect.min`.
    pub pan: Vec2,
}

impl Viewport {
    pub fn to_screen(&self, doc: (f64, f64)) -> Pos2 {
        self.rect.min + self.pan + Vec2::new(doc.0 as f32, doc.1 as f32) * self.zoom
    }

    pub fn to_doc(&self, screen: Pos2) -> (f64, f64) {
        let v = (screen - self.rect.min - self.pan) / self.zoom;
        (v.x as f64, v.y as f64)
    }

    pub fn screen_rect(&self, b: &PageBox) -> Rect {
        Rect::from_min_size(
            self.to_screen(b.origin),
            Vec2::new(b.size.0 as f32, b.size.1 as f32) * self.zoom,
        )
    }

    /// Zoom about a fixed screen position, so the document does not slide out
    /// from under the pointer.
    pub fn zoom_about(&mut self, screen: Pos2, factor: f32, limits: (f32, f32)) {
        let before = self.to_doc(screen);
        self.zoom = (self.zoom * factor).clamp(limits.0, limits.1);
        let after = self.to_doc(screen);
        self.pan += Vec2::new((after.0 - before.0) as f32, (after.1 - before.1) as f32) * self.zoom;
    }

    /// Scale and centre so `size` fits inside `rect` with a little air.
    pub fn fit(rect: Rect, size: (f64, f64), margin: f32) -> Viewport {
        let avail = rect.size() - Vec2::splat(margin * 2.0);
        let zoom = if size.0 > 0.0 && size.1 > 0.0 {
            (avail.x / size.0 as f32)
                .min(avail.y / size.1 as f32)
                .clamp(0.05, 8.0)
        } else {
            1.0
        };
        let scaled = Vec2::new(size.0 as f32, size.1 as f32) * zoom;
        Viewport {
            rect,
            zoom,
            pan: (rect.size() - scaled) / 2.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vp() -> Viewport {
        Viewport {
            rect: Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(400.0, 300.0)),
            zoom: 2.0,
            pan: Vec2::new(5.0, 7.0),
        }
    }

    #[test]
    fn screen_and_document_round_trip() {
        let v = vp();
        for p in [(0.0, 0.0), (100.0, 42.5), (-3.0, 900.0)] {
            let back = v.to_doc(v.to_screen(p));
            assert!(
                (back.0 - p.0).abs() < 1e-3 && (back.1 - p.1).abs() < 1e-3,
                "{back:?}"
            );
        }
    }

    #[test]
    fn zoom_keeps_the_point_under_the_cursor() {
        let mut v = vp();
        let cursor = Pos2::new(123.0, 210.0);
        let before = v.to_doc(cursor);
        v.zoom_about(cursor, 1.7, (0.05, 8.0));
        let after = v.to_doc(cursor);
        assert!(
            (before.0 - after.0).abs() < 1e-3 && (before.1 - after.1).abs() < 1e-3,
            "{before:?} vs {after:?}"
        );
        assert!((v.zoom - 3.4).abs() < 1e-5);
    }

    #[test]
    fn zoom_respects_its_limits() {
        let mut v = vp();
        for _ in 0..50 {
            v.zoom_about(v.rect.center(), 2.0, (0.05, 8.0));
        }
        assert_eq!(v.zoom, 8.0);
    }

    #[test]
    fn fit_centres_the_document() {
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 200.0));
        let v = Viewport::fit(rect, (100.0, 50.0), 10.0);
        // Limited by height: (200 - 20) / 50 = 3.6
        assert!((v.zoom - 3.6).abs() < 1e-5, "{}", v.zoom);
        let centre = v.to_screen((50.0, 25.0));
        assert!((centre - rect.center()).length() < 1e-3, "{centre:?}");
    }

    #[test]
    fn pages_stack_centred_with_a_gap() {
        let boxes = stack_pages(&[(100.0, 50.0), (60.0, 40.0)], 10.0);
        assert_eq!(boxes[0].origin, (0.0, 0.0));
        assert_eq!(boxes[1].origin, (20.0, 60.0));
        assert_eq!(stacked_size(&boxes), (100.0, 100.0));
        assert!(boxes[1].contains((25.0, 65.0)));
        assert!(!boxes[0].contains((25.0, 65.0)));
        assert_eq!(boxes[1].to_page((25.0, 65.0)), (5.0, 5.0));
    }
}
