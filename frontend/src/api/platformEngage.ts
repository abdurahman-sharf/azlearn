import { platformFetch } from '@/lib/platformApi'

export interface AppNotification {
  id: string
  kind: string
  data: Record<string, string | number | null>
  link: string | null
  read: boolean
  created_at: number
}
export interface NotificationList { unread: number; items: AppNotification[] }
export interface Review { id: string; rating: number; comment: string | null; student_name: string; mine: boolean; created_at: number }
export interface ReviewSummary { average: number; count: number; can_review: boolean; mine: Review | null; items: Review[] }
export interface ProgressItem { course_id: string; title: string; subject_name: string; total: number; completed: number }

export const notifications = () => platformFetch<NotificationList>('/notifications')
export const markRead = (ids?: string[]) => platformFetch<void>('/notifications/read', { body: { ids } })

export const courseProgress = (id: string) => platformFetch<{ completed: string[] }>(`/courses/${id}/progress`)
export const setLessonDone = (id: string, done: boolean) =>
  platformFetch<void>(`/lessons/${id}/complete`, { method: done ? 'PUT' : 'DELETE', ...(done ? { body: {} } : {}) })
export const myProgress = () => platformFetch<ProgressItem[]>('/my/progress')

export const reviews = (type: 'course' | 'teacher', id: string) => platformFetch<ReviewSummary>(`/reviews/${type}/${id}`)
export const putReview = (b: { target_type: 'course' | 'teacher'; target_id: string; rating: number; comment?: string }) =>
  platformFetch<void>('/reviews', { method: 'PUT', body: b })
export const deleteReview = (id: string) => platformFetch<void>(`/reviews/${id}`, { method: 'DELETE' })
