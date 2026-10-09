const $ = (id) => document.getElementById(id);
const state = {
  scenarios: [],
  selected: null,
  run: null,
  activeRunId: null,
  status: null,
  starting: false,
  polling: false,
  asking: false,
};
const phases = [
  ["baseline", "Baseline"],
  ["during", "Fault active"],
  ["recovery", "Recovery"],
];
const terminalStates = new Set([
  "completed",
  "complete",
  "failed",
  "stopped",
  "cancelled",
  "aborted",
  "error",
  "interrupted",
]);

async function api(path, options = {}) {
  const response = await fetch(`/api${path}`, {
    credentials: "same-origin",
    ...options,
    headers: { "Content-Type": "application/json", ...options.headers },
    ...(options.body !== undefined
      ? { body: JSON.stringify(options.body) }
      : {}),
  });
  const data = await response.json().catch(() => null);
  if (!response.ok) {
    const detail = data?.detail ?? data?.error ?? data?.message;
    throw new Error(
      typeof detail === "string"
        ? detail
        : `The lab request failed (${response.status}). Please retry.`,
    );
  }
  return data;
}

function node(tag, text, className) {
  const element = document.createElement(tag);
  if (text !== undefined) element.textContent = String(text);
  if (className) element.className = className;
  return element;
}

function notice(message = "") {
  $("notice").textContent = message;
  $("notice").hidden = !message;
}

function formatTime(value, full = false) {
  const date = new Date(value);
  return Number.isNaN(date.getTime())
    ? ""
    : full
      ? date.toLocaleString()
      : date.toLocaleTimeString();
}

function readable(value) {
  if (value === undefined || value === null) return "";
  if (typeof value === "object") return JSON.stringify(value);
  return String(value).replaceAll("_", " ");
}

function isActive(run) {
  return !!run && !terminalStates.has(String(run.status).toLowerCase());
}

function updateRunControls() {
  $("run-start").disabled =
    !state.selected ||
    !$("armed").checked ||
    state.starting ||
    !!state.activeRunId ||
    !state.status?.engine?.available ||
    !state.status?.target?.healthy;
  $("run-start").textContent = state.starting
    ? "Starting experiment…"
    : "Run experiment";
}

function renderProvider(provider = {}) {
  state.provider = provider;
  $("provider-status").textContent = provider.configured
    ? `Configured · ${readable(provider.provider)} / ${provider.model}`
    : "Optional · connect a model in AI settings";
}

function renderStatus(status) {
  state.status = status;
  state.activeRunId = status.active_run_id || null;
  const engine = status.engine || {};
  $("engine-status").textContent = engine.available
    ? `✓ Tumult ${(engine.version || "engine").replace(/^tumult\s+/i, "")} ready`
    : "⚠ Tumult engine unavailable";
  $("target-status").textContent = status.target?.healthy
    ? "✓ Sandbox responding"
    : "⚠ Sandbox is not healthy";
  renderProvider(status.provider);
  updateRunControls();
}

function renderScenarios(scenarios) {
  state.scenarios = scenarios;
  const cards = scenarios.map((scenario, index) => {
    const button = node("button", undefined, "scenario-card");
    button.type = "button";
    button.dataset.scenario = scenario.id;
    button.setAttribute(
      "aria-pressed",
      String(scenario.id === state.selected?.id),
    );
    button.append(
      node(
        "span",
        `EXPERIMENT ${String(index + 1).padStart(2, "0")}`,
        "scenario-number",
      ),
      node("strong", scenario.title),
      node("span", scenario.description),
    );
    button.addEventListener("click", () => selectScenario(scenario));
    return button;
  });
  $("scenarios").replaceChildren(
    ...(cards.length
      ? cards
      : [
          node(
            "p",
            "No experiments are available. Check the lab service configuration.",
            "muted",
          ),
        ]),
  );
}

