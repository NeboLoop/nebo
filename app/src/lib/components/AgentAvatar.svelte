<!--
  The ONE employee avatar: two initials on a solid chip in the employee's
  colour. Every roster row, card, header, and settings panel draws this, so
  an employee looks the same everywhere it appears.
-->
<script lang="ts">
  import { AGENT_COLORS_MAP, initialsOf } from '$lib/tokens';

  let {
    name,
    color = null,
    size = 'md',
    class: cls = ''
  }: {
    name: string;
    /** Colour name from the map; unknown or missing falls back to the brand teal. */
    color?: string | null;
    size?: 'xs' | 'sm' | 'md' | 'lg';
    class?: string;
  } = $props();

  const SIZES = {
    xs: 'w-5 h-5 rounded-md text-[9px]',
    sm: 'w-7 h-7 rounded-lg text-[10px]',
    md: 'w-9 h-9 rounded-xl text-xs',
    lg: 'w-12 h-12 rounded-xl text-sm'
  } as const;

  const c = $derived(AGENT_COLORS_MAP[color ?? ''] ?? AGENT_COLORS_MAP.teal);
</script>

<span class="inline-flex items-center justify-center font-semibold tracking-wide shrink-0 select-none {SIZES[size]} {c.solidClass} {cls}" aria-hidden="true">{initialsOf(name)}</span>
