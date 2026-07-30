// Distributions. Each dataset is its own argument, and its position along x
// comes from `x:` -- `auto` by default, which lilaq resolves to 1, 2, 3...
//
// Click a box or a violin to select the call that drew it. lilook reports how
// many datasets there are and how many values went into each; it does not
// recompute the quartiles, so what it claims to know is only what it read.
#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 6pt)

#let control = (4.1, 4.4, 4.8, 5.0, 5.1, 5.3, 5.6, 6.2, 7.9)
#let treated = (5.2, 5.5, 5.9, 6.0, 6.1, 6.4, 6.6, 7.1)
#let recovery = (4.6, 4.9, 5.2, 5.4, 5.5, 5.7, 6.0, 6.3, 6.8, 8.4)

#lq.diagram(
  width: 8cm,
  height: 5cm,
  ylabel: [response],
  xaxis: (ticks: ((1, [control]), (2, [treated]), (3, [recovery]))),
  lq.boxplot(control, treated, recovery, fill: blue.lighten(70%)),
)
