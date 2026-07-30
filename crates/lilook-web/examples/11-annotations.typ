// Annotations: geometry placed on the figure rather than data read from it.
//
// Drag the label, the box, the circle or either end of the line. Each kind keeps
// its coordinates somewhere different -- `place(x, y, ..)` in its arguments, a
// line in an `(x, y)` array per vertex -- and lilook rewrites whichever applies.
#import "@preview/lilaq:0.6.0" as lq
#set page(width: auto, height: auto, margin: 6pt)

#let t = lq.linspace(0, 10, num: 40)

#lq.diagram(
  width: 8cm,
  height: 5cm,
  xlim: (0, 10),
  ylim: (-1.5, 1.5),
  xlabel: [phase],
  lq.plot(t, t.map(x => calc.sin(x)), mark: none),
  lq.rect(1.2, 1.1, width: 2.4, height: 0.7, fill: yellow.lighten(60%), stroke: none),
  lq.place(1.4, 0.95, [first peak]),
  lq.line((1.6, 0.85), (1.57, 1)),
  lq.ellipse(7.85, -1, width: 1.2, height: 0.5, stroke: red),
)
