// ============================================================================
// singleSelectToggleValue - Base UI's ToggleGroup always reports its value as
// an array, even in single-select mode, where an empty array means the
// pressed item was deselected. Extracts the one selected value, typed to the
// caller's option union, and skips the deselection case (a single-select
// group should always keep one item pressed).
// ============================================================================

export function singleSelectToggleValue<T extends string>(
  values: string[],
  onChange: (value: T) => void,
) {
  const [selected] = values;
  if (!selected) return;
  // SAFETY: callers only ever populate their group's items from `T`'s
  // members, so any value the group reports back is one of them.
  onChange(selected as T);
}
