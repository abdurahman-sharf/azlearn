<script setup lang="ts">
import { ref } from 'vue'
import { useAuthStore } from '@/stores/auth'
import { usePt, platformErrorMessage } from '@/i18n/platform'
import { changePassword, deleteAccount } from '@/api/platformOps'
import { useSignOut } from '@/composables/useSignOut'

const pt = usePt()
const auth = useAuthStore()
const signOut = useSignOut()

const current = ref('')
const next = ref('')
const msg = ref('')
const error = ref('')
const delPassword = ref('')
const delError = ref('')

async function savePassword() {
  msg.value = error.value = ''
  try {
    await changePassword(current.value, next.value)
    current.value = next.value = ''
    msg.value = pt('passwordChangedOk')
  } catch (e) {
    error.value = platformErrorMessage(pt, e)
  }
}

async function remove() {
  delError.value = ''
  if (!window.confirm(pt('deleteAccountHint'))) return
  try {
    await deleteAccount(delPassword.value)
    await signOut()
  } catch (e) {
    delError.value = platformErrorMessage(pt, e)
  }
}
</script>

<template>
  <div class="max-w-md mx-auto pb-8 space-y-6">
    <div>
      <router-link to="/platform" class="text-body-sm underline">{{ pt('back') }}</router-link>
      <h1 class="text-display-sm font-bold tracking-tight my-3">{{ pt('accountSettings') }}</h1>
      <p class="text-body-sm" dir="ltr" style="color: rgb(var(--md-on-surface-variant))">{{ auth.profile?.email }}</p>
    </div>

    <form class="card-elevated p-5 space-y-3" @submit.prevent="savePassword">
      <h2 class="font-bold">{{ pt('changePassword') }}</h2>
      <input v-model="current" type="password" required autocomplete="current-password" dir="ltr" :placeholder="pt('currentPassword')" class="input-outlined w-full" data-testid="pw-current" />
      <input v-model="next" type="password" required minlength="8" maxlength="128" autocomplete="new-password" dir="ltr" :placeholder="pt('newPassword')" class="input-outlined w-full" data-testid="pw-new" />
      <p v-if="error" class="text-body-sm" role="alert" style="color: rgb(var(--md-error))">{{ error }}</p>
      <p v-if="msg" class="text-body-sm" role="status">{{ msg }}</p>
      <button type="submit" class="btn-filled" data-testid="pw-save">{{ pt('save') }}</button>
    </form>

    <form class="card-filled p-5 space-y-3" @submit.prevent="remove">
      <h2 class="font-bold" style="color: rgb(var(--md-error))">{{ pt('deleteAccount') }}</h2>
      <p class="text-body-sm">{{ pt('deleteAccountHint') }}</p>
      <input v-model="delPassword" type="password" required autocomplete="current-password" dir="ltr" :placeholder="pt('deleteAccountConfirm')" class="input-outlined w-full" data-testid="del-password" />
      <p v-if="delError" class="text-body-sm" role="alert" style="color: rgb(var(--md-error))">{{ delError }}</p>
      <button type="submit" class="btn-outlined" data-testid="del-submit">{{ pt('deleteAccount') }}</button>
    </form>
  </div>
</template>