function selectScenario(scenario) {
  state.selected = scenario;
  $("armed").checked = false;
  $("scenario-detail").hidden = false;
  $("selected-title").textContent = scenario.title;
  $("scenario-description").textContent = scenario.description;
  $("scenario-duration").textContent =
    `Up to ${scenario.duration_s}s fault window`;
  $("scenario-hypothesis").textContent = scenario.hypothesis;
  $("scenario-target").textContent = `Target: ${readable(scenario.target)}`;
  $("scenario-fault").textContent = `Fault: ${readable(scenario.fault)}`;
  const learning = Array.isArray(scenario.learning)
    ? scenario.learning
    : [scenario.learning];
  $("scenario-learning").replaceChildren(
    ...learning.filter(Boolean).map((item) => node("li", item)),
  );
  document
    .querySelectorAll("[data-scenario]")
    .forEach((card) =>
      card.setAttribute(
        "aria-pressed",
        String(card.dataset.scenario === scenario.id),
      ),
    );
  updateRunControls();
}

function metricValue(value) {
  return Number.isFinite(value) ? `${Math.round(value)} ms` : "—";
}

function renderMetrics(run) {
  const maximum = Math.max(
    1,
    ...phases.map(([key]) => Number(run[key]?.p95_ms) || 0),
  );
  const descriptions = [];
  $("metrics").replaceChildren(
    ...phases.map(([key, title]) => {
      const metric = run[key];
      const card = node("div", undefined, "metric");
      card.append(
        node("span", title, "metric-label"),
        node("strong", metricValue(metric?.p95_ms), "metric-value"),
      );
      const errors =
        metric && Number.isFinite(metric.error_rate)
          ? `${(metric.error_rate * 100).toFixed(1)}% errors`
          : "Awaiting samples";
      card.append(
        node(
          "span",
          metric ? `${errors} · ${metric.requests} requests` : errors,
          "metric-detail",
        ),
      );
      return card;
    }),
  );
  $("latency-chart").replaceChildren(
    ...phases.map(([key, title]) => {
      const value = run[key]?.p95_ms;
      const row = node("div", undefined, "chart-row");
      const track = node("div", undefined, "chart-bar-track");
      const bar = node("progress", undefined, "chart-bar");
      bar.max = maximum;
      bar.value = Number.isFinite(value) ? Math.max(0, value) : 0;
      bar.setAttribute("aria-label", `${title} p95 response time`);
      track.append(bar);
      row.append(
        node("span", title),
        track,
        node("span", metricValue(value), "chart-value"),
      );
      descriptions.push(`${title}: ${metricValue(value)}`);
      return row;
    }),
  );
  $("latency-chart").setAttribute(
    "aria-label",
    `Measured p95 response times. ${descriptions.join(". ")}`,
  );
}

function renderRun(run) {
  state.run = run;
  const active = isActive(run);
  if (active) state.activeRunId = run.id;
  else if (state.activeRunId === run.id) state.activeRunId = null;
  $("run-panel").hidden = false;
  $("run-title").textContent =
    run.title ||
    state.scenarios.find((item) => item.id === run.scenario_id)?.title ||
    "Experiment results";
  $("run-status").textContent = readable(run.status);
  $("run-description").textContent = active
    ? `Experiment in progress · ${readable(run.phase)}. Measurements appear as each phase finishes.`
    : `Experiment ${readable(run.status)}${run.finished_at ? ` · ${formatTime(run.finished_at, true)}` : ""}.`;
  const phaseIndex = phases.findIndex(([key]) => key === run.phase);
  document.querySelectorAll("[data-phase]").forEach((element, index) => {
    element.classList.toggle("done", !!run[element.dataset.phase]);
    element.classList.toggle("current", active && index === phaseIndex);
  });
  renderMetrics(run);
  $("verdict-panel").hidden = !run.verdict && !run.explanation;
  $("verdict-title").textContent = readable(run.verdict) || "What happened";
  $("verdict-explanation").textContent = run.explanation || "";
  $("run-stop").hidden = !active;
  $("run-stop").disabled = false;
  $("run-stop").textContent = "Stop experiment";
  $("export-run").hidden = active;
  $("export-run").href = `/api/runs/${encodeURIComponent(run.id)}/export`;
  $("export-journal").hidden = !run.journal_available;
  $("export-journal").href = `/api/runs/${encodeURIComponent(run.id)}/journal`;
  $("export-experiment").href =
    `/api/runs/${encodeURIComponent(run.id)}/experiment`;
  $("export-experiment").hidden = !run.journal_available;
  $("run-events").replaceChildren(
    ...(run.events || []).map((event) => {
      const item = node("li");
      const time = node("time", formatTime(event.at));
      time.dateTime = event.at;
      item.append(time, document.createTextNode(event.message));
      return item;
    }),
  );
  updateRunControls();
}

