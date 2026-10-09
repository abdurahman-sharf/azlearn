<script setup lang="ts">
import ExamEditor from '@/components/platform/ExamEditor.vue'
</script>

<template>
  <!-- The admin's exam builder is the shared editor in admin mode (server bank, AI generation, any subject). -->
  <ExamEditor mode="admin" />
</template>
