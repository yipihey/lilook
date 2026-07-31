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

impl Decoration {
    /// The diagram argument that controls it.
    pub fn param(self) -> &'static str {
        match self {
            Decoration::Legend => "legend",
            Decoration::Title => "title",
            Decoration::Label => "xlabel",
        }
    }
}

/// One series' evaluated data, in data units.
#[derive(Debug, Clone, PartialEq)]
pub struct SeriesGeom {
    /// The call site that drew it.
    pub node: usize,
    /// How to read what follows. Carried rather than re-derived so that a
    /// consumer holding only a `Scene` -- the canvas, a host frontend -- reads it
    /// the same way the probe wrote it.
    pub shape: SeriesShape,
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
    /// Set for a mesh-shaped series -- `colormesh`, `contour`, `mesh` -- as
    /// `(columns, rows)`: the lengths of the x and y *axes*.
    ///
    /// Those axes are independent, so there are no paired points and `points` is
    /// empty. A mesh is picked by the area it covers rather than by a marker,
    /// which is also what it looks like on the page.
    pub grid: Option<(usize, usize)>,
}

impl SeriesGeom {
    /// A channel by name, `x` and `y` included.
    ///
    /// For a mesh, `x` and `y` come from the stored axes rather than from paired
    /// points -- they have different lengths, which is the whole point.
    pub fn channel(&self, name: &str) -> Option<Vec<f64>> {
        if let Some((_, v)) = self.channels.iter().find(|(n, _)| n == name) {
            return Some(v.clone());
        }
        match name {
            "x" => Some(self.points.iter().map(|p| p.0).collect()),
            "y" => Some(self.points.iter().map(|p| p.1).collect()),
            _ => None,
        }
    }

    /// The field value under a mesh hit, addressed by the row-major cell index
    /// `hit_mesh` reports. `None` for any other shape, and when `z` was not
    /// recovered -- a field can be a function whose evaluation lilaq accepted
    /// but the probe did not.
    pub fn field_at(&self, index: usize) -> Option<f64> {
        if self.shape != SeriesShape::Mesh {
            return None;
        }
        self.channel("z")?.get(index).copied()
    }

    /// Every channel's name and length, for a UI that wants to list them.
    pub fn channel_lengths(&self) -> Vec<(String, usize)> {
        if self.grid.is_some() {
            return self
                .channels
                .iter()
                .map(|(n, v)| (n.clone(), v.len()))
                .collect();
        }
        let mut out = vec![
            ("x".to_string(), self.points.len()),
            ("y".to_string(), self.points.len()),
        ];
        out.extend(self.channels.iter().map(|(n, v)| (n.clone(), v.len())));
        out
    }

    /// How a UI should describe this series' data in one short phrase.
    ///
    /// Here rather than in the editor so that a mesh cannot be described as
    /// "0 pts" by one frontend and as a grid by another -- and so a test can
    /// assert it without driving a UI. The tree label silently kept saying
    /// "0 pts" for a colormesh after the shape landed, because the edit that was
    /// supposed to change it never matched.
    pub fn summary(&self) -> String {
        // A handle is not a data point: saying "1 pts" for an annotation both
        // reads badly and implies there is a series here to embed.
        if let SeriesShape::Anchor = self.shape {
            return match self.points.first() {
                Some((x, y)) => format!("at ({}, {})", crate::data_num(*x), crate::data_num(*y)),
                None => "unplaced".to_string(),
            };
        }
        if let SeriesShape::Vertices = self.shape {
            return match self.points.len() {
                1 => "1 vertex".to_string(),
                n => format!("{n} vertices"),
            };
        }
        if let SeriesShape::Distributions(_) = self.shape {
            let n = self.distributions().len();
            return match n {
                1 => "1 distribution".to_string(),
                n => format!("{n} distributions"),
            };
        }
        if let SeriesShape::Rules(axis) = self.shape {
            let n = self.rules().len();
            let which = match axis {
                Axis::X => "vertical",
                Axis::Y => "horizontal",
            };
            return match n {
                1 => format!("1 {which} line"),
                n => format!("{n} {which} lines"),
            };
        }
        match self.grid {
            Some((cols, rows)) => format!("{cols}×{rows} grid"),
            None => {
                let extra: String = self
                    .channels
                    .iter()
                    .map(|(n, v)| format!(" · {n} {}", v.len()))
                    .collect();
                format!("{} pts{extra}", self.points.len())
            }
        }
    }

