//! The AWS steps of a run, through the executor the runner calls.
//!
//! A local server stands in for AWS: it keeps what it is asked and gives the
//! answers the test set. The executor, its arguments, the credentials from
//! the environment, the signer and the HTTP client are the real ones. They
//! find the server through `AWS_ENDPOINT_URL_FIS` and `AWS_ENDPOINT_URL_EC2`,
//! the variables the AWS tools read.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{Method, StatusCode, Uri};
use axum::Router;
use serde_json::{json, Value};
use tumult_cloud::CloudExecutor;
use tumult_plugin::native::{NativeArgs, NativeError, NativeExecutor};
use tumult_test_utils::EnvGuard;

/// What the server was asked, and the answers it gives.
#[derive(Clone, Default)]
struct Exchange {
    asked: Arc<Mutex<Vec<(String, String)>>>,
    /// Given to the next requests, one each, in order.
    queued: Arc<Mutex<VecDeque<String>>>,
    /// Given when nothing is queued.
    answer: Arc<Mutex<String>>,
    /// The request that gets HTTP 500, whatever is queued.
    refused: Arc<Mutex<Option<String>>>,
}

async fn keep_and_answer(
    State(exchange): State<Exchange>,
    method: Method,
    uri: Uri,
    body: String,
) -> (StatusCode, String) {
    let target = uri
        .path_and_query()
        .map_or(uri.path(), |target| target.as_str());
    let request = format!("{method} {target}");
    let refused = exchange.refused.lock().expect("lock").as_ref() == Some(&request);
    exchange.asked.lock().expect("lock").push((request, body));
    if refused {
        let message = json!({"message": "the lab refuses this one"}).to_string();
        return (StatusCode::INTERNAL_SERVER_ERROR, message);
    }
    if let Some(next) = exchange.queued.lock().expect("lock").pop_front() {
        return (StatusCode::OK, next);
    }
    (
        StatusCode::OK,
        exchange.answer.lock().expect("lock").clone(),
    )
}

/// A local server in the place of one AWS service.
struct StandIn {
    url: String,
    exchange: Exchange,
}

impl StandIn {
    async fn start() -> Self {
        let exchange = Exchange::default();
        let app = Router::new()
            .fallback(keep_and_answer)
            .with_state(exchange.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let url = format!("http://{}", listener.local_addr().expect("address"));
        tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        Self { url, exchange }
    }

    fn answer(&self, body: &str) {
        *self.exchange.answer.lock().expect("lock") = body.to_string();
    }

    /// Answers for the next requests, one each, in this order.
    fn answer_in_turn(&self, bodies: &[String]) {
        let mut queued = self.exchange.queued.lock().expect("lock");
        queued.extend(bodies.iter().cloned());
    }

    /// The request, as `METHOD /path?query`, that gets HTTP 500.
    fn refuse(&self, request: &str) {
        *self.exchange.refused.lock().expect("lock") = Some(request.to_string());
    }

    /// The requests so far, as `METHOD /path?query`.
    fn asked(&self) -> Vec<String> {
        let asked = self.exchange.asked.lock().expect("lock");
        asked.iter().map(|(request, _)| request.clone()).collect()
    }

    /// The body of request number `index`.
    fn body(&self, index: usize) -> String {
        self.exchange.asked.lock().expect("lock")[index].1.clone()
    }
}

/// Credentials of no account, a region no AWS host has, and the stand-in as
/// the endpoint of `service`.
fn aws_env(env: &mut EnvGuard, service: &str, url: &str) {
    env.set("AWS_ACCESS_KEY_ID", "lab-key")
        .set("AWS_SECRET_ACCESS_KEY", "lab-secret")
        .remove("AWS_SESSION_TOKEN")
        .set("AWS_REGION", "lab-1")
        .set(format!("AWS_ENDPOINT_URL_{service}"), url);
}

/// The answer FIS gives about one experiment.
fn experiment(id: &str, status: &str) -> String {
    json!({"experiment": {"id": id, "state": {"status": status}}}).to_string()
}

/// One experiment as a list of experiments shows it.
fn listed(id: &str, template: &str, status: &str, created: f64) -> Value {
    json!({
        "id": id,
        "experimentTemplateId": template,
        "state": {"status": status},
        "creationTime": created,
    })
}

/// One page of a list of experiments.
fn page(experiments: &[Value], next_token: Option<&str>) -> String {
    let mut page = json!({"experiments": experiments});
    if let Some(token) = next_token {
        page["nextToken"] = json!(token);
    }
    page.to_string()
}

fn args(pairs: &[(&str, &str)]) -> NativeArgs {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), json!(value)))
        .collect()
}

