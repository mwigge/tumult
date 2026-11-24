//! `tumult manual`: the manual registry of the daemon from the command
//! line. Every command is a request to the daemon API (`/api/manual/*`),
//! with the URL and the token of `tumult gate`; the CLI never opens the
//! lake. The daemon checks the roles, the scopes, the lifecycle and the
//! sign-off rules, and refuses a field it does not know.
//!
//! While auth is open (no user in the daemon), `--by` names who acts. With
//! auth on, the user of the token acts and `--by` is not used.

mod daemon;
mod render;

use std::path::Path;

use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};

pub use daemon::Daemon;
pub use tumult_mcp::tools::ManualFilter;

use super::gate::encode_segment;

/// The filter of `tumult manual list`. `from` and `to` are a date
/// (`YYYY-MM-DD`, UTC) or an RFC 3339 time; a date as `to` counts its
/// whole day.
///
/// # Errors
///
/// Returns an error when `from` or `to` is neither.
pub fn list_filter(
    status: Option<String>,
    exercise_type: Option<String>,
    target: Option<String>,
    from: Option<&str>,
    to: Option<&str>,
) -> Result<ManualFilter> {
    let edge = |text: Option<&str>, parse: fn(&str) -> Result<i64, String>| {
        text.map(parse).transpose().map_err(anyhow::Error::msg)
    };
    Ok(ManualFilter {
        status,
        exercise_type,
        target,
        from_ns: edge(from, tumult_mcp::tools::window_start)?,
        to_ns: edge(to, tumult_mcp::tools::window_end)?,
    })
}

fn print(json: bool, body: &Value, text: impl FnOnce() -> String) -> Result<()> {
    if json {
        crate::outln!("{}", serde_json::to_string_pretty(body)?);
    } else {
        crate::out!("{}", text());
    }
    Ok(())
}

fn record_path(id: &str) -> String {
    format!("/api/manual/experiments/{}", encode_segment(id))
}

fn plan_path(id: &str) -> String {
    format!("/api/manual/plans/{}", encode_segment(id))
}

/// The id in a create answer.
fn id_of(answer: &Value) -> Result<&str> {
    answer["id"]
        .as_str()
        .context("the daemon answered without an id")
}

/// The body in `file`: JSON when its name ends in `.json`, TOON otherwise.
/// It must be an object of fields; the daemon checks the fields.
fn body_of(file: &Path) -> Result<Map<String, Value>> {
    let text = std::fs::read_to_string(file)
        .with_context(|| format!("failed to read {}", file.display()))?;
    let is_json = file
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"));
    let value: Value = if is_json {
        serde_json::from_str(&text)
            .with_context(|| format!("{} is not valid JSON", file.display()))?
    } else {
        toon_format::decode_default(&text)
            .with_context(|| format!("{} is not valid TOON", file.display()))?
    };
    match value {
        Value::Object(fields) => Ok(fields),
        _ => bail!("{} must hold one object of fields", file.display()),
    }
}

/// A body with the fields that are set: `None` is left out.
fn fields(pairs: &[(&str, Option<&str>)]) -> Value {
    let set: Map<String, Value> = pairs
        .iter()
        .filter_map(|(name, value)| value.map(|v| ((*name).to_owned(), json!(v))))
        .collect();
    Value::Object(set)
}

/// `tumult manual list`: the records `filter` lets through, newest entry
/// first. The daemon filters by status; the rest is filtered here.
///
/// # Errors
///
/// Returns an error when the daemon refuses the request.
pub async fn cmd_manual_list(daemon: &Daemon, filter: &ManualFilter, json: bool) -> Result<()> {
    let query: Vec<(&str, String)> = filter
        .status
        .iter()
        .map(|status| ("status", status.clone()))
        .collect();
    let answer = daemon.get("/api/manual/experiments", &query).await?;
    let mut records = answer["records"]
        .as_array()
        .cloned()
        .context("the daemon answered without records")?;
    records.retain(|record| filter.matches(record));
    print(json, &json!({ "records": records }), || {
        render::records(&records)
    })
}

/// `tumult manual show <id>`: one record with its parts and sign-offs.
///
/// # Errors
///
/// Returns an error when the daemon refuses the request.
pub async fn cmd_manual_show(daemon: &Daemon, id: &str, json: bool) -> Result<()> {
    let detail = daemon.get(&record_path(id), &[]).await?;
    print(json, &detail, || render::record(&detail))
}

