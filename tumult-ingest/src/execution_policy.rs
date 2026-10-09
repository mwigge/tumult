//! Operator-owned bindings from resolved execution inputs to an environment.
//! A request's label never lowers the risk of an unbound fault.
use crate::approvals::{classify, introspect, Tier, TierInput};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tumult_core::types::Experiment;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    sha256: String,
    env: String,
    target: Option<String>,
}

/// Fingerprint the resolved execution artifact, including resolved target vars.
/// JSON object keys are sorted recursively, independent of HashMap iteration.
/// This is a private credential verifier: never expose it to ordinary API
/// readers. Public previews use [`preview_token`]; trusted administrators may
/// obtain this digest only to configure operator-owned execution bindings.
pub fn execution_hash(
    experiment: &Experiment,
    injected: &std::collections::HashMap<String, String>,
) -> Result<String, String> {
    fn canonical(value: serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(map) => {
                let sorted: std::collections::BTreeMap<_, _> =
                    map.into_iter().map(|(k, v)| (k, canonical(v))).collect();
                serde_json::Value::Object(sorted.into_iter().collect())
            }
            serde_json::Value::Array(items) => {
                serde_json::Value::Array(items.into_iter().map(canonical).collect())
            }
            other => other,
        }
    }
    let value = serde_json::json!({"experiment": experiment, "injected": injected});
    let bytes = serde_json::to_vec(&canonical(value))
        .map_err(|_| "cannot fingerprint execution".to_string())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

