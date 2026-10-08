<script setup lang="ts">
import { ref, onMounted, watch } from 'vue'
import { useAuthStore } from '@/stores/auth'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import { reviews, putReview, deleteReview, type ReviewSummary } from '@/api/platformEngage'

const props = defineProps<{ targetType: 'course' | 'teacher'; targetId: string }>()
const pt = usePt()
const auth = useAuthStore()

const data = ref<ReviewSummary | null>(null)
const rating = ref(5)
const comment = ref('')
const error = ref('')

const stars = (n: number) => '★'.repeat(Math.round(n)) + '☆'.repeat(5 - Math.round(n))

async function load() {
  try {
    data.value = await reviews(props.targetType, props.targetId)
    if (data.value.mine) {
      rating.value = data.value.mine.rating
      comment.value = data.value.mine.comment ?? ''
    }
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}

async function submit() {
  error.value = ''
  try {
    await putReview({ target_type: props.targetType, target_id: props.targetId, rating: rating.value, comment: comment.value || undefined })
    await load()
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}

async function remove(id: string) {
  try {
    await deleteReview(id)
    comment.value = ''
    await load()
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}

onMounted(load)
watch(() => props.targetId, load)
</script>

<template>
  <section v-if="data" class="space-y-3" data-testid="reviews">
    <h2 class="text-title-md font-bold">
      {{ pt('reviewsTitle') }}
      <span v-if="data.count" class="text-body-lg" style="color: rgb(var(--md-primary))">
        <span aria-hidden="true">{{ stars(data.average) }}</span> {{ data.average }} ({{ data.count }} {{ pt('reviewsCount') }})
      </span>
    </h2>

    <form v-if="data.can_review" class="card-elevated p-3 space-y-2" @submit.prevent="submit">
      <div class="font-semibold">{{ pt('yourRating') }}</div>
      <div class="flex gap-1" role="radiogroup" :aria-label="pt('yourRating')">
        <button v-for="n in 5" :key="n" type="button" role="radio" :aria-checked="rating === n" :aria-label="`${n}`" class="text-2xl leading-none px-1" :style="{ color: n <= rating ? 'rgb(var(--md-primary))' : 'rgb(var(--md-outline))' }" @click="rating = n">★</button>
      </div>
      <textarea v-model="comment" maxlength="1000" rows="2" dir="auto" :placeholder="pt('yourComment')" class="input-outlined w-full"></textarea>
      <p v-if="error" class="text-body-sm" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
      <button type="submit" class="btn-filled">{{ pt('submitReview') }}</button>
    </form>
    <p v-else-if="auth.role === 'student'" class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('onlyEnrolledReview') }}</p>
    <p v-if="error && !data.can_review" class="text-body-sm" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>

    <p v-if="!data.items.length" class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ pt('noReviews') }}</p>
    <ul class="space-y-2">
      <li v-for="r in data.items" :key="r.id" class="card-filled p-3">
        <div class="flex items-center gap-2">
          <span class="font-semibold">{{ r.student_name }}</span>
          <span aria-hidden="true" style="color: rgb(var(--md-primary))">{{ stars(r.rating) }}</span>
          <span class="sr-only">{{ r.rating }}/5</span>
          <button v-if="r.mine || auth.role === 'admin'" class="btn-text ms-auto" :aria-label="`${pt('deleteReview')}: ${r.student_name}`" @click="remove(r.id)">{{ pt('deleteReview') }}</button>
        </div>
        <!-- plain text only -->
        <p v-if="r.comment" class="text-body-lg whitespace-pre-line break-words" dir="auto">{{ r.comment }}</p>
      </li>
    </ul>
  </section>
</template>
