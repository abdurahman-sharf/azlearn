import { computed, ref } from 'vue'
import { deleteBookmark, myBookmarks, putBookmark, type BookmarkRow, type BookmarkType } from '@/api/platformStudent'
import { mergeKeys, mergeRows } from '@/utils/bookmarkMerge'

// The student's saved posts and courses, shared by every screen that shows a save button (post page, course page) and by
// the "Following & saved" page. The list is read ONCE per session (the pages only ask `has()`); the saved page asks for a
// fresh copy when it opens. A toggle is optimistic and rolls back when the server refuses. Reset on sign-out and on a lost
// session (like the student stats) so the next account never inherits the previous one's list.

const rows = ref<BookmarkRow[]>([])
const keys = ref<Set<string>>(new Set())
const loaded = ref(false)
const failed = ref(false)
let inflight: Promise<void> | null = null
// Bumped on sign-out / a lost session: an answer that arrives after it must not leak into the next account.
let generation = 0
/** keys with a request in flight (a second click on the same item waits for the first) */
const pending = ref<Set<string>>(new Set())
/**
 * What this device saved (`true`) or removed (`false`) since the current list request began. The server may have answered
 * before such a change was written, so the answer is merged with it (never allowed to undo the person's last click).
 */
let changedSince = new Map<string, boolean>()

export const bookmarkKey = (type: BookmarkType, id: string): string => `${type}:${id}`

/** Forgets the list (called on sign-out and on a lost session). */
export function resetBookmarks() {
  generation++
  rows.value = []
  keys.value = new Set()
  loaded.value = false
  failed.value = false
  inflight = null
  pending.value = new Set()
  changedSince = new Map()
}

export function useBookmarks() {
  /** Reads the list from the server (always; the pages that only need `has()` call `ensureLoaded`). */
  const refresh = (): Promise<void> => {
    if (!inflight) {
      const mine = generation
      changedSince = new Map()
      const p: Promise<void> = myBookmarks()
        .then((r) => {
          if (mine !== generation) return
          const keyOf = (b: BookmarkRow) => bookmarkKey(b.target_type, b.target_id)
          rows.value = mergeRows(r.items, keyOf, changedSince)
          keys.value = mergeKeys(r.items.map(keyOf), changedSince)
          loaded.value = true
          failed.value = false
        })
        .catch(() => {
          if (mine === generation) failed.value = true // the save button is optional; the saved page shows its own error
        })
        .finally(() => {
          if (inflight === p) inflight = null
        })
      inflight = p
    }
    return inflight
  }
  /** The first call of a session reads the list; later calls cost nothing. */
  const ensureLoaded = (): Promise<void> => (loaded.value ? Promise.resolve() : refresh())

  const has = (type: BookmarkType, id: string): boolean => keys.value.has(bookmarkKey(type, id))
  const busy = (type: BookmarkType, id: string): boolean => pending.value.has(bookmarkKey(type, id))

  /**
   * Saves (`on`) or removes an item. The change shows at once and is undone if the server refuses; the error is rethrown so
   * the caller can say why (`too_many_bookmarks`, `not_found` for an item that was hidden meanwhile).
   */
  async function set(type: BookmarkType, id: string, on: boolean): Promise<void> {
    const k = bookmarkKey(type, id)
    if (pending.value.has(k)) return
    const mine = generation
    const had = keys.value.has(k)
    const before = rows.value
    const mark = (next: Set<string>) => { keys.value = next }
    pending.value = new Set(pending.value).add(k)
    changedSince.set(k, on)
    const next = new Set(keys.value)
    if (on) next.add(k)
    else next.delete(k)
    mark(next)
    if (!on) rows.value = rows.value.filter((b) => bookmarkKey(b.target_type, b.target_id) !== k)
    try {
      await (on ? putBookmark(type, id) : deleteBookmark(type, id))
    } catch (e) {
      if (mine === generation) {
        changedSince.delete(k) // refused: the server's state is still the old one
        const back = new Set(keys.value)
        if (had) back.add(k)
        else back.delete(k)
        mark(back)
        rows.value = before
      }
      throw e
    } finally {
      if (mine === generation) {
        const rest = new Set(pending.value)
        rest.delete(k)
        pending.value = rest
      }
    }
  }

  return {
    rows,
    loaded,
    failed,
    count: computed(() => keys.value.size),
    refresh,
    ensureLoaded,
    has,
    busy,
    set,
  }
}
