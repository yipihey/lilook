// A log axis. Pan and zoom work in data space here too.
#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 6pt)

#let x = lq.linspace(1, 100, num: 40)

#lq.diagram(
  width: 9cm, height: 5cm,
  yscale: "log",
  legend: (position: bottom + right),
  xlabel: [$n$], ylabel: [operations],
  lq.plot(x, x.map(n => n * calc.log(n + 1)), mark: none, label: [$n log n$]),
  lq.plot(x, x.map(n => n * n), mark: none, label: [$n^2$]),
)
