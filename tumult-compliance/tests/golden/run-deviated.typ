#set page(
  paper: "a4",
  margin: (top: 20mm, bottom: 20mm, left: 18mm, right: 18mm),
  header: context [
    #text(size: 7.5pt, fill: luma(110))[#smallcaps[Game\-day report — Payments tolerate a slow ledger] #h(1fr) Internal]
    #v(2pt)
    #line(length: 100%, stroke: 0.5pt + luma(200))
  ],
  footer: context [
    #text(size: 7.5pt, fill: luma(110))[KRK\-R3\-20261120\-6cf89e #h(1fr) Page #counter(page).display("1 of 1", both: true)]
  ],
)
#set text(font: "Inter", size: 10.5pt, lang: "en")
#set par(justify: true, leading: 0.6em)
#show heading: set text(font: "Source Serif 4", weight: "regular")
#show heading.where(level: 1): set text(size: 16pt)
#show heading.where(level: 2): set text(size: 12.5pt)
#show heading.where(level: 3): set text(size: 11pt)
#text(size: 9pt, weight: "bold", tracking: 0.3em)[KRÖNIKA]
#v(6pt)
#line(length: 100%, stroke: 1.2pt + rgb("#0072B2"))
#v(96pt)
#box(stroke: 0.6pt + luma(110), radius: 9pt, inset: (x: 9pt, y: 3.5pt))[#text(size: 7.5pt, tracking: 0.18em, fill: luma(70))[#smallcaps[Internal]]]
#v(16pt)
#text(font: "Source Serif 4", size: 27pt)[Game\-day report — Payments tolerate a slow ledger]
#v(12pt)
#text(size: 12pt, fill: luma(80))[R3 \u{2014} Game-day report]
#v(8pt)
#text(size: 11.5pt, weight: "semibold")[Data as of 2026\-11\-19]
#v(1fr)
#table(
  columns: (30%, 70%),
  stroke: none,
  inset: (x: 0pt, y: 5.5pt),
  table.hline(stroke: 0.8pt + luma(140)),
  [#text(size: 8.5pt, fill: luma(100))[#smallcaps[Document]]], [#text(size: 10pt)[KRK\-R3\-20261120\-6cf89e]],
  table.hline(stroke: 0.5pt + luma(215)),
  [#text(size: 8.5pt, fill: luma(100))[#smallcaps[Version]]], [#text(size: 10pt)[\{version\}]],
  table.hline(stroke: 0.5pt + luma(215)),
  [#text(size: 8.5pt, fill: luma(100))[#smallcaps[Generated]]], [#text(size: 10pt)[2026\-11\-20 00:00 UTC]],
  table.hline(stroke: 0.5pt + luma(215)),
  [#text(size: 8.5pt, fill: luma(100))[#smallcaps[Data as of]]], [#text(size: 10pt)[2026\-11\-19]],
  table.hline(stroke: 0.5pt + luma(215)),
  [#text(size: 8.5pt, fill: luma(100))[#smallcaps[Run id]]], [#text(size: 10pt)[cad5df03\-83b7\-473b\-b5bc\-fec2cefd49e2]],
  table.hline(stroke: 0.5pt + luma(215)),
)
#pagebreak()

#v(6pt)
= Run summary


#table(
  columns: (auto, 1fr),
  stroke: none,
  inset: (x: 0pt, y: 3.5pt),
  column-gutter: 16pt,
  [#text(size: 9pt, fill: luma(100))[#smallcaps[Experiment]]], [Payments tolerate a slow ledger],
  [#text(size: 9pt, fill: luma(100))[#smallcaps[Run id]]], [cad5df03\-83b7\-473b\-b5bc\-fec2cefd49e2],
  [#text(size: 9pt, fill: luma(100))[#smallcaps[Date]]], [2026\-11\-19 11:13 UTC],
  [#text(size: 9pt, fill: luma(100))[#smallcaps[Target]]], [payments],
  [#text(size: 9pt, fill: luma(100))[#smallcaps[Environment]]], [prod],
  [#text(size: 9pt, fill: luma(100))[#smallcaps[Scenario]]], [slow\-ledger],
  [#text(size: 9pt, fill: luma(100))[#smallcaps[Severity]]], [not recorded],
  [#text(size: 9pt, fill: luma(100))[#smallcaps[Hypothesis]]], [#text(fill: rgb("#D55E00"))[×] #text(number-width: "tabular")[Not met]],
  [#text(size: 9pt, fill: luma(100))[#smallcaps[Outcome]]], [#text(fill: rgb("#D55E00"))[×] #text(number-width: "tabular")[deviated]],
)

== Blast radius and safety


#table(
  columns: (auto, 1fr),
  stroke: none,
  inset: (x: 0pt, y: 3.5pt),
  column-gutter: 16pt,
  [#text(size: 9pt, fill: luma(100))[#smallcaps[Blast radius]]], [the ledger connection of one instance],
  [#text(size: 9pt, fill: luma(100))[#smallcaps[Recovery time]]], [not recorded],
  [#text(size: 9pt, fill: luma(100))[#smallcaps[Rollback exercised]]], [yes],
)

== Timeline


#[
#set par(justify: false)
#set text(hyphenate: false)
#table(
  columns: (1fr, 2fr, 1.7fr, 1.1fr, 0.8fr),
  stroke: none,
  inset: (x: 6pt, y: 4.5pt),
  align: (right, left, left, right, left),
  table.hline(stroke: 0.8pt + luma(140)),
  table.header(
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[t\+ (s)]]],
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[Span]]],
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[Step]]],
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[Duration (ms)]]],
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[Status]]],
  ),
  table.hline(stroke: 0.5pt + luma(200)),
  [#text(number-width: "tabular")[0.0]],
  [resilience.experiment],
  [—],
  [#text(number-width: "tabular")[29066.0]],
  [#text(fill: rgb("#D55E00"))[×] #text(number-width: "tabular")[error]],
  [#text(number-width: "tabular")[0.0]],
  [resilience.hypothesis.before],
  [—],
  [#text(number-width: "tabular")[1.8]],
  [—],
  [#text(number-width: "tabular")[0.0]],
  [resilience.probe],
  [payments\-answer],
  [#text(number-width: "tabular")[1.8]],
  [—],
  [#text(number-width: "tabular")[0.0]],
  [resilience.action],
  [slow\-ledger],
  [#text(number-width: "tabular")[1.4]],
  [—],
  [#text(number-width: "tabular")[0.0 – 29.1]],
  [resilience.probe],
  [payments\-answer × 31],
  [#text(number-width: "tabular")[1.6 – 2.2]],
  [#text(fill: rgb("#D55E00"))[×] #text(number-width: "tabular")[error]],
  [#text(number-width: "tabular")[0.0]],
  [resilience.probe],
  [payments\-during\-fault],
  [#text(number-width: "tabular")[1.7]],
  [—],
  [#text(number-width: "tabular")[29.1]],
  [resilience.hypothesis.after],
  [—],
  [#text(number-width: "tabular")[1.7]],
  [—],
  [#text(number-width: "tabular")[29.1]],
  [resilience.probe],
  [payments\-answer],
  [#text(number-width: "tabular")[1.7]],
  [#text(fill: rgb("#D55E00"))[×] #text(number-width: "tabular")[error]],
  [#text(number-width: "tabular")[29.1]],
  [resilience.rollback],
  [—],
  [#text(number-width: "tabular")[1.2]],
  [—],
  [#text(number-width: "tabular")[29.1]],
  [resilience.action],
  [restore\-ledger],
  [#text(number-width: "tabular")[1.2]],
  [—],
  table.hline(stroke: 0.8pt + luma(140)),
)
]

== Verdict


Outcome: deviated. Steady\-state hypothesis: not met.


== Findings


#list(
  [\[ERROR\] payments\-answer (resilience.probe): the output is outside the tolerance (× 32)],
)

== Rollback


The rollback ran 1 step (restore\-ledger); status: all clean.


#pagebreak()

== Configuration appendix


#table(
  columns: (auto, 1fr),
  stroke: none,
  inset: (x: 0pt, y: 3.5pt),
  column-gutter: 16pt,
  [#text(size: 9pt, fill: luma(100))[#smallcaps[resilience.target.criticality]]], [high],
  [#text(size: 9pt, fill: luma(100))[#smallcaps[resilience.target.version]]], [5.1.3],
  [#text(size: 9pt, fill: luma(100))[#smallcaps[resilience.target.version\_source]]], [declared],
  [#text(size: 9pt, fill: luma(100))[#smallcaps[resilience.test.kind]]], [chaos],
  [#text(size: 9pt, fill: luma(100))[#smallcaps[start.experiment\_id]]], [cad5df03\-83b7\-473b\-b5bc\-fec2cefd49e2],
  [#text(size: 9pt, fill: luma(100))[#smallcaps[start.experiment\_title]]], [Payments tolerate a slow ledger],
  [#text(size: 9pt, fill: luma(100))[#smallcaps[start.started\_at\_ns]]], [2026\-11\-19 11:13 UTC],
  [#text(size: 9pt, fill: luma(100))[#smallcaps[start.user]]], [operator],
)

#v(6pt)
#text(size: 8pt, fill: luma(110))[Generated by Tumult v\{version\} from the run's trace and logs.]


#v(12pt)
#grid(
  columns: (1fr, 1fr),
  gutter: 28pt,
  [
    #v(20pt)
    #line(length: 100%, stroke: 0.6pt + luma(110))
    #v(2pt)
    #text(size: 8pt, fill: luma(100))[#smallcaps[Run operator]]
  ],
  [
    #v(20pt)
    #line(length: 100%, stroke: 0.6pt + luma(110))
    #v(2pt)
    #text(size: 8pt, fill: luma(100))[#smallcaps[Reviewed by]]
  ],
)