#[tokio::test]
async fn a_stop_and_a_status_by_template_mean_the_experiments_fis_lists_for_it() {
    let fis = StandIn::start().await;
    let mut env = EnvGuard::lock();
    aws_env(&mut env, "FIS", &fis.url);
    let template = args(&[("experiment_template_id", "EXT-lab")]);
    let running = [listed(
        "EXP-of-this-run",
        "EXT-lab",
        "running",
        1_700_000_000.0,
    )];
    fis.answer_in_turn(&[
        experiment("EXP-of-this-run", "initiating"),
        page(&running, None),
        experiment("EXP-of-this-run", "running"),
        experiment("EXP-of-another-run", "completed"),
        page(&running, None),
        experiment("EXP-of-this-run", "stopping"),
    ]);

    let started = CloudExecutor
        .execute("aws_fis_start_experiment", &template)
        .await
        .expect("start");
    let status = CloudExecutor
        .execute("aws_fis_experiment_status", &template)
        .await
        .expect("status");
    // A step that names an experiment means that one, whatever template it
    // also names.
    let both = args(&[
        ("experiment_id", "EXP-of-another-run"),
        ("experiment_template_id", "EXT-lab"),
    ]);
    CloudExecutor
        .execute("aws_fis_experiment_status", &both)
        .await
        .expect("status of the other one");
    let stopped = CloudExecutor
        .execute("aws_fis_stop_experiment", &template)
        .await
        .expect("stop");

    assert_eq!(
        started,
        "started FIS experiment EXP-of-this-run (status: initiating)"
    );
    assert_eq!(
        status,
        "status of FIS experiment EXP-of-this-run (status: running)"
    );
    assert_eq!(
        stopped,
        "stopped FIS experiment EXP-of-this-run (status: stopping)"
    );
    assert_eq!(
        fis.asked(),
        [
            "POST /experiments",
            "GET /experiments?experimentTemplateId=EXT-lab&maxResults=100",
            "GET /experiments/EXP-of-this-run",
            "GET /experiments/EXP-of-another-run",
            "GET /experiments?experimentTemplateId=EXT-lab&maxResults=100",
            "DELETE /experiments/EXP-of-this-run",
        ]
    );
    let start: Value = serde_json::from_str(&fis.body(0)).expect("the start is JSON");
    assert_eq!(start["experimentTemplateId"], "EXT-lab");
}

#[tokio::test]
async fn a_stop_by_template_stops_only_the_experiment_of_that_template() {
    let fis = StandIn::start().await;
    let mut env = EnvGuard::lock();
    aws_env(&mut env, "FIS", &fis.url);
    fis.answer_in_turn(&[
        experiment("EXP-a", "initiating"),
        experiment("EXP-b", "initiating"),
        page(
            &[listed("EXP-a", "EXT-a", "running", 1_700_000_000.0)],
            None,
        ),
        experiment("EXP-a", "stopping"),
    ]);
    let template_a = args(&[("experiment_template_id", "EXT-a")]);
    let template_b = args(&[("experiment_template_id", "EXT-b")]);

    // Two runs in one process, one after the other.
    CloudExecutor
        .execute("aws_fis_start_experiment", &template_a)
        .await
        .expect("start a");
    CloudExecutor
        .execute("aws_fis_start_experiment", &template_b)
        .await
        .expect("start b");
    let stopped = CloudExecutor
        .execute("aws_fis_stop_experiment", &template_a)
        .await
        .expect("stop a");

    assert_eq!(
        fis.asked(),
        [
            "POST /experiments",
            "POST /experiments",
            "GET /experiments?experimentTemplateId=EXT-a&maxResults=100",
            "DELETE /experiments/EXP-a",
        ]
    );
    assert_eq!(stopped, "stopped FIS experiment EXP-a (status: stopping)");
}

