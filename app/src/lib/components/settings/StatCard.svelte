<script lang="ts">
  // A single metric card, matching the MCP Servers summary cards.
  let {
    label,
    value,
    accent = 'default',
  }: {
    label: string;
    value: string | number;
    accent?: 'default' | 'success';
  } = $props();
</script>

<div class="flex-1 p-3.5 rounded-lg border border-base-300 bg-base-100">
  <div class="text-xs text-base-content/50 mb-0.5">{label}</div>
  <div class="text-lg font-bold {accent === 'success' ? 'text-success' : ''}">{value}</div>
</div>