    /// Each distribution's position and the values that went into it.
    ///
    /// The position comes from the call's `x:`/`y:`, already resolved -- `auto`
    /// means `1..n`, which is lilaq's default and the commonest case.
    pub fn distributions(&self) -> Vec<(f64, Vec<f64>)> {
        let SeriesShape::Distributions(axis) = self.shape else {
            return vec![];
        };
        let positions = match axis {
            Axis::X => self.channel("x"),
            Axis::Y => self.channel("y"),
        }
        .unwrap_or_default();
        positions
            .into_iter()
            .enumerate()
            .map(|(i, at)| {
                let values = self
                    .channels
                    .iter()
                    .find(|(n, _)| n == &format!("d{i}"))
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default();
                (at, values)
            })
            .collect()
    }

    /// The coordinates of a rules series, one per line.
    pub fn rules(&self) -> Vec<f64> {
        match self.shape {
            SeriesShape::Rules(Axis::X) => self.channel("x").unwrap_or_default(),
            SeriesShape::Rules(Axis::Y) => self.channel("y").unwrap_or_default(),
            _ => vec![],
        }
    }

    /// The rectangle a mesh covers, in data units, from its axes.
    pub fn extent(&self) -> Option<((f64, f64), (f64, f64))> {
        self.grid?;
        let x = self.channel("x")?;
        let y = self.channel("y")?;
        let span = |v: &[f64]| {
            let mut it = v.iter().copied().filter(|f| f.is_finite());
            let first = it.next()?;
            Some(it.fold((first, first), |(lo, hi), f| (lo.min(f), hi.max(f))))
        };
        Some((span(&x)?, span(&y)?))
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
    ///
    /// The test is *relative*. It used to be `f64::EPSILON * 8.0`, an absolute
    /// threshold, which called every axis narrower than about 1e-15 degenerate --
    /// so an axis spanning 1e-64 to 1e-59, which is small but perfectly ordinary
    /// after panning into a log plot, was replaced by `(-1, 1)`. A probe then went
    /// to data −1 on a logarithmic axis and lilaq refused the figure: "value must
    /// be strictly positive".
    ///
    /// Padding also keeps the sign of the data it is padding, for the same reason:
    /// widening a positive axis must not reach through zero.
    pub fn padded(self) -> Bounds {
        let pad = |a: f64, b: f64| {
            let span = b - a;
            let magnitude = a.abs().max(b.abs());
            if span > magnitude * 1e-12 {
                (a, b)
            } else if magnitude > 0.0 {
                // Half the magnitude either side: for a single positive value this
                // gives `v/2 .. 3v/2`, which stays positive.
                (a - magnitude * 0.5, b + magnitude * 0.5)
            } else {
                // Genuinely nothing to scale by -- every value is zero.
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
    /// Parts of the figure that are drawn but are not series, with where they
    /// landed on the page. Pickable, and a legend is draggable.
    pub decorations: Vec<(Decoration, (f64, f64))>,
    /// The `lq.diagram` call site.
    pub figure: usize,
    /// Which page it landed on.
    pub page: usize,
    /// The data area on that page, in points: (x0, y0, x1, y1), y growing down.
    pub area: (f64, f64, f64, f64),
    pub transform: Transform,
    pub series: Vec<SeriesGeom>,
    /// Whether each axis's data is numbers at all.
    ///
    /// lilaq plots `datetime` coordinates too, and everything lilook does in data
    /// space assumes numbers: the probe recovers none, so the series is invisible
    /// to the canvas, and a pan would write `xlim: (0, 100)` -- which *compiles*,
    /// and silently replaces a calendar axis with a numeric one. Refusing the
    /// gesture is the honest answer until datetimes can be read and written back.
    pub numeric: (bool, bool),
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

    /// The distribution under this point, if any.
    ///
    /// Picked by nearest position, and only when the pointer is within the range
    /// of values that went into that box. lilook does not compute the quartiles,
    /// so the region is the data's own extent rather than a claim about where the
    /// whiskers ended up -- enough to select, and honest about what it knows.
    pub fn hit_distribution(&self, page_pt: (f64, f64), tolerance_pt: f64) -> Option<SceneHit> {
        let mut best: Option<SceneHit> = None;
        for s in &self.series {
            let SeriesShape::Distributions(axis) = s.shape else {
                continue;
            };
            let boxes = s.distributions();
            // Half the gap to the next box, so adjacent categories do not both
            // claim the same pixel. One box on its own gets the tolerance.
            let mut gaps: Vec<f64> = boxes.windows(2).map(|w| (w[1].0 - w[0].0).abs()).collect();
            gaps.retain(|g| g.is_finite() && *g > 0.0);
            let spacing = gaps.into_iter().fold(f64::INFINITY, f64::min);
            for (index, (at, values)) in boxes.iter().enumerate() {
                if values.is_empty() {
                    continue;
                }
                let (lo, hi) = values
                    .iter()
                    .fold((f64::MAX, f64::MIN), |(l, h), v| (l.min(*v), h.max(*v)));
                let (along, across, span_lo, span_hi) = match axis {
                    Axis::X => (
                        self.transform.x.to_page(*at),
                        page_pt.0,
                        self.transform.y.to_page(hi),
                        self.transform.y.to_page(lo),
                    ),
                    Axis::Y => (
                        self.transform.y.to_page(*at),
                        page_pt.1,
                        self.transform.x.to_page(lo),
                        self.transform.x.to_page(hi),
                    ),
                };
                let value_pt = match axis {
                    Axis::X => page_pt.1,
                    Axis::Y => page_pt.0,
                };
                if value_pt < span_lo - tolerance_pt || value_pt > span_hi + tolerance_pt {
                    continue;
                }
                let half = if spacing.is_finite() {
                    (self.transform.x.scale.abs().max(f64::MIN_POSITIVE) * spacing / 2.0)
                        .max(tolerance_pt)
                } else {
                    tolerance_pt * 3.0
                };
                let d = (along - across).abs();
                if d <= half && best.as_ref().is_none_or(|b| d < b.distance_pt) {
                    best = Some(SceneHit {
                        node: s.node,
                        index,
                        data: self.transform.to_data(page_pt),
                        distance_pt: d,
                    });
                }
            }
        }
        best
    }

    /// The rule line under this point, if any.
    ///
    /// A rule spans the whole frame, so only the distance across it matters --
    /// which is also why it cannot be found by `hit`: there is no vertex, and its
    /// other coordinate does not exist.
    ///
    /// `index` is the positional argument the line came from, because that is what
    /// an edit has to rewrite.
    pub fn hit_rule(&self, page_pt: (f64, f64), tolerance_pt: f64) -> Option<SceneHit> {
        let mut best: Option<SceneHit> = None;
        for s in &self.series {
            let SeriesShape::Rules(axis) = s.shape else {
                continue;
            };
            for (index, &coord) in s.rules().iter().enumerate() {
                let (at, across) = match axis {
                    Axis::X => (self.transform.x.to_page(coord), page_pt.0),
                    Axis::Y => (self.transform.y.to_page(coord), page_pt.1),
                };
                let d = (at - across).abs();
                if d <= tolerance_pt && best.as_ref().is_none_or(|b| d < b.distance_pt) {
                    let along = self.transform.to_data(page_pt);
                    best = Some(SceneHit {
                        node: s.node,
                        index,
                        // The grabbed coordinate on its own axis; the other one
                        // follows the pointer, since the line has none.
                        data: match axis {
                            Axis::X => (coord, along.1),
                            Axis::Y => (along.0, coord),
                        },
                        distance_pt: d,
                    });
                }
            }
        }
        best
    }

    /// The mesh under this point, if any.
    ///
    /// A mesh is picked by the area it covers rather than by a marker, because
    /// that is what it is: a field over a grid, with no vertex to aim at. Without
    /// this a click on a colormesh fell through to the diagram, so the series
    /// itself could only be reached from the tree.
    ///
    /// The nearest grid indices come back too, so a UI can say which cell.
    /// The field value a hit reads, if it landed on a mesh.
    pub fn field_at(&self, hit: &SceneHit) -> Option<f64> {
        self.series
            .iter()
            .find(|s| s.node == hit.node)?
            .field_at(hit.index)
    }

    pub fn hit_mesh(&self, page_pt: (f64, f64)) -> Option<SceneHit> {
        let data = self.transform.to_data(page_pt);
        for s in &self.series {
            let Some(((x0, x1), (y0, y1))) = s.extent() else {
                continue;
            };
            if data.0 < x0 || data.0 > x1 || data.1 < y0 || data.1 > y1 {
                continue;
            }
            // Index of the nearest node on each axis, so the caller can read the
            // field at the cursor.
            let nearest = |v: &[f64], at: f64| {
                v.iter()
                    .enumerate()
                    .min_by(|a, b| {
                        (a.1 - at)
                            .abs()
                            .partial_cmp(&(b.1 - at).abs())
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .map(|(i, _)| i)
                    .unwrap_or(0)
            };
            let (xs, ys) = (s.channel("x")?, s.channel("y")?);
            let (col, row) = (nearest(&xs, data.0), nearest(&ys, data.1));
            let cols = s.grid.map(|(c, _)| c).unwrap_or(1).max(1);
            return Some(SceneHit {
                node: s.node,
                // Row-major, so one index still names one cell.
                index: row * cols + col,
                data,
                distance_pt: 0.0,
            });
        }
        None
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
            numeric: (true, true),
            decorations: vec![],
            page: 0,
            area: (0.0, 0.0, 100.0, 100.0),
            transform: Transform {
                x: AxisMap {
                    origin: 0.0,
                    kind: AxisScale::Linear,
                    scale: 10.0,
                    min: 0.0,
                    max: 10.0,
                },
                y: AxisMap {
                    origin: 100.0,
                    kind: AxisScale::Linear,
                    scale: -10.0,
                    min: 0.0,
                    max: 10.0,
                },
            },
            series: vec![
                SeriesGeom {
                    node: 7,
                    shape: SeriesShape::Points,
                    channels: vec![],
                    grid: None,
                    points: vec![(0.0, 0.0), (5.0, 5.0), (10.0, 0.0)],
                },
                SeriesGeom {
                    node: 9,
                    shape: SeriesShape::Points,
                    channels: vec![],
                    grid: None,
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

    /// Small is not degenerate. An absolute epsilon called any axis narrower than
    /// ~1e-15 degenerate and replaced it with `(-1, 1)`; a probe then landed at
    /// data -1 on a log axis, which lilaq refuses outright.
    #[test]
    fn a_small_range_is_not_a_degenerate_one() {
        let tiny = Bounds {
            x: (3.48e-64, 1.05e-59),
            y: (1e-20, 2e-18),
        }
        .padded();
        assert_eq!(tiny.x, (3.48e-64, 1.05e-59), "left alone, not widened");
        assert_eq!(tiny.y, (1e-20, 2e-18));

        // Padding a single positive value stays positive, whatever its size.
        for v in [5.0, 1e-9, 1e-60, 1e60] {
            let p = Bounds {
                x: (v, v),
                y: (v, v),
            }
            .padded();
            assert!(p.x.0 > 0.0 && p.x.1 > p.x.0, "{v} padded to {:?}", p.x);
            // And the pad is proportionate, so a probe placed inside it is too.
            assert!((p.x.1 / p.x.0 - 3.0).abs() < 1e-9, "{:?}", p.x);
        }
        // Only an all-zero axis has no magnitude to scale by.
        let zero = Bounds {
            x: (0.0, 0.0),
            y: (0.0, 0.0),
        }
        .padded();
        assert_eq!(zero.x, (-1.0, 1.0));
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

/// Where a legend may sit, as lilaq spells it.
///
/// Nine alignments and nothing between them: dragging snaps, because a legend
/// half a millimetre off a corner reads as a mistake and lilaq has a name for
/// each of the nine.
pub const LEGEND_POSITIONS: [(&str, f64, f64); 9] = [
    ("top + left", 0.0, 0.0),
    ("top + center", 0.5, 0.0),
    ("top + right", 1.0, 0.0),
    ("horizon + left", 0.0, 0.5),
    ("horizon + center", 0.5, 0.5),
    ("horizon + right", 1.0, 0.5),
    ("bottom + left", 0.0, 1.0),
    ("bottom + center", 0.5, 1.0),
    ("bottom + right", 1.0, 1.0),
];

impl Scene {
    /// Which decoration is under a point, if any.
    ///
    /// A generous radius: these are small marks, and the thing a user aims at is
    /// the legend box rather than the anchor typst reported.
    pub fn hit_decoration(&self, page_pt: (f64, f64), tol: f64) -> Option<Decoration> {
        self.decorations
            .iter()
            .map(|(k, at)| {
                let d = (at.0 - page_pt.0).hypot(at.1 - page_pt.1);
                (*k, d)
            })
            .filter(|(_, d)| *d <= tol)
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(k, _)| k)
    }

    /// The legend position nearest a point in the data area, as lilaq spells it.
    pub fn nearest_legend_position(&self, page_pt: (f64, f64)) -> &'static str {
        let (x0, y0, x1, y1) = self.area;
        let (w, h) = ((x1 - x0).max(1.0), (y1 - y0).max(1.0));
        let (fx, fy) = ((page_pt.0 - x0) / w, (page_pt.1 - y0) / h);
        LEGEND_POSITIONS
            .iter()
            .min_by(|a, b| {
                let d = |p: &(&str, f64, f64)| (p.1 - fx).powi(2) + (p.2 - fy).powi(2);
                d(a).partial_cmp(&d(b)).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|p| p.0)
            .unwrap_or("top + left")
    }
}

#[cfg(test)]
use crate::compile::AxisScale;
use crate::compile::Transform;
use crate::doc::{Axis, SeriesShape};

/// A part of a figure that is drawn but is not a series: a legend, a title, an
/// axis label.
///
/// None of these is a call site -- they are *arguments* of the diagram -- so they
/// cannot be found the way a series is. typst can locate them, and which diagram
/// each belongs to is decided by where it landed rather than by counting, which
/// is the same principle as everything else here: identity is geometric.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decoration {
    Legend,
    Title,
    Label,
}
