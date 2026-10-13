<script setup lang="ts">
import ExamEditor from '@/components/platform/ExamEditor.vue'
</script>

<template>
  <!-- The teacher's exam builder is the shared editor in teacher mode (Excel import, manual entry, a subset of the local banks). -->
  <ExamEditor mode="teacher" />
</template>
