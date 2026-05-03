<script lang="ts">
  let { children } = $props();
</script>

<div class="flex-1 overflow-y-auto p-7">
  {@render children()}
</div>
