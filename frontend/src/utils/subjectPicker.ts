import type { TreeSubject, TreeUnit } from './structureTree'

// Pure rules of the SubjectPicker's `statusBySubject` / `activeOnly` options, kept out of the component so a plain script
// can test them.

/** A teacher's assignment status for a subject. */
export type AssignmentStatus = 'pending' | 'approved' | 'rejected'

/**
 * Whether a subject may still be picked for a new teaching request: one that is waiting or already approved cannot be
 * asked for again; a rejected one can (that is the "ask again" path); a subject with no assignment at all is free.
 */
export function subjectSelectable(status: AssignmentStatus | undefined): boolean {
  return status !== 'pending' && status !== 'approved'
}

/**
 * Drops what a teacher must not be offered: inactive units (together with everything below them) and subjects that are
 * inactive or hang under an inactive unit. The relative order is kept. A unit whose parent is missing counts as a root
 * and a looping parent chain ends at the repeated unit, so broken data never hangs the page.
 */
export function activeSubset<U extends TreeUnit, S extends TreeSubject>(units: U[], subjects: S[]): { units: U[]; subjects: S[] } {
  const byId = new Map(units.map((u) => [u.id, u]))
  // "usable" = the unit and every unit above it are active
  const usable = new Map<string, boolean>()
  const check = (id: string): boolean => {
    const known = usable.get(id)
    if (known !== undefined) return known
    const chain: string[] = []
    const seen = new Set<string>()
    let ok = true
    let cur: U | undefined = byId.get(id)
    while (cur && !seen.has(cur.id)) {
      seen.add(cur.id)
      chain.push(cur.id)
      if (!cur.is_active) {
        ok = false
        break
      }
      cur = cur.parent_id !== null ? byId.get(cur.parent_id) : undefined
    }
    // everything walked is below the first inactive unit (or below nothing inactive at all)
    for (const c of chain) usable.set(c, ok)
    return ok
  }
  return {
    units: units.filter((u) => check(u.id)),
    subjects: subjects.filter((s) => s.is_active && (s.unit_id === null || !byId.has(s.unit_id) || check(s.unit_id))),
  }
}

/**
 * Keeps only the given subjects and the units on the way to them (a unit's ancestors stay so the cascade of lists can
 * still be walked down to the subject). Used when a teacher may only pick the subjects they are approved for. The relative
 * order is kept; a looping or broken parent chain ends at the repeated or missing unit, so bad data never hangs the page.
 */
export function restrictToSubjects<U extends TreeUnit, S extends TreeSubject>(units: U[], subjects: S[], allowed: ReadonlySet<string>): { units: U[]; subjects: S[] } {
  const kept = subjects.filter((s) => allowed.has(s.id))
  const byId = new Map(units.map((u) => [u.id, u]))
  const keep = new Set<string>()
  for (const s of kept) {
    let cur = s.unit_id !== null ? byId.get(s.unit_id) : undefined
    while (cur && !keep.has(cur.id)) {
      keep.add(cur.id)
      cur = cur.parent_id !== null ? byId.get(cur.parent_id) : undefined
    }
  }
  return { units: units.filter((u) => keep.has(u.id)), subjects: kept }
}
