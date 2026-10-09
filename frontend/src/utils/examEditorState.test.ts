import { editorState, examsSwitch, type EditorExam } from './examEditorState.ts'

// Plain-script style like the other tests in this folder (type-checked without Node typings).
function eq(a: unknown, b: unknown, msg: string): void {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`)
}
const ex = (p: Partial<EditorExam> = {}): EditorExam => ({ status: 'draft', attempt_count: 0, can_edit: true, teacher_id: 't1', owned: true, locked: false, ...p })

// a new exam: everything open, publish available, no notice
eq(editorState(null), { frozen: false, archived: false, readOnly: false, contentLocked: false, settingsLocked: false, canPublish: true, notice: null }, 'new exam')

// own draft
eq(editorState(ex()).notice, null, 'own draft: no notice')
eq(editorState(ex()).canPublish, true, 'own draft can be published')
eq(editorState(ex({ status: 'published' })).canPublish, false, 'published: save changes only')
eq(editorState(ex({ status: 'closed' })).canPublish, false, 'closed: save changes only')

// frozen (attempts exist, still editable): content locked, settings open
const frozen = editorState(ex({ status: 'published', attempt_count: 3 }))
eq([frozen.frozen, frozen.contentLocked, frozen.settingsLocked, frozen.readOnly, frozen.notice], [true, true, false, false, 'frozen'], 'frozen: content locked, the tail settings stay open')

// archived by the owner: read-only, says archived
const arch = editorState(ex({ status: 'archived', can_edit: false }))
eq([arch.readOnly, arch.contentLocked, arch.settingsLocked, arch.notice], [true, true, true, 'archived'], 'archived: everything read-only')

// locked by an admin: read-only, says locked (also wins over archived)
eq(editorState(ex({ status: 'closed', can_edit: false, locked: true })).notice, 'locked', 'locked by an admin')
eq(editorState(ex({ status: 'archived', can_edit: false, locked: true })).notice, 'locked', 'archived by an admin: locked wins')

// an admin looking at a teacher's exam
const theirs = editorState(ex({ owned: false, can_edit: false, status: 'published', attempt_count: 2 }))
eq([theirs.readOnly, theirs.notice], [true, 'teacher-owned'], 'not owned: moderation only')
// a server answer without `owned`: an exam with a teacher is not the admin's own
eq(editorState({ status: 'published', attempt_count: 0, can_edit: false, teacher_id: 't9' }).notice, 'teacher-owned', 'owned missing, has a teacher')
eq(editorState({ status: 'archived', attempt_count: 0, can_edit: false, teacher_id: null }).notice, 'archived', 'owned missing, admin exam archived')

// the switch
eq(examsSwitch('teacher', false, null), { createBlocked: false, publishBlocked: false }, 'switch on: nothing blocked')
eq(examsSwitch('teacher', true, null), { createBlocked: true, publishBlocked: true }, 'switch off: a new exam is blocked')
eq(examsSwitch('teacher', true, { status: 'draft' }), { createBlocked: false, publishBlocked: true }, 'switch off: a draft cannot be published')
eq(examsSwitch('teacher', true, { status: 'published' }), { createBlocked: false, publishBlocked: false }, 'switch off: a published exam stays manageable')
eq(examsSwitch('teacher', true, { status: 'closed' }), { createBlocked: false, publishBlocked: false }, 'switch off: a closed exam stays manageable')
eq(examsSwitch('admin', true, null), { createBlocked: false, publishBlocked: false }, 'the admin is never blocked by the switch')

console.log('exam editor state tests ok')
