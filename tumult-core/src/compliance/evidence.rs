//! Evidence classification: the kind and strength of chaos-engineering
//! evidence a Tumult experiment supplies toward a regulatory control.

/// The kind of chaos-engineering evidence a Tumult experiment supplies toward
/// a regulatory control. Deliberately narrow: chaos experiments produce
/// resilience/recovery evidence, not (for example) threat-led penetration
/// testing or incident-reporting-timeline evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceType {
    /// Scenario-based fault injection proving a system behaves under stress.
    ResilienceTesting,
    /// Measured recovery (RTO/MTTR, rollback, data integrity) after a fault.
    RecoveryValidation,
    /// Business-continuity exercising/testing under a plausible scenario.
    ContinuityExercise,
    /// Baseline-vs-fault comparison demonstrating control effectiveness.
    EffectivenessAssessment,
    /// Observability/detection coverage evidenced during an experiment.
    Monitoring,
    /// Exercising of the incident-response process by a controlled fault.
    IncidentResponseTesting,
}

impl EvidenceType {
    /// Short human-readable label.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ResilienceTesting => "resilience testing",
            Self::RecoveryValidation => "recovery validation",
            Self::ContinuityExercise => "continuity exercise",
            Self::EffectivenessAssessment => "effectiveness assessment",
            Self::Monitoring => "monitoring",
            Self::IncidentResponseTesting => "incident-response testing",
        }
    }
}

/// How strongly a Tumult experiment can evidence a control. This is an
/// evidence-strength grade, not a compliance determination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceStrength {
    /// The control is directly about resilience/recovery testing; a passing
    /// experiment is first-class evidence toward it.
    Direct,
    /// The experiment corroborates the control but is not the whole of it.
    Supporting,
    /// The mapping is indirect; the experiment touches the control's subject
    /// but does not, on its own, evidence what the control actually requires.
    Indirect,
}

impl EvidenceStrength {
    /// Short human-readable label.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Supporting => "supporting",
            Self::Indirect => "indirect",
        }
    }
}
