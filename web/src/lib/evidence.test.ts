import { describe, expect, it } from 'vitest';

import { chartTokens } from './echarts';
import {
    axisAt,
    fmtMargin,
    fmtValue,
    loadFacts,
    loadLatencyOption,
    loadThroughputOption,
    objectiveTarget,
    phaseLabel,
    probeSamplesOption,
    recoveryRows,
    runAxis,
    samplesNote,
    seriesTip,
    steadyState,
    stepOffset,
} from './evidence';
import type {
    EvidenceLoad,
    EvidenceObjective,
    EvidenceProbeSample,
    LoadWindow,
    RunEvidence,
} from './types';

const S = 1e9;
// A whole number of seconds this large is still an exact number.
const START = 1_794_852_855 * S;
// The tokens the page itself draws with.
const TOKENS = chartTokens('tumult-dark');

function sample(
    second: number,
    ms: number,
    phase: string,
    ok = true,
    probe = 'health',
): EvidenceProbeSample {
    return { probe, phase, ts_ns: START + second * S, duration_ms: ms, ok };
}

function window(second: number, values: Partial<LoadWindow> = {}): LoadWindow {
    return {
        ts_ns: START + second * S,
        window_s: 10,
        throughput_rps: 100,
        latency_p50_ms: 20,
        latency_p95_ms: 80,
        latency_p99_ms: 100,
        error_rate: 0,
        ...values,
    };
}

const LOAD: EvidenceLoad = {
    tool: 'oha',
    started_at_ns: START,
    ended_at_ns: START + 30 * S,
    duration_s: 30,
    vus: 400,
    throughput_rps: 2466.927934954826,
    latency_p50_ms: 20,
    latency_p95_ms: 80.04,
    latency_p99_ms: 100.06,
    error_rate: 0.0125,
    total_requests: 302618,
    thresholds_met: true,
    windows: [],
};

function run(parts: Partial<RunEvidence> = {}): RunEvidence {
    return {
        kind: 'chaos',
        started_at_ns: START,
        ended_at_ns: START + 120 * S,
        hypothesis: { before_met: true, after_met: true },
        recovery: {
            rto_target_s: null,
            rto_measured_s: null,
            rpo_target_s: null,
            rpo_measured_s: null,
            impact_observed: null,
            outage_started_at_ns: null,
            recovered_at_ns: null,
        },
        steps: [],
        objectives: [],
        load: null,
        probe_samples: [],
        probe_samples_total: 0,
        ...parts,
    };
}

interface Lines {
    xAxis: { name: string; min: number };
    yAxis: unknown;
    series: {
        name: string;
        data: unknown[];
        markArea?: { data: { name?: string; xAxis: number }[][] };
        yAxisIndex?: number;
    }[];
    tooltip: { trigger: string; backgroundColor: string; formatter: (params: unknown) => string };
}

const lines = (option: unknown) => option as Lines;

describe('fmtValue', () => {
    it('prints a number as short as a person reads it', () => {
        expect(fmtValue(40)).toBe('40');
        expect(fmtValue(0)).toBe('0');
        expect(fmtValue(13.333333)).toBe('13.33');
        expect(fmtValue(0.050000000000000044)).toBe('0.05');
        expect(fmtValue(-0.5)).toBe('-0.5');
        expect(fmtValue(999.96)).toBe('1000');
    });

    it('prints a large number whole, with every digit', () => {
        expect(fmtValue(2466.927934954826)).toBe('2467');
        expect(fmtValue(122684)).toBe('122684');
        expect(fmtValue(-1000.4)).toBe('-1000');
    });

    it('prints a dash for what was not measured', () => {
        expect(fmtValue(null)).toBe('—');
    });
});

describe('fmtMargin', () => {
    it('gives a margin its sign: room left is a plus', () => {
        expect(fmtMargin(120)).toBe('+120');
        expect(fmtMargin(0.050000000000000044)).toBe('+0.05');
        expect(fmtMargin(-97382)).toBe('-97382');
        expect(fmtMargin(-0.5)).toBe('-0.5');
    });

    it('gives no sign to no margin, and a dash to one not measured', () => {
        expect(fmtMargin(0)).toBe('0');
        expect(fmtMargin(null)).toBe('—');
    });
});

describe('stepOffset', () => {
    const run = { started_at_ns: START };

    it('says how long after the start of the run a step began', () => {
        expect(stepOffset(run, START)).toBe('+0.0 s');
        expect(stepOffset(run, START + 8.26 * S)).toBe('+8.3 s');
        expect(stepOffset(run, START + 119 * S)).toBe('+119.0 s');
    });

    it('puts a step that began before the run at its start', () => {
        expect(stepOffset(run, START - 2 * S)).toBe('+0.0 s');
    });
});

