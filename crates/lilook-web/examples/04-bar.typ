// A bar chart. Every argument here is editable from the inspector.
#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 6pt)

#lq.diagram(
  width: 9cm, height: 5cm,
  ylabel: [share],
  xaxis: (ticks: range(5).zip(([a], [b], [c], [d], [e]))),
  lq.bar(range(5), (3.2, 4.8, 2.1, 5.4, 3.9), fill: teal, stroke: none),
)
