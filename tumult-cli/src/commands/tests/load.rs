//! Tests for load-related parsing helpers: durations, var args, load
//! overrides, and the k6 metric / JSON summary parsers.

use super::super::*;
use super::helpers::*;

// ── parse_duration_str tests ──────────────────────────────

#[test]
fn parse_duration_seconds() {
    assert!((parse_duration_str("30s") - 30.0).abs() < f64::EPSILON);
}

#[test]
fn parse_duration_minutes() {
    assert!((parse_duration_str("5m") - 300.0).abs() < f64::EPSILON);
}

#[test]
fn parse_duration_hours() {
    assert!((parse_duration_str("2h") - 7200.0).abs() < f64::EPSILON);
}

#[test]
fn parse_duration_bare_number() {
    assert!((parse_duration_str("45") - 45.0).abs() < f64::EPSILON);
}

#[test]
fn parse_duration_invalid_falls_back_to_default() {
    assert!((parse_duration_str("nonsense") - 30.0).abs() < f64::EPSILON);
}

// ── parse_var_args tests ──────────────────────────────────

#[test]
fn parse_var_args_valid() {
    let vars = vec!["HOST=localhost".to_string(), "PORT=8080".to_string()];
    let map = parse_var_args(&vars).unwrap();
    assert_eq!(map.get("HOST").map(String::as_str), Some("localhost"));
    assert_eq!(map.get("PORT").map(String::as_str), Some("8080"));
}

#[test]
fn parse_var_args_empty_is_ok() {
    let map = parse_var_args(&[]).unwrap();
    assert!(map.is_empty());
}

#[test]
fn parse_var_args_missing_equals_is_error() {
    let vars = vec!["NOEQUALSSIGN".to_string()];
    let result = parse_var_args(&vars);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("KEY=VALUE format"));
}

#[test]
fn parse_var_args_value_with_equals() {
    // KEY=VALUE=WITH=EQUALS should split on first = only
    let vars = vec!["URL=http://localhost:8080/path?a=1".to_string()];
    let map = parse_var_args(&vars).unwrap();
    assert_eq!(
        map.get("URL").map(String::as_str),
        Some("http://localhost:8080/path?a=1")
    );
}

// ── build_load_override tests ─────────────────────────────

#[test]
fn build_load_override_none_tool_returns_none() {
    let result = build_load_override(None, None, None, None);
    assert!(result.is_none());
}

#[test]
fn build_load_override_explicit_none_returns_none() {
    let result = build_load_override(Some(LoadToolArg::None), None, None, None);
    assert!(result.is_none());
}

#[test]
fn build_load_override_k6_returns_config() {
    let result = build_load_override(
        Some(LoadToolArg::K6),
        None,
        Some(20),
        Some("60s".to_string()),
    );
    let config = result.unwrap();
    assert_eq!(config.vus, Some(20));
    assert!((config.duration_s.unwrap() - 60.0).abs() < f64::EPSILON);
    assert!(matches!(config.tool, tumult_core::types::LoadTool::K6));
}

#[test]
fn build_load_override_jmeter_returns_config() {
    let result = build_load_override(Some(LoadToolArg::Jmeter), None, None, None);
    let config = result.unwrap();
    assert!(matches!(config.tool, tumult_core::types::LoadTool::Jmeter));
    assert_eq!(config.vus, Some(10)); // default
    assert!((config.duration_s.unwrap() - 30.0).abs() < f64::EPSILON); // default
}

// ── k6 metric parser tests (CLI-TEST-01) ─────────────────────────────

#[test]
fn parse_k6_metric_extracts_stat_value() {
    let output = "iteration_duration...: avg=97.77ms min=55.75ms med=63.81ms max=201.09ms p(90)=67.34ms p(95)=148.01ms";
    assert!((parse_k6_metric(output, "iteration_duration", "avg").unwrap() - 97.77).abs() < 0.001);
    assert!((parse_k6_metric(output, "iteration_duration", "med").unwrap() - 63.81).abs() < 0.001);
    assert!(
        (parse_k6_metric(output, "iteration_duration", "p(95)").unwrap() - 148.01).abs() < 0.001
    );
}

#[test]
fn parse_k6_metric_returns_none_for_missing_metric() {
    let output = "iteration_duration...: avg=10ms";
    assert!(parse_k6_metric(output, "missing_metric", "avg").is_none());
}