describe('the words of a run', () => {
    it('names each phase of the steps, and shows an unknown one as it is', () => {
        expect(phaseLabel('hypothesis_before')).toBe('Steady state before');
        expect(phaseLabel('method')).toBe('Method');
        expect(phaseLabel('hypothesis_after')).toBe('Steady state after');
        expect(phaseLabel('rollback')).toBe('Rollback');
        expect(phaseLabel('cleanup')).toBe('cleanup');
    });

    it('says whether a steady state was met, not met or not checked', () => {
        expect(steadyState(true)).toBe('met');
        expect(steadyState(false)).toBe('not met');
        expect(steadyState(null)).toBe('not checked');
    });

    it('writes the target of an objective with its sign', () => {
        expect(objectiveTarget({ operator: 'lte', threshold: 200 })).toBe('≤ 200');
        expect(objectiveTarget({ operator: 'gte', threshold: 0.95 })).toBe('≥ 0.95');
        expect(objectiveTarget({ operator: 'eq', threshold: 1 })).toBe('= 1');
        expect(objectiveTarget({ operator: 'near', threshold: 3 })).toBe('near 3');
    });
});

describe('recoveryRows', () => {
    it('has no row for a run without a recovery target or measure', () => {
        expect(recoveryRows(run().recovery)).toEqual([]);
    });

    it('has a row for each of the two that has a target or a measure', () => {
        const recovery = {
            rto_target_s: 30,
            rto_measured_s: 0,
            rpo_target_s: 5,
            rpo_measured_s: null,
        };
        expect(recoveryRows(recovery)).toEqual([
            { label: 'RTO', target: '30 s', measured: '0 s' },
            { label: 'RPO', target: '5 s', measured: 'not measured' },
        ]);
    });

    it('says that a measure has no target', () => {
        const recovery = {
            rto_target_s: null,
            rto_measured_s: null,
            rpo_target_s: null,
            rpo_measured_s: 2.5,
        };
        expect(recoveryRows(recovery)).toEqual([
            { label: 'RPO', target: 'not set', measured: '2.5 s' },
        ]);
    });
});

/** The verdict on the RTO a run declares, of `threshold` seconds. */
function rto(threshold: number, parts: Partial<EvidenceObjective> = {}): EvidenceObjective {
    return {
        nfr_id: 'rto',
        kind: 'rto',
        metric: 'recovery.mttr_s',
        operator: 'lte',
        threshold,
        observed: null,
        state: 'inconclusive',
        margin: null,
        criticality: 'blocking',
        recovered: null,
        reason: null,
        ...parts,
    };
}

describe('recoveryRows of a run that measured no recovery', () => {
    it('says how long a run that did not recover watched, and that it did not recover', () => {
        const recovery = { ...run().recovery, rto_target_s: 2 };
        const never = rto(2, { observed: 29.1, state: 'breached', recovered: false });
        expect(recoveryRows(recovery, [never])).toEqual([
            { label: 'RTO', target: '2 s', measured: 'at least 29.1 s, not recovered' },
        ]);
    });

    it('gives the reason of a verdict that measured nothing', () => {
        const recovery = { ...run().recovery, rto_target_s: 30 };
        const unaffected = rto(30, { reason: 'no outage observed' });
        expect(recoveryRows(recovery, [unaffected])).toEqual([
            { label: 'RTO', target: '30 s', measured: 'no outage observed' },
        ]);
    });

    it('reads only the verdict on the RTO the run declares', () => {
        const recovery = { ...run().recovery, rto_target_s: 30 };
        const other = rto(60, { nfr_id: 'slow-back', recovered: false, observed: 90 });
        expect(recoveryRows(recovery, [other])).toEqual([
            { label: 'RTO', target: '30 s', measured: 'not measured' },
        ]);
    });
});

describe('loadFacts', () => {
    it('lists the totals of a load in the units of the run summary', () => {
        expect(loadFacts(LOAD)).toEqual([
            ['Tool', 'oha'],
            ['Virtual users', '400'],
            ['Requests', '302618'],
            ['Throughput', '2466.9 requests/s'],
            ['Latency p50', '20.0 ms'],
            ['Latency p95', '80.0 ms'],
            ['Latency p99', '100.1 ms'],
            ['Error rate', '1.25 %'],
            ['Thresholds', 'met'],
        ]);
    });

    it('says that the thresholds were not met', () => {
        const facts = loadFacts({ ...LOAD, thresholds_met: false });
        expect(facts[facts.length - 1]).toEqual(['Thresholds', 'not met']);
    });
});

