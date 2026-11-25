// One choice of a Segmented control or of Tabs: a value and the label that
// shows it. A plain string is its own label.

export type Choice<T extends string = string> = T | { value: T; label: string };

/** The value and the label of a choice. */
export const choiceOf = <T extends string>(choice: Choice<T>): { value: T; label: string } =>
    typeof choice === 'string' ? { value: choice, label: choice } : choice;
