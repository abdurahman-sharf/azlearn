import { getToken, platformFetch, PlatformError } from '@/lib/platformApi'

export type PostKind = 'article' | 'summary'
export type PubStatus = 'draft' | 'published'
/**
 * Why an owner's published item is NOT shown to students: the teacher has no approved assignment for the subject
 * (`no_assignment`), the subject or its institution was switched off, or the teacher's own account is not active.
 */
export type HiddenReason = 'no_assignment' | 'subject_inactive' | 'institution_inactive' | 'account_inactive'
/** Only the owner's own lists (`/my/content`) carry these; public lists show visible items only. */
export interface OwnerView {
  visible?: boolean
  hidden_reason?: HiddenReason | null
}
export interface FileInfo { id: string; name: string; size: number }
export interface Post extends OwnerView {
  id: string; teacher_id: string; teacher_name: string; subject_id: string; subject_name: string
  kind: PostKind; title: string; excerpt: string; body: string | null; status: PubStatus
  file: FileInfo | null; created_at: number; updated_at: number
}
export interface Course extends OwnerView {
  id: string; teacher_id: string; teacher_name: string; subject_id: string; subject_name: string
  title: string; description: string | null; status: PubStatus; lesson_count: number; updated_at: number
}
export interface Lesson {
  id: string; course_id: string; position: number; section: string | null; title: string
  description: string | null; video_url: string; embed_url: string | null
  video_kind: 'youtube' | 'vimeo' | 'file' | 'link'
}
export interface CourseDetail extends Course { lessons: Lesson[] }
export interface Live extends OwnerView {
  id: string; teacher_id: string; teacher_name: string; subject_id: string; subject_name: string
  title: string; description: string | null; starts_at: number; duration_min: number
  join_url: string; status: 'scheduled' | 'cancelled'
  /** the session's time has passed (only on the owner's lists and `GET /live/{id}`) */
  ended?: boolean
}
export interface Bundle { posts: Post[]; courses: Course[]; live: Live[] }
/** `/my/content`: the same lists plus how many items match the filters (before limit/offset), per type. */
export interface MyBundle extends Bundle { totals: { posts: number; courses: number; live: number } }

/** `hidden` = published/scheduled but not visible to students. */
export type ContentStatusFilter = 'draft' | 'published' | 'hidden'
export type ContentTypeFilter = 'post' | 'course' | 'live'
export interface ContentQuery {
  q?: string
  status?: ContentStatusFilter
  subject_id?: string
  /** only that list is filled, the others come back empty */
  type?: ContentTypeFilter
  /** `updated` (default, latest change first) or `title` (A-Z); applied before paging */
  sort?: 'updated' | 'title'
  /** per type (default 50, at most 100) */
  limit?: number
  offset?: number
}

function query(o: Record<string, string | number | undefined>): string {
  const p = new URLSearchParams()
  for (const [k, v] of Object.entries(o)) if (v !== undefined && v !== '') p.set(k, String(v))
  const s = p.toString()
  return s ? `?${s}` : ''
}

export const subjectContent = (id: string) => platformFetch<Bundle>(`/subjects/${id}/content`)
export const teacherContent = (id: string) => platformFetch<Bundle>(`/teachers/${id}/content`)
export const myContent = (c: ContentQuery = {}) => platformFetch<MyBundle>(`/my/content${query({ ...c })}`)
export const feed = () => platformFetch<Bundle>('/feed')

export const createPost = (b: { subject_id: string; kind: PostKind; title: string; body: string; status: PubStatus }) =>
  platformFetch<Post>('/posts', { body: b })
export const getPost = (id: string) => platformFetch<Post>(`/posts/${id}`)
export const updatePost = (id: string, b: Partial<{ kind: PostKind; title: string; body: string; status: PubStatus }>) =>
  platformFetch<Post>(`/posts/${id}`, { method: 'PATCH', body: b })
