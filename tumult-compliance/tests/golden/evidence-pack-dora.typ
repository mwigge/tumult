#set page(
  paper: "a4",
  margin: (top: 20mm, bottom: 20mm, left: 18mm, right: 18mm),
  header: context [
    #text(size: 7.5pt, fill: luma(110))[#smallcaps[Evidence pack — DORA] #h(1fr) Internal]
    #v(2pt)
    #line(length: 100%, stroke: 0.5pt + luma(200))
  ],
  footer: context [
    #text(size: 7.5pt, fill: luma(110))[KRK\-R2\-20261120\-dfe44f #h(1fr) Page #counter(page).display("1 of 1", both: true)]
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
#text(font: "Source Serif 4", size: 27pt)[Evidence pack — DORA]
#v(12pt)
#text(size: 12pt, fill: luma(80))[R2 \u{2014} Compliance evidence pack]
#v(8pt)
#text(size: 11.5pt, weight: "semibold")[Reporting period: 2026\-10\-21 – 2026\-11\-20]
#v(1fr)
#table(
  columns: (30%, 70%),
  stroke: none,
  inset: (x: 0pt, y: 5.5pt),
  table.hline(stroke: 0.8pt + luma(140)),
  [#text(size: 8.5pt, fill: luma(100))[#smallcaps[Document]]], [#text(size: 10pt)[KRK\-R2\-20261120\-dfe44f]],
  table.hline(stroke: 0.5pt + luma(215)),
  [#text(size: 8.5pt, fill: luma(100))[#smallcaps[Version]]], [#text(size: 10pt)[\{version\}]],
  table.hline(stroke: 0.5pt + luma(215)),
  [#text(size: 8.5pt, fill: luma(100))[#smallcaps[Generated]]], [#text(size: 10pt)[2026\-11\-20 00:00 UTC]],
  table.hline(stroke: 0.5pt + luma(215)),
  [#text(size: 8.5pt, fill: luma(100))[#smallcaps[Data as of]]], [#text(size: 10pt)[2026\-11\-20]],
  table.hline(stroke: 0.5pt + luma(215)),
  [#text(size: 8.5pt, fill: luma(100))[#smallcaps[Period]]], [#text(size: 10pt)[2026\-10\-21 – 2026\-11\-20]],
  table.hline(stroke: 0.5pt + luma(215)),
  [#text(size: 8.5pt, fill: luma(100))[#smallcaps[Framework]]], [#text(size: 10pt)[DORA]],
  table.hline(stroke: 0.5pt + luma(215)),
)
#pagebreak()

#v(6pt)
= Scope and methodology


This pack summarises the resilience testing evidence Tumult recorded for the DORA framework over the period 2026\-10\-21 – 2026\-11\-20; 5 experiments are on record. An automated experiment is a chaos or continuity test that Tumult ran. The outcome of a run is the status that run recorded; a manual exercise has the outcome of its verified record.


== Independence


Independence has not been verified by this report. Organizational separation of test design\/execution from service operation requires a reviewed attestation. Manual evidence review records identify the enterer and verifier; those identities alone do not prove organizational independence.


== Traceability matrix


Each row is a row of the DORA requirement registry, version 2026.11.1, scored for no registered legal entity under the full profile as of 2026\-11\-20, as GET \/api\/dora scores it. The status is the evidence status of the row: none, designed, existing or operating, or unknown, no events or not applicable when it cannot be scored. The evidence is the best run or manual record behind each function the row binds. A status says what evidence is held. It is not a compliance verdict.


#[
#set par(justify: false)
#set text(hyphenate: false)
#table(
  columns: (1.1fr, 2.4fr, 0.9fr, 2.8fr),
  stroke: none,
  inset: (x: 6pt, y: 4.5pt),
  align: (left, left, left, left),
  table.hline(stroke: 0.8pt + luma(140)),
  table.header(
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[Article and paragraph]]],
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[Requirement]]],
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[Status]]],
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[Evidence]]],
  ),
  table.hline(stroke: 0.5pt + luma(200)),
  [Art. 5(2)],
  [Management body approves and oversees the ICT risk arrangements],
  [not applicable],
  [binds no function in scope],
  [Art. 6(5)],
  [ICT risk management framework reviewed and the review recorded],
  [not applicable],
  [binds no function in scope],
  [Art. 9(1)],
  [ICT systems supporting the function are continuously monitored],
  [not applicable],
  [binds no function in scope],
  [Art. 10(1)],
  [Single points of failure identified from the observed topology],
  [not applicable],
  [binds no function in scope],
  [Art. 10(2)],
  [Alert thresholds and criteria defined for the function],
  [not applicable],
  [binds no function in scope],
  [Art. 11(4)],
  [Business continuity and response plans tested periodically],
  [not applicable],
  [binds no function in scope],
  [Art. 11(6)(a)],
  [Continuity and recovery plans tested at least yearly],
  [not applicable],
  [binds no function in scope],
  [Art. 11(6)],
  [Switchover between primary and redundant capacity tested yearly],
  [not applicable],
  [binds no function in scope],
  [Art. 11(6)(b)],
  [Crisis communication plans tested],
  [not applicable],
  [binds no function in scope],
  [Art. 11(7)],
  [Crisis management function established],
  [not applicable],
  [binds no function in scope],
  [Art. 11(8)],
  [Records of activities kept during disruption events],
  [not applicable],
  [binds no function in scope],
  [Art. 11(9)],
  [Central securities depository: continuity test records provided to the authority],
  [not applicable],
  [the profile of the legal entity excludes this row],
  [Art. 12(1)],
  [Backup policies and restoration procedures documented],
  [not applicable],
  [binds no function in scope],
  [Art. 12(2)],
  [Backup and restoration procedures tested periodically],
  [not applicable],
  [binds no function in scope],
  [Art. 12(3)],
  [Restoration uses segregated systems],
  [not applicable],
  [binds no function in scope],
  [Art. 12(3)],
  [Central counterparty: recovery plans enable recovery of all transactions],
  [not applicable],
  [the profile of the legal entity excludes this row],
  [Art. 12(4)],
  [Redundant ICT capacity exercised],
  [not applicable],
  [binds no function in scope],
  [Art. 12(5)],
  [Central securities depository: secondary processing site maintained],
  [not applicable],
  [the profile of the legal entity excludes this row],
  [Art. 12(6)],
  [Recovery time and recovery point objectives declared and met],
  [not applicable],
  [binds no function in scope],
  [Art. 12(7)],
  [Recovered data checked against its recovery point],
  [not applicable],
  [binds no function in scope],
  [Art. 13(3)],
  [Lessons from incidents and threat\-led tests fed into the risk assessment],
  [not applicable],
  [binds no function in scope],
  [Art. 13(5)],
  [Yearly report to the management body],
  [not applicable],
  [binds no function in scope],
  [RTS 2024\/1774 Art. 25],
  [Continuity plan tests cover the required scenarios and content],
  [not applicable],
  [binds no function in scope],
  [Art. 16(1)(a)],
  [Sound and documented ICT risk management framework in place],
  [not applicable],
  [the profile of the legal entity excludes this row],
  [Art. 16(1)(f)],
  [Continuity ensured through backup and restoration measures],
  [not applicable],
  [the profile of the legal entity excludes this row],
  [Art. 16(1)(g)],
  [Plans and controls tested regularly],
  [not applicable],
  [the profile of the legal entity excludes this row],
  [Art. 16(1)(h)],
  [Lessons from tests and incidents fed into the ICT risk assessment and into training],
  [not applicable],
  [the profile of the legal entity excludes this row],
  [Art. 16(2)],
  [Simplified framework reviewed and the review recorded],
  [not applicable],
  [the profile of the legal entity excludes this row],
  [RTS 2024\/1774 Art. 40],
  [Backup and restore procedures tested at least once every year],
  [not applicable],
  [the profile of the legal entity excludes this row],
  [Art. 17(3)(a)],
  [Early warning indicators in place],
  [not applicable],
  [binds no function in scope],
  [Art. 18(1)],
  [ICT\-related incidents classified],
  [not applicable],
  [binds no function in scope],
  [Art. 19(4)(a)],
  [Initial notification of a major incident submitted],
  [not applicable],
  [binds no function in scope],
  [Art. 19(4)(b)],
  [Intermediate report of a major incident submitted],
  [not applicable],
  [binds no function in scope],
  [Art. 19(4)(c)],
  [Final report of a major incident submitted],
  [not applicable],
  [binds no function in scope],
  [Art. 24(1)],
  [Testing programme declared for the function],
  [not applicable],
  [binds no function in scope],
  [Art. 24(3)],
  [Criticality assessed, so testing follows a risk\-based approach],
  [not applicable],
  [binds no function in scope],
  [Art. 24(4)],
  [Tests undertaken by independent parties],
  [not applicable],
  [binds no function in scope],
  [Art. 24(5)],
  [Issues revealed by tests remediated and re\-tested],
  [not applicable],
  [binds no function in scope],
  [Art. 24(6)],
  [Systems supporting critical or important functions tested at least yearly],
  [not applicable],
  [binds no function in scope],
  [Art. 25(1)],
  [Testing programme spans more than one kind of test],
  [not applicable],
  [binds no function in scope],
  [Art. 25(2)],
  [Central securities depository or counterparty: vulnerability assessment before deployment],
  [not applicable],
  [the profile of the legal entity excludes this row],
  [Art. 25(3)],
  [Microenterprise: planned tests executed within their planned interval],
  [not applicable],
  [the profile of the legal entity excludes this row],
  [Art. 26(1)],
  [Threat\-led penetration test carried out at least every three years],
  [not applicable],
  [the profile of the legal entity excludes this row],
  [Art. 28(2)],
  [Strategy on ICT third\-party risk adopted and reviewed],
  [not applicable],
  [binds no function in scope],
  [Art. 28(3)],
  [Register of information exported and reconciled],
  [not applicable],
  [binds no function in scope],
  [Art. 28(8)],
  [Exit plan tested],
  [not applicable],
  [binds no function in scope],
  table.hline(stroke: 0.8pt + luma(140)),
)
]

