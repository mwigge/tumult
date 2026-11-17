#set page(
  paper: "a4",
  margin: (top: 20mm, bottom: 20mm, left: 18mm, right: 18mm),
  header: context [
    #text(size: 7.5pt, fill: luma(110))[#smallcaps[Game\-day report — Payments tolerate a slow ledger] #h(1fr) Internal]
    #v(2pt)
    #line(length: 100%, stroke: 0.5pt + luma(200))
  ],
  footer: context [
    #text(size: 7.5pt, fill: luma(110))[KRK\-R3\-20261118\-db8405 #h(1fr) Page #counter(page).display("1 of 1", both: true)]
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
#text(size: 11.5pt, weight: "semibold")[Data as of 2026\-11\-17]
#v(1fr)
#table(
  columns: (30%, 70%),
  stroke: none,
  inset: (x: 0pt, y: 5.5pt),
  table.hline(stroke: 0.8pt + luma(140)),
  [#text(size: 8.5pt, fill: luma(100))[#smallcaps[Document]]], [#text(size: 10pt)[KRK\-R3\-20261118\-db8405]],
  table.hline(stroke: 0.5pt + luma(215)),
  [#text(size: 8.5pt, fill: luma(100))[#smallcaps[Version]]], [#text(size: 10pt)[\{version\}]],
  table.hline(stroke: 0.5pt + luma(215)),
  [#text(size: 8.5pt, fill: luma(100))[#smallcaps[Generated]]], [#text(size: 10pt)[2026\-11\-18 00:00 UTC]],
  table.hline(stroke: 0.5pt + luma(215)),
  [#text(size: 8.5pt, fill: luma(100))[#smallcaps[Data as of]]], [#text(size: 10pt)[2026\-11\-17]],
  table.hline(stroke: 0.5pt + luma(215)),
  [#text(size: 8.5pt, fill: luma(100))[#smallcaps[Experiment]]], [#text(size: 10pt)[181ded0c\-116a\-4b6e\-8240\-fb28f2142a69]],
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
  [#text(size: 9pt, fill: luma(100))[#smallcaps[Run id]]], [181ded0c\-116a\-4b6e\-8240\-fb28f2142a69],
  [#text(size: 9pt, fill: luma(100))[#smallcaps[Date]]], [2026\-11\-17 04:11 UTC],
  [#text(size: 9pt, fill: luma(100))[#smallcaps[Target]]], [payments],
  [#text(size: 9pt, fill: luma(100))[#smallcaps[Environment]]], [prod],
  [#text(size: 9pt, fill: luma(100))[#smallcaps[Scenario]]], [—],
  [#text(size: 9pt, fill: luma(100))[#smallcaps[Severity]]], [not recorded],
  [#text(size: 9pt, fill: luma(100))[#smallcaps[Hypothesis]]], [#text(fill: rgb("#D55E00"))[×] #text(number-width: "tabular")[Not met]],
  [#text(size: 9pt, fill: luma(100))[#smallcaps[Outcome]]], [#text(fill: rgb("#E69F00"))[▲] #text(number-width: "tabular")[deviated]],
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
  columns: (0.7fr, 2.6fr, 0.9fr, 1fr, 0.9fr),
  stroke: none,
  inset: (x: 6pt, y: 4.5pt),
  align: (right, left, left, right, left),
  table.hline(stroke: 0.8pt + luma(140)),
  table.header(
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[t\+ (s)]]],
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[Span]]],
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[Kind]]],
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[Duration (ms)]]],
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[Status]]],
  ),
  table.hline(stroke: 0.5pt + luma(200)),
  [#text(number-width: "tabular")[0.0]],
  [resilience.experiment],
  [Internal],
  [#text(number-width: "tabular")[29185.8]],
  [#text(fill: rgb("#D55E00"))[×] #text(number-width: "tabular")[error]],
  [#text(number-width: "tabular")[0.0]],
  [resilience.hypothesis.before],
  [Internal],
  [#text(number-width: "tabular")[2.5]],
  [—],
  [#text(number-width: "tabular")[0.0]],
  [resilience.probe],
  [Internal],
  [#text(number-width: "tabular")[2.4]],
  [—],
  [#text(number-width: "tabular")[0.0]],
  [resilience.action],
  [Internal],
  [#text(number-width: "tabular")[2.1]],
  [—],
  [#text(number-width: "tabular")[0.0]],
  [resilience.probe],
  [Internal],
  [#text(number-width: "tabular")[2.2]],
  [—],
  [#text(number-width: "tabular")[0.0]],
  [resilience.probe],
  [Internal],
  [#text(number-width: "tabular")[2.6]],
  [—],
  [#text(number-width: "tabular")[0.0]],
  [resilience.probe],
  [Internal],
  [#text(number-width: "tabular")[2.4]],
  [—],
  [#text(number-width: "tabular")[1.0]],
  [resilience.probe],
  [Internal],
  [#text(number-width: "tabular")[3.1]],
  [—],
  [#text(number-width: "tabular")[2.0]],
  [resilience.probe],
  [Internal],
  [#text(number-width: "tabular")[3.3]],
  [—],
  [#text(number-width: "tabular")[3.0]],
  [resilience.probe],
  [Internal],
  [#text(number-width: "tabular")[4.3]],
  [—],
  [#text(number-width: "tabular")[4.0]],
  [resilience.probe],
  [Internal],
  [#text(number-width: "tabular")[4.3]],
  [—],
  [#text(number-width: "tabular")[5.0]],
  [resilience.probe],
  [Internal],
  [#text(number-width: "tabular")[3.0]],
  [—],
  [#text(number-width: "tabular")[6.0]],
  [resilience.probe],
  [Internal],
  [#text(number-width: "tabular")[4.8]],
  [—],
  [#text(number-width: "tabular")[7.0]],
  [resilience.probe],
  [Internal],
  [#text(number-width: "tabular")[3.3]],
  [—],
  [#text(number-width: "tabular")[8.0]],
  [resilience.probe],
  [Internal],
  [#text(number-width: "tabular")[5.2]],
  [—],
  [#text(number-width: "tabular")[9.0]],
  [resilience.probe],
  [Internal],
  [#text(number-width: "tabular")[7.5]],
  [—],
  [#text(number-width: "tabular")[10.1]],
  [resilience.probe],
  [Internal],
  [#text(number-width: "tabular")[6.0]],
  [—],
  [#text(number-width: "tabular")[11.1]],
  [resilience.probe],
  [Internal],
  [#text(number-width: "tabular")[7.8]],
  [—],
  [#text(number-width: "tabular")[12.1]],
  [resilience.probe],
  [Internal],
  [#text(number-width: "tabular")[3.2]],
  [—],
  [#text(number-width: "tabular")[13.1]],
  [resilience.probe],
  [Internal],
  [#text(number-width: "tabular")[3.5]],
  [—],
  [#text(number-width: "tabular")[14.1]],
  [resilience.probe],
  [Internal],
  [#text(number-width: "tabular")[3.5]],
  [—],
  [#text(number-width: "tabular")[15.1]],
  [resilience.probe],
  [Internal],
  [#text(number-width: "tabular")[3.7]],
  [—],
  [#text(number-width: "tabular")[16.1]],
  [resilience.probe],
  [Internal],
  [#text(number-width: "tabular")[3.2]],
  [—],
  [#text(number-width: "tabular")[17.1]],
  [resilience.probe],
  [Internal],
  [#text(number-width: "tabular")[3.1]],
  [—],
  [#text(number-width: "tabular")[18.1]],
  [resilience.probe],
  [Internal],
  [#text(number-width: "tabular")[9.1]],
  [—],
  [#text(number-width: "tabular")[19.1]],
  [resilience.probe],
  [Internal],
  [#text(number-width: "tabular")[7.3]],
  [—],
  [#text(number-width: "tabular")[20.1]],
  [resilience.probe],
  [Internal],
  [#text(number-width: "tabular")[3.3]],
  [—],
  [#text(number-width: "tabular")[21.1]],
  [resilience.probe],
  [Internal],
  [#text(number-width: "tabular")[4.6]],
  [—],
  [#text(number-width: "tabular")[22.1]],
  [resilience.probe],
  [Internal],
  [#text(number-width: "tabular")[9.4]],
  [—],
  [#text(number-width: "tabular")[23.1]],
  [resilience.probe],
  [Internal],
  [#text(number-width: "tabular")[7.4]],
  [—],
  [#text(number-width: "tabular")[24.1]],
  [resilience.probe],
  [Internal],
  [#text(number-width: "tabular")[3.7]],
  [—],
  [#text(number-width: "tabular")[25.1]],
  [resilience.probe],
  [Internal],
  [#text(number-width: "tabular")[4.7]],
  [—],
  [#text(number-width: "tabular")[26.1]],
  [resilience.probe],
  [Internal],
  [#text(number-width: "tabular")[3.2]],
  [—],
  [#text(number-width: "tabular")[27.1]],
  [resilience.probe],
  [Internal],
  [#text(number-width: "tabular")[4.1]],
  [—],
  [#text(number-width: "tabular")[28.1]],
  [resilience.probe],
  [Internal],
  [#text(number-width: "tabular")[5.2]],
  [—],
  [#text(number-width: "tabular")[29.2]],
  [resilience.probe],
  [Internal],
  [#text(number-width: "tabular")[4.4]],
  [—],
  [#text(number-width: "tabular")[29.2]],
  [resilience.hypothesis.after],
  [Internal],
  [#text(number-width: "tabular")[6.0]],
  [—],
  [#text(number-width: "tabular")[29.2]],
  [resilience.probe],
  [Internal],
  [#text(number-width: "tabular")[5.9]],
  [#text(fill: rgb("#D55E00"))[×] #text(number-width: "tabular")[error]],
  [#text(number-width: "tabular")[29.2]],
  [resilience.rollback],
  [Internal],
  [#text(number-width: "tabular")[3.7]],
  [—],
  [#text(number-width: "tabular")[29.2]],
  [resilience.action],
  [Internal],
  [#text(number-width: "tabular")[3.3]],
  [—],
  table.hline(stroke: 0.8pt + luma(140)),
)
]

== Verdict


Outcome: deviated. Steady\-state hypothesis: not met.


== Findings


No warnings or errors were logged during this run.


== Rollback


1 rollback span(s) executed; status: all clean.


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
  [#text(size: 9pt, fill: luma(100))[#smallcaps[start.experiment\_id]]], [181ded0c\-116a\-4b6e\-8240\-fb28f2142a69],
  [#text(size: 9pt, fill: luma(100))[#smallcaps[start.experiment\_title]]], [Payments tolerate a slow ledger],
  [#text(size: 9pt, fill: luma(100))[#smallcaps[start.started\_at\_ns]]], [1794888680300994831],
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
