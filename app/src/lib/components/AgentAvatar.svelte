<!--
  The ONE employee avatar. Every roster row, card, header, chip and settings
  panel draws this, so an employee looks the same everywhere it appears.

  An employee is its mountain character (`$lib/characters`) in its colour,
  with eyes that blink; the primary is the assistant character in brand
  blue. The server decides the shape (`shape` on the roster); an employee it
  sent none for still wears a stable character from its id or name.
-->
<script lang="ts">
  import { AGENT_COLORS_MAP, agentColorName } from '$lib/tokens';
  import { characterSvg } from '$lib/characters';

  let {
    name,
    id = '',
    color = null,
    shape = null,
    size = 'md',
    class: cls = ''
  }: {
    name: string;
    /** The employee's id: the stable key when the server sent no shape. */
    id?: string;
    /** Colour name from the map. Brand colour belongs to the assistant alone:
     *  an employee with no colour (or teal) wears its stable palette colour. */
    color?: string | null;
    /** The character the server resolved for this employee. */
    shape?: string | null;
    size?: 'xs' | 'sm' | 'md' | 'lg' | 'xl';
    class?: string;
  } = $props();

  const SIZES = {
    xs: 'w-5 h-5',
    sm: 'w-7 h-7',
    md: 'w-9 h-9',
    lg: 'w-12 h-12',
    xl: 'w-24 h-24'
  } as const;

  const c = $derived(
    (color && color !== 'teal' && AGENT_COLORS_MAP[color]) || AGENT_COLORS_MAP[agentColorName(id || name, null)]
  );
</script>

{@html characterSvg(shape ?? '', id || name, c.fillClass, `${SIZES[size]} ${cls}`)}