#v(6pt)
#text(size: 8pt, fill: luma(110))[Tumult experiments produce technical EVIDENCE toward the controls below. Passing experiments do not by themselves establish regulatory compliance or constitute a legal attestation; a compliance determination requires assessment by a qualified auditor against the official source text. Citations are dated (last\_verified) and must be re\-checked against the official source before relying on them.]


#v(6pt)
#text(size: 8pt, fill: luma(110))[Clause references should be verified against the licensed framework text before submission.]


== Test register


#[
#set par(justify: false)
#set text(hyphenate: false)
#table(
  columns: (1.8fr, 0.8fr, 0.7fr, 1.2fr, 1.2fr, 0.9fr, 1fr, 0.6fr),
  stroke: none,
  inset: (x: 6pt, y: 4.5pt),
  align: (left, left, left, left, left, left, left, right),
  table.hline(stroke: 0.8pt + luma(140)),
  table.header(
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[Experiment]]],
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[Origin]]],
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[Target \/ environment]]],
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[Executed]]],
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[Entered]]],
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[Verifier]]],
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[Outcome]]],
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[Score]]],
  ),
  table.hline(stroke: 0.5pt + luma(200)),
  [Payments tolerate a slow ledger],
  [automated],
  [payments \/ prod],
  [2026\-11\-19],
  [—],
  [—],
  [#text(fill: rgb("#D55E00"))[×] #text(number-width: "tabular")[deviated]],
  [#text(number-width: "tabular")[50]],
  [Regional failover drill],
  [manual],
  [checkout \/ prod],
  [2026\-11\-19],
  [\{entered\}],
  [bob],
  [#text(fill: rgb("#E69F00"))[▲] #text(number-width: "tabular")[partial]],
  [#text(number-width: "tabular")[75]],
  [Billing export resumes after a restart],
  [automated],
  [billing \/ staging],
  [2026\-11\-19],
  [—],
  [—],
  [#text(fill: rgb("#009E73"))[●] #text(number-width: "tabular")[completed]],
  [#text(number-width: "tabular")[100]],
  [Checkout survives a cache restart],
  [automated],
  [checkout \/ prod],
  [2026\-11\-19],
  [—],
  [—],
  [#text(fill: rgb("#009E73"))[●] #text(number-width: "tabular")[completed]],
  [#text(number-width: "tabular")[100]],
  [Search answers after the loss of a node],
  [automated],
  [search \/ staging],
  [2026\-11\-19],
  [—],
  [—],
  [#text(fill: rgb("#009E73"))[●] #text(number-width: "tabular")[completed]],
  [#text(number-width: "tabular")[100]],
  table.hline(stroke: 0.8pt + luma(140)),
)
]

The kind of each test, and the RTO and the RPO it planned and measured, in the order of the register:


#[
#set par(justify: false)
#set text(hyphenate: false)
#table(
  columns: (2.4fr, 1fr, 1fr, 1.2fr, 1fr, 1.1fr),
  stroke: none,
  inset: (x: 6pt, y: 4.5pt),
  align: (left, left, left, left, left, left),
  table.hline(stroke: 0.8pt + luma(140)),
  table.header(
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[Experiment]]],
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[Kind]]],
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[RTO planned]]],
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[RTO actual]]],
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[RPO planned]]],
    [#text(size: 8.5pt, fill: luma(90))[#smallcaps[RPO actual]]],
  ),
  table.hline(stroke: 0.5pt + luma(200)),
  [Payments tolerate a slow ledger],
  [chaos],
  [2.0 s],
  [not measured],
  [not set],
  [not measured],
  [Regional failover drill],
  [failover],
  [not set],
  [540.0 s],
  [not set],
  [not measured],
  [Billing export resumes after a restart],
  [chaos],
  [2.0 s],
  [0.0 s],
  [not set],
  [not measured],
  [Checkout survives a cache restart],
  [chaos],
  [2.0 s],
  [0.0 s],
  [not set],
  [not measured],
  [Search answers after the loss of a node],
  [chaos],
  [2.0 s],
  [0.0 s],
  [not set],
  [not measured],
  table.hline(stroke: 0.8pt + luma(140)),
)
]