export const deletePost = (id: string) => platformFetch<void>(`/posts/${id}`, { method: 'DELETE' })
export const removePostFile = (id: string) => platformFetch<void>(`/posts/${id}/file`, { method: 'DELETE' })
/** Copies a post as a DRAFT (the body, not the attachment). `title` lets the copy say it is a copy. */
export const duplicatePost = (id: string, title?: string) => platformFetch<Post>(`/posts/${id}/duplicate`, { method: 'POST', body: { title } })

export async function uploadPostFile(id: string, file: File): Promise<FileInfo> {
  const fd = new FormData()
  fd.append('file', file)
  const token = getToken()
  const res = await fetch(`/api/platform/posts/${id}/file`, {
    method: 'POST',
    headers: token ? { Authorization: `Bearer ${token}` } : {},
    body: fd,
  }).catch(() => { throw new PlatformError('network', 0) })
  const data = await res.json().catch(() => ({}))
  if (!res.ok) throw new PlatformError((data as { error?: string }).error ?? (res.status === 413 ? 'invalid_file_size' : 'unknown'), res.status)
  return data as FileInfo
}

/** Downloads through fetch so the Bearer token is sent, then saves the blob. */
export async function downloadFile(f: FileInfo): Promise<void> {
  const token = getToken()
  const res = await fetch(`/api/platform/files/${f.id}`, { headers: token ? { Authorization: `Bearer ${token}` } : {} })
  if (!res.ok) throw new PlatformError('not_found', res.status)
  const url = URL.createObjectURL(await res.blob())
  const a = document.createElement('a')
  a.href = url
  a.download = f.name
  a.style.display = 'none'
  document.body.appendChild(a) // some browsers ignore `download` on detached anchors
  a.click()
  a.remove()
  setTimeout(() => URL.revokeObjectURL(url), 10_000)
}

export const createCourse = (b: { subject_id: string; title: string; description?: string; status: PubStatus }) =>
  platformFetch<CourseDetail>('/courses', { body: b })
export const getCourse = (id: string) => platformFetch<CourseDetail>(`/courses/${id}`)
export const updateCourse = (id: string, b: Partial<{ title: string; description: string; status: PubStatus }>) =>
  platformFetch<CourseDetail>(`/courses/${id}`, { method: 'PATCH', body: b })
export const deleteCourse = (id: string) => platformFetch<void>(`/courses/${id}`, { method: 'DELETE' })
/** Copies a course with its lessons as a DRAFT. */
export const duplicateCourse = (id: string, title?: string) => platformFetch<CourseDetail>(`/courses/${id}/duplicate`, { method: 'POST', body: { title } })
export const addLesson = (courseId: string, b: { title: string; video_url: string; description?: string; section?: string }) =>
  platformFetch<Lesson>(`/courses/${courseId}/lessons`, { body: b })
/** Edits a lesson in place (the id, and so students' completion marks, stay). Only the fields given change; an empty section/description clears it. */
export const updateLesson = (id: string, b: Partial<{ title: string; video_url: string; description: string; section: string }>) =>
  platformFetch<void>(`/lessons/${id}`, { method: 'PATCH', body: b })
export const deleteLesson = (id: string) => platformFetch<void>(`/lessons/${id}`, { method: 'DELETE' })
export const moveLesson = (id: string, direction: 'up' | 'down') =>
  platformFetch<void>(`/lessons/${id}/move`, { body: { direction } })

export const createLive = (b: { subject_id: string; title: string; description?: string; starts_at: number; duration_min: number; join_url: string }) =>
  platformFetch<Live>('/live', { body: b })
/** The reply is the session in the owner's shape (with `ended`, `visible`, `hidden_reason`), the same as `getLive`. */
export const updateLive = (id: string, b: Partial<{ title: string; description: string; starts_at: number; duration_min: number; join_url: string; status: 'scheduled' | 'cancelled' }>) =>
  platformFetch<Live>(`/live/${id}`, { method: 'PATCH', body: b })
export const deleteLive = (id: string) => platformFetch<void>(`/live/${id}`, { method: 'DELETE' })
export const getLive = (id: string) => platformFetch<Live>(`/live/${id}`)
