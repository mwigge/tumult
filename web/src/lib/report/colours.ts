// A chart of the CLI report names its colours by token, because the report
// is written before anyone knows whether it is read in light or in dark.

const PREFIX = 'token:';

/**
 * `option` with every `color: "token:<name>"` replaced by the colour of
 * that name in `tokens`. Only a `color` key names a token: a text of the
 * journal that happens to read `token:ok` stays what it is. A name that
 * `tokens` does not have is left as written.
 */
export function withColours(option: unknown, tokens: Readonly<Record<string, unknown>>): unknown {
    if (Array.isArray(option)) return option.map((item) => withColours(item, tokens));
    if (option === null || typeof option !== 'object') return option;
    return Object.fromEntries(
        Object.entries(option).map(([key, value]) => [
            key,
            key === 'color' ? colour(value, tokens) : withColours(value, tokens),
        ]),
    );
}

function colour(value: unknown, tokens: Readonly<Record<string, unknown>>): unknown {
    if (typeof value !== 'string' || !value.startsWith(PREFIX)) return withColours(value, tokens);
    const named = tokens[value.slice(PREFIX.length)];
    return typeof named === 'string' ? named : value;
}
