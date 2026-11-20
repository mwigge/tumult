#set page(
  paper: "a4",
  margin: (top: 20mm, bottom: 20mm, left: 18mm, right: 18mm),
  header: context [
    #text(size: 7.5pt, fill: luma(110))[#smallcaps[Executive resilience digest] #h(1fr) Internal]
    #v(2pt)
    #line(length: 100%, stroke: 0.5pt + luma(200))
  ],
  footer: context [
    #text(size: 7.5pt, fill: luma(110))[KRK\-R1\-20270119\-129f91 #h(1fr) Page #counter(page).display("1 of 1", both: true)]
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
#text(font: "Source Serif 4", size: 27pt)[Executive resilience digest]
#v(12pt)
#text(size: 12pt, fill: luma(80))[R1 \u{2014} Executive resilience digest]
#v(8pt)
#text(size: 11.5pt, weight: "semibold")[Reporting period: 2026\-12\-20 – 2027\-01\-19]
#v(1fr)
#table(
  columns: (30%, 70%),
  stroke: none,
  inset: (x: 0pt, y: 5.5pt),
  table.hline(stroke: 0.8pt + luma(140)),
  [#text(size: 8.5pt, fill: luma(100))[#smallcaps[Document]]], [#text(size: 10pt)[KRK\-R1\-20270119\-129f91]],
  table.hline(stroke: 0.5pt + luma(215)),
  [#text(size: 8.5pt, fill: luma(100))[#smallcaps[Version]]], [#text(size: 10pt)[\{version\}]],
  table.hline(stroke: 0.5pt + luma(215)),
  [#text(size: 8.5pt, fill: luma(100))[#smallcaps[Generated]]], [#text(size: 10pt)[2027\-01\-19 00:00 UTC]],
  table.hline(stroke: 0.5pt + luma(215)),
  [#text(size: 8.5pt, fill: luma(100))[#smallcaps[Data as of]]], [#text(size: 10pt)[2027\-01\-19]],
  table.hline(stroke: 0.5pt + luma(215)),
  [#text(size: 8.5pt, fill: luma(100))[#smallcaps[Period]]], [#text(size: 10pt)[2026\-12\-20 – 2027\-01\-19]],
  table.hline(stroke: 0.5pt + luma(215)),
)
#pagebreak()

#v(6pt)
= Bottom line


Portfolio compliance score is fair (68.8\/100) and flat (\+0.0 vs the previous period). Weakest targets: payments (50.0), billing (75.0), checkout (75.0). 2 open weaknesses need a decision: re\-run, remediate, or accept.


#grid(
  columns: (1fr, 1fr, 1fr),
  gutter: 10pt,
  block(
    width: 100%,
    stroke: 0.5pt + luma(210),
    radius: 3pt,
    inset: (x: 10pt, y: 8pt),
  )[
    #text(size: 7.5pt, fill: luma(100), tracking: 0.08em)[#smallcaps[Portfolio score]]
    #v(3pt)
    #text(size: 16pt, weight: "bold")[68.8]
    #v(2pt)
    #text(size: 8pt, fill: luma(100))[fair]
  ],
  block(
    width: 100%,
    stroke: 0.5pt + luma(210),
    radius: 3pt,
    inset: (x: 10pt, y: 8pt),
  )[
    #text(size: 7.5pt, fill: luma(100), tracking: 0.08em)[#smallcaps[Δ vs previous period]]
    #v(3pt)
    #text(size: 16pt, weight: "bold")[\+0.0]
  ],
  block(
    width: 100%,
    stroke: 0.5pt + luma(210),
    radius: 3pt,
    inset: (x: 10pt, y: 8pt),
  )[
    #text(size: 7.5pt, fill: luma(100), tracking: 0.08em)[#smallcaps[Experiments tested]]
    #v(3pt)
    #text(size: 16pt, weight: "bold")[0 \/ 5]
    #v(2pt)
    #text(size: 8pt, fill: luma(100))[in the period]
  ],
  block(
    width: 100%,
    stroke: 0.5pt + luma(210),
    radius: 3pt,
    inset: (x: 10pt, y: 8pt),
  )[
    #text(size: 7.5pt, fill: luma(100), tracking: 0.08em)[#smallcaps[Open weaknesses]]
    #v(3pt)
    #text(size: 16pt, weight: "bold")[2]
  ],
  block(
    width: 100%,
    stroke: 0.5pt + luma(210),
    radius: 3pt,
    inset: (x: 10pt, y: 8pt),
  )[
    #text(size: 7.5pt, fill: luma(100), tracking: 0.08em)[#smallcaps[Issues fixed]]
    #v(3pt)
    #text(size: 16pt, weight: "bold")[0 \/ 0]
  ],
)

The portfolio score is the compliance score. Latest run of each experiment: pass 100, pass older than 30 days 75, failure 50, never run 0. Averaged per target.


== Score trend


#figure(
  image("/charts/c0.svg", width: 100%),
  caption: [Source: Tumult · data as of 2027\-01\-19],
)


== Experiment scores


#figure(
  image("/charts/c1.svg", width: 100%),
  caption: [Source: Tumult · data as of 2027\-01\-19],
)


== By domain


#[
#set par(justify: false)
#set text(hyphenate: false)
#table(
  columns: (2.6fr, 0.8fr, 1fr, 1fr, 2.2fr),
  stroke: none,
  inset: (x: 6pt, y: 4.5pt),
  align: (left, right, left, left, left),
  table.hline(stroke: 0.8pt + luma(140)),
  table.header(
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[Domain]]],
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[Score]]],
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[Band]]],
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[Coverage]]],
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[Weakest member]]],
  ),
  table.hline(stroke: 0.5pt + luma(200)),
  [commerce],
  [#text(number-width: "tabular")[62.5]],
  [#text(fill: rgb("#E69F00"))[▲] #text(number-width: "tabular")[fair]],
  [2\/2],
  [Payments tolerate a slow ledger],
  [(unassigned)],
  [#text(number-width: "tabular")[75.0]],
  [#text(fill: rgb("#009E73"))[●] #text(number-width: "tabular")[good]],
  [2\/2],
  [Billing export resumes after a restart],
  [platform],
  [#text(number-width: "tabular")[75.0]],
  [#text(fill: rgb("#009E73"))[●] #text(number-width: "tabular")[good]],
  [1\/1],
  [Search answers after the loss of a node],
  table.hline(stroke: 0.8pt + luma(140)),
)
]

== Coverage


#figure(
  image("/charts/c2.svg", width: 100%),
  caption: [Source: Tumult · data as of 2027\-01\-19],
)


== Target scores


#[
#set par(justify: false)
#set text(hyphenate: false)
#table(
  columns: (3fr, 1.1fr, 0.8fr, 0.8fr, 1fr),
  stroke: none,
  inset: (x: 6pt, y: 4.5pt),
  align: (left, right, right, right, left),
  table.hline(stroke: 0.8pt + luma(140)),
  table.header(
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[Target]]],
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[Experiments]]],
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[Runs]]],
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[Score]]],
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[Band]]],
  ),
  table.hline(stroke: 0.5pt + luma(200)),
  [payments],
  [#text(number-width: "tabular")[1]],
  [#text(number-width: "tabular")[1]],
  [#text(number-width: "tabular")[50.0]],
  [#text(fill: rgb("#E69F00"))[▲] #text(number-width: "tabular")[fair]],
  [billing],
  [#text(number-width: "tabular")[1]],
  [#text(number-width: "tabular")[1]],
  [#text(number-width: "tabular")[75.0]],
  [#text(fill: rgb("#009E73"))[●] #text(number-width: "tabular")[good]],
  [checkout],
  [#text(number-width: "tabular")[2]],
  [#text(number-width: "tabular")[2]],
  [#text(number-width: "tabular")[75.0]],
  [#text(fill: rgb("#009E73"))[●] #text(number-width: "tabular")[good]],
  [search],
  [#text(number-width: "tabular")[1]],
  [#text(number-width: "tabular")[2]],
  [#text(number-width: "tabular")[75.0]],
  [#text(fill: rgb("#009E73"))[●] #text(number-width: "tabular")[good]],
  table.hline(stroke: 0.8pt + luma(140)),
)
]

== Issues discovered and fixed


In the period 2026\-12\-20 – 2027\-01\-19, no experiment run deviated or failed.


== Open weaknesses and decisions needed


#[
#set par(justify: false)
#set text(hyphenate: false)
#table(
  columns: (1.2fr, 3fr, 1fr, 0.7fr, 2fr),
  stroke: none,
  inset: (x: 6pt, y: 4.5pt),
  align: (left, left, left, right, left),
  table.hline(stroke: 0.8pt + luma(140)),
  table.header(
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[Severity]]],
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[Experiment]]],
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[Last run]]],
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[Age (d)]]],
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[Decision needed]]],
  ),
  table.hline(stroke: 0.5pt + luma(200)),
  [not recorded],
  [Payments tolerate a slow ledger],
  [2026\-11\-19],
  [#text(number-width: "tabular")[60]],
  [Re\-run, remediate, or accept],
  [not recorded],
  [Regional failover drill],
  [2026\-11\-19],
  [#text(number-width: "tabular")[60]],
  [Re\-run to a full pass, or accept],
  table.hline(stroke: 0.8pt + luma(140)),
)
]

== Outlook


Next period priorities: re\-run 3 experiments whose last pass is over 30 days old; focus next game\-day on payments, the weakest target.


#v(6pt)
#text(size: 8pt, fill: luma(110))[Scores: passed 100, stale pass (\>30d) 75, failed 50, never run 0; bands \>70 good, 50–70 fair, \<50 poor. Evidence mix: 4 automated, 1 verified manual experiment (manual partials score 75; inconclusive manual outcomes are excluded). Generated by Tumult v\{version\}; data as of 2027\-01\-19.]


#v(12pt)
#grid(
  columns: (1fr, 1fr),
  gutter: 28pt,
  [
    #v(20pt)
    #line(length: 100%, stroke: 0.6pt + luma(110))
    #v(2pt)
    #text(size: 8pt, fill: luma(100))[#smallcaps[Prepared by]]
    #text(size: 9.5pt)[Tumult]
  ],
  [
    #v(20pt)
    #line(length: 100%, stroke: 0.6pt + luma(110))
    #v(2pt)
    #text(size: 8pt, fill: luma(100))[#smallcaps[Approved by]]
  ],
)
