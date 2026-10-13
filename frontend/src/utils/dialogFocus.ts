// Which of a dialog's focusable elements are real TAB STOPS. A browser tabs through a group of radio buttons as ONE stop (the
// checked radio, or the first while none is checked), so a focus trap that treats every enabled radio as a stop mis-wraps:
// with the second radio chosen, Shift+Tab from it is not "from the first element", and focus would leave the dialog.
// Kept free of the DOM so a plain script can test it.

export interface FocusCandidate {
  /** an `<input type="radio">` */
  isRadio: boolean
  /** its group (the `name` attribute); a radio without one is its own group */
  name: string
  checked: boolean
}

/** The indexes (ascending) of the candidates that are tab stops. */
export function tabStopIndexes(items: readonly FocusCandidate[]): number[] {
  const groups = new Map<string, number[]>()
  items.forEach((it, i) => {
    if (it.isRadio && it.name) groups.set(it.name, [...(groups.get(it.name) ?? []), i])
  })
  const stop = new Set<number>()
  for (const idx of groups.values()) stop.add(idx.find((i) => items[i]!.checked) ?? idx[0]!)
  return items.map((_, i) => i).filter((i) => !items[i]!.isRadio || !items[i]!.name || stop.has(i))
}
