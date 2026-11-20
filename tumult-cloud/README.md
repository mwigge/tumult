# tumult-cloud

Thin connectors to cloud providers' **own** fault / chaos APIs.

This crate does **not** reimplement cloud faults. It is a small, thin bridge
that drives each provider's managed fault service (where one exists) and adds a
couple of direct high-signal faults that do not need a preconfigured template.
The goal is not to chase AWS FIS's breadth — it is to *connect* Tumult
experiments to it.

Registered as the `tumult-cloud` native plugin, so every function below is
addressable from an experiment as:

```toon
provider:
  type: native
  plugin: tumult-cloud
  function: aws_fis_start_experiment
  arguments:
    experiment_template_id: EXTabc123EXAMPLE
    region: us-east-1
```

## Functions

| Function | Provider API | HTTP | Required arguments | Optional |
| --- | --- | --- | --- | --- |
| `aws_fis_start_experiment` | FIS `StartExperiment` | `POST /experiments` | `experiment_template_id` | `region` |
| `aws_fis_stop_experiment` | FIS `StopExperiment`, by template also `ListExperiments` | `DELETE /experiments/{id}` | `experiment_id` or `experiment_template_id` | `region` |
| `aws_fis_experiment_status` | FIS `GetExperiment`, by template also `ListExperiments` | `GET /experiments/{id}` | `experiment_id` or `experiment_template_id` | `region` |
| `aws_ec2_stop_instance` | EC2 `StopInstances` | `POST /` (Query) | `instance_id` | `region` |
| `aws_ec2_terminate_instance` | EC2 `TerminateInstances` | `POST /` (Query) | `instance_id` | `region` |
| `azure_chaos_start` | Chaos Studio `Experiments.Start` | `POST …/start` | `subscription`, `resource_group`, `experiment_id` | — |
| `azure_chaos_cancel` | Chaos Studio `Experiments.Cancel` | `POST …/cancel` | `subscription`, `resource_group`, `experiment_id` | — |
| `azure_chaos_status` | Chaos Studio `Experiments.Get` | `GET …` | `subscription`, `resource_group`, `experiment_id` | — |
| `gcp_compute_stop_instance` | Compute `instances.stop` | `POST …/stop` | `project`, `zone`, `instance` | — |

`region` defaults to `AWS_REGION` / `AWS_DEFAULT_REGION` when omitted.

A stop or status step names `experiment_id`, or else
`experiment_template_id`. FIS gives an experiment its id when it starts,
so an experiment file names the template. Tumult then asks FIS for the
experiments of that template (`ListExperiments`):

- A stop stops every experiment of the template that is `pending`,
  `initiating` or `running`, also one that someone else started. When none
  is, it succeeds and sends no stop, so a rollback is safe to run twice.
  When FIS refuses one stop, the others are still stopped, and the step
  fails and names the one that was refused.
- A status is the status of the newest experiment of the template, in any
  state. A template with no experiment is an error.

This works the same in any process, also in the rollback of a run that a
restarted daemon finishes. A stop or status by template needs
`fis:ListExperiments`. Tumult reads at most 20 pages of 100 experiments
for one template. The query and the answer of `ListExperiments` that
Tumult uses (`experiments[].id`, `experimentTemplateId`, `state.status`,
`creationTime`, `nextToken`) follow the API reference as read in
November 2026. They are not yet run against AWS.

## Provider endpoints (verified July 2026)

