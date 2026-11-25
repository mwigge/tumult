// How a drawer behaves. Drawer.svelte attaches this to its <dialog>: it
// opens the dialog, moves the focus in, closes it on Escape and on a click
// on its backdrop, and gives the focus back to the element that opened it
// when the caller removes the drawer. The caller closes a drawer by
// removing it, so every way to close it ends the same way.

export interface DrawerOptions {
    /** Modal: over the page, which cannot be used meanwhile. Not modal: a panel in the page. */
    modal: boolean;
    /** A click on the backdrop of a modal drawer closes it. */
    backdrop: boolean;
    /** The id of the element that takes the focus on open; null leaves it to the browser. */
    focus: string | null;
    /** Close the drawer: the caller removes it. */
    close: () => void;
}

/** Escape in a field belongs to the field: it clears a search box. */
const FIELD = 'input, textarea, select, [contenteditable]';

const inField = (target: EventTarget | null) =>
    Boolean((target as Element | null)?.closest?.(FIELD));

/** True when the point of a mouse event is outside the box of the dialog: on its backdrop. */
function outside(dialog: HTMLDialogElement, event: Event): boolean {
    const { clientX: x, clientY: y } = event as MouseEvent;
    const box = dialog.getBoundingClientRect();
    return x < box.left || x > box.right || y < box.top || y > box.bottom;
}

/** The attachment of a drawer to its dialog; it returns what runs when the drawer is removed. */
export function drawer({ modal, backdrop, focus, close }: DrawerOptions) {
    return (dialog: HTMLDialogElement) => {
        const page = dialog.ownerDocument;
        const opener = page.activeElement as HTMLElement | null;
        // A drag that starts in the drawer and ends beside it is no click on the backdrop.
        let pressedOutside = false;

        // A modal dialog hears Escape itself, as a cancel. The caller then
        // removes it, so the dialog does not close on its own.
        const onCancel = (event: Event) => {
            event.preventDefault();
            close();
        };
        const onKey = (event: Event) => {
            const { key, defaultPrevented, target } = event as KeyboardEvent;
            if (modal || key !== 'Escape' || defaultPrevented || inField(target)) return;
            event.preventDefault();
            close();
        };
        const onPress = (event: Event) => {
            pressedOutside = event.target === dialog && outside(dialog, event);
        };
        const onClick = (event: Event) => {
            if (!pressedOutside || event.target !== dialog || !outside(dialog, event)) return;
            close();
        };

        dialog.addEventListener('cancel', onCancel);
        page.defaultView?.addEventListener('keydown', onKey);
        if (backdrop) {
            dialog.addEventListener('pointerdown', onPress);
            dialog.addEventListener('click', onClick);
        }
        if (modal) dialog.showModal();
        else dialog.show();
        if (focus) page.getElementById(focus)?.focus();

        return () => {
            dialog.removeEventListener('cancel', onCancel);
            page.defaultView?.removeEventListener('keydown', onKey);
            dialog.removeEventListener('pointerdown', onPress);
            dialog.removeEventListener('click', onClick);
            if (opener?.isConnected) opener.focus();
        };
    };
}