== Approval chain (change management)


Approval\-gated runs (tiers T1–T3) execute only after explicit human approval — the change\-management evidence for SOC 2 CC8.1. Each request pins the exact change content by hash, enforces an approver quorum with segregation of duties, and lapses on a TTL; approvals are single\-use and break\-glass overrides carry a mandatory justification. Pin hashes are re\-verified at dispatch, and the full per\-run, hash\-chained \`run\_audit\` trail is available via the API. Runs are shown by their id prefix; the full id and pin hash are available via the run\-detail API.


Approval\-chain appendix: at most the 500 newest installation records are sampled, then filtered to this scope and period. This appendix is not a complete approval export; use the portable run\_audit and approval tables for complete evidence.


No approval\-gated runs in the selected sample for this scope and period.


== Manual attestation appendix


Each manually executed test below was entered under the quoted attestation and verified by a reviewer other than the person who entered it. Entries are immutable once verified; corrections require a new record.


=== Regional failover drill — 2026\-11\-19


#table(
  columns: (auto, 1fr),
  stroke: none,
  inset: (x: 0pt, y: 3.5pt),
  column-gutter: 16pt,
  [#text(size: 9pt, fill: luma(100))[#smallcaps[Exercise type]]], [failover],
  [#text(size: 9pt, fill: luma(100))[#smallcaps[Executed]]], [2026\-11\-19],
  [#text(size: 9pt, fill: luma(100))[#smallcaps[Entered by]]], [alice on \{entered\}],
  [#text(size: 9pt, fill: luma(100))[#smallcaps[Verified by]]], [bob on \{verified\}],
  [#text(size: 9pt, fill: luma(100))[#smallcaps[Frameworks]]], [DORA Art. 24(6), ISO 27001 A.5.29],
  [#text(size: 9pt, fill: luma(100))[#smallcaps[Attestation]]], [I attest that this record reflects the exercise.],
  [#text(size: 9pt, fill: luma(100))[#smallcaps[RTO planned \/ actual]]], [not set \/ 540.0 s],
  [#text(size: 9pt, fill: luma(100))[#smallcaps[RPO planned \/ actual]]], [not set \/ not measured],
  [#text(size: 9pt, fill: luma(100))[#smallcaps[Data verified]]], [not checked],
  [#text(size: 9pt, fill: luma(100))[#smallcaps[Plan]]], [none],
  [#text(size: 9pt, fill: luma(100))[#smallcaps[Sign\-off]]], [bob, reviewer, approved on \{signed\}],
)

== Findings log


#list(
  [Payments tolerate a slow ledger (payments): deviated — last run 2026\-11\-19],
  [Regional failover drill (checkout): partial — last run 2026\-11\-19],
)

#v(6pt)
#text(size: 8pt, fill: luma(110))[Clause references should be verified against the licensed framework text before submission.]


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
    #text(size: 8pt, fill: luma(100))[#smallcaps[Compliance officer]]
  ],
)
