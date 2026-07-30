// Threshold lines. Each coordinate is its own argument, so each line can be
// dragged on its own -- grab one anywhere along its length and it follows the
// pointer, rewriting just that number.
#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 6pt)

#let t = lq.linspace(0, 12, num: 60)

#lq.diagram(
  width: 8cm,
  height: 5cm,
  xlabel: [time / h],
  ylabel: [signal],
  lq.plot(t, t.map(x => 3 + 2 * calc.sin(x) + x / 6), mark: none),
  lq.hlines(4, 6.5, stroke: (paint: red, dash: "dashed"), label: [limits]),
  lq.vlines(8, stroke: (paint: blue, dash: "dotted"), label: [cutoff]),
)