#[tokio::test]
async fn the_rollback_in_a_new_process_stops_the_running_experiment_of_its_template() {
    let fis = StandIn::start().await;
    let mut env = EnvGuard::lock();
    aws_env(&mut env, "FIS", &fis.url);
    fis.answer_in_turn(&[
        page(
            &[
                listed("EXP-done", "EXT-lab", "completed", 1_600_000_000.0),
                listed("EXP-left-running", "EXT-lab", "running", 1_700_000_000.0),
            ],
            None,
        ),
        experiment("EXP-left-running", "stopping"),
    ]);

    // Nothing was started by this process: the run that started the
    // experiment ended with the process before it.
    let stopped = CloudExecutor
        .execute(
            "aws_fis_stop_experiment",
            &args(&[("experiment_template_id", "EXT-lab")]),
        )
        .await
        .expect("stop");

    assert_eq!(
        stopped,
        "stopped FIS experiment EXP-left-running (status: stopping)"
    );
    assert_eq!(
        fis.asked(),
        [
            "GET /experiments?experimentTemplateId=EXT-lab&maxResults=100",
            "DELETE /experiments/EXP-left-running",
        ]
    );
}

#[tokio::test]
async fn a_stop_by_template_with_nothing_running_succeeds_and_stops_nothing() {
    let fis = StandIn::start().await;
    let mut env = EnvGuard::lock();
    aws_env(&mut env, "FIS", &fis.url);
    fis.answer(&page(
        &[
            listed("EXP-done", "EXT-lab", "completed", 1_600_000_000.0),
            listed("EXP-stopped", "EXT-lab", "stopped", 1_700_000_000.0),
            listed("EXP-stopping", "EXT-lab", "stopping", 1_650_000_000.0),
        ],
        None,
    ));
    let template = args(&[("experiment_template_id", "EXT-lab")]);

    // A rollback is safe to run twice.
    let first = CloudExecutor
        .execute("aws_fis_stop_experiment", &template)
        .await
        .expect("first stop");
    let second = CloudExecutor
        .execute("aws_fis_stop_experiment", &template)
        .await
        .expect("second stop");

    assert!(first.contains("nothing to stop"), "{first}");
    assert_eq!(second, first);
    assert_eq!(
        fis.asked(),
        [
            "GET /experiments?experimentTemplateId=EXT-lab&maxResults=100",
            "GET /experiments?experimentTemplateId=EXT-lab&maxResults=100",
        ]
    );
}

#[tokio::test]
async fn a_stop_that_fis_refuses_does_not_keep_the_other_experiments_running() {
    let fis = StandIn::start().await;
    let mut env = EnvGuard::lock();
    aws_env(&mut env, "FIS", &fis.url);
    fis.refuse("DELETE /experiments/EXP-2");
    fis.answer_in_turn(&[
        page(
            &[
                listed("EXP-1", "EXT-lab", "pending", 1_700_000_001.0),
                listed("EXP-2", "EXT-lab", "running", 1_700_000_002.0),
                listed("EXP-3", "EXT-lab", "initiating", 1_700_000_003.0),
            ],
            None,
        ),
        experiment("EXP-1", "stopping"),
        experiment("EXP-3", "stopping"),
    ]);

    let error = CloudExecutor
        .execute(
            "aws_fis_stop_experiment",
            &args(&[("experiment_template_id", "EXT-lab")]),
        )
        .await
        .expect_err("one stop was refused")
        .to_string();

    assert_eq!(
        fis.asked(),
        [
            "GET /experiments?experimentTemplateId=EXT-lab&maxResults=100",
            "DELETE /experiments/EXP-1",
            "DELETE /experiments/EXP-2",
            "DELETE /experiments/EXP-3",
        ]
    );
    assert!(
        error.contains("could not stop FIS experiment EXP-2: "),
        "{error}"
    );
    assert!(error.contains("HTTP 500"), "{error}");
    assert!(
        error.contains("stopped FIS experiment EXP-1 (status: stopping)"),
        "{error}"
    );
    assert!(
        error.contains("stopped FIS experiment EXP-3 (status: stopping)"),
        "{error}"
    );
}