/// `tumult manual create --file`: a draft from the record body in `file`.
/// `by` is the `entered_by` of the body.
///
/// # Errors
///
/// Returns an error when the file cannot be read or the daemon refuses
/// the record, such as for a field it does not know.
pub async fn cmd_manual_create(daemon: &Daemon, file: &Path, by: Option<&str>) -> Result<()> {
    let mut body = body_of(file)?;
    if let Some(by) = by {
        body.insert("entered_by".into(), json!(by));
    }
    let answer = daemon
        .post("/api/manual/experiments", &Value::Object(body))
        .await?;
    crate::outln!("created draft {}", id_of(&answer)?);
    Ok(())
}

/// `tumult manual submit <id>`: lock a draft for sign-off.
///
/// # Errors
///
/// Returns an error when the daemon refuses the submit.
pub async fn cmd_manual_submit(
    daemon: &Daemon,
    id: &str,
    attestation: Option<&str>,
    by: Option<&str>,
) -> Result<()> {
    let body = fields(&[("by", by), ("attestation", attestation)]);
    daemon
        .post(&format!("{}/submit", record_path(id)), &body)
        .await?;
    crate::outln!("submitted {id}");
    Ok(())
}

/// `tumult manual sign-off <id>`: approve or reject a submitted record.
///
/// # Errors
///
/// Returns an error when the daemon refuses the sign-off, such as from
/// the user who entered the record.
pub async fn cmd_manual_sign_off(
    daemon: &Daemon,
    id: &str,
    approve: bool,
    note: Option<&str>,
    by: Option<&str>,
) -> Result<()> {
    let (route, decision) = if approve {
        ("verify", "approved")
    } else {
        ("reject", "rejected")
    };
    let body = fields(&[("reviewer", by), ("note", note)]);
    let answer = daemon
        .post(&format!("{}/{route}", record_path(id)), &body)
        .await?;
    crate::out!("{}", render::signed(id, decision, &answer));
    Ok(())
}

/// `tumult manual attach <id> <file>`: the bytes of `file` as evidence.
///
/// # Errors
///
/// Returns an error when the file cannot be read or the daemon refuses
/// it, such as for its type or its size.
pub async fn cmd_manual_attach_file(
    daemon: &Daemon,
    id: &str,
    file: &Path,
    by: Option<&str>,
) -> Result<()> {
    let name = file
        .file_name()
        .and_then(|n| n.to_str())
        .with_context(|| format!("{} has no file name", file.display()))?;
    let bytes =
        std::fs::read(file).with_context(|| format!("failed to read {}", file.display()))?;
    let mut query = vec![("name", name.to_owned())];
    query.extend(by.map(|by| ("added_by", by.to_owned())));
    let answer = daemon
        .post_file(&format!("{}/attachments", record_path(id)), &query, bytes)
        .await?;
    crate::outln!(
        "attached {name} to {id} as {}: {}, {} bytes, sha256 {}",
        id_of(&answer)?,
        answer["media_type"].as_str().unwrap_or("-"),
        answer["size_bytes"],
        answer["file_hash"].as_str().unwrap_or("-"),
    );
    Ok(())
}

/// `tumult manual attach <id> --link <uri>`: a link as evidence.
///
/// # Errors
///
/// Returns an error when the daemon refuses the link.
pub async fn cmd_manual_attach_link(
    daemon: &Daemon,
    id: &str,
    uri: &str,
    label: Option<&str>,
    by: Option<&str>,
) -> Result<()> {
    let body = fields(&[
        ("kind", Some("url")),
        ("uri", Some(uri)),
        ("label", label),
        ("added_by", by),
    ]);
    let answer = daemon
        .post(&format!("{}/attachments", record_path(id)), &body)
        .await?;
    crate::outln!("attached {uri} to {id} as {}", id_of(&answer)?);
    Ok(())
}

/// `tumult manual verify <id>`: the audit verify of the record. Prints
/// each problem, then fails when it is not ok.
///
/// # Errors
///
/// Returns an error when the record is not ok, or the daemon refuses the
/// request.
pub async fn cmd_manual_verify(daemon: &Daemon, id: &str, json: bool) -> Result<()> {
    let verdict = daemon
        .get(&format!("{}/audit/verify", record_path(id)), &[])
        .await?;
    print(json, &verdict, || render::verdict(id, &verdict))?;
    if verdict["ok"] != true {
        bail!("the audit trail of manual record {id} is not ok");
    }
    Ok(())
}

