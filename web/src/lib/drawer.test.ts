import { describe, expect, it } from 'vitest';

import { drawer } from './drawer.js';
import type { DrawerOptions } from './drawer.js';

// The tests run without a browser, so a small stand-in plays the page: a
// <dialog> 480px wide at the right edge of a 1440 x 900 window, the
// element that opened it, and the heading of the drawer. The code under
// test is the real drawer behaviour; only the DOM is a stand-in.

/** An element that can take the focus, and leave the page. */
class Element extends EventTarget {
    isConnected = true;
    constructor(
        private readonly page: Page,
        readonly field = false,
    ) {
        super();
    }
    focus() {
        this.page.document.activeElement = this;
    }
    closest(selector: string) {
        return this.field && selector.includes('input') ? this : null;
    }
}

class Page {
    readonly window = new EventTarget();
    readonly document = {
        activeElement: null as Element | null,
        defaultView: this.window,
        getElementById: (id: string) => (id === 'heading' ? this.heading : null),
    };
    readonly opener = new Element(this);
    readonly heading = new Element(this);
    readonly field = new Element(this, true);
    readonly dialog = Object.assign(new EventTarget(), {
        ownerDocument: this.document,
        opened: '',
        showModal() {
            this.opened = 'modal';
        },
        show() {
            this.opened = 'in the page';
        },
        getBoundingClientRect: () => ({ left: 960, right: 1440, top: 0, bottom: 900 }),
    });
    closed = 0;

    constructor() {
        this.opener.focus();
    }

    /** Open a drawer on the dialog; returns what the caller runs when it removes the drawer. */
    open(options: Partial<DrawerOptions> = {}): () => void {
        const attach = drawer({
            modal: true,
            backdrop: false,
            focus: null,
            close: () => (this.closed += 1),
            ...options,
        });
        const remove = attach(this.dialog as unknown as HTMLDialogElement);
        return remove ?? (() => {});
    }
}

const event = (type: string, fields: Record<string, unknown> = {}) =>
    Object.assign(new Event(type, { cancelable: true }), fields);

const key = (name: string, target?: EventTarget) => {
    const pressed = event('keydown', { key: name });
    if (target) Object.defineProperty(pressed, 'target', { value: target });
    return pressed;
};

/** A press and a release of the mouse at x, y and at x2, y2. */
function click(page: Page, [x, y]: number[], [x2, y2] = [x, y]) {
    page.dialog.dispatchEvent(event('pointerdown', { clientX: x, clientY: y }));
    page.dialog.dispatchEvent(event('click', { clientX: x2, clientY: y2 }));
}

describe('a drawer', () => {
    it('opens over the page when it is modal, and in the flow of the page when not', () => {
        const page = new Page();
        page.open();
        expect(page.dialog.opened).toBe('modal');

        const other = new Page();
        other.open({ modal: false });
        expect(other.dialog.opened).toBe('in the page');
    });

    it('moves the focus to its heading on open when asked, else leaves it to the browser', () => {
        const page = new Page();
        page.open({ focus: 'heading' });
        expect(page.document.activeElement).toBe(page.heading);

        // The browser gives the first control of a modal dialog the focus.
        const other = new Page();
        other.open();
        expect(other.document.activeElement).toBe(other.opener);
    });

    it('closes through its caller on Escape when modal, and keeps the dialog open itself', () => {
        const page = new Page();
        page.open();
        const cancel = event('cancel');
        page.dialog.dispatchEvent(cancel);
        expect(page.closed).toBe(1);
        expect(cancel.defaultPrevented).toBe(true);

        // A key on the page is the dialog's to handle, so it does not close twice.
        page.window.dispatchEvent(key('Escape'));
        expect(page.closed).toBe(1);
    });

    it('closes on Escape when it is in the page, but not on Escape in a field', () => {
        const page = new Page();
        page.open({ modal: false });
        page.window.dispatchEvent(key('Enter'));
        page.window.dispatchEvent(key('Escape', page.field));
        expect(page.closed).toBe(0);

        const escape = key('Escape');
        page.window.dispatchEvent(escape);
        expect(page.closed).toBe(1);
        expect(escape.defaultPrevented).toBe(true);
    });

    it('lets an Escape that something else handled pass', () => {
        const page = new Page();
        page.open({ modal: false });
        const handled = key('Escape');
        handled.preventDefault();
        page.window.dispatchEvent(handled);
        expect(page.closed).toBe(0);
    });

    it('closes on a click on its backdrop, not on a click in it or a drag out of it', () => {
        const page = new Page();
        page.open({ backdrop: true });
        click(page, [1000, 400]);
        click(page, [1000, 400], [200, 400]);
        expect(page.closed).toBe(0);

        click(page, [200, 400]);
        expect(page.closed).toBe(1);
    });

    it('stays open on a click beside it when it has no backdrop', () => {
        const page = new Page();
        page.open();
        click(page, [200, 400]);
        expect(page.closed).toBe(0);
    });

    it('gives the focus back to the element that opened it when it is removed', () => {
        const page = new Page();
        const remove = page.open({ focus: 'heading' });
        expect(page.document.activeElement).toBe(page.heading);
        remove();
        expect(page.document.activeElement).toBe(page.opener);
    });

    it('keeps the focus where it is when its opener left the page', () => {
        const page = new Page();
        const remove = page.open({ focus: 'heading' });
        page.opener.isConnected = false;
        remove();
        expect(page.document.activeElement).toBe(page.heading);
    });

    it('hears no key and no click once it is removed', () => {
        const page = new Page();
        page.open({ modal: false, backdrop: true })();
        page.window.dispatchEvent(key('Escape'));
        page.dialog.dispatchEvent(event('cancel'));
        click(page, [200, 400]);
        expect(page.closed).toBe(0);
    });
});
