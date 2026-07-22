//! Curated starter templates.
//!
//! Each template is a parameterized TOON experiment (with `${param}`
//! placeholders) plus a set of documented parameters and their defaults.
//! Instantiation substitutes the parameters (defaults overlaid with caller
//! overrides) via the core [`apply_vars`]
//! machinery, then validates the result — so every template, with its
//! defaults or any override, produces an experiment that passes
//! `tumult validate`.

use std::collections::HashMap;

use indexmap::IndexMap;

use tumult_core::engine::{apply_vars, parse_experiment, validate_experiment};
use tumult_core::types::Experiment;

use crate::builder::{encode_experiment, AuthoringError};
use crate::catalog::Domain;

/// A single template parameter with a documented default.
#[derive(Debug, Clone, Copy)]
pub struct TemplateParam {
    /// Parameter name, referenced as `${name}` in the template body.
    pub name: &'static str,
    /// Default value used when the caller does not override it.
    pub default: &'static str,
    /// One-line human description.
    pub description: &'static str,
}

/// A curated starter template.
#[derive(Debug, Clone, Copy)]
pub struct Template {
    /// Stable identifier used with `tumult new --from <name>`.
    pub name: &'static str,
    /// One-line description.
    pub description: &'static str,
    /// Fault domain this template belongs to.
    pub domain: Domain,
    /// Fill-in parameters and their defaults.
    pub params: &'static [TemplateParam],
    /// Embedded TOON body with `${param}` placeholders.
    body: &'static str,
}

macro_rules! param {
    ($name:literal, $default:literal, $desc:literal) => {
        TemplateParam {
            name: $name,
            default: $default,
            description: $desc,
        }
    };
}

const NET_LATENCY_PARAMS: &[TemplateParam] = &[
    param!(
        "target",
        "127.0.0.1",
        "Host to ping for the steady-state probe"
    ),
    param!("iface", "lo", "Network interface to apply netem to"),
    param!("delay", "100ms", "Added latency, e.g. 100ms"),
];

const PG_KILL_PARAMS: &[TemplateParam] = &[
    param!(
        "container",
        "docker-postgres-1",
        "PostgreSQL container name"
    ),
    param!("pg_user", "tumult", "PostgreSQL user"),
    param!("pg_database", "tumult_test", "PostgreSQL database"),
];

const CPU_STRESS_PARAMS: &[TemplateParam] = &[
    param!("target", "localhost", "Logical target label"),
    param!("cpus", "2", "Number of CPU workers"),
    param!("duration", "5s", "Stress duration, e.g. 5s"),
];

const MEMORY_STRESS_PARAMS: &[TemplateParam] = &[
    param!("target", "localhost", "Logical target label"),
    param!("workers", "1", "Number of VM stress workers"),
    param!("bytes", "256M", "Memory per worker, e.g. 256M"),
    param!("duration", "5s", "Stress duration, e.g. 5s"),
];

const REDIS_FLUSH_PARAMS: &[TemplateParam] = &[
    param!("container", "docker-redis-1", "Redis container name"),
    param!(
        "scratch_db",
        "15",
        "Redis logical DB index to seed and flush"
    ),
];

const CONTAINER_PAUSE_PARAMS: &[TemplateParam] = &[param!(
    "container",
    "docker-redis-1",
    "Container to pause and unpause"
)];

const SSH_STRESS_PARAMS: &[TemplateParam] = &[
    param!("ssh_host", "localhost", "SSH host"),
    param!("ssh_port", "12222", "SSH port"),
    param!("ssh_user", "tumult", "SSH user"),
    param!("ssh_key", "/tmp/tumult-test-key", "SSH private key path"),
    param!("cpus", "1", "Number of remote CPU workers"),
    param!("duration", "3s", "Remote stress duration, e.g. 3s"),
];

const K8S_POD_PARAMS: &[TemplateParam] = &[
    param!("namespace", "default", "Kubernetes namespace"),
    param!("deployment", "nginx-test", "Deployment name to scale"),
    param!(
        "label_selector",
        "app=nginx-test",
        "Pod label selector for readiness"
    ),
    param!("replicas", "2", "Steady-state replica count to restore"),
    param!(
        "scaled_replicas",
        "1",
        "Reduced replica count during the fault"
    ),
];

