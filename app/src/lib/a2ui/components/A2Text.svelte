<script lang="ts">
  let { text = '', variant = 'body', class: className = '' }: {
    text?: string;
    variant?: string;
    class?: string;
  } = $props();

  const variantMap: Record<string, string> = {
    h1: 'text-base font-semibold',
    h2: 'text-base font-semibold',
    h3: 'text-2xl font-semibold tracking-tight',
    caption: 'text-xs text-base-content/70',
    label: 'text-xs font-semibold uppercase tracking-wider text-base-content/50',
    body: 'text-sm',
    mono: 'text-xs font-mono',
    meta: 'text-xs text-base-content/50 font-mono',
    'body-medium': 'text-sm font-medium',
    accent: 'text-sm text-primary',
  };
</script>

<span class="{variantMap[variant] ?? variantMap.body} {className}">{text}</span>