/// `tumult manual plans list`: the plans, the soonest due first.
///
/// # Errors
///
/// Returns an error when the daemon refuses the request.
pub async fn cmd_manual_plans_list(
    daemon: &Daemon,
    status: Option<&str>,
    target: Option<&str>,
    json: bool,
) -> Result<()> {
    let query: Vec<(&str, String)> = [("status", status), ("target", target)]
        .into_iter()
        .filter_map(|(name, value)| value.map(|v| (name, v.to_owned())))
        .collect();
    let answer = daemon.get("/api/manual/plans", &query).await?;
    let plans = answer["plans"]
        .as_array()
        .context("the daemon answered without plans")?;
    print(json, &answer, || render::plans(plans))
}

/// `tumult manual plans show <id>`: one plan with its audit trail.
///
/// # Errors
///
/// Returns an error when the daemon refuses the request.
pub async fn cmd_manual_plans_show(daemon: &Daemon, id: &str, json: bool) -> Result<()> {
    let detail = daemon.get(&plan_path(id), &[]).await?;
    print(json, &detail, || render::plan(&detail))
}

/// `tumult manual plans create --file`: a plan from the plan body in
/// `file`. `by` is the `created_by` of the body.
///
/// # Errors
///
/// Returns an error when the file cannot be read or the daemon refuses
/// the plan.
pub async fn cmd_manual_plans_create(daemon: &Daemon, file: &Path, by: Option<&str>) -> Result<()> {
    let mut body = body_of(file)?;
    if let Some(by) = by {
        body.insert("created_by".into(), json!(by));
    }
    let answer = daemon
        .post("/api/manual/plans", &Value::Object(body))
        .await?;
    crate::outln!("created plan {}", id_of(&answer)?);
    Ok(())
}

/// `tumult manual plans cancel <id>`: cancel a planned plan.
///
/// # Errors
///
/// Returns an error when the daemon refuses the cancel.
pub async fn cmd_manual_plans_cancel(
    daemon: &Daemon,
    id: &str,
    reason: Option<&str>,
    by: Option<&str>,
) -> Result<()> {
    let body = fields(&[("by", by), ("reason", reason)]);
    daemon
        .post(&format!("{}/cancel", plan_path(id)), &body)
        .await?;
    crate::outln!("cancelled plan {id}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_body_is_read_as_json_or_as_toon_by_the_name_of_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let json_file = dir.path().join("record.JSON");
        std::fs::write(
            &json_file,
            r#"{"experiment_name": "Restore", "executed_at_ns": 1794000000000000000}"#,
        )
        .unwrap();
        let toon_file = dir.path().join("record.toon");
        std::fs::write(
            &toon_file,
            "experiment_name: Restore\nexecuted_at_ns: 1794000000000000000\n",
        )
        .unwrap();
        for file in [&json_file, &toon_file] {
            let body = body_of(file).unwrap();
            assert_eq!(body["experiment_name"], "Restore", "{}", file.display());
            assert_eq!(
                body["executed_at_ns"],
                1_794_000_000_000_000_000_i64,
                "{}",
                file.display()
            );
        }
        let list = dir.path().join("list.json");
        std::fs::write(&list, "[1, 2]").unwrap();
        let refused = body_of(&list).unwrap_err().to_string();
        assert!(refused.contains("one object of fields"), "{refused}");
        let broken = dir.path().join("broken.json");
        std::fs::write(&broken, "{").unwrap();
        assert!(body_of(&broken)
            .unwrap_err()
            .to_string()
            .contains("not valid JSON"));
    }

    #[test]
    fn a_body_leaves_out_what_is_not_set() {
        assert_eq!(
            fields(&[("by", Some("alice")), ("attestation", None)]),
            json!({"by": "alice"})
        );
    }

    #[test]
    fn a_list_filter_reads_dates_and_times_and_refuses_anything_else() {
        let filter = list_filter(None, None, None, Some("2026-11-06"), Some("2026-11-06")).unwrap();
        assert_eq!(filter.from_ns, Some(1_793_923_200_000_000_000));
        assert_eq!(filter.to_ns, Some(1_794_009_600_000_000_000));
        let refused = list_filter(None, None, None, None, Some("soon")).unwrap_err();
        assert!(refused.to_string().contains("'soon'"), "{refused}");
    }
}
