<!--
  kronika — dashboard skeleton (static mock).
  Shows the intended layout regions and design language before any data
  plumbing exists: KPI row → rollup trend → dimension leaderboard →
  drill-down panel (future home of the span-waterfall).
  View state will be URL-serialized (time range, filters, selection).
-->
<script lang="ts">
	const kpis = [
		{ label: 'hypothesis pass rate', value: '—', note: 'of experiment runs' },
		{ label: 'MTTR', value: '—', note: 'mean recovery, seconds' },
		{ label: 'deviation rate', value: '—', note: 'runs that deviated' },
		{ label: 'coverage', value: '—', note: 'target systems exercised' }
	];
</script>

<svelte:head>
	<title>kronika — the chronicle of your resilience work</title>
</svelte:head>

<header class="topbar">
	<h1>kronika</h1>
	<span class="window">window: last 7 days · rollup: day · n = —</span>
</header>

<main>
	<!-- KPI row: saturated color is reserved for status -->
	<section class="kpi-row" aria-label="key indicators">
		{#each kpis as kpi}
			<div class="kpi-card">
				<div class="kpi-label">{kpi.label}</div>
				<div class="kpi-value">{kpi.value}</div>
				<div class="kpi-note">{kpi.note}</div>
				<div class="sparkline" aria-hidden="true"></div>
			</div>
		{/each}
	</section>

	<section class="grid">
		<!-- Rollup trend (uPlot lives here) -->
		<div class="panel trend">
			<h2>experiment outcomes — trend</h2>
			<div class="chart-placeholder">uPlot time-series · hour / day / week rollups</div>
		</div>

		<!-- Dimension leaderboard -->
		<div class="panel leaderboard">
			<h2>leaderboard — by target system</h2>
			<table>
				<thead>
					<tr><th>target system</th><th>runs</th><th>pass rate</th></tr>
				</thead>
				<tbody>
					<tr><td colspan="3" class="empty">waiting for telemetry…</td></tr>
				</tbody>
			</table>
		</div>
	</section>

	<!-- Drill-down: fleet → experiment → operation/span (span-waterfall) -->
	<section class="panel drilldown">
		<h2>drill-down</h2>
		<div class="breadcrumb">fleet / experiment / operation · span</div>
		<div class="waterfall-placeholder">
			custom span-waterfall component — the signature piece
			(resilience.experiment → hypothesis / action / probe / rollback)
		</div>
	</section>
</main>

<style>
	:global(body) {
		margin: 0;
		font-family: -apple-system, 'Segoe UI', Roboto, sans-serif;
		background: #fafafa;
		color: #1c1e21;
	}
	.topbar {
		display: flex;
		align-items: baseline;
		gap: 1rem;
		padding: 0.75rem 1.5rem;
		background: #fff;
		border-bottom: 1px solid #e5e7eb;
	}
	.topbar h1 {
		font-size: 1.1rem;
		font-weight: 600;
		margin: 0;
	}
	.window {
		color: #6b7280;
		font-size: 0.8rem;
	}
	main {
		max-width: 72rem;
		margin: 0 auto;
		padding: 1.5rem;
	}
	.kpi-row {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(11rem, 1fr));
		gap: 0.75rem;
		margin-bottom: 1rem;
	}
	.kpi-card {
		background: #fff;
		border: 1px solid #e5e7eb;
		border-radius: 6px;
		padding: 0.75rem 1rem;
	}
	.kpi-label {
		color: #6b7280;
		font-size: 0.72rem;
		text-transform: uppercase;
		letter-spacing: 0.05em;
	}
	.kpi-value {
		font-size: 1.6rem;
		font-weight: 600;
	}
	.kpi-note {
		color: #6b7280;
		font-size: 0.78rem;
	}
	.sparkline {
		height: 2rem;
		margin-top: 0.5rem;
		background: repeating-linear-gradient(
			90deg,
			#e5e7eb 0,
			#e5e7eb 2px,
			transparent 2px,
			transparent 8px
		);
	}
	.grid {
		display: grid;
		grid-template-columns: 3fr 2fr;
		gap: 0.75rem;
		margin-bottom: 1rem;
	}
	.panel {
		background: #fff;
		border: 1px solid #e5e7eb;
		border-radius: 6px;
		padding: 1rem;
	}
	.panel h2 {
		font-size: 0.85rem;
		font-weight: 600;
		color: #374151;
		margin: 0 0 0.75rem;
	}
	.chart-placeholder {
		height: 14rem;
		display: grid;
		place-items: center;
		color: #9ca3af;
		font-size: 0.85rem;
		background: linear-gradient(#f9fafb, #f3f4f6);
		border-radius: 4px;
	}
	table {
		width: 100%;
		border-collapse: collapse;
		font-size: 0.85rem;
	}
	th {
		text-align: left;
		color: #6b7280;
		font-weight: 500;
		padding: 0.35rem 0.5rem;
		border-bottom: 1px solid #e5e7eb;
	}
	td {
		padding: 0.35rem 0.5rem;
	}
	.empty {
		color: #9ca3af;
	}
	.breadcrumb {
		color: #6b7280;
		font-size: 0.8rem;
		margin-bottom: 0.5rem;
	}
	.waterfall-placeholder {
		height: 10rem;
		display: grid;
		place-items: center;
		color: #9ca3af;
		font-size: 0.85rem;
		border: 1px dashed #d1d5db;
		border-radius: 4px;
	}
</style>
