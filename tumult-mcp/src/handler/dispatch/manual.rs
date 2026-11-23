//! Dispatch bodies for the manual registry tools: `manual_list`,
//! `manual_get`, `manual_create_draft` and `manual_plans_list`. There is
//! no body that submits, signs off, verifies or attaches: an agent may
//! draft, a person signs.

use rust_mcp_sdk::schema::CallToolRequestParams;
use tumult_lake::{
    ActionItem, ExerciseDetail, ExerciseLink, ExerciseStep, ExerciseType, LinkKind, ManualError,
    ManualOutcome, NewManualExperiment, Participant, PlanFilter, Retest, RetestKind, Severity,
    StepResult, Tester, TesterKind, TlptDetail,
};

use crate::error::ToolError;
use crate::handler::schema::{
    ManualActionArg, ManualCreateDraftTool, ManualGetTool, ManualListTool, ManualPlansListTool,
    ManualStepArg, ManualTesterArg,
};
use crate::handler::Role;
use crate::tools;

use super::{parse_args, store_path_for, validate_page, Dispatched, ToolOutput};

/// Dispatch `tumult_manual_list`: the records the filters let through, one
/// page of them. A viewer reads the store of the server.
pub(super) fn manual_list(params: &CallToolRequestParams, role: Option<Role>) -> Dispatched {
    let args: ManualListTool = parse_args(params)?;
    let (limit, offset) = validate_page(args.limit, args.offset)?;
    let store_path = store_path_for(role, &args.store_path);
    let filter = match list_filter(args) {
        Ok(filter) => filter,
        Err(why) => return Ok(Err(ToolError::InvalidInput(why))),
    };
    Ok(
        tokio::task::block_in_place(|| tools::manual_list(&store_path, &filter, limit, offset))
            .map(ToolOutput::from),
    )
}

/// Dispatch `tumult_manual_get`: one record with everything it holds.
pub(super) fn manual_get(params: &CallToolRequestParams, role: Option<Role>) -> Dispatched {
    let args: ManualGetTool = parse_args(params)?;
    let store_path = store_path_for(role, &args.store_path);
    Ok(
        tokio::task::block_in_place(|| tools::manual_get(&store_path, &args.id))
            .map(ToolOutput::from),
    )
}

/// Dispatch `tumult_manual_create_draft`: write the body as a draft. The
/// caller is an operator; a viewer never gets here.
pub(super) fn manual_create_draft(params: &CallToolRequestParams) -> Dispatched {
    let args: ManualCreateDraftTool = parse_args(params)?;
    let store_path = args.store_path.clone();
    let draft = match new_record(args) {
        Ok(draft) => draft,
        Err(e) => return Ok(Err(ToolError::InvalidInput(e.to_string()))),
    };
    Ok(
        tokio::task::block_in_place(|| tools::manual_create_draft(&store_path, &draft))
            .map(ToolOutput::from),
    )
}

/// Dispatch `tumult_manual_plans_list`: the plans the filters let through,
/// one page of them.
pub(super) fn manual_plans_list(params: &CallToolRequestParams, role: Option<Role>) -> Dispatched {
    let args: ManualPlansListTool = parse_args(params)?;
    let (limit, offset) = validate_page(args.limit, args.offset)?;
    let store_path = store_path_for(role, &args.store_path);
    let filter = PlanFilter {
        status: args.status,
        target_system: args.target,
        due_before_ns: None,
    };
    Ok(tokio::task::block_in_place(|| {
        tools::manual_plans_list(&store_path, &filter, limit, offset)
    })
    .map(ToolOutput::from))
}

/// The filter the arguments of `tumult_manual_list` ask for.
fn list_filter(args: ManualListTool) -> Result<tools::ManualFilter, String> {
    Ok(tools::ManualFilter {
        from_ns: args.from.as_deref().map(tools::window_start).transpose()?,
        to_ns: args.to.as_deref().map(tools::window_end).transpose()?,
        status: args.status,
        exercise_type: args.exercise_type,
        target: args.target,
    })
}

