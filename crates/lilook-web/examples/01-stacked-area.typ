// Stacked area chart, after lilaq.org/docs/examples/stacked-area-chart
#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 6pt)

#let stacked-area(x, ..y) = {
  y.pos()
    .fold(((0,) * x.len(),), (ys-stacked, ys) => {
      let previous = ys-stacked.last()
      ys-stacked.push(lq.vec.add(previous, ys))
      ys-stacked
    })
    .windows(2)
    .map(((y1, y2)) => lq.fill-between(x, y1, y2: y2))
}

#let x = range(10)
#let y1 = (0, 1, 3, 9, 5, 4, 2, 2, 1, 0)
#let y2 = (5, 3, 2, 0, 1, 2, 2, 2, 3, 2)
#let y3 = (0, 0, 0, 0, 1, 2, 4, 5, 5, 9)

#lq.diagram(
  width: 9cm, height: 5cm,
  xlabel: [day], ylabel: [count],
  ..stacked-area(x, y1, y2, y3),
)
