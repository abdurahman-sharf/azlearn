/**
 * The study structure of an institution (department › level › year › term, with subjects hanging on any node or on the
 * institution itself) as plain functions, shared by the admin drill-down pages and the subject picker.
 * Everything is cycle-safe and orphan-safe: a unit whose parent is missing is treated as a root.
 */

export interface TreeUnit {
  id: string
  parent_id: string | null
  kind: string
  name_ar: string
  is_active: boolean
}
export interface TreeSubject {
  id: string
  unit_id: string | null
  is_active: boolean
}
export interface SubjectNumbers {
  exams: number
  attempts: number
  questions: number
}
export interface Rollup extends SubjectNumbers {
  childUnits: number
  descendantUnits: number
  subjects: number
}

export interface TreeIndex<U extends TreeUnit, S extends TreeSubject> {
  units: Map<string, U>
  /** children by parent id (`null` = roots), in the order the units were given */
  kids: Map<string | null, U[]>
  /** subjects by unit id (`null` = hanging on the institution itself) */
  subjectsAt: Map<string | null, S[]>
  subjects: S[]
}

/** Ranks of the unit kinds: a child's rank must be greater than its parent's (mirrors the server). */
export const KIND_RANK: Record<string, number> = { department: 0, level: 1, year: 2, term: 3 }

/** Kinds that may be created under `parentKind` (`null` = at the top); schools have no departments. */
export function allowedKinds(parentKind: string | null, isSchool: boolean): string[] {
  const min = parentKind === null ? 0 : (KIND_RANK[parentKind] ?? 99) + 1
  return Object.keys(KIND_RANK).filter((k) => (KIND_RANK[k] as number) >= min && !(isSchool && k === 'department'))
}

export function buildIndex<U extends TreeUnit, S extends TreeSubject>(units: U[], subjects: S[]): TreeIndex<U, S> {
  const byId = new Map(units.map((u) => [u.id, u]))
  const kids = new Map<string | null, U[]>()
  for (const u of units) {
    const parent = u.parent_id !== null && byId.has(u.parent_id) && u.parent_id !== u.id ? u.parent_id : null
    const list = kids.get(parent)
    if (list) list.push(u)
    else kids.set(parent, [u])
  }
  const subjectsAt = new Map<string | null, S[]>()
  for (const s of subjects) {
    const at = s.unit_id !== null && byId.has(s.unit_id) ? s.unit_id : null
    const list = subjectsAt.get(at)
    if (list) list.push(s)
    else subjectsAt.set(at, [s])
  }
  return { units: byId, kids, subjectsAt, subjects }
}

export function children<U extends TreeUnit, S extends TreeSubject>(idx: TreeIndex<U, S>, parentId: string | null): U[] {
  return idx.kids.get(parentId) ?? []
}

/** Every unit below `id` (not `id` itself), parents before children. */
export function descendants<U extends TreeUnit, S extends TreeSubject>(idx: TreeIndex<U, S>, id: string | null): U[] {
  const out: U[] = []
  const seen = new Set<string>()
  const walk = (parent: string | null) => {
    for (const u of children(idx, parent)) {
      if (seen.has(u.id)) continue
      seen.add(u.id)
      out.push(u)
      walk(u.id)
    }
  }
  walk(id)
  return out
}

/** Subjects at `id` and everywhere below it; `null` = every subject of the institution. */
export function subtreeSubjects<U extends TreeUnit, S extends TreeSubject>(idx: TreeIndex<U, S>, id: string | null): S[] {
  if (id === null) return idx.subjects
  const out = [...(idx.subjectsAt.get(id) ?? [])]
  for (const u of descendants(idx, id)) out.push(...(idx.subjectsAt.get(u.id) ?? []))
  return out
}

/** Units from the root down to (excluding) `id`. Empty for a root or an unknown id. */
export function ancestors<U extends TreeUnit, S extends TreeSubject>(idx: TreeIndex<U, S>, id: string): U[] {
  const out: U[] = []
  const seen = new Set<string>([id])
  let cur = idx.units.get(id)
  while (cur && cur.parent_id !== null) {
    const parent = idx.units.get(cur.parent_id)
    if (!parent || seen.has(parent.id)) break
    seen.add(parent.id)
    out.unshift(parent)
    cur = parent
  }
  return out
}

/** The chain of units a subject sits in, root first and including its own unit; empty when it hangs on the institution. */
export function pathToSubject<U extends TreeUnit, S extends TreeSubject>(idx: TreeIndex<U, S>, subjectId: string): U[] {
  const s = idx.subjects.find((x) => x.id === subjectId)
  if (!s || s.unit_id === null) return []
  const unit = idx.units.get(s.unit_id)
  return unit ? [...ancestors(idx, unit.id), unit] : []
}

/** Counts for a node (`null` = the whole institution): sub-levels, subjects, and the summed per-subject numbers. */
export function rollup<U extends TreeUnit, S extends TreeSubject>(idx: TreeIndex<U, S>, id: string | null, numbers?: Map<string, SubjectNumbers>): Rollup {
  const subs = subtreeSubjects(idx, id)
  const total: Rollup = { childUnits: children(idx, id).length, descendantUnits: descendants(idx, id).length, subjects: subs.length, exams: 0, attempts: 0, questions: 0 }
  for (const s of subs) {
    const n = numbers?.get(s.id)
    if (!n) continue
    total.exams += n.exams
    total.attempts += n.attempts
    total.questions += n.questions
  }
  return total
}