- **AWS FIS** — `https://fis.<region>.amazonaws.com`, service `fis`, API date
  `2020-12-01`. REST-JSON protocol.
  - [StartExperiment](https://docs.aws.amazon.com/fis/latest/APIReference/API_StartExperiment.html)
    `POST /experiments`, body `{ clientToken, experimentTemplateId }`.
  - [StopExperiment](https://docs.aws.amazon.com/fis/latest/APIReference/API_StopExperiment.html)
    `DELETE /experiments/{id}`.
  - [GetExperiment](https://docs.aws.amazon.com/fis/latest/APIReference/API_GetExperiment.html)
    `GET /experiments/{id}`.
  - [ListExperiments](https://docs.aws.amazon.com/fis/latest/APIReference/API_ListExperiments.html)
    `GET /experiments?experimentTemplateId={id}&maxResults=100`, then
    `&nextToken={token}`. Read from the reference in November 2026, not run
    against AWS; see above.
- **AWS EC2** — `https://ec2.<region>.amazonaws.com`, service `ec2`, Query
  protocol, `Version=2016-11-15`. `Action=StopInstances` /
  `Action=TerminateInstances`, `InstanceId.1=<id>`.
- **Azure Chaos Studio** — `https://management.azure.com`, resource provider
  `Microsoft.Chaos`, `api-version=2024-01-01`.
  [Experiments – Start / Cancel / Get](https://learn.microsoft.com/en-us/rest/api/chaosstudio/experiments/cancel?view=rest-chaosstudio-2025-01-01).
- **GCP Compute** — `https://compute.googleapis.com`,
  [`instances.stop`](https://cloud.google.com/compute/docs/reference/rest/v1/instances/stop)
  `POST /compute/v1/projects/{project}/zones/{zone}/instances/{instance}/stop`.

## Credentials and permissions

No secret is ever hardcoded. Every connector resolves credentials from the
standard environment chain and **fails fast** — before any network call — with
a message naming the exact missing variable.

| Provider | Environment variable | How to obtain | IAM / RBAC needed |
| --- | --- | --- | --- |
| AWS | `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, optional `AWS_SESSION_TOKEN` | standard AWS chain (env / instance profile) | `fis:StartExperiment`, `fis:StopExperiment`, `fis:GetExperiment`, `fis:ListExperiments` (a stop or status by template); `ec2:StopInstances`, `ec2:TerminateInstances` |
| Azure | `AZURE_ACCESS_TOKEN` | `az account get-access-token --resource https://management.azure.com` or a managed-identity token | `Microsoft.Chaos/experiments/start/action`, `.../cancel/action`, `.../read` |
| GCP | `GOOGLE_OAUTH_ACCESS_TOKEN` (or `CLOUDSDK_AUTH_ACCESS_TOKEN`) | `gcloud auth print-access-token` | `compute.instances.stop` |

AWS requests are signed with a hand-rolled **Signature Version 4** signer
(`src/sigv4.rs`). Azure and GCP use a `Bearer` token.

`AWS_ENDPOINT_URL_FIS` and `AWS_ENDPOINT_URL_EC2` send the AWS steps to
another endpoint, for example a VPC interface endpoint. They are the same
variables the AWS command line reads. When they are not set, the endpoint is
the public one of the region.

## GCP has no managed chaos service

This is deliberate, not an omission. Google does not offer a first-party
equivalent of AWS FIS or Azure Chaos Studio; its own guidance points users at
third-party tools (Chaos Toolkit, Gremlin, Litmus). We therefore expose only a
single direct Compute Engine fault (`gcp_compute_stop_instance`) and do **not**
pretend a managed GCP chaos API exists.

## SDK vs thin HTTP — the decision

We use a plain `reqwest` client plus a ~120-line SigV4 signer instead of the
`aws-sdk-fis` / `aws-sdk-ec2` (or `aws-sigv4`) crates.

- The AWS SDK crates pull in the `aws-smithy-*` runtime, `aws-config`, and a
  large transitive tree — heavy for four endpoints, and at odds with Tumult's
  single-binary ethos.
- The surface we need is tiny and stable: four FIS REST-JSON calls, two EC2
  Query calls, and a handful of ARM / Compute bearer-token calls.
- A hand-rolled signer stays correct because it is pinned to the canonical AWS
  SigV4 `get-vanilla` test-suite vector (`src/sigv4.rs` tests), so a regression
  in the signing math fails a unit test rather than silently producing bad
  signatures.

Dependencies added: `sha2`, `hmac`, `hex` (pure-Rust crypto) plus the
already-present `reqwest`, `serde`, `chrono`. No `aws-sdk-*`.

## What is proven here vs what needs real cloud credentials

**Hermetically proven** (mocked-HTTP tests in `tests/hermetic.rs`, plus unit
tests — no cloud account touched):

- correct HTTP method, path, and query for every function;
- presence of the SigV4 `Authorization` header with the correct
  `region/service/aws4_request` scope (AWS), and the `Bearer` token
  (Azure / GCP);
- correct request bodies (FIS JSON `experimentTemplateId` + `clientToken`;
  EC2 form `Action=…&InstanceId.1=…`);
- success responses parse into typed one-line outcomes;
- error mapping: `403` → `Auth`, `404` → `NotFound`, `429` / `ThrottlingException`
  → `Throttled`, else → `Api` — never a panic;
- credential absence fails fast naming the exact missing variable, with **no**
  network call;
- a stop or status by template against a local stand-in for FIS
  (`tests/aws_steps.rs`): the list query and its pages, what is stopped and
  which status is read. The stand-in answers in the shape Tumult reads, so
  this does not prove that FIS answers in it;
- SigV4 correctness pinned to the AWS test-suite `get-vanilla` vector.

**Needs real cloud credentials** (not exercised in CI):

- an end-to-end round trip against live AWS / Azure / GCP endpoints;
- that FIS answers `ListExperiments` in the shape Tumult reads;
- that a specific IAM policy / RBAC role actually grants the call;
- provider-side side effects (an experiment really starts, an instance really
  stops).

Like the Kubernetes connector, `tumult-cloud` is **exempt from the docker-demo
repeated-run gate** — the demo has no cloud account. Use
`scripts/cloud-smoke.sh` (documented, read-only-ish) to exercise one real call
once you have credentials.
