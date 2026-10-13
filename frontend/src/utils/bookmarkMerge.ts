// Pure helper of the shared bookmark list (`lib/bookmarks.ts`): putting the server's list together with what THIS device
// changed while that list was being fetched.
//
// A save or a removal made while `GET /my/bookmarks` is in flight can be missing from (or still present in) the answer,
// because the server may have answered before the change was written. The answer must never undo a change the person just
// made, so every change made since the request began is applied on top of the server's list.

/** key -> `true` (saved) / `false` (removed) for the changes made since the list request began. */
export type LocalChanges = ReadonlyMap<string, boolean>

/** The server's keys with this device's newer changes applied. */
export function mergeKeys(serverKeys: Iterable<string>, changes: LocalChanges): Set<string> {
  const out = new Set(serverKeys)
  for (const [key, saved] of changes) {
    if (saved) out.add(key)
    else out.delete(key)
  }
  return out
}

/** The server's rows without the ones this device removed meanwhile (a row for an item saved meanwhile arrives with the next read). */
export function mergeRows<T>(rows: readonly T[], keyOf: (row: T) => string, changes: LocalChanges): T[] {
  return rows.filter((r) => changes.get(keyOf(r)) !== false)
}