#[tokio::test]
async fn a_status_by_template_is_the_status_of_its_newest_experiment() {
    let fis = StandIn::start().await;
    let mut env = EnvGuard::lock();
    aws_env(&mut env, "FIS", &fis.url);
    // Listed neither newest first nor newest last.
    fis.answer_in_turn(&[
        page(
            &[
                listed("EXP-middle", "EXT-lab", "stopped", 1_650_000_000.0),
                listed("EXP-newest", "EXT-lab", "completed", 1_700_000_000.5),
                listed("EXP-oldest", "EXT-lab", "running", 1_600_000_000.0),
            ],
            None,
        ),
        experiment("EXP-newest", "completed"),
    ]);

    let status = CloudExecutor
        .execute(
            "aws_fis_experiment_status",
            &args(&[("experiment_template_id", "EXT-lab")]),
        )
        .await
        .expect("status");

    assert_eq!(
        status,
        "status of FIS experiment EXP-newest (status: completed)"
    );
    assert_eq!(
        fis.asked(),
        [
            "GET /experiments?experimentTemplateId=EXT-lab&maxResults=100",
            "GET /experiments/EXP-newest",
        ]
    );
}

#[tokio::test]
async fn a_status_by_template_without_experiments_is_an_error() {
    let fis = StandIn::start().await;
    let mut env = EnvGuard::lock();
    aws_env(&mut env, "FIS", &fis.url);
    fis.answer(&page(&[], None));

    let error = CloudExecutor
        .execute(
            "aws_fis_experiment_status",
            &args(&[("experiment_template_id", "EXT-lab")]),
        )
        .await
        .expect_err("a probe that finds no experiment has not measured");

    assert!(error.to_string().contains("EXT-lab"), "{error}");
    assert_eq!(
        fis.asked(),
        ["GET /experiments?experimentTemplateId=EXT-lab&maxResults=100"]
    );
}

#[tokio::test]
async fn an_experiment_on_the_second_page_is_found() {
    let fis = StandIn::start().await;
    let mut env = EnvGuard::lock();
    aws_env(&mut env, "FIS", &fis.url);
    fis.answer_in_turn(&[
        page(
            &[listed("EXP-done", "EXT-lab", "completed", 1_600_000_000.0)],
            Some("page/2+="),
        ),
        page(
            &[listed(
                "EXP-on-page-2",
                "EXT-lab",
                "running",
                1_700_000_000.0,
            )],
            None,
        ),
        experiment("EXP-on-page-2", "stopping"),
    ]);

    let stopped = CloudExecutor
        .execute(
            "aws_fis_stop_experiment",
            &args(&[("experiment_template_id", "EXT-lab")]),
        )
        .await
        .expect("stop");

    assert_eq!(
        stopped,
        "stopped FIS experiment EXP-on-page-2 (status: stopping)"
    );
    assert_eq!(
        fis.asked(),
        [
            "GET /experiments?experimentTemplateId=EXT-lab&maxResults=100",
            "GET /experiments?experimentTemplateId=EXT-lab&maxResults=100&nextToken=page%2F2%2B%3D",
            "DELETE /experiments/EXP-on-page-2",
        ]
    );
}

#[tokio::test]
async fn a_stop_or_a_status_that_names_neither_is_a_missing_argument_before_any_request() {
    let fis = StandIn::start().await;
    let mut env = EnvGuard::lock();
    aws_env(&mut env, "FIS", &fis.url);
    let region_only = args(&[("region", "lab-1")]);

    for function in ["aws_fis_stop_experiment", "aws_fis_experiment_status"] {
        let error = CloudExecutor
            .execute(function, &region_only)
            .await
            .expect_err("neither argument");
        assert!(
            matches!(
                &error,
                NativeError::MissingArgument { argument }
                    if argument == "experiment_id or experiment_template_id"
            ),
            "{function}: {error:?}"
        );
    }
    assert_eq!(fis.asked(), Vec::<String>::new());
}

#[tokio::test]
async fn an_instance_step_goes_to_the_endpoint_set_for_ec2() {
    let ec2 = StandIn::start().await;
    let mut env = EnvGuard::lock();
    aws_env(&mut env, "EC2", &ec2.url);
    ec2.answer(
        "<StopInstancesResponse><instancesSet><item><instanceId>i-0lab</instanceId>\
         <currentState><code>64</code><name>stopping</name></currentState></item>\
         </instancesSet></StopInstancesResponse>",
    );
    let instance = NativeArgs::from([("instance_id".into(), json!("i-0lab"))]);

    let stopped = CloudExecutor
        .execute("aws_ec2_stop_instance", &instance)
        .await
        .expect("stop");

    assert!(stopped.contains("stopping"), "{stopped}");
    assert_eq!(ec2.asked(), ["POST /"]);
    let body = ec2.body(0);
    assert!(body.contains("Action=StopInstances"), "{body}");
    assert!(body.contains("InstanceId.1=i-0lab"), "{body}");
}
