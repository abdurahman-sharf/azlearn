// Small pure rules of the platform's navigation guard and sign-in notice, kept out of the router/views so they can be
// tested with a plain script.

/**
 * A teacher whose account still awaits approval may enter exactly the routes flagged `pendingTeacherOk` (the subject
 * request page): the server allows a pending teacher only that page's calls. Every other route that requires an active
 * account keeps sending them to the waiting page.
 */
export function pendingTeacherMayEnter(meta: Record<PropertyKey, unknown>, role: string | null | undefined, status: string | null | undefined): boolean {
  return meta.pendingTeacherOk === true && role === 'teacher' && status === 'pending'
}

/** The exam-taking page. When a session ends there, the answers typed so far stay in a local draft on the device. */
export function isExamTakePath(path: unknown): boolean {
  return typeof path === 'string' && /^\/platform\/assessments\/[^/?#]+\/take(?:[?#].*)?$/.test(path)
}