const CLOCK_SKEW_PARAMS: &[TemplateParam] = &[
    param!("target", "demo-app", "Logical target label"),
    param!(
        "health_url",
        "http://demo-app:8080/health",
        "Health endpoint URL"
    ),
    param!(
        "health_pattern",
        "ok",
        "Regex the health response must match"
    ),
    param!(
        "skew_seconds",
        "3600",
        "Seconds to skew the perceived clock forward"
    ),
    param!(
        "plugins_dir",
        "/opt/tumult/plugins",
        "Directory the timewarp plugin is installed under"
    ),
];

const AGENTIC_SMOKE_PARAMS: &[TemplateParam] = &[
    param!(
        "scenario",
        "malformed-json-recovery",
        "Agentic scenario pack name"
    ),
    param!("adapter", "fake-http", "Deterministic local adapter"),
    param!(
        "contract",
        "recovers-from-malformed-output",
        "Contract asserted after the fault"
    ),
];

/// All curated templates, in a stable order.
#[must_use]
pub fn all_templates() -> Vec<Template> {
    vec![
        Template {
            name: "net-latency",
            description: "Add network latency (tc netem) and verify connectivity survives",
            domain: Domain::Network,
            params: NET_LATENCY_PARAMS,
            body: include_str!("templates/net-latency.toon"),
        },
        Template {
            name: "pg-kill-connections",
            description: "Terminate idle PostgreSQL backends and verify recovery",
            domain: Domain::Database,
            params: PG_KILL_PARAMS,
            body: include_str!("templates/pg-kill-connections.toon"),
        },
        Template {
            name: "cpu-stress",
            description: "Inject CPU stress with stress-ng and verify responsiveness",
            domain: Domain::Resource,
            params: CPU_STRESS_PARAMS,
            body: include_str!("templates/cpu-stress.toon"),
        },
        Template {
            name: "memory-stress",
            description: "Inject memory pressure with stress-ng and verify responsiveness",
            domain: Domain::Resource,
            params: MEMORY_STRESS_PARAMS,
            body: include_str!("templates/memory-stress.toon"),
        },
        Template {
            name: "redis-flush",
            description: "Seed and flush a Redis scratch DB and verify Redis recovers",
            domain: Domain::State,
            params: REDIS_FLUSH_PARAMS,
            body: include_str!("templates/redis-flush.toon"),
        },
        Template {
            name: "container-pause",
            description: "Pause and unpause a container and verify it recovers",
            domain: Domain::Container,
            params: CONTAINER_PAUSE_PARAMS,
            body: include_str!("templates/container-pause.toon"),
        },
        Template {
            name: "ssh-stress",
            description: "Run stress-ng on a remote host over SSH and verify reachability",
            domain: Domain::Process,
            params: SSH_STRESS_PARAMS,
            body: include_str!("templates/ssh-stress.toon"),
        },
        Template {
            name: "k8s-pod-fault",
            description: "Scale a Kubernetes deployment down and back up via the native plugin",
            domain: Domain::Container,
            params: K8S_POD_PARAMS,
            body: include_str!("templates/k8s-pod-fault.toon"),
        },
        Template {
            name: "clock-skew",
            description: "Skew the perceived clock (timewarp) and verify the service stays healthy",
            domain: Domain::Time,
            params: CLOCK_SKEW_PARAMS,
            body: include_str!("templates/clock-skew.toon"),
        },
        Template {
            name: "agentic-smoke",
            description: "Deterministic local agentic AI malformed-output smoke test",
            domain: Domain::Agentic,
            params: AGENTIC_SMOKE_PARAMS,
            body: include_str!("templates/agentic-smoke.toon"),
        },
    ]
}

/// Look up a template by name.
#[must_use]
pub fn find_template(name: &str) -> Option<Template> {
    all_templates().into_iter().find(|t| t.name == name)
}

impl Template {
    /// The parameter with the given name, if any.
    #[must_use]
    pub fn param(&self, name: &str) -> Option<&TemplateParam> {
        self.params.iter().find(|p| p.name == name)
    }

