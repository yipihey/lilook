// A grid of diagrams, after lilaq.org/docs/tutorials/plot-grids
//
// Plot grids are Typst's own `grid`, with `lq.layout` aligning the axes across
// cells so the frames line up even though each diagram sizes itself.
#import "@preview/lilaq:0.6.0" as lq
#set page(width: 10cm, height: auto, margin: 6pt)

#show: lq.layout
#show: lq.set-diagram(width: 100%, height: 100%)

#let mesh = lq.contour(
  lq.linspace(0, 1),
  lq.linspace(0, 9),
  (x, y) => 2 * x * y,
)

#grid(
  columns: 3,
  rows: (4cm, 3cm),
  fill: rgb("#ff856620"),
  column-gutter: 0.8em,
  row-gutter: 0.8em,
  grid.cell(
    colspan: 3,
    lq.diagram(
      xlabel: [Time],
      ylabel: [Intensity],
      lq.plot(range(10), (0, 3, 2, 5, 4, 6, 5, 8, 7, 9)),
    ),
  ),
  lq.diagram(
    title: [A],
    lq.plot((0, 1, 2), (2, 3, 5)),
  ),
  grid.cell(
    rowspan: 2,
    lq.diagram(
      ylabel: [offset],
      ylim: (0, 9),
      mesh,
    ),
  ),
  grid.cell(
    rowspan: 2,
    lq.colorbar(mesh),
  ),
  lq.diagram(
    title: [B],
    lq.plot((0, 1, 2), (2, 4, 5)),
  ),
)
