<script setup>
import { computed } from 'vue'
import { searchTextParts } from '../searchHighlight.js'
const props = defineProps({ text: { type: String, default: '' }, query: { type: String, default: '' } })
const parts = computed(() => searchTextParts(props.text, props.query))
</script>

<template><template v-for="(part, index) in parts" :key="index"><mark v-if="part.matched" class="course-search-match">{{ part.text }}</mark><template v-else>{{ part.text }}</template></template></template>
