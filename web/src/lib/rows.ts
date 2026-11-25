// A table row that opens something on a click: a page, a detail row under
// it, or a choice. The cell that names the row holds a link (`.row-link`)
// or a button (`.row-button`), so that the keyboard opens the row too. A
// click on the rest of the row does the same, for a mouse.

/** The controls of a row that act by themselves on a click. */
const CONTROLS = 'a, button, input, select, textarea, label';

/**
 * The click handler of a row that `open` opens. A click on a link, a
 * button or a field in the row is left to it, so that the row does not
 * open twice, or open and close again.
 */
export function onRow(open: () => void): (event: MouseEvent) => void {
    return (event) => {
        if (event.target instanceof Element && event.target.closest(CONTROLS)) return;
        open();
    };
}