/// An opaque preview token. A fresh process key invalidates previews on restart
/// without publishing a fast verifier of resolved credentials.
#[must_use]
pub fn preview_token(private_hash: &str) -> String {
    static KEY: std::sync::OnceLock<[u8; 32]> = std::sync::OnceLock::new();
    let key = KEY.get_or_init(|| {
        let mut key = [0u8; 32];
        key[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        key[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        key
    });
    token_for_key(private_hash, key)
}

fn token_for_key(private_hash: &str, key: &[u8; 32]) -> String {
    use hmac::{Hmac, Mac};
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts a 32-byte key");
    mac.update(b"tumult-execution-preview-v1:");
    mac.update(private_hash.as_bytes());
    format!("{:x}", mac.finalize().into_bytes())
}

/// Read the server-owned JSON binding file. Invalid configuration fails closed.
/// Scoped principals require an exact binding. Every unbound executable plan
/// requires T3: a caller can label arbitrary process code as a probe.
pub fn classify_execution(
    db_path: &std::path::Path,
    experiment: &Experiment,
    injected: &std::collections::HashMap<String, String>,
    env: &str,
    target: Option<&str>,
    require_binding: bool,
) -> Result<Tier, String> {
    let bindings = match std::env::var("TUMULTD_EXECUTION_BINDINGS") {
        Ok(path) if !path.trim().is_empty() => {
            let raw = std::fs::read_to_string(path)
                .map_err(|_| "execution bindings cannot be read".to_string())?;
            serde_json::from_str::<Vec<Binding>>(&raw)
                .map_err(|_| "execution bindings are invalid".to_string())?
        }
        _ => {
            let path = db_path.with_extension("execution-bindings.json");
            match std::fs::read_to_string(path) {
                Ok(raw) => serde_json::from_str::<Vec<Binding>>(&raw)
                    .map_err(|_| "execution bindings are invalid".to_string())?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
                Err(_) => return Err("execution bindings cannot be read".into()),
            }
        }
    };
    classify_bound(
        experiment,
        injected,
        env,
        target,
        require_binding,
        &bindings,
    )
}

fn classify_bound(
    experiment: &Experiment,
    injected: &std::collections::HashMap<String, String>,
    env: &str,
    target: Option<&str>,
    require_binding: bool,
    bindings: &[Binding],
) -> Result<Tier, String> {
    if env.trim().is_empty() {
        return Err("an explicit execution environment is required".into());
    }
    let hash = execution_hash(experiment, injected)?;
    let bound = bindings
        .iter()
        .any(|b| b.sha256 == hash && b.env == env && b.target.as_deref() == target);
    if !bound && (require_binding || bindings.iter().any(|b| b.sha256 == hash)) {
        return Err("execution is not bound to this environment and target; ask the platform administrator to review its fingerprint and configure TUMULTD_EXECUTION_BINDINGS".into());
    }
    let shape = introspect(experiment);
    if !bound {
        return Ok(Tier::T3);
    }
    Ok(classify(&TierInput {
        env: env.into(),
        catalog_matched: false,
        introspection: shape,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fault() -> Experiment {
        let action = tumult_core::types::Activity::default();
        Experiment {
            method: vec![action.clone()],
            rollbacks: vec![action],
            ..Experiment::default()
        }
    }
    #[test]
    fn preview_token_hides_private_digest_and_expires_with_process_key() {
        let hash = execution_hash(&fault(), &Default::default()).unwrap();
        assert_ne!(preview_token(&hash), hash);
        assert_eq!(preview_token(&hash), preview_token(&hash));
        assert_ne!(
            token_for_key(&hash, &[1; 32]),
            token_for_key(&hash, &[2; 32])
        );
        assert_ne!(
            token_for_key(&hash, &[1; 32]),
            token_for_key("changed", &[1; 32])
        );
    }
    #[test]
    fn caller_label_cannot_lower_unbound_fault_risk() {
        assert_eq!(
            classify_bound(&fault(), &Default::default(), "dev", None, false, &[]).unwrap(),
            Tier::T3
        );
        assert!(classify_bound(&fault(), &Default::default(), "dev", None, true, &[]).is_err());
    }
    #[test]
    fn calling_a_process_a_probe_does_not_bypass_review() {
        let mut plan = fault();
        plan.method[0].activity_type = tumult_core::types::ActivityType::Probe;
        assert_eq!(
            classify_bound(&plan, &Default::default(), "dev", None, false, &[]).unwrap(),
            Tier::T3
        );
    }

    #[test]
    fn fingerprint_binds_injected_values_and_is_order_independent() {
        let a = std::collections::HashMap::from([
            ("A".into(), "first".into()),
            ("B".into(), "second".into()),
        ]);
        let b = std::collections::HashMap::from([
            ("B".into(), "second".into()),
            ("A".into(), "first".into()),
        ]);
        assert_eq!(
            execution_hash(&fault(), &a).unwrap(),
            execution_hash(&fault(), &b).unwrap()
        );
        let changed = std::collections::HashMap::from([
            ("A".into(), "different-destination".into()),
            ("B".into(), "second".into()),
        ]);
        assert_ne!(
            execution_hash(&fault(), &a).unwrap(),
            execution_hash(&fault(), &changed).unwrap()
        );
    }

    #[test]
    fn binding_pins_resolved_target_and_environment() {
        let original = fault();
        let bindings = vec![Binding {
            sha256: execution_hash(&original, &Default::default()).unwrap(),
            env: "staging".into(),
            target: Some("db-a".into()),
        }];
        assert_eq!(
            classify_bound(
                &original,
                &Default::default(),
                "staging",
                Some("db-a"),
                true,
                &bindings
            )
            .unwrap(),
            Tier::T2
        );
        assert!(classify_bound(
            &original,
            &Default::default(),
            "dev",
            Some("db-a"),
            false,
            &bindings
        )
        .is_err());
        assert!(classify_bound(
            &original,
            &Default::default(),
            "staging",
            Some("db-b"),
            true,
            &bindings
        )
        .is_err());
        let mut changed = original;
        changed.method[0].provider = tumult_core::types::Provider::Process {
            path: "remote-fault".into(),
            arguments: vec!["other-host".into()],
            env: Default::default(),
            timeout_s: None,
        };
        assert!(classify_bound(
            &changed,
            &Default::default(),
            "staging",
            Some("db-a"),
            true,
            &bindings
        )
        .is_err());
    }
}

/// Revalidate a recurring execution's original author, including disabled users,
/// demotions and scope changes made after schedule/campaign creation.
pub fn actor_requires_binding(
    db_path: &std::path::Path,
    actor: Option<&str>,
    env: &str,
) -> Result<bool, String> {
    let reader = tumult_lake::Store::at(db_path)
        .read_only()
        .map_err(|_| "cannot authorize execution owner".to_string())?;
    if !reader
        .real_users_exist()
        .map_err(|_| "cannot authorize execution owner".to_string())?
    {
        return Ok(false);
    }
    let actor = actor.ok_or_else(|| "execution has no authenticated owner".to_string())?;
    let user = reader
        .user_by_username(actor)
        .map_err(|_| "cannot authorize execution owner".to_string())?
        .ok_or_else(|| "execution owner no longer exists".to_string())?;
    if user.disabled
        || tumult_auth::Role::parse(&user.role)
            .is_none_or(|role| role < tumult_auth::Role::Operator)
    {
        return Err("execution owner is disabled or no longer an operator".into());
    }
    let scopes = reader
        .user_env_scopes(&user.id)
        .map_err(|_| "cannot authorize execution owner".to_string())?;
    if !scopes.is_empty() && !scopes.iter().any(|scope| scope == env) {
        return Err("execution environment is outside its owner's scopes".into());
    }
    Ok(!scopes.is_empty())
}
