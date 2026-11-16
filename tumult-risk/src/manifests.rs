//! Kubernetes manifests as scanner input. Only the fields the rules read
//! are kept; unknown kinds and unparseable documents are skipped with a
//! note, never fatal, so one odd file cannot blank the posture page.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_yaml::Value;

use crate::model::{Container, DisruptionBudget, Workload};

/// What a manifest directory yielded.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Manifests {
    pub workloads: Vec<Workload>,
    pub budgets: Vec<DisruptionBudget>,
    /// Files or documents skipped, with why.
    pub notes: Vec<String>,
}

fn string_map(v: Option<&Value>) -> BTreeMap<String, String> {
    v.and_then(Value::as_mapping)
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| Some((k.as_str()?.to_string(), v.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

fn containers(spec: &Value) -> Vec<Container> {
    spec.get("template")
        .and_then(|t| t.get("spec"))
        .and_then(|s| s.get("containers"))
        .and_then(Value::as_sequence)
        .map(|cs| {
            cs.iter()
                .map(|c| Container {
                    name: c
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("(unnamed)")
                        .to_string(),
                    has_limits: c
                        .get("resources")
                        .and_then(|r| r.get("limits"))
                        .and_then(Value::as_mapping)
                        .is_some_and(|m| !m.is_empty()),
                    has_readiness_probe: c.get("readinessProbe").is_some(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parse every document in one YAML string. `source` labels the documents
/// (a file path).
#[must_use]
pub fn parse(yaml: &str, source: &str) -> Manifests {
    let mut out = Manifests::default();
    for (i, doc) in serde_yaml::Deserializer::from_str(yaml).enumerate() {
        let value = match Value::deserialize(doc) {
            Ok(v) => v,
            Err(e) => {
                // serde_yaml's multi-document iterator does not advance past a
                // document that failed to parse, so `continue` would yield the
                // same error forever. Note it and stop reading this source.
                out.notes.push(format!("{source}#{i}: {e}"));
                break;
            }
        };
        let Some(kind) = value.get("kind").and_then(Value::as_str) else {
            continue;
        };
        let metadata = value.get("metadata");
        let name = metadata
            .and_then(|m| m.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("(unnamed)")
            .to_string();
        let namespace = metadata
            .and_then(|m| m.get("namespace"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let spec = value.get("spec").cloned().unwrap_or(Value::Null);
        match kind {
            "Deployment" | "StatefulSet" | "DaemonSet" => {
                let replicas = if kind == "DaemonSet" {
                    None
                } else {
                    Some(spec.get("replicas").and_then(Value::as_i64).unwrap_or(1))
                };
                let mut labels = string_map(metadata.and_then(|m| m.get("labels")));
                for (k, v) in string_map(
                    spec.get("template")
                        .and_then(|t| t.get("metadata"))
                        .and_then(|m| m.get("labels")),
                ) {
                    labels.entry(k).or_insert(v);
                }
                out.workloads.push(Workload {
                    name,
                    namespace,
                    kind: kind.to_string(),
                    replicas,
                    labels,
                    containers: containers(&spec),
                    source: source.to_string(),
                });
            }
            "PodDisruptionBudget" => out.budgets.push(DisruptionBudget {
                name,
                namespace,
                selector: string_map(spec.get("selector").and_then(|s| s.get("matchLabels"))),
            }),
            _ => {}
        }
    }
    out
}

fn yaml_files(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            yaml_files(&path, out)?;
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("yaml" | "yml")
        ) {
            out.push(path);
        }
    }
    Ok(())
}

/// Parse every `*.yaml` / `*.yml` under `dir`, recursively, in path order.
///
/// # Errors
/// Returns the I/O error when the directory cannot be listed. Unreadable
/// files inside it become notes, not errors.
pub fn load_dir(dir: &Path) -> std::io::Result<Manifests> {
    let mut files = Vec::new();
    yaml_files(dir, &mut files)?;
    files.sort();
    let mut out = Manifests::default();
    for path in files {
        let label = path
            .strip_prefix(dir)
            .unwrap_or(&path)
            .display()
            .to_string();
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let parsed = parse(&text, &label);
                out.workloads.extend(parsed.workloads);
                out.budgets.extend(parsed.budgets);
                out.notes.extend(parsed.notes);
            }
            Err(e) => out.notes.push(format!("{label}: {e}")),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEPLOYMENT: &str = "
apiVersion: apps/v1
kind: Deployment
metadata:
  name: api
  namespace: prod
  labels:
    app: api
spec:
  replicas: 1
  template:
    metadata:
      labels:
        app: api
        tier: web
    spec:
      containers:
        - name: api
          image: api:1
          resources:
            limits:
              cpu: 500m
          readinessProbe:
            httpGet: {path: /healthz, port: 8080}
        - name: sidecar
          image: side:1
---
apiVersion: policy/v1
kind: PodDisruptionBudget
metadata:
  name: api-pdb
  namespace: prod
spec:
  minAvailable: 1
  selector:
    matchLabels:
      app: api
---
apiVersion: v1
kind: Service
metadata:
  name: api
";

    #[test]
    fn parses_workloads_budgets_and_skips_other_kinds() {
        let m = parse(DEPLOYMENT, "api.yaml");
        assert_eq!(m.workloads.len(), 1);
        assert_eq!(m.budgets.len(), 1);
        assert!(m.notes.is_empty(), "{:?}", m.notes);
        let w = &m.workloads[0];
        assert_eq!(w.kind, "Deployment");
        assert_eq!(w.replicas, Some(1));
        assert_eq!(w.namespace.as_deref(), Some("prod"));
        assert_eq!(w.labels.get("tier").map(String::as_str), Some("web"));
        assert_eq!(w.containers.len(), 2);
        assert!(w.containers[0].has_limits && w.containers[0].has_readiness_probe);
        assert!(!w.containers[1].has_limits && !w.containers[1].has_readiness_probe);
        assert!(m.budgets[0].covers(w));
        assert_eq!(w.source, "api.yaml");
    }

    #[test]
    fn daemonsets_have_no_replica_count_and_missing_replicas_default_to_one() {
        let m = parse(
            "kind: DaemonSet\nmetadata: {name: agent}\nspec: {template: {spec: {containers: []}}}\n---\nkind: StatefulSet\nmetadata: {name: db}\nspec: {}\n",
            "x.yaml",
        );
        assert_eq!(m.workloads[0].replicas, None);
        assert_eq!(m.workloads[1].replicas, Some(1));
    }

    #[test]
    fn bad_documents_become_notes() {
        let m = parse(
            "kind: Deployment\nmetadata: {name: ok}\n---\n: : bad\n",
            "bad.yaml",
        );
        assert_eq!(m.workloads.len(), 1);
        assert_eq!(m.notes.len(), 1);
        assert!(m.notes[0].starts_with("bad.yaml#1"), "{}", m.notes[0]);
    }

    #[test]
    fn load_dir_walks_recursively_in_order() {
        let dir =
            std::env::temp_dir().join(format!("tumult-risk-manifests-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("b")).unwrap();
        std::fs::write(
            dir.join("b/db.yml"),
            "kind: StatefulSet\nmetadata: {name: db}\nspec: {replicas: 3}\n",
        )
        .unwrap();
        std::fs::write(dir.join("a.yaml"), DEPLOYMENT).unwrap();
        std::fs::write(dir.join("notes.txt"), "ignored").unwrap();
        let m = load_dir(&dir).unwrap();
        let names: Vec<&str> = m.workloads.iter().map(|w| w.name.as_str()).collect();
        assert_eq!(names, vec!["api", "db"]);
        assert_eq!(m.workloads[1].source, "b/db.yml");
        assert_eq!(m.budgets.len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_dir_is_an_error() {
        assert!(load_dir(Path::new("/nonexistent/tumult-risk")).is_err());
    }
}
