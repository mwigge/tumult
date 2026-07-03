use crate::adapters::{adapter_failure_expectation, AdapterSmokeExpectation};
use crate::model::AgenticRunResult;

#[derive(Debug, Clone, PartialEq)]
pub struct SmokeReport {
    pub adapter: String,
    pub scenario: String,
    pub fault: String,
    pub contract: String,
    pub expected: String,
    pub actual: String,
    pub next_diagnostic_command: String,
    pub passed: bool,
    pub run_result: AgenticRunResult,
}

impl SmokeReport {
    #[must_use]
    pub fn feedback_line(&self) -> String {
        if self.passed {
            format!(
                "pass adapter={} scenario={} fault={} contract={} expected={} actual={} next_diagnostic_command={}",
                self.adapter,
                self.scenario,
                self.fault,
                self.contract,
                self.expected,
                self.actual,
                self.next_diagnostic_command
            )
        } else {
            self.expectation().failure_message()
        }
    }

    #[must_use]
    pub fn expectation(&self) -> AdapterSmokeExpectation {
        adapter_failure_expectation(
            self.adapter.clone(),
            self.scenario.clone(),
            self.fault.clone(),
            self.contract.clone(),
            self.expected.clone(),
            self.actual.clone(),
            self.next_diagnostic_command.clone(),
        )
    }
}

#[must_use]
pub fn smoke_failure_output(report: &SmokeReport) -> Option<String> {
    if report.passed {
        None
    } else {
        Some(report.feedback_line())
    }
}
