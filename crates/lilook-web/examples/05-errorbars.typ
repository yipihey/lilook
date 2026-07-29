// Measurements with uncertainties.
#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 6pt)

#lq.diagram(
  width: 9cm, height: 5cm,
  xlabel: [time (s)], ylabel: [signal],
  lq.plot(
    (1, 2, 3, 4, 5, 6),
    (2.1, 3.4, 3.9, 4.2, 4.4, 4.5),
    yerr: (0.3, 0.25, 0.4, 0.2, 0.35, 0.3),
    mark: "o", stroke: 1pt + purple,
  ),
)