fn step(arg: ManualStepArg) -> Result<ExerciseStep, ManualError> {
    Ok(ExerciseStep {
        title: arg.title,
        actor: arg.actor,
        started_at_ns: arg.started_at_ns,
        ended_at_ns: arg.ended_at_ns,
        result: arg.result.as_deref().map(StepResult::parse).transpose()?,
        note: arg.note,
    })
}

fn action(arg: ManualActionArg) -> Result<ActionItem, ManualError> {
    let retest = arg
        .retest
        .map(|r| -> Result<Retest, ManualError> {
            Ok(Retest {
                kind: RetestKind::parse(&r.kind)?,
                id: r.id,
            })
        })
        .transpose()?;
    Ok(ActionItem {
        title: arg.title,
        owner: arg.owner,
        due_at_ns: arg.due_at_ns,
        ticket: arg.ticket,
        severity: arg.severity.as_deref().map(Severity::parse).transpose()?,
        retest,
    })
}

fn tester(arg: ManualTesterArg) -> Result<Tester, ManualError> {
    let kind = TesterKind::parse(&arg.kind).ok_or_else(|| {
        ManualError::Invalid(format!(
            "unknown tester kind '{}' (internal|external)",
            arg.kind
        ))
    })?;
    Ok(Tester {
        name: arg.name,
        kind,
    })
}

/// The record the body of a draft describes, as the API reads its body.
fn new_record(args: ManualCreateDraftTool) -> Result<NewManualExperiment, ManualError> {
    let exercise = ExerciseDetail {
        rto_target_s: args.rto_target_s,
        rpo_target_s: args.rpo_target_s,
        rpo_measured_s: args.rpo_measured_s,
        data_verified: args.data_verified,
        participants: args
            .participants
            .unwrap_or_default()
            .into_iter()
            .map(|p| Participant {
                name: p.name,
                role: p.role,
            })
            .collect(),
        steps: args
            .steps
            .unwrap_or_default()
            .into_iter()
            .map(step)
            .collect::<Result<_, _>>()?,
        actions: args
            .actions
            .unwrap_or_default()
            .into_iter()
            .map(action)
            .collect::<Result<_, _>>()?,
        links: args
            .links
            .unwrap_or_default()
            .into_iter()
            .map(|l| {
                Ok(ExerciseLink {
                    kind: LinkKind::parse(&l.kind)?,
                    linked_id: l.linked_id,
                    label: l.label,
                })
            })
            .collect::<Result<_, ManualError>>()?,
        plan_id: args.plan_id,
        findings_severity: args
            .findings_severity
            .as_deref()
            .map(Severity::parse)
            .transpose()?,
        tester: args.tester.map(tester).transpose()?,
        tlpt: args.tlpt.map(|t| TlptDetail {
            attestation_ref: t.attestation_ref,
            attested_at_ns: t.attested_at_ns,
            summary_report_ref: t.summary_report_ref,
        }),
    };
    Ok(NewManualExperiment {
        experiment_name: args.experiment_name,
        exercise_type: ExerciseType::parse(&args.exercise_type)?,
        executed_at_ns: args.executed_at_ns,
        hypothesis: args.hypothesis,
        method: args.method,
        outcome: ManualOutcome::parse(&args.outcome_status)?,
        hypothesis_met: args.hypothesis_met,
        findings: args.findings,
        action_items: args.action_items.unwrap_or_default(),
        target_system: args.target_system,
        target_environment: args.target_environment,
        blast_radius: args.blast_radius,
        recovery_time_s: args.recovery_time_s,
        duration_s: args.duration_s,
        entered_by: args.entered_by,
        attestation: args.attestation,
        renewal_due_ns: args.renewal_due_ns,
        framework_refs: args.framework_refs.unwrap_or_default(),
        exercise,
    })
}
