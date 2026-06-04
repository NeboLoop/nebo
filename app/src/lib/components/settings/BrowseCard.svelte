<script lang="ts">
  import ExternalLink from 'lucide-svelte/icons/external-link';

  // Marketplace footer card, matching the MCP Servers "Browse Connectors" block.
  let {
    title,
    description,
    href,
  }: {
    title: string;
    description: string;
    href: string;
  } = $props();
</script>

<div class="p-4 rounded-lg border border-base-300 bg-base-100">
  <div class="flex items-center justify-between">
    <div>
      <div class="text-sm font-semibold mb-0.5">{title}</div>
      <div class="text-xs text-base-content/70">{description}</div>
    </div>
    <a
      {href}
      class="flex items-center gap-1.5 px-3 py-1.5 rounded-lg border border-base-300 text-sm font-medium hover:bg-base-200 transition-colors"
    >
      Marketplace <ExternalLink class="w-3.5 h-3.5" />
    </a>
  </div>
</div>
