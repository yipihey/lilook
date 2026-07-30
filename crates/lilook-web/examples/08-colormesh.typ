// A colormesh: a field over a grid, with the colour scale beside it.
//
// The two axes are independent -- 60 columns against 40 rows -- so there are no
// paired points here and nothing to drag. Click anywhere on the field to select
// it; the inspector reports the grid rather than a point count.
#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 6pt)

#let xs = lq.linspace(-3, 3, num: 60)
#let ys = lq.linspace(-2, 2, num: 40)

#let field = lq.colormesh(
  xs,
  ys,
  (x, y) => calc.exp(-(x * x + y * y) / 3) * calc.cos(3 * x),
  map: color.map.viridis,
)

// The colorbar is a diagram of its own, so it sits *beside* the field rather
// than inside it -- `lq.layout` lines the two frames up.
#show: lq.layout

#grid(
  columns: 2,
  column-gutter: 0.6em,
  lq.diagram(
    width: 8cm,
    height: 5cm,
    xlabel: [$x$],
    ylabel: [$y$],
    field,
  ),
  lq.colorbar(field, label: [amplitude]),
)
