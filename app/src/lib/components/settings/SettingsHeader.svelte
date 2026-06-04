<script lang="ts">
  // The single settings page header. Every settings page uses this so the
  // title size, weight, spacing, and subtitle are identical everywhere.
  let {
    title,
    description,
  }: {
    title: string;
    description: string;
  } = $props();
</script>

<div class="mb-7">
  <h2 class="text-lg font-bold mb-1">{title}</h2>
  <p class="text-xs text-base-content/70">{description}</p>
</div>