#[test]
fn parse_k6_metric_returns_none_for_missing_stat() {
    let output = "iteration_duration...: avg=10ms";
    assert!(parse_k6_metric(output, "iteration_duration", "p(99)").is_none());
}

#[test]
fn parse_k6_counter_extracts_integer_value() {
    let output = "iterations...........: 1025 51.006998/s";
    assert_eq!(parse_k6_counter(output, "iterations"), Some(1025));
}

#[test]
fn parse_k6_counter_returns_none_for_missing_counter() {
    let output = "iterations...........: 1025 51.006998/s";
    assert!(parse_k6_counter(output, "checks_total").is_none());
}

#[test]
fn parse_k6_counter_handles_zero_value() {
    let output = "checks_failed.......: 0 0/s";
    assert_eq!(parse_k6_counter(output, "checks_failed"), Some(0));
}

#[test]
fn parse_k6_rate_extracts_per_second_value() {
    let output = "iterations...........: 300 29.82/s";
    let rate = parse_k6_rate(output, "iterations").unwrap();
    assert!((rate - 29.82).abs() < 0.001);
}

#[test]
fn parse_k6_rate_returns_none_for_missing_counter() {
    let output = "iterations...........: 300 29.82/s";
    assert!(parse_k6_rate(output, "http_reqs").is_none());
}

#[test]
fn parse_k6_rate_handles_integer_rate() {
    let output = "http_reqs...........: 100 10/s";
    let rate = parse_k6_rate(output, "http_reqs").unwrap();
    assert!((rate - 10.0).abs() < 0.001);
}

#[test]
fn parse_k6_metric_multiline_finds_correct_metric() {
    let output = "\
http_req_duration...: avg=12ms p(95)=50ms\n\
iteration_duration..: avg=97ms p(95)=148ms\n\
";
    assert!(
        (parse_k6_metric(output, "iteration_duration", "p(95)").unwrap() - 148.0).abs() < 0.001
    );
    assert!((parse_k6_metric(output, "http_req_duration", "avg").unwrap() - 12.0).abs() < 0.001);
}

// ── k6 JSON summary parser tests (T-9) ───────────────────────────────

#[test]
fn k6_summary_metric_extracts_value() {
    let summary = sample_k6_summary();
    assert!(
        (k6_summary_metric(Some(&summary), "iteration_duration", "p(95)").unwrap() - 148.01).abs()
            < 0.001
    );
    assert!(
        (k6_summary_metric(Some(&summary), "iterations", "rate").unwrap() - 51.006998).abs()
            < 0.001
    );
}

#[test]
fn k6_summary_metric_returns_none_for_missing_metric() {
    let summary = sample_k6_summary();
    assert!(k6_summary_metric(Some(&summary), "http_req_duration", "p(95)").is_none());
    assert!(k6_summary_metric(None, "iterations", "rate").is_none());
}

#[test]
fn k6_summary_count_extracts_value() {
    let summary = sample_k6_summary();
    assert_eq!(k6_summary_count(Some(&summary), "checks_total"), Some(1025));
    assert_eq!(k6_summary_count(Some(&summary), "checks_failed"), Some(5));
    assert!(k6_summary_count(Some(&summary), "missing_counter").is_none());
}

#[test]
fn k6_metric_or_warn_returns_parsed_value() {
    assert!((k6_metric_or_warn(Some(42.5), "some.metric") - 42.5).abs() < 0.001);
}

#[test]
fn k6_metric_or_warn_defaults_to_zero_when_missing() {
    assert_eq!(k6_metric_or_warn(None, "some.metric"), 0.0);
}

#[test]
fn read_k6_summary_returns_none_for_missing_file() {
    let path = std::path::Path::new("/nonexistent/tumult-k6-summary.json");
    assert!(read_k6_summary(path).is_none());
}

#[test]
fn read_k6_summary_returns_none_for_invalid_json() {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    std::io::Write::write_all(&mut file, b"not json").unwrap();
    assert!(read_k6_summary(file.path()).is_none());
}

#[test]
fn read_k6_summary_parses_valid_json() {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    let summary = sample_k6_summary();
    std::io::Write::write_all(&mut file, summary.to_string().as_bytes()).unwrap();
    let parsed = read_k6_summary(file.path()).unwrap();
    assert_eq!(k6_summary_count(Some(&parsed), "checks_total"), Some(1025));
}
