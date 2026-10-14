<!--
  The ONE employee avatar. Every roster row, card, header, and settings
  panel draws this, so an employee looks the same everywhere it appears.

  An employee is its mountain character (`$lib/characters`) in its colour,
  with eyes that blink; the primary is the assistant character in brand
  blue. The server decides the shape (`shape` on the roster). With no shape
  it is two initials on a chip in the employee's colour.
-->
<script lang="ts">
  import { AGENT_COLORS_MAP, initialsOf } from '$lib/tokens';
  import { characterOf, EYE_WHITE, PUPIL, PUPIL_DX, PUPIL_DY, PUPIL_R } from '$lib/characters';

  let {
    name,
    color = null,
    shape = null,
    size = 'md',
    class: cls = ''
  }: {
    name: string;
    /** Colour name from the map; unknown or missing falls back to the brand teal. */
    color?: string | null;
    /** The character the server resolved for this employee. */
    shape?: string | null;
    size?: 'xs' | 'sm' | 'md' | 'lg' | 'xl';
    class?: string;
  } = $props();

  const SIZES = {
    xs: 'w-5 h-5 rounded-md text-[9px]',
    sm: 'w-7 h-7 rounded-lg text-[10px]',
    md: 'w-9 h-9 rounded-xl text-xs',
    lg: 'w-12 h-12 rounded-xl text-sm',
    xl: 'w-24 h-24 rounded-2xl text-2xl'
  } as const;

  const c = $derived(AGENT_COLORS_MAP[color ?? ''] ?? AGENT_COLORS_MAP.teal);
  const spec = $derived(characterOf(shape));
  // A roster blinks out of step: each name keeps its own moment.
  const beat = $derived(`character-beat-${[...name].reduce((h, ch) => (h * 31 + ch.charCodeAt(0)) >>> 0, 0) % 5}`);
</script>

{#if spec}
  <svg class="character shrink-0 select-none {SIZES[size]} {cls}" viewBox="0 0 100 100" aria-hidden="true">
    {#each spec.layers as layer, i (i)}
      {@const fillCls = layer.fill === 'body' ? c.fillClass : ''}
      {@const fill = layer.fill === 'body' ? undefined : layer.fill}
      {@const anim = layer.anim ? `character-${layer.anim}` : ''}
      {#if layer.circle}
        <circle cx={layer.circle[0]} cy={layer.circle[1]} r={layer.circle[2]} {fill} class="{fillCls} {anim}" />
      {:else}
        <path d={layer.d} {fill} class="{fillCls} {anim}" />
      {/if}
    {/each}
    {#each spec.eyes as [x, y], i (i)}
      <g class="character-eye {beat}">
        <circle cx={x} cy={y} r={spec.eyeR} fill={EYE_WHITE} />
        <circle cx={x + spec.eyeR * PUPIL_DX} cy={y + spec.eyeR * PUPIL_DY} r={spec.eyeR * PUPIL_R} fill={PUPIL} />
      </g>
    {/each}
  </svg>
{:else}
  <span class="inline-flex items-center justify-center font-semibold tracking-wide shrink-0 select-none {SIZES[size]} {c.solidClass} {cls}" aria-hidden="true">{initialsOf(name)}</span>
{/if}