    /// Instantiate this template into a validated [`Experiment`].
    ///
    /// `overrides` supplies `key=value` parameter values that replace the
    /// defaults. Every override key must name a declared parameter.
    ///
    /// # Errors
    ///
    /// Returns [`AuthoringError::BadOverride`] if an override names an unknown
    /// parameter, [`AuthoringError::Instantiate`] if parsing or substitution
    /// fails, or [`AuthoringError::Validation`] if the result does not
    /// validate.
    pub fn instantiate<S: std::hash::BuildHasher>(
        &self,
        overrides: &HashMap<String, String, S>,
    ) -> Result<Experiment, AuthoringError> {
        // Reject unknown override keys so typos surface immediately.
        for key in overrides.keys() {
            if self.param(key).is_none() {
                let valid: Vec<&str> = self.params.iter().map(|p| p.name).collect();
                return Err(AuthoringError::BadOverride(format!(
                    "'{key}' is not a parameter of template '{}' (valid: {})",
                    self.name,
                    valid.join(", ")
                )));
            }
        }

        // Defaults overlaid with overrides.
        let mut vars: HashMap<String, String> = self
            .params
            .iter()
            .map(|p| (p.name.to_string(), p.default.to_string()))
            .collect();
        for (k, v) in overrides {
            vars.insert(k.clone(), v.clone());
        }

        let experiment =
            parse_experiment(self.body).map_err(|e| AuthoringError::Instantiate(e.to_string()))?;
        let substituted = apply_vars(&experiment, &vars)
            .map_err(|e| AuthoringError::Instantiate(e.to_string()))?;
        validate_experiment(&substituted).map_err(|e| AuthoringError::Validation(e.to_string()))?;
        Ok(substituted)
    }

    /// Instantiate and serialize to TOON.
    ///
    /// # Errors
    ///
    /// Propagates errors from [`Template::instantiate`] and TOON encoding.
    pub fn instantiate_toon<S: std::hash::BuildHasher>(
        &self,
        overrides: &HashMap<String, String, S>,
    ) -> Result<String, AuthoringError> {
        let experiment = self.instantiate(overrides)?;
        encode_experiment(&experiment)
    }
}

/// Parse `key=value` override strings into a map.
///
/// # Errors
///
/// Returns [`AuthoringError::BadOverride`] for any entry lacking a `=`.
pub fn parse_overrides(sets: &[String]) -> Result<IndexMap<String, String>, AuthoringError> {
    let mut map = IndexMap::new();
    for entry in sets {
        let (k, v) = entry
            .split_once('=')
            .ok_or_else(|| AuthoringError::BadOverride(entry.clone()))?;
        map.insert(k.trim().to_string(), v.to_string());
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn there_are_ten_templates_with_unique_names() {
        let templates = all_templates();
        assert_eq!(templates.len(), 10);
        let mut names: Vec<&str> = templates.iter().map(|t| t.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 10, "template names must be unique");
    }

    #[test]
    fn every_template_validates_with_defaults() {
        for template in all_templates() {
            let empty: HashMap<String, String> = HashMap::new();
            let exp = template.instantiate(&empty).unwrap_or_else(|e| {
                panic!("template '{}' failed to instantiate: {e}", template.name)
            });
            assert!(
                !exp.method.is_empty(),
                "template '{}' must have a method",
                template.name
            );
            assert!(
                exp.steady_state_hypothesis.is_some(),
                "template '{}' must declare a steady-state hypothesis",
                template.name
            );
        }
    }

    #[test]
    fn every_template_round_trips_through_toon() {
        for template in all_templates() {
            let empty: HashMap<String, String> = HashMap::new();
            let toon = template
                .instantiate_toon(&empty)
                .unwrap_or_else(|e| panic!("template '{}' encode failed: {e}", template.name));
            let parsed = parse_experiment(&toon)
                .unwrap_or_else(|e| panic!("template '{}' re-parse failed: {e}", template.name));
            assert!(validate_experiment(&parsed).is_ok());
        }
    }

    #[test]
    fn override_applies_and_unknown_key_is_rejected() {
        let template = find_template("net-latency").unwrap();
        let overrides = HashMap::from([("target".to_string(), "demo-app".to_string())]);
        let exp = template.instantiate(&overrides).unwrap();
        assert!(exp.title.contains("demo-app"));

        let bad = HashMap::from([("nope".to_string(), "x".to_string())]);
        assert!(matches!(
            template.instantiate(&bad),
            Err(AuthoringError::BadOverride(_))
        ));
    }

    #[test]
    fn parse_overrides_splits_key_value() {
        let parsed = parse_overrides(&["target=demo".into(), "delay=200ms".into()]).unwrap();
        assert_eq!(parsed["target"], "demo");
        assert_eq!(parsed["delay"], "200ms");
        assert!(parse_overrides(&["novalue".into()]).is_err());
    }
}