describe('samplesNote', () => {
    it('says nothing when the page carries every sample', () => {
        const samples = [sample(1, 5, 'during'), sample(2, 5, 'during')];
        expect(samplesNote(run({ probe_samples: samples, probe_samples_total: 2 }))).toBeNull();
    });

    it('says how many samples the run has when the page carries fewer', () => {
        const samples = [sample(1, 5, 'during'), sample(2, 5, 'during')];
        expect(samplesNote(run({ probe_samples: samples, probe_samples_total: 5003 }))).toBe(
            'The chart shows the first 2 of the 5003 samples of this run.',
        );
    });
});

describe('runAxis', () => {
    const lasting = (seconds: number) =>
        runAxis({ started_at_ns: START, ended_at_ns: START + seconds * S });

    it('counts a short run in seconds, a long one in minutes and a very long one in hours', () => {
        expect(lasting(599).unit).toBe('s');
        expect(lasting(600).unit).toBe('min');
        expect(lasting(35_999).unit).toBe('min');
        expect(lasting(36_000).unit).toBe('h');
    });

    it('puts a time where it is from the start of the run, in the unit of the axis', () => {
        expect(axisAt(lasting(120), START + 2.5 * S)).toBe(2.5);
        expect(axisAt(lasting(1200), START + 90 * S)).toBe(1.5);
        expect(axisAt(lasting(72_000), START + 5400 * S)).toBe(1.5);
        expect(axisAt(lasting(120), START + 1_234_567_890)).toBe(1.235);
    });

    it('puts a time before the run at its start', () => {
        expect(axisAt(lasting(120), START - S)).toBe(0);
    });
});

describe('probeSamplesOption', () => {
    const samples = [
        sample(1, 6, 'baseline'),
        sample(1, 9, 'baseline', true, 'db'),
        sample(2, 20, 'during'),
        sample(3, 97, 'during', false),
        sample(3, 11, 'during', true, 'db'),
        sample(4, 5, 'post'),
    ];
    const option = lines(
        probeSamplesOption(run({ probe_samples: samples, probe_samples_total: 6 }), TOKENS),
    );

    it('draws one line a probe, in the order the probes first sampled', () => {
        expect(option.series.map((series) => series.name)).toEqual(['health', 'db']);
        expect(option.series[1].data).toEqual([
            [1, 9],
            [3, 11],
        ]);
    });

    it('marks a sample out of its tolerance in the colour of a failure', () => {
        expect(option.series[0].data).toEqual([
            [1, 6],
            [2, 20],
            {
                value: [3, 97],
                symbol: 'circle',
                symbolSize: 9,
                itemStyle: { color: TOKENS.fail },
            },
            [4, 5],
        ]);
    });

    it('marks the time of the samples taken during the fault, once', () => {
        expect(option.series[0].markArea?.data).toEqual([
            [{ name: 'during the fault', xAxis: 2 }, { xAxis: 3 }],
        ]);
        expect(option.series[1].markArea).toBeUndefined();
    });

    it('marks nothing for a run without a sample during the fault', () => {
        const calm = [sample(1, 6, 'baseline'), sample(4, 5, 'post')];
        const drawn = lines(probeSamplesOption(run({ probe_samples: calm }), TOKENS));
        expect(drawn.series[0].markArea).toBeUndefined();
    });

    it('marks nothing for one sample during the fault: a time without a length', () => {
        const brief = [sample(1, 6, 'baseline'), sample(2, 20, 'during'), sample(4, 5, 'post')];
        const drawn = lines(probeSamplesOption(run({ probe_samples: brief }), TOKENS));
        expect(drawn.series[0].markArea).toBeUndefined();
    });

    it('names its axes and takes the tooltip of the theme', () => {
        expect(option.xAxis).toMatchObject({
            name: 'Time from the start of the run (s)',
            min: 0,
        });
        expect(option.yAxis).toMatchObject({ name: 'Probe time (ms)' });
        expect(option.tooltip).toMatchObject({ ...TOKENS.tooltip, trigger: 'axis' });
    });

    it('shows a probe name in its tooltip as text', () => {
        const html = option.tooltip.formatter([
            { seriesName: '<b>health</b>', color: '#5eb1ef', value: [3, 97], data: [3, 97] },
        ]);
        expect(html).toContain('&lt;b&gt;health&lt;/b&gt;: 97 ms');
        expect(html).not.toContain('<b>health');
    });
});

