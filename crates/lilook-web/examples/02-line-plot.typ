// The simplest thing that is still a figure: two lines with literal data.
// Click a curve to select it, drag a point to move it.
#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 6pt)

#lq.diagram(
  width: 9cm, height: 5cm,
  legend: (position: top + left),
  xlabel: [dose (mg)], ylabel: [response],
  lq.plot(
    (0, 1, 2, 3, 4, 5),
    (0.2, 1.1, 2.4, 2.9, 3.6, 3.8),
    mark: "o", stroke: 1pt + red, label: [treated],
  ),
  lq.plot(
    (0, 1, 2, 3, 4, 5),
    (0.1, 0.6, 1.4, 2.6, 3.9, 4.4),
    mark: "s", stroke: 1pt + blue, label: [control],
  ),
)
