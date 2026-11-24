#!/usr/bin/env python3
"""Unit tests for check-proof-journals.py (journals against expectations)."""

import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("check-proof-journals.py")


def load_checker():
    spec = importlib.util.spec_from_file_location("check_proof_journals", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


checker = load_checker()

JOURNAL = """experiment_title: Checkout sheds load
experiment_id: "76c6d757-524f-4441-97ad-d13740a70a43"
status: completed
started_at_ns: 1794831257110653046
duration_ms: 123499
steady_state_before:
  title: Checkout answers
  met: true
  probe_results[1]{name,activity_type,status,duration_ms,trace_id}:
    health,probe,succeeded,26,""
steady_state_after:
  title: Checkout answers
  met: true
  probe_results[1]{name,activity_type,status,duration_ms,trace_id}:
    health,probe,succeeded,10,""
method_results[2]:
  - name: "process-still-up"
    activity_type: probe
    status: succeeded
    output: "status: deviated"
  - name: "cpu-stress"
    activity_type: action
    status: succeeded
rollback_results[1]:
  - name: stop-stress
    activity_type: action
    status: succeeded
rollback_failures: 0
estimate: null
load_result:
  tool: oha
  duration_s: 30.2
  latency_p99_ms: 139.1
  error_rate: 0
  total_requests: 26283
kind: robustness
objectives:
  rto_measured_s: 0
  verdicts[2]{nfr_id,kind,metric,operator,threshold,observed,state,margin,criticality}:
    "health-survives",availability,during.availability,gte,0.95,1,met,0.05,blocking
    recovers,availability,post.availability,gte,0.999,0.5,breached,-0.499,major
probe_samples[2]{probe,phase,ts_ns,duration_ms,ok}:
  health,during,1,2,true
  health,post,3,4,false
"""

LOAD = {
    "tool": "oha",
    "duration_s": 30.2,
    "latency_p99_ms": 139.1,
    "error_rate": 0,
    "total_requests": 26283,
}

EXPERIMENT = """title: Checkout sheds load
load:
  tool: oha
  script: http://127.0.0.1:8080/checkout/1
  duration_s: 30
method[0]:
"""

EXPECT = {
    "status": "completed",
    "hypothesis_before": True,
    "hypothesis_after": True,
    "rollbacks": 1,
    "objectives": {"health-survives": "met", "recovers": "breached"},
}

GAMEDAY_JOURNAL = """gameday_id: "94f279c8"
title: Q2
experiment_journals[2]:
  - experiment_title: first
    status: completed
    steady_state_before:
      title: up
      met: true
    steady_state_after:
      title: up
      met: true
    method_results[1]:
      - name: kill
        status: succeeded
    rollback_results[0]:
    rollback_failures: 0
    load_result: null
  - experiment_title: second
    status: deviated
    steady_state_before:
      title: up
      met: true
    steady_state_after:
      title: up
      met: false
    method_results[0]:
    rollback_results[1]:
      - name: undo
        status: succeeded
    rollback_failures: 0
    load_result: null
load_result:
  tool: k6
  duration_s: 60.5
  latency_p99_ms: 71.8
  total_requests: 100
compliance_status: COMPLIANT
"""

GAMEDAY_LOAD = (
    "load_result:\n  tool: k6\n  duration_s: 60.5\n"
    "  latency_p99_ms: 71.8\n  total_requests: 100\n"
)

GAMEDAY = """title: Q2
load:
  tool: k6
  script: examples/k6/pg-load.js
  duration_s: 60.0
experiments[2]:
  - path: first.toon
    compliance_maps[1]: Art. 25
  - path: second.toon
    compliance_maps[2]: Art. 25, Art. 11
"""


def expectation(status, before, after, rollbacks, objectives=None):
    return {
        "status": status,
        "hypothesis_before": before,
        "hypothesis_after": after,
        "rollbacks": rollbacks,
        "objectives": objectives or {},
    }


class ReadToonTests(unittest.TestCase):
    def test_scalars_keep_their_type(self):
        read = checker.read_toon(
            'a: 1\nb: 1.5\nc: true\nd: null\ne: plain text\nf: "7"\ng: "q: \\"x\\""\n'
        )
        self.assertEqual(
            read,
            {
                "a": 1,
                "b": 1.5,
                "c": True,
                "d": None,
                "e": "plain text",
                "f": "7",
                "g": 'q: "x"',
            },
        )

    def test_nested_objects_and_empty_lists(self):
        read = checker.read_toon("a:\n  b:\n    c: 1\n  d: 2\nrows[0]:\ne: 3\n")
        self.assertEqual(read, {"a": {"b": {"c": 1}, "d": 2}, "rows": [], "e": 3})

    def test_a_list_of_objects_and_an_inline_list(self):
        read = checker.read_toon(
            'steps[2]:\n  - name: one\n    tags[2]: a,"b, c"\n  - name: two\n    inner:\n      x: 1\nafter: yes\n'
        )
        self.assertEqual(
            read,
            {
                "steps": [
                    {"name": "one", "tags": ["a", "b, c"]},
                    {"name": "two", "inner": {"x": 1}},
                ],
                "after": "yes",
            },
        )

    def test_a_table_is_a_list_of_rows_by_field_name(self):
        read = checker.read_toon(
            't[2]{id,state,n}:\n  "a,b",met,1\n  c,breached,-0.5\nnext: 1\n'
        )
        self.assertEqual(
            read["t"],
            [
                {"id": "a,b", "state": "met", "n": 1},
                {"id": "c", "state": "breached", "n": -0.5},
            ],
        )
        self.assertEqual(read["next"], 1)

    def test_a_list_of_plain_values(self):
        read = checker.read_toon('names[2]:\n  - one\n  - "two: 2"\n')
        self.assertEqual(read, {"names": ["one", "two: 2"]})

    def test_a_table_as_the_first_key_of_a_list_item(self):
        read = checker.read_toon(
            "items[1]:\n  - rows[1]{k,v}:\n      a,1\n    name: x\n"
        )
        self.assertEqual(read, {"items": [{"rows": [{"k": "a", "v": 1}], "name": "x"}]})

    def test_a_line_that_is_no_key_is_an_error(self):
        with self.assertRaises(ValueError) as raised:
            checker.read_toon("a: 1\nnot a key line\n")
        self.assertIn("line 2", str(raised.exception))

    def test_a_list_shorter_than_its_header_is_an_error(self):
        with self.assertRaises(ValueError) as raised:
            checker.read_toon("rows[2]{a}:\n  1\nb: 2\n")
        self.assertIn("rows", str(raised.exception))


class FactsTests(unittest.TestCase):
    def test_the_facts_of_a_journal(self):
        self.assertEqual(
            checker.facts(checker.read_toon(JOURNAL)),
            {
                **EXPECT,
                "rollback_failures": 0,
                "load_result": True,
                "rpo_measured_s": None,
            },
        )

    def test_the_rpo_a_run_measured_is_a_fact(self):
        journal = JOURNAL.replace(
            "  rto_measured_s: 0\n", "  rto_measured_s: 0\n  rpo_measured_s: 4.2\n"
        )
        seen = checker.facts(checker.read_toon(journal))
        self.assertEqual(seen["rpo_measured_s"], 4.2)

    def test_a_hypothesis_that_did_not_run_is_none(self):
        journal = checker.read_toon(
            "status: halted\nsteady_state_before: null\nsteady_state_after: null\nrollback_results[0]:\nrollback_failures: 0\n"
        )
        seen = checker.facts(journal)
        self.assertEqual(seen["status"], "halted")
        self.assertIsNone(seen["hypothesis_before"])
        self.assertIsNone(seen["hypothesis_after"])
        self.assertEqual(seen["objectives"], {})

    def test_a_status_inside_probe_output_is_not_the_status(self):
        self.assertEqual(
            checker.facts(checker.read_toon(JOURNAL))["status"], "completed"
        )


class DifferencesTests(unittest.TestCase):
    def seen(self):
        return checker.facts(checker.read_toon(JOURNAL))

    def test_a_journal_that_matches_has_no_difference(self):
        self.assertEqual(checker.differences(EXPECT, self.seen()), [])

    def test_each_wrong_expectation_is_named(self):
        wrong = {
            "status": "deviated",
            "hypothesis_before": False,
            "hypothesis_after": None,
            "rollbacks": 0,
            "objectives": {"health-survives": "breached", "recovers": "breached"},
        }
        found = checker.differences(wrong, self.seen())
        self.assertEqual(
            found,
            [
                "status: expected deviated, journal has completed",
                "hypothesis_before: expected false, journal has true",
                "hypothesis_after: expected null, journal has true",
                "rollbacks: expected 0, journal has 1",
                "objective health-survives: expected breached, journal has met",
            ],
        )

    def test_an_objective_the_journal_lacks_and_one_nobody_expected(self):
        expect = {**EXPECT, "objectives": {"health-survives": "met", "p95": "met"}}
        found = checker.differences(expect, self.seen())
        self.assertEqual(
            found,
            [
                "objective p95: expected met, journal has none",
                "objective recovers: expected none, journal has breached",
            ],
        )

    def test_a_failed_rollback_is_a_difference_unless_expected(self):
        seen = {**self.seen(), "rollback_failures": 1}
        self.assertEqual(
            checker.differences(EXPECT, seen),
            ["rollback_failures: expected 0, journal has 1"],
        )
        self.assertEqual(
            checker.differences({**EXPECT, "rollback_failures": 1}, seen), []
        )

    def test_a_load_result_is_compared_only_when_the_expectation_names_it(self):
        with_load, without = self.seen(), {**self.seen(), "load_result": False}
        self.assertEqual(checker.differences(EXPECT, without), [])
        self.assertEqual(
            checker.differences({**EXPECT, "load_result": True}, with_load), []
        )
        self.assertEqual(
            checker.differences({**EXPECT, "load_result": True}, without),
            ["load_result: expected one, journal has none"],
        )
        self.assertEqual(
            checker.differences({**EXPECT, "load_result": False}, with_load),
            ["load_result: expected none, journal has one"],
        )

    def test_an_rpo_inside_the_expected_range_has_no_difference(self):
        expect = {**EXPECT, "rpo_measured_s": {"from": 3, "to": 10}}
        for rpo in (3, 4.2, 10):
            with self.subTest(rpo=rpo):
                seen = {**self.seen(), "rpo_measured_s": rpo}
                self.assertEqual(checker.differences(expect, seen), [])

    def test_an_rpo_outside_the_range_or_not_measured_is_named(self):
        expect = {**EXPECT, "rpo_measured_s": {"from": 3, "to": 10}}
        for rpo, shown in ((2.9, "2.9"), (10.5, "10.5"), (None, "none")):
            with self.subTest(rpo=rpo):
                seen = {**self.seen(), "rpo_measured_s": rpo}
                self.assertEqual(
                    checker.differences(expect, seen),
                    [f"rpo_measured_s: expected 3 to 10, journal has {shown}"],
                )

    def test_an_rpo_is_compared_only_when_the_expectation_names_it(self):
        seen = {**self.seen(), "rpo_measured_s": 99}
        self.assertEqual(checker.differences(EXPECT, seen), [])

    def test_an_expectation_must_name_every_fact(self):
        incomplete = {"status": "completed"}
        found = checker.differences(incomplete, self.seen())
        self.assertEqual(
            found,
            [
                "expectation has no hypothesis_before, hypothesis_after, rollbacks, objectives"
            ],
        )


class LoadProblemsTests(unittest.TestCase):
    def test_a_load_that_ran_its_plan_and_measured_has_no_problem(self):
        self.assertEqual(checker.load_problems(LOAD, 30), [])
        self.assertEqual(checker.load_problems(LOAD, 30.2), [])

    def test_a_load_without_a_planned_time_is_not_timed(self):
        self.assertEqual(checker.load_problems(LOAD, None), [])

    def test_a_load_that_ended_before_its_plan(self):
        self.assertEqual(
            checker.load_problems(LOAD, 60),
            ["load_result: the load ran 30.2s of a plan of 60s"],
        )
        self.assertEqual(
            checker.load_problems({**LOAD, "duration_s": None}, 60),
            ["load_result: the load ran nulls of a plan of 60s"],
        )

    def test_a_load_that_measured_nothing(self):
        self.assertEqual(
            checker.load_problems({**LOAD, "total_requests": 0}, 30),
            ["load_result: total_requests is 0, expected more than 0"],
        )
        self.assertEqual(
            checker.load_problems({"tool": "k6", "latency_p99_ms": 0}, None),
            [
                "load_result: total_requests is null, expected more than 0",
                "load_result: latency_p99_ms is 0, expected more than 0",
            ],
        )
        self.assertEqual(
            checker.load_problems({**LOAD, "total_requests": True}, 30),
            ["load_result: total_requests is true, expected more than 0"],
        )

    def test_a_journal_without_a_load_result_has_no_load_problem(self):
        self.assertEqual(checker.load_problems(None, 60), [])


class ProofSetCase(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.journals = self.root / "journals"
        self.journals.mkdir()

    def write_expectations(self, body):
        path = self.root / "expectations.json"
        path.write_text(json.dumps(body))
        return path

    def journal(self, name, text=JOURNAL):
        (self.journals / name).write_text(text)

    def check(self, body, proof="examples-proof"):
        return checker.check_set(proof, self.write_expectations(body), self.journals)


class ProofSetTests(ProofSetCase):
    def test_a_matching_journal_is_ok(self):
        self.journal("saturation.journal.toon")
        lines = self.check(
            {
                "experiments": {
                    "saturation.toon": {"proof": "examples-proof", "expect": EXPECT}
                }
            }
        )
        self.assertEqual(lines, [("ok", "saturation.toon", [])])

    def test_a_wrong_expectation_fails_with_the_difference(self):
        self.journal("saturation.journal.toon")
        expect = {**EXPECT, "status": "deviated"}
        lines = self.check(
            {
                "experiments": {
                    "saturation.toon": {"proof": "examples-proof", "expect": expect}
                }
            }
        )
        self.assertEqual(
            lines,
            [
                (
                    "FAIL",
                    "saturation.toon",
                    ["status: expected deviated, journal has completed"],
                )
            ],
        )

    def with_load(self, journal, definition=EXPERIMENT):
        (self.root / "a.toon").write_text(definition)
        self.journal("a.journal.toon", journal)
        expect = {**EXPECT, "load_result": True}
        return self.check(
            {"experiments": {"a.toon": {"proof": "examples-proof", "expect": expect}}}
        )

    def test_a_load_that_ran_its_plan_is_ok(self):
        self.assertEqual(self.with_load(JOURNAL), [("ok", "a.toon", [])])

    def test_a_load_that_ended_before_its_plan_fails(self):
        early = JOURNAL.replace("duration_s: 30.2", "duration_s: 4.5")
        self.assertEqual(
            self.with_load(early),
            [("FAIL", "a.toon", ["load_result: the load ran 4.5s of a plan of 30s"])],
        )

    def test_a_load_that_measured_nothing_fails(self):
        blind = JOURNAL.replace("latency_p99_ms: 139.1", "latency_p99_ms: 0")
        self.assertEqual(
            self.with_load(blind),
            [
                (
                    "FAIL",
                    "a.toon",
                    ["load_result: latency_p99_ms is 0, expected more than 0"],
                )
            ],
        )

    def test_a_load_nobody_expected_is_not_looked_at(self):
        (self.root / "a.toon").write_text(EXPERIMENT)
        self.journal("a.journal.toon", JOURNAL.replace("30.2", "4.5"))
        body = {
            "experiments": {"a.toon": {"proof": "examples-proof", "expect": EXPECT}}
        }
        self.assertEqual(self.check(body), [("ok", "a.toon", [])])

    def test_a_definition_without_a_load_plan_does_not_time_the_load(self):
        early = JOURNAL.replace("duration_s: 30.2", "duration_s: 4.5")
        no_plan = EXPERIMENT.replace("  duration_s: 30\n", "")
        self.assertEqual(self.with_load(early, no_plan), [("ok", "a.toon", [])])
        self.assertEqual(
            self.with_load(early, "title: no load\nmethod[0]:\n"),
            [("ok", "a.toon", [])],
        )

    def test_a_missing_journal_fails(self):
        lines = self.check(
            {
                "experiments": {
                    "saturation.toon": {"proof": "examples-proof", "expect": EXPECT}
                }
            }
        )
        self.assertEqual(
            lines, [("FAIL", "saturation.toon", ["no journal saturation.journal.toon"])]
        )

    def test_a_journal_that_cannot_be_read_fails(self):
        self.journal("saturation.journal.toon", "status completed\n")
        lines = self.check(
            {
                "experiments": {
                    "saturation.toon": {"proof": "examples-proof", "expect": EXPECT}
                }
            }
        )
        self.assertEqual(lines[0][0], "FAIL")
        self.assertIn("line 1", lines[0][2][0])

    def test_only_the_entries_of_the_named_proof_are_checked(self):
        self.journal("saturation.journal.toon")
        body = {
            "experiments": {
                "saturation.toon": {"proof": "examples-proof", "expect": EXPECT},
                "k8s.toon": {"proof": "validate-only", "reason": "needs a cluster"},
                "demo.toon": {"proof": "demo-check", "expect": EXPECT},
            }
        }
        self.assertEqual([line[1] for line in self.check(body)], ["saturation.toon"])
        self.assertEqual(self.check(body, proof="kronika-seed"), [])

    def test_a_gameday_journal_is_checked_record_by_record(self):
        (self.root / "q2.gameday.toon").write_text(GAMEDAY)
        self.journal("q2.gameday.journal.toon", GAMEDAY_JOURNAL)
        body = {
            "experiments": {
                "first.toon": {
                    "proof": "examples-proof",
                    "within": "q2.gameday.toon",
                    "expect": expectation("completed", True, True, 0),
                },
                "second.toon": {
                    "proof": "examples-proof",
                    "within": "q2.gameday.toon",
                    "expect": expectation("deviated", True, False, 1),
                },
            },
            "gamedays": {
                "q2.gameday.toon": {
                    "proof": "examples-proof",
                    "expect": {"load_result": True},
                }
            },
        }
        self.assertEqual(
            self.check(body),
            [
                ("ok", "q2.gameday.toon", []),
                ("ok", "q2.gameday.toon: first.toon", []),
                ("ok", "q2.gameday.toon: second.toon", []),
            ],
        )

    def test_a_gameday_without_its_load_result_or_a_record_fails(self):
        (self.root / "q2.gameday.toon").write_text(GAMEDAY)
        short = GAMEDAY_JOURNAL.replace(GAMEDAY_LOAD, "load_result: null\n")
        short = short.replace("experiment_journals[2]:", "experiment_journals[1]:")
        short = (
            short[: short.index("  - experiment_title: second")]
            + short[short.index("load_result: null\ncompliance") :]
        )
        self.journal("q2.gameday.journal.toon", short)
        body = {
            "experiments": {
                "first.toon": {
                    "proof": "examples-proof",
                    "within": "q2.gameday.toon",
                    "expect": expectation("deviated", True, True, 0),
                },
                "second.toon": {
                    "proof": "examples-proof",
                    "within": "q2.gameday.toon",
                    "expect": expectation("deviated", True, False, 1),
                },
            },
            "gamedays": {
                "q2.gameday.toon": {
                    "proof": "examples-proof",
                    "expect": {"load_result": True},
                }
            },
        }
        self.assertEqual(
            self.check(body),
            [
                (
                    "FAIL",
                    "q2.gameday.toon",
                    ["load_result: expected one, journal has none"],
                ),
                (
                    "FAIL",
                    "q2.gameday.toon: first.toon",
                    ["status: expected deviated, journal has completed"],
                ),
                (
                    "FAIL",
                    "q2.gameday.toon: second.toon",
                    ["no record 2 in the journal"],
                ),
            ],
        )


class GameDayLoadTests(ProofSetCase):
    BODY = {
        "experiments": {
            "first.toon": {
                "proof": "examples-proof",
                "within": "q2.gameday.toon",
                "expect": expectation("completed", True, True, 0),
            },
            "second.toon": {
                "proof": "examples-proof",
                "within": "q2.gameday.toon",
                "expect": expectation("deviated", True, False, 1),
            },
        },
        "gamedays": {
            "q2.gameday.toon": {
                "proof": "examples-proof",
                "expect": {"load_result": True},
            }
        },
    }

    def gameday_line(self, definition, journal):
        (self.root / "q2.gameday.toon").write_text(definition)
        self.journal("q2.gameday.journal.toon", journal)
        return self.check(self.BODY)[0]

    def test_a_gameday_load_that_ended_before_its_plan_fails(self):
        early = GAMEDAY_JOURNAL.replace("duration_s: 60.5", "duration_s: 12.25")
        self.assertEqual(
            self.gameday_line(GAMEDAY, early),
            (
                "FAIL",
                "q2.gameday.toon",
                ["load_result: the load ran 12.25s of a plan of 60.0s"],
            ),
        )

    def test_a_gameday_load_that_ran_exactly_its_plan_is_ok(self):
        exact = GAMEDAY_JOURNAL.replace("duration_s: 60.5", "duration_s: 60.0")
        self.assertEqual(
            self.gameday_line(GAMEDAY, exact), ("ok", "q2.gameday.toon", [])
        )

    def test_a_gameday_load_that_measured_nothing_fails(self):
        blind = GAMEDAY_JOURNAL.replace("latency_p99_ms: 71.8", "latency_p99_ms: 0")
        self.assertEqual(
            self.gameday_line(GAMEDAY, blind),
            (
                "FAIL",
                "q2.gameday.toon",
                ["load_result: latency_p99_ms is 0, expected more than 0"],
            ),
        )


class GameDayScoreTests(ProofSetCase):
    SCORE = (
        "resilience_score:\n  pass_rate: 0.5\n  recovery_compliance: 0.5\n"
        "  load_impact_tolerance: 1\n  compliance_coverage: 1\n  overall: 0.775\n"
        "compliance_status: PARTIAL\n"
    )

    def body(self, score):
        body = json.loads(json.dumps(GameDayLoadTests.BODY))
        body["gamedays"]["q2.gameday.toon"]["expect"]["score"] = score
        return body

    def gameday_line(self, score, journal):
        (self.root / "q2.gameday.toon").write_text(GAMEDAY)
        self.journal("q2.gameday.journal.toon", journal)
        return self.check(self.body(score))[0]

    def scored(self, score_block):
        return GAMEDAY_JOURNAL.replace("compliance_status: COMPLIANT\n", score_block)

    def test_a_gameday_with_the_expected_score_is_ok(self):
        score = {"status": "PARTIAL", "recovery_compliance": 0.5}
        self.assertEqual(
            self.gameday_line(score, self.scored(self.SCORE)),
            ("ok", "q2.gameday.toon", []),
        )

    def test_a_gameday_with_another_status_and_recovery_fails_with_both(self):
        score = {"status": "COMPLIANT", "recovery_compliance": 1.0}
        self.assertEqual(
            self.gameday_line(score, self.scored(self.SCORE)),
            (
                "FAIL",
                "q2.gameday.toon",
                [
                    "score status: expected COMPLIANT, journal has PARTIAL",
                    "score recovery_compliance: expected 1.0, journal has 0.5",
                ],
            ),
        )

    def test_a_recovery_that_was_not_measured_is_null(self):
        unmeasured = self.SCORE.replace("  recovery_compliance: 0.5\n", "")
        score = {"status": "PARTIAL", "recovery_compliance": None}
        self.assertEqual(
            self.gameday_line(score, self.scored(unmeasured)),
            ("ok", "q2.gameday.toon", []),
        )

    def test_the_recovery_is_compared_to_two_decimals(self):
        thirds = self.SCORE.replace(
            "recovery_compliance: 0.5", "recovery_compliance: 0.6666666666666666"
        )
        score = {"status": "PARTIAL", "recovery_compliance": 0.67}
        self.assertEqual(
            self.gameday_line(score, self.scored(thirds)),
            ("ok", "q2.gameday.toon", []),
        )

    def test_a_score_expectation_must_name_the_status_and_the_recovery(self):
        self.assertEqual(
            self.gameday_line({"status": "PARTIAL"}, self.scored(self.SCORE)),
            (
                "FAIL",
                "q2.gameday.toon",
                ["score expectation has no recovery_compliance"],
            ),
        )


class CommandLineTests(unittest.TestCase):
    def run_script(self, *args):
        return subprocess.run(
            [sys.executable, str(SCRIPT), *args],
            capture_output=True,
            text=True,
            check=False,
        )

    def test_no_arguments_is_an_error(self):
        result = self.run_script()
        self.assertEqual(result.returncode, 2)

    def test_exit_code_and_lines(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "j").mkdir()
            (root / "j" / "a.journal.toon").write_text(JOURNAL)
            (root / "j" / "b.journal.toon").write_text(JOURNAL)
            body = {
                "experiments": {
                    "a.toon": {"proof": "examples-proof", "expect": EXPECT},
                    "b.toon": {
                        "proof": "examples-proof",
                        "expect": {**EXPECT, "rollbacks": 3},
                    },
                }
            }
            (root / "expectations.json").write_text(json.dumps(body))
            result = self.run_script(
                "--proof",
                "examples-proof",
                str(root / "expectations.json"),
                str(root / "j"),
            )
        self.assertEqual(result.returncode, 1)
        self.assertEqual(
            result.stdout.splitlines(),
            ["ok   a.toon", "FAIL b.toon", "     rollbacks: expected 3, journal has 1"],
        )

    def test_a_proof_with_nothing_to_check_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "expectations.json").write_text(json.dumps({"experiments": {}}))
            result = self.run_script(
                "--proof", "examples-proof", str(root / "expectations.json"), str(root)
            )
        self.assertEqual(result.returncode, 1)
        self.assertIn("nothing to check", result.stderr)

    def test_describe_prints_the_facts_of_a_journal(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "a.journal.toon"
            path.write_text(JOURNAL)
            result = self.run_script("--describe", str(path))
        self.assertEqual(result.returncode, 0)
        self.assertEqual(
            json.loads(result.stdout),
            {
                **EXPECT,
                "rollback_failures": 0,
                "load_result": True,
                "rpo_measured_s": None,
            },
        )

    def test_list_prints_what_the_proof_runs_on_its_own_in_file_order(self):
        body = {
            "experiments": {
                "b.toon": {"proof": "examples-proof", "expect": EXPECT},
                "k8s.toon": {"proof": "validate-only", "reason": "needs a cluster"},
                "child.toon": {
                    "proof": "examples-proof",
                    "within": "q2.gameday.toon",
                    "expect": EXPECT,
                },
                "a.toon": {"proof": "examples-proof", "expect": EXPECT},
            }
        }
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "expectations.json"
            path.write_text(json.dumps(body))
            listed = self.run_script("--proof", "examples-proof", "--list", str(path))
            nothing = self.run_script("--proof", "demo-check", "--list", str(path))
        self.assertEqual(listed.returncode, 0)
        self.assertEqual(listed.stdout.splitlines(), ["b.toon", "a.toon"])
        self.assertEqual(nothing.returncode, 1)
        self.assertIn("nothing to list", nothing.stderr)


if __name__ == "__main__":
    unittest.main()