async function refreshHistory() {
  const runs = await api("/runs");
  $("history-list").replaceChildren(
    ...(runs.length
      ? runs.map((run) => {
          const button = node("button", undefined, "history-item");
          const description = node("span");
          description.append(
            node("strong", run.title || run.scenario_id),
            node("time", formatTime(run.started_at, true)),
          );
          button.append(description, node("span", readable(run.status), "tag"));
          button.addEventListener("click", () =>
            openRun(run.id).catch((error) => notice(error.message)),
          );
          return button;
        })
      : [
          node(
            "p",
            "Your first run starts a trail of evidence.",
            "empty-state",
          ),
        ]),
  );
}

async function openRun(id) {
  renderRun(await api(`/runs/${encodeURIComponent(id)}`));
  $("run-panel").scrollIntoView({ behavior: "smooth", block: "nearest" });
}

async function startRun() {
  if ($("run-start").disabled) return;
  state.starting = true;
  notice();
  updateRunControls();
  try {
    renderRun(
      await api("/runs", {
        method: "POST",
        body: { scenario_id: state.selected.id, armed: true },
      }),
    );
    $("armed").checked = false;
    $("run-panel").scrollIntoView({ behavior: "smooth", block: "nearest" });
    await refreshHistory();
  } catch (error) {
    notice(error.message);
  } finally {
    state.starting = false;
    updateRunControls();
  }
}

async function stopRun() {
  if (!state.run || !isActive(state.run)) return;
  const runId = state.run.id;
  $("run-stop").disabled = true;
  $("run-stop").textContent = "Stopping and cleaning up…";
  try {
    const result = await api(`/runs/${encodeURIComponent(runId)}/stop`, {
      method: "POST",
      body: {},
    });
    renderRun(
      result?.id ? result : await api(`/runs/${encodeURIComponent(runId)}`),
    );
    await refreshHistory();
  } catch (error) {
    notice(error.message);
    $("run-stop").disabled = false;
  }
}

async function poll() {
  if (state.polling || document.hidden) return;
  state.polling = true;
  try {
    const previousActive = state.activeRunId;
    const status = await api("/status");
    renderStatus(status);
    const id =
      state.run && isActive(state.run)
        ? state.run.id
        : !state.run
          ? status.active_run_id
          : null;
    if (id) renderRun(await api(`/runs/${encodeURIComponent(id)}`));
    if (previousActive && !state.activeRunId) await refreshHistory();
  } catch (error) {
    $("engine-status").textContent = "⚠ Lab connection interrupted";
    $("target-status").textContent = `Retrying automatically: ${error.message}`;
    state.status = null;
    updateRunControls();
  } finally {
    state.polling = false;
  }
}

function providerHelp() {
  const local = $("provider").value === "ollama";
  $("api-key").required = !local;
  $("provider-help").textContent = local
    ? "Local Ollama usually needs no key. Enter the name of an installed model. The lab uses the Ollama address configured by its operator."
    : "Enter a model ID available to your account and an API key. Requests may incur provider charges.";
  $("api-key").placeholder = local
    ? "Optional for local Ollama"
    : "Paste your provider API key";
}

