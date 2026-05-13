<script lang="ts">
  import { page } from '$app/stores';
</script>

<div class="h-screen flex items-center justify-center bg-base-100 p-8">
  <div class="text-center max-w-lg">
    <div class="text-base font-semibold mb-2">App Error ({$page.status})</div>
    <pre class="text-xs text-base-content/70 whitespace-pre-wrap text-left bg-base-200 p-4 rounded-lg overflow-auto max-h-96">{$page.error?.message || 'Unknown error'}</pre>
  </div>
</div>
