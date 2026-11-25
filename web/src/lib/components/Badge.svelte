<script lang="ts">
    // A badge: a short word in the colour of its tone. The tone is given,
    // or the tone module reads it from the kind of the word and its value.
    // The word is the only child of the span; other attributes, such as a
    // title or a test id, go to the span too.
    import type { HTMLAttributes } from 'svelte/elements';

    import { toneOf, type Tone, type ToneKind } from '#lib/tone.js';

    type Choice =
        | { tone: Tone; kind?: never; value?: never }
        | { kind: ToneKind; value: string | null | undefined; tone?: never };

    type Props = Omit<HTMLAttributes<HTMLSpanElement>, 'children'> &
        Choice & {
            /** The word shown; the value when there is none. */
            label?: string | null;
            /** The large badge of a headline. */
            big?: boolean;
        };

    let { tone, kind, value, label, big = false, class: extra, ...rest }: Props = $props();

    const shown = $derived(tone ?? (kind === undefined ? 'neutral' : toneOf(kind, value)));
</script>

<span {...rest} class={['badge', { big }, shown, extra]}>{label ?? value}</span>
