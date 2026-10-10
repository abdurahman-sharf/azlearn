import { platformFetch } from '@/lib/platformApi'

export interface AppNotification {
  id: string
  kind: string
  data: Record<string, string | number | null>
  link: string | null
  read: boolean
  created_at: number
}
/** `unread` is the user's total unread count (unaffected by the filters); `next` is the cursor of the following page, null on the last. */
export interface NotificationList { unread: number; items: AppNotification[]; next?: string | null }
export type NotificationGroup = 'exams' | 'content' | 'people' | 'account'
export interface NotificationQuery { limit?: number; cursor?: string; group?: NotificationGroup; unread?: boolean }
export interface Review { id: string; rating: number; comment: string | null; student_name: string; mine: boolean; created_at: number }
export interface ReviewSummary { average: number; count: number; can_review: boolean; mine: Review | null; items: Review[] }
export interface ProgressItem { course_id: string; title: string; subject_name: string; total: number; completed: number }

export const notifications = (q: NotificationQuery = {}) => {
  const p = new URLSearchParams()
  if (q.limit) p.set('limit', String(q.limit))
  if (q.cursor) p.set('cursor', q.cursor)
  if (q.group) p.set('group', q.group)
  if (q.unread) p.set('unread', '1')
  const s = p.toString()
  return platformFetch<NotificationList>(`/notifications${s ? `?${s}` : ''}`)
}
export const markRead = (ids?: string[]) => platformFetch<void>('/notifications/read', { body: { ids } })

export const courseProgress = (id: string) => platformFetch<{ completed: string[] }>(`/courses/${id}/progress`)
export const setLessonDone = (id: string, done: boolean) =>
  platformFetch<void>(`/lessons/${id}/complete`, { method: done ? 'PUT' : 'DELETE', ...(done ? { body: {} } : {}) })
export const myProgress = () => platformFetch<ProgressItem[]>('/my/progress')

export const reviews = (type: 'course' | 'teacher', id: string) => platformFetch<ReviewSummary>(`/reviews/${type}/${id}`)
export const putReview = (b: { target_type: 'course' | 'teacher'; target_id: string; rating: number; comment?: string }) =>
  platformFetch<void>('/reviews', { method: 'PUT', body: b })
export const deleteReview = (id: string) => platformFetch<void>(`/reviews/${id}`, { method: 'DELETE' })
