import { getToken, platformFetch, PlatformError } from '@/lib/platformApi'

export type PostKind = 'article' | 'summary'
export type PubStatus = 'draft' | 'published'
export interface FileInfo { id: string; name: string; size: number }
export interface Post {
  id: string; teacher_id: string; teacher_name: string; subject_id: string; subject_name: string
  kind: PostKind; title: string; excerpt: string; body: string | null; status: PubStatus
  file: FileInfo | null; created_at: number; updated_at: number
}
export interface Course {
  id: string; teacher_id: string; teacher_name: string; subject_id: string; subject_name: string
  title: string; description: string | null; status: PubStatus; lesson_count: number; updated_at: number
}
export interface Lesson {
  id: string; course_id: string; position: number; section: string | null; title: string
  description: string | null; video_url: string; embed_url: string | null
  video_kind: 'youtube' | 'vimeo' | 'file' | 'link'
}
export interface CourseDetail extends Course { lessons: Lesson[] }
export interface Live {
  id: string; teacher_id: string; teacher_name: string; subject_id: string; subject_name: string
  title: string; description: string | null; starts_at: number; duration_min: number
  join_url: string; status: 'scheduled' | 'cancelled'
}
export interface Bundle { posts: Post[]; courses: Course[]; live: Live[] }

export const subjectContent = (id: string) => platformFetch<Bundle>(`/subjects/${id}/content`)
export const teacherContent = (id: string) => platformFetch<Bundle>(`/teachers/${id}/content`)
export const myContent = () => platformFetch<Bundle>('/my/content')
export const feed = () => platformFetch<Bundle>('/feed')

export const createPost = (b: { subject_id: string; kind: PostKind; title: string; body: string; status: PubStatus }) =>
  platformFetch<Post>('/posts', { body: b })
export const getPost = (id: string) => platformFetch<Post>(`/posts/${id}`)
export const updatePost = (id: string, b: Partial<{ kind: PostKind; title: string; body: string; status: PubStatus }>) =>
  platformFetch<Post>(`/posts/${id}`, { method: 'PATCH', body: b })
export const deletePost = (id: string) => platformFetch<void>(`/posts/${id}`, { method: 'DELETE' })
export const removePostFile = (id: string) => platformFetch<void>(`/posts/${id}/file`, { method: 'DELETE' })

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
export const addLesson = (courseId: string, b: { title: string; video_url: string; description?: string; section?: string }) =>
  platformFetch<Lesson>(`/courses/${courseId}/lessons`, { body: b })
export const deleteLesson = (id: string) => platformFetch<void>(`/lessons/${id}`, { method: 'DELETE' })
export const moveLesson = (id: string, direction: 'up' | 'down') =>
  platformFetch<void>(`/lessons/${id}/move`, { body: { direction } })

export const createLive = (b: { subject_id: string; title: string; description?: string; starts_at: number; duration_min: number; join_url: string }) =>
  platformFetch<Live>('/live', { body: b })
export const updateLive = (id: string, b: Partial<{ title: string; description: string; starts_at: number; duration_min: number; join_url: string; status: 'scheduled' | 'cancelled' }>) =>
  platformFetch<Live>(`/live/${id}`, { method: 'PATCH', body: b })
export const deleteLive = (id: string) => platformFetch<void>(`/live/${id}`, { method: 'DELETE' })
