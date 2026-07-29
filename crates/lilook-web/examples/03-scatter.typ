// Scatter with a colour map: `color` takes an array, one value per point.
#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 6pt)

#let x = lq.linspace(0, 10, num: 60)
#let y = x.map(t => calc.sin(t) + 0.1 * t)

#lq.diagram(
  width: 9cm, height: 5cm,
  xlabel: $x$, ylabel: $sin x + x/10$,
  lq.scatter(x, y, size: 6pt, color: y, map: color.map.viridis),
)