function openSettings() {
  $("settings-error").hidden = true;
  $("api-key").value = "";
  $("provider").value = state.provider?.provider || "openai";
  $("model").value =
    state.provider?.model ||
    state.provider?.defaults?.[$("provider").value] ||
    "";
  providerHelp();
  $("settings-dialog").showModal();
}

async function saveSettings(event) {
  event.preventDefault();
  $("settings-save").disabled = true;
  $("settings-error").hidden = true;
  const body = {
    provider: $("provider").value,
    model: $("model").value.trim(),
    api_key: $("api-key").value.trim(),
  };
  $("api-key").value = "";
  try {
    renderProvider(await api("/settings", { method: "POST", body }));
    $("settings-dialog").close();
  } catch (error) {
    $("settings-error").textContent = error.message;
    $("settings-error").hidden = false;
  } finally {
    $("settings-save").disabled = false;
  }
}

async function disconnect() {
  $("settings-clear").disabled = true;
  try {
    renderProvider(await api("/settings", { method: "DELETE" }));
    $("api-key").value = "";
    $("settings-dialog").close();
  } catch (error) {
    $("settings-error").textContent = error.message;
    $("settings-error").hidden = false;
  } finally {
    $("settings-clear").disabled = false;
  }
}

async function askTutor(event) {
  event.preventDefault();
  const q = $("tutor-question").value.trim();
  if (!q || state.asking) return;
  if (!state.provider?.configured) {
    openSettings();
    return;
  }
  state.asking = true;
  $("tutor-submit").disabled = true;
  $("tutor-submit").textContent = "Thinking…";
  $("tutor-answer").hidden = false;
  $("tutor-answer").textContent = "Your tutor is reviewing the question…";
  try {
    const answer = await api("/ask", {
      method: "POST",
      body: { q, ...(state.run ? { run_id: state.run.id } : {}) },
    });
    $("tutor-answer").textContent = answer.answer;
  } catch (error) {
    $("tutor-answer").textContent =
      `Couldn’t reach your tutor: ${error.message}`;
  } finally {
    state.asking = false;
    $("tutor-submit").disabled = false;
    $("tutor-submit").textContent = "Ask tutor";
  }
}

$("armed").addEventListener("change", updateRunControls);
$("run-start").addEventListener("click", startRun);
$("run-stop").addEventListener("click", stopRun);
$("refresh-history").addEventListener("click", () =>
  refreshHistory().catch((error) => notice(error.message)),
);
$("settings-open").addEventListener("click", openSettings);
$("settings-close").addEventListener("click", () =>
  $("settings-dialog").close(),
);
$("settings-dialog").addEventListener("close", () => {
  $("api-key").value = "";
});
$("provider").addEventListener("change", () => {
  $("model").value = state.provider?.defaults?.[$("provider").value] || "";
  $("api-key").value = "";
  providerHelp();
});
$("settings-form").addEventListener("submit", saveSettings);
$("settings-clear").addEventListener("click", disconnect);
$("tutor-form").addEventListener("submit", askTutor);
document.querySelectorAll("[data-question]").forEach((button) =>
  button.addEventListener("click", () => {
    $("tutor-question").value = button.dataset.question;
    $("tutor-question").focus();
  }),
);

async function initialize() {
  try {
    const [status, scenarios] = await Promise.all([
      api("/status"),
      api("/scenarios"),
    ]);
    renderStatus(status);
    renderScenarios(scenarios);
    if (scenarios.length) selectScenario(scenarios[0]);
    await refreshHistory();
    if (status.active_run_id) await openRun(status.active_run_id);
  } catch (error) {
    notice(
      `Couldn’t load the lab: ${error.message} Reload this page after the service is available.`,
    );
    $("scenarios").replaceChildren(
      node("p", "Experiments could not be loaded.", "muted"),
    );
  }
  setInterval(poll, 1500);
}

initialize();
