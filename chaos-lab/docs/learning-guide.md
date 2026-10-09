# Learn from a fault, not just a red result

The lab teaches a repeatable reasoning process: **forecast → baseline → fault → recover → reflect**. Start without an AI key. Add the tutor when you want help interpreting the evidence, and ask it to distinguish what was measured from what it inferred.

## 1. Forecast

Choose one experiment and write down your prediction before running it:

- Will responses become slower, fail, or both?
- Will the API still satisfy the displayed hypothesis?
- What should happen after the fault is removed?
- Which observation would contradict your prediction?

The shared hypothesis is that every sampled request succeeds and measured p95 latency stays at or below **200 ms**. A useful hypothesis combines a user-visible outcome with a measurable threshold. “The server stays up” alone misses slow or failed requests.

## 2. Establish a baseline

Run the experiment and inspect its baseline. The lab sends eight sequential requests to the teaching API and records status, duration, and dependency information. The baseline must be healthy before the fault comparison makes sense.

If your baseline is already slow or failing, the result is inconclusive. Investigate host load and sandbox readiness, then repeat. Do not attribute a pre-existing problem to the injected fault.

## 3. Change one behavior

The real Tumult executor runs a fixed definition. Its activity asks the teaching API to enable one fault, then samples that API while the behavior is active. The lab never turns tutor text into an executable command.

| Experiment        | Expected observation, not a fabricated result                               | Question to investigate                                                               |
| ----------------- | --------------------------------------------------------------------------- | ------------------------------------------------------------------------------------- |
| Slow responses    | Latency increases by approximately 400 ms while responses can still succeed | Why is a successful status insufficient for a good user experience?                   |
| HTTP failures     | HTTP 503 responses may arrive quickly                                       | How could a latency-only dashboard miss this outage?                                  |
| Unavailable cache | A simulated cache failure causes the API request to fail                    | Should this endpoint serve an uncached result, a stale value, or an explicit failure? |
| Database timeout  | Roughly 600 ms elapses before a 503 response                                | Where should the request budget be spent, and which timeout should expire first?      |

Actual values come from your run. Host scheduling, client overhead, and load affect timing. Cache and database dependencies are represented by application code; they are not separate database services in this lab. A 400 ms application sleep is not evidence of kernel packet delay or retransmission behavior.

Only one experiment runs at a time. The fault has a bounded lease, normal cleanup resets it, and **Stop experiment** requests cancellation and cleanup. If a process fails, the target stops applying the fault when its lease expires. No recorded recovery samples means recovery has not been demonstrated by that run.

## 4. Recover and compare

Compare the three phases:

- **Baseline:** what the API did before the fault.
- **Fault active:** what it did while the injected behavior was present.
- **Recovery:** what it did after cleanup.

For each phase, examine both response time and errors. A fast error is still an error. A slow successful response can still violate the hypothesis. Recovery returning to baseline is evidence that the temporary change was reversed in this run; it does not prove every future recovery will succeed.

### Read a small sample honestly

Each phase currently contains **eight requests**. p95 uses the nearest-rank calculation: `ceil(8 × 0.95) = 8`. Therefore, the displayed p95 is the **largest observed latency in that phase**. The mean and raw samples are available in the JSON evidence.

Eight sequential requests are enough to make a controlled mechanism visible. They do not establish a statistically reliable production p95, an availability SLO, load capacity, or independence between repeated runs. One failure is 12.5% of this tiny sample, not a claim that production availability is 87.5%. A repeated result supports a hypothesis about this sandbox under these conditions; confidence in a real service needs a representative workload and longer observation.

### Interpret the two results

There are two distinct records:

1. **Tumult execution journal:** describes the generated experiment's activities, rollback, and steady-state checks. Completion can mean the executor ran its method and the post-fault state recovered successfully.
2. **Lab verdict:** compares the measured baseline, fault-window, and recovery phases. `deviated` means the objective broke during injection while baseline and recovery held; `held` means those samples satisfied the objective; `inconclusive` means the intended comparison was not established.

A completed native journal alongside a deviated lab verdict is consistent: an experiment can execute correctly and expose a weakness. Export both records and the definition to see which question each answers. Consult the [Tumult experiment format](https://tumult.rs/guides/experiment-format.html) for the native fields.

## 5. Reflect, then choose the next question

Record a short conclusion that separates observation from inference:

> During the slow-response run, sampled latency exceeded 200 ms while responses succeeded. The recovery samples returned below the threshold. This supports the conclusion that the injected application delay violated this sandbox's latency objective. It does not establish how a real network delay would affect retries or concurrent requests.

Use your own exported numbers in that statement. Useful tutor questions include:

- “Which part of my conclusion is measured, and which part is an inference?”
- “Why is this a latency failure even though all eight requests succeeded?”
- “How would retries change the experiment, including the risk of extra load?”
- “What evidence would distinguish a useful fallback from a hidden failure?”

The current lab exposes fixed lessons, not resilience feature switches. For a deeper development exercise, modify a separate copy of the target, add tests for the changed behavior, and rerun the same lesson:

| Proposed change   | Prediction to test                                                 | Tradeoff to examine                                   |
| ----------------- | ------------------------------------------------------------------ | ----------------------------------------------------- |
| Cache fallback    | Requests remain successful when the cache is unavailable           | Freshness and additional database work                |
| Bounded retry     | Some transient failures recover within a fixed request budget      | Retry amplification, duplicate work, and elapsed time |
| Timeout budget    | Dependency work stops before the overall request deadline          | False timeouts and lost useful work                   |
| Circuit breaker   | Repeated dependency failures stop causing repeated expensive calls | Recovery probing and the time until traffic resumes   |
| Concurrency limit | Slow dependency calls cannot consume all workers                   | Queueing, rejection, and fairness                     |

These changes are learning extensions, not built-in controls or already validated behaviors. For genuine Redis/PostgreSQL failure modes or network faults, create a separate disposable environment with the actual dependency and a bounded experiment. Transfer the measurement discipline from this lab; do not assume its results certify that environment.

## A record you can share

Keep the experiment definition, JSON evidence, and native journal together. Add:

- Your original prediction and the hypothesis you tested.
- The exact scenario and run ID.
- Baseline, fault-window, and recovery observations, including failures.
- Whether the run completed, stopped, failed, or was interrupted.
- A conclusion bounded by the small sample and simulated dependency.
- The next question you would test and why.

A useful experiment can expose a weakness. The learning goal is a defensible explanation of what happened and what to investigate next.