describe('the charts of a load', () => {
    const windows = [
        window(0, { throughput_rps: 2400.5, error_rate: 0 }),
        window(10, {
            throughput_rps: 2500,
            error_rate: 0.0125,
            latency_p50_ms: 22,
            latency_p95_ms: 90.5,
            latency_p99_ms: 140,
        }),
    ];
    const during = [sample(2, 20, 'during'), sample(9, 20, 'during')];
    const loaded = run({ load: { ...LOAD, windows }, probe_samples: during });

    it('draws the three latencies of each window at the start of the window', () => {
        const option = lines(loadLatencyOption(loaded, TOKENS));
        expect(option.series.map((series) => [series.name, series.data])).toEqual([
            [
                'p50',
                [
                    [0, 20],
                    [10, 22],
                ],
            ],
            [
                'p95',
                [
                    [0, 80],
                    [10, 90.5],
                ],
            ],
            [
                'p99',
                [
                    [0, 100],
                    [10, 140],
                ],
            ],
        ]);
        expect(option.yAxis).toMatchObject({ name: 'Latency (ms)' });
        expect(option.series[0].markArea?.data).toEqual([
            [{ name: 'during the fault', xAxis: 2 }, { xAxis: 9 }],
        ]);
    });

    it('draws the throughput, and the error rate in percent on an axis of its own', () => {
        const option = lines(loadThroughputOption(loaded, TOKENS));
        expect(option.series.map((series) => [series.name, series.data])).toEqual([
            [
                'throughput',
                [
                    [0, 2400.5],
                    [10, 2500],
                ],
            ],
            [
                'error rate',
                [
                    [0, 0],
                    [10, 1.25],
                ],
            ],
        ]);
        expect(option.series[0].yAxisIndex).toBeUndefined();
        expect(option.series[1].yAxisIndex).toBe(1);
        expect(option.yAxis).toMatchObject([{ name: 'Requests/s' }, { name: 'Errors (%)' }]);
    });

    it('names requests for the throughput and per cent for the error rate in the tooltip', () => {
        const option = lines(loadThroughputOption(loaded, TOKENS));
        const html = option.tooltip.formatter([
            { seriesName: 'throughput', color: '#5eb1ef', value: [10, 2500], data: [10, 2500] },
            { seriesName: 'error rate', color: '#f00', value: [10, 1.25], data: [10, 1.25] },
        ]);
        expect(html).toContain('throughput: 2500 requests/s');
        expect(html).toContain('error rate: 1.25 %');
    });

    it('leaves a gap where a window has no value: a zero would be a measure', () => {
        const holed = [window(0), { ts_ns: START + 10 * S }, window(20)];
        const option = lines(loadLatencyOption(run({ load: { ...LOAD, windows: holed } }), TOKENS));
        expect(option.series[1].data).toEqual([
            [0, 80],
            [10, null],
            [20, 80],
        ]);
    });

    it('has no chart for a load of fewer than two windows: its totals say all there is', () => {
        const one = run({ load: { ...LOAD, windows: windows.slice(0, 1) } });
        expect(loadLatencyOption(one, TOKENS)).toBeNull();
        expect(loadThroughputOption(one, TOKENS)).toBeNull();
        expect(loadLatencyOption(run(), TOKENS)).toBeNull();
        expect(loadThroughputOption(run(), TOKENS)).toBeNull();
    });
});

describe('seriesTip', () => {
    const unitOf = (series: string) => (series === 'error rate' ? '%' : 'requests/s');

    it('heads the tooltip with the time and gives each series its value and unit', () => {
        const built = seriesTip(
            'min',
            unitOf,
        )([
            { seriesName: 'throughput', color: '#5eb1ef', value: [1.5, 2466.93], data: [1.5, 1] },
            { seriesName: 'error rate', color: '#f00', value: [1.5, 1.25], data: [1.5, 1.25] },
        ]);
        expect(built.html).toMatch(/^1\.5 min<br\/>/);
        expect(built.html).toContain('throughput: 2467 requests/s');
        expect(built.html).toContain('error rate: 1.25 %');
    });

    it('says of a marked sample that it was out of tolerance', () => {
        const marked = { value: [3, 97], symbol: 'circle' };
        const built = seriesTip(
            's',
            () => 'ms',
        )([
            { seriesName: 'health', color: '#5eb1ef', value: [3, 97], data: marked },
            { seriesName: 'db', color: '#E69F00', value: [3, 11], data: [3, 11] },
        ]);
        expect(built.html).toContain('health: 97 ms, out of tolerance');
        expect(built.html).toMatch(/db: 11 ms$/);
    });

    it('takes one point as it takes a list of them', () => {
        const point = { seriesName: 'p95', color: '#5eb1ef', value: [0, 80], data: [0, 80] };
        expect(seriesTip('s', () => 'ms')(point).html).toContain('p95: 80 ms');
    });
});
