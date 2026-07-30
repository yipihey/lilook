//! What the canvas needs to know about a compiled figure.
//!
//! A `Scene` is the bridge between pixels and byte ranges: it carries, for one
//! diagram, where its data area landed on the page, the data<->page transform,
//! and the evaluated points of every series *paired with the call site that
//! produced them*. That last part is why clicking a curve can select the right
//! `lq.plot(..)` even though the curve itself carries no span -- see ADR-0008.
//!
//! The type lives in core rather than in the compile backend so that the UI can
//! consume it without depending on a typesetter.

use crate::compile::Transform;

/// One series' evaluated data, in data units.
#[derive(Debug, Clone, PartialEq)]
pub struct SeriesGeom {
    /// The call site that drew it.
    pub node: usize,
    /// The positions the series drew, which are the only values that can be
    /// hit-tested or dragged. Kept as pairs rather than folded into `channels`
    /// because that is a real distinction: `x` and `y` are geometry, and an error
    /// bar is not a place on the page you can pick up.
    pub points: Vec<(f64, f64)>,
    /// Other numeric arrays the call passed, by argument name -- `yerr`, `xerr`
    /// and anything else that carries data rather than style.
    ///
    /// Needed because Veusz's ASCII descriptor names error columns (`+-`, `+`,
    /// `-`), so a linked dataset can feed `yerr:`. Without this such a column
    /// would be linkable but invisible: no length to check, no staleness, no
    /// unlock.
    pub channels: Vec<(String, Vec<f64>)>,
}

impl SeriesGeom {
    /// A channel by name, `x` and `y` included.
    pub fn channel(&self, name: &str) -> Option<Vec<f64>> {
        match name {
            "x" => Some(self.points.iter().map(|p| p.0).collect()),
            "y" => Some(self.points.iter().map(|p| p.1).collect()),
            _ => self
                .channels
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.clone()),
        }
    }

    /// Every channel's name and length, for a UI that wants to list them.
    pub fn channel_lengths(&self) -> Vec<(String, usize)> {
        let mut out = vec![
            ("x".to_string(), self.points.len()),
            ("y".to_string(), self.points.len()),
        ];
        out.extend(self.channels.iter().map(|(n, v)| (n.clone(), v.len())));
        out
    }
}

/// Axis-aligned extent in data units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub x: (f64, f64),
    pub y: (f64, f64),
}

impl Bounds {
    pub fn of(points: &[(f64, f64)]) -> Option<Bounds> {
        let mut it = points.iter().filter(|p| p.0.is_finite() && p.1.is_finite());
        let first = it.next()?;
        let mut b = Bounds {
            x: (first.0, first.0),
            y: (first.1, first.1),
        };
        for p in it {
            b.x.0 = b.x.0.min(p.0);
            b.x.1 = b.x.1.max(p.0);
            b.y.0 = b.y.0.min(p.1);
            b.y.1 = b.y.1.max(p.1);
        }
        Some(b)
    }

    pub fn union(self, other: Bounds) -> Bounds {
        Bounds {
            x: (self.x.0.min(other.x.0), self.x.1.max(other.x.1)),
            y: (self.y.0.min(other.y.0), self.y.1.max(other.y.1)),
        }
    }

    pub fn contains(&self, p: (f64, f64)) -> bool {
        p.0 >= self.x.0 && p.0 <= self.x.1 && p.1 >= self.y.0 && p.1 <= self.y.1
    }

    /// A point `t` of the way across, used to place probes well inside the
    /// axis limits -- probes outside them displace the layout origin.
    pub fn lerp(&self, t: (f64, f64)) -> (f64, f64) {
        (
            self.x.0 + t.0 * (self.x.1 - self.x.0),
            self.y.0 + t.1 * (self.y.1 - self.y.0),
        )
    }

    /// Degenerate axes (a single-valued series, a constant) would give a
    /// zero-separation probe pair and an unsolvable transform.
    pub fn padded(self) -> Bounds {
        let pad = |a: f64, b: f64| {
            if (b - a).abs() > f64::EPSILON * 8.0 {
                (a, b)
            } else if a.abs() > f64::EPSILON {
                (a - a.abs() * 0.5, b + b.abs() * 0.5)
            } else {
                (-1.0, 1.0)
            }
        };
        Bounds {
            x: pad(self.x.0, self.x.1),
            y: pad(self.y.0, self.y.1),
        }
    }
}

/// A hit in data space, resolved all the way back to a call site.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneHit {
    pub node: usize,
    pub index: usize,
    pub data: (f64, f64),
    pub distance_pt: f64,
}

/// One diagram, as it was laid out.
#[derive(Debug, Clone, PartialEq)]
pub struct Scene {
    /// The `lq.diagram` call site.
    pub figure: usize,
    /// Which page it landed on.
    pub page: usize,
    /// The data area on that page, in points: (x0, y0, x1, y1), y growing down.
    pub area: (f64, f64, f64, f64),
    pub transform: Transform,
    pub series: Vec<SeriesGeom>,
}

