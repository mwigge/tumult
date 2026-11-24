// The words of a chart legend: each colour or sign a chart draws, and
// what it stands for. `ChartLegend` shows them as one line under the chart.

/** How the swatch of an item is drawn: a box, a line, a dashed line, or hatched. */
export type LegendMark = 'box' | 'line' | 'dashed' | 'hatch';

/** One item of a legend. With no colour, no hatch and no glyph it is plain text. */
export interface LegendItem {
    label: string;
    /** The colour the chart draws it in. */
    color?: string;
    /** How its swatch is drawn; a box when not given. */
    mark?: LegendMark;
    /** A sign shown in place of a swatch, as the chart writes it in a cell or
        draws it as a mark; in `color` when it has one. */
    glyph?: string;
}
