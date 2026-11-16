//! The AWS steps of a run, through the executor the runner calls.
//!
//! A local server stands in for AWS: it keeps what it is asked and gives the
//! answer the test set. The executor, its arguments, the credentials from
//! the environment, the signer and the HTTP client are the real ones. They
//! find the server through `AWS_ENDPOINT_URL_FIS` and `AWS_ENDPOINT_URL_EC2`,
//! the variables the AWS tools read.

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{Method, Uri};
use axum::Router;
use serde_json::json;
use tumult_cloud::CloudExecutor;
use tumult_plugin::native::{NativeArgs, NativeError, NativeExecutor};
use tumult_test_utils::EnvGuard;

/// What the server was asked, and the answer it gives.
#[derive(Clone, Default)]
struct Exchange {
    asked: Arc<Mutex<Vec<(String, String)>>>,
    answer: Arc<Mutex<String>>,
}

async fn keep_and_answer(
    State(exchange): State<Exchange>,
    method: Method,
    uri: Uri,
    body: String,
) -> String {
    let request = format!("{method} {}", uri.path());
    exchange.asked.lock().expect("lock").push((request, body));
    exchange.answer.lock().expect("lock").clone()
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

    /// The requests so far, as `METHOD /path`.
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

fn experiment(status: &str) -> String {
    json!({"experiment": {"id": "EXP-of-this-run", "state": {"status": status}}}).to_string()
}

#[tokio::test]
async fn a_stop_and_a_status_without_an_id_mean_the_experiment_the_run_started() {
    let fis = StandIn::start().await;
    let mut env = EnvGuard::lock();
    aws_env(&mut env, "FIS", &fis.url);
    let no_id = NativeArgs::new();

    // Nothing is started yet: there is no experiment to ask about.
    let error = CloudExecutor
        .execute("aws_fis_experiment_status", &no_id)
        .await
        .expect_err("no experiment yet");
    assert!(
        matches!(&error, NativeError::MissingArgument { argument } if argument == "experiment_id"),
        "{error:?}"
    );
    assert_eq!(fis.asked(), Vec::<String>::new());

    fis.answer(&experiment("initiating"));
    let template = NativeArgs::from([("experiment_template_id".into(), json!("EXT-lab"))]);
    let started = CloudExecutor
        .execute("aws_fis_start_experiment", &template)
        .await
        .expect("start");
    assert_eq!(
        started,
        "started FIS experiment EXP-of-this-run (status: initiating)"
    );

    fis.answer(&experiment("running"));
    let status = CloudExecutor
        .execute("aws_fis_experiment_status", &no_id)
        .await
        .expect("status");
    assert_eq!(
        status,
        "status of FIS experiment EXP-of-this-run (status: running)"
    );
    // A step that names an experiment means that one. The next step without
    // a name still means the one this run started.
    let other = NativeArgs::from([("experiment_id".into(), json!("EXP-of-another-run"))]);
    CloudExecutor
        .execute("aws_fis_experiment_status", &other)
        .await
        .expect("status of the other one");
    CloudExecutor
        .execute("aws_fis_stop_experiment", &no_id)
        .await
        .expect("stop");

    assert_eq!(
        fis.asked(),
        [
            "POST /experiments",
            "GET /experiments/EXP-of-this-run",
            "GET /experiments/EXP-of-another-run",
            "DELETE /experiments/EXP-of-this-run",
        ]
    );
    let start: serde_json::Value = serde_json::from_str(&fis.body(0)).expect("the start is JSON");
    assert_eq!(start["experimentTemplateId"], "EXT-lab");
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
