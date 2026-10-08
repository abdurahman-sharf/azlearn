<script setup lang="ts">
import { ref } from 'vue'
import { usePt } from '@/i18n/platform'
import { useDialog } from '@/composables/useDialog'
import { answerClearLocalBanks, signOutBankOpen } from '@/composables/signOutBank'

// "Clear the local question banks on this device?" - asked at sign-out, only when the browser actually holds banks or
// generated questions. Keeping them is the default (focus starts on it and Escape means keep): a bank is the teacher's
// own work and losing it is worse than leaving it on a computer that is not shared.
const pt = usePt()
const panel = ref<HTMLElement | null>(null)
useDialog(signOutBankOpen, panel, () => answerClearLocalBanks(false))
</script>

<template>
  <div v-if="signOutBankOpen" class="fixed inset-0 z-50 flex items-center justify-center p-4" style="background: rgb(0 0 0 / 0.5)" data-testid="signout-bank-dialog">
    <div ref="panel" class="card-elevated p-5 w-full max-w-md space-y-4" role="alertdialog" aria-modal="true" aria-labelledby="signout-bank-title" aria-describedby="signout-bank-msg">
      <h2 id="signout-bank-title" class="text-title-lg font-bold">{{ pt('soBankTitle') }}</h2>
      <p id="signout-bank-msg" class="text-body-md">{{ pt('soBankMsg') }}</p>
      <div class="flex flex-wrap gap-2 justify-end">
        <button type="button" class="btn-filled" data-autofocus data-testid="signout-bank-keep" @click="answerClearLocalBanks(false)">{{ pt('soBankKeep') }}</button>
        <button type="button" class="btn-outlined" data-testid="signout-bank-clear" @click="answerClearLocalBanks(true)">{{ pt('soBankClear') }}</button>
      </div>
    </div>
  </div>
</template>