impl Scene {
    /// Nearest data point to a position on the page, with the tolerance in page
    /// points so selection behaves the same at any zoom.
    pub fn hit(&self, page_pt: (f64, f64), tolerance_pt: f64) -> Option<SceneHit> {
        let mut best: Option<SceneHit> = None;
        for s in &self.series {
            for (index, &p) in s.points.iter().enumerate() {
                let q = self.transform.to_page(p);
                let d = ((q.0 - page_pt.0).powi(2) + (q.1 - page_pt.1).powi(2)).sqrt();
                if d <= tolerance_pt && best.as_ref().is_none_or(|b| d < b.distance_pt) {
                    best = Some(SceneHit {
                        node: s.node,
                        index,
                        data: p,
                        distance_pt: d,
                    });
                }
            }
        }
        best
    }

    /// Nearest point *on a segment* rather than at a vertex, so a line drawn
    /// through few points is still clickable between them.
    pub fn hit_segment(&self, page_pt: (f64, f64), tolerance_pt: f64) -> Option<SceneHit> {
        let mut best: Option<SceneHit> = None;
        for s in &self.series {
            for (index, pair) in s.points.windows(2).enumerate() {
                let (a, b) = (
                    self.transform.to_page(pair[0]),
                    self.transform.to_page(pair[1]),
                );
                let (dx, dy) = (b.0 - a.0, b.1 - a.1);
                let len2 = dx * dx + dy * dy;
                let t = if len2 <= f64::EPSILON {
                    0.0
                } else {
                    (((page_pt.0 - a.0) * dx + (page_pt.1 - a.1) * dy) / len2).clamp(0.0, 1.0)
                };
                let q = (a.0 + t * dx, a.1 + t * dy);
                let d = ((q.0 - page_pt.0).powi(2) + (q.1 - page_pt.1).powi(2)).sqrt();
                if d <= tolerance_pt && best.as_ref().is_none_or(|x| d < x.distance_pt) {
                    // Report the nearer endpoint: what the user grabs is a
                    // point, even when what they aimed at was the line.
                    let index = if t < 0.5 { index } else { index + 1 };
                    best = Some(SceneHit {
                        node: s.node,
                        index,
                        data: s.points[index],
                        distance_pt: d,
                    });
                }
            }
        }
        best
    }

    pub fn contains_page_point(&self, p: (f64, f64)) -> bool {
        p.0 >= self.area.0 && p.0 <= self.area.2 && p.1 >= self.area.1 && p.1 <= self.area.3
    }

    pub fn bounds(&self) -> Option<Bounds> {
        self.series
            .iter()
            .filter_map(|s| Bounds::of(&s.points))
            .reduce(Bounds::union)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::AxisMap;

    fn scene() -> Scene {
        // 10 data units across 100 pt, y inverted as on a page.
        Scene {
            figure: 0,
            page: 0,
            area: (0.0, 0.0, 100.0, 100.0),
            transform: Transform {
                x: AxisMap {
                    origin: 0.0,
                    scale: 10.0,
                    min: 0.0,
                    max: 10.0,
                },
                y: AxisMap {
                    origin: 100.0,
                    scale: -10.0,
                    min: 0.0,
                    max: 10.0,
                },
            },
            series: vec![
                SeriesGeom {
                    node: 7,
                    channels: vec![],
                    points: vec![(0.0, 0.0), (5.0, 5.0), (10.0, 0.0)],
                },
                SeriesGeom {
                    node: 9,
                    channels: vec![],
                    points: vec![(0.0, 9.0), (10.0, 9.0)],
                },
            ],
        }
    }

    #[test]
    fn a_hit_names_the_call_site_that_drew_it() {
        let s = scene();
        let near = s.transform.to_page((5.0, 5.0));
        let hit = s.hit((near.0 + 2.0, near.1 - 1.0), 6.0).expect("hit");
        assert_eq!((hit.node, hit.index), (7, 1));

        let other = s.transform.to_page((0.0, 9.0));
        assert_eq!(s.hit(other, 6.0).unwrap().node, 9);
    }

    #[test]
    fn tolerance_is_in_page_points() {
        let s = scene();
        let near = s.transform.to_page((5.0, 5.0));
        assert!(s.hit((near.0 + 20.0, near.1), 6.0).is_none());
    }

    #[test]
    fn segments_are_clickable_between_their_vertices() {
        let s = scene();
        // Halfway along the first segment: no vertex within tolerance...
        let mid = s.transform.to_page((2.5, 2.5));
        assert!(s.hit(mid, 4.0).is_none());
        // ...but the segment is there, and it reports the nearer end.
        let hit = s
            .hit_segment((mid.0, mid.1 + 1.0), 4.0)
            .expect("segment hit");
        assert_eq!(hit.node, 7);
        assert!(hit.index == 0 || hit.index == 1);
    }

    #[test]
    fn bounds_pad_a_degenerate_axis() {
        let b = Bounds::of(&[(1.0, 3.0), (2.0, 3.0)]).unwrap();
        assert_eq!(b.y, (3.0, 3.0));
        let p = b.padded();
        assert!(
            p.y.1 > p.y.0,
            "a flat series must still give a solvable axis"
        );
        assert_eq!(p.x, (1.0, 2.0));
    }

    #[test]
    fn lerp_places_probes_inside_the_data() {
        let b = Bounds {
            x: (0.0, 10.0),
            y: (-1.0, 1.0),
        };
        assert_eq!(b.lerp((0.1, 0.1)), (1.0, -0.8));
        assert!(b.contains(b.lerp((0.9, 0.9))));
    }
}
