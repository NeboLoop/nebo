<script lang="ts">
  import { N } from '$lib/tokens.js';
  let { ch, size = 22, tone = 'idle' } = $props();

  const palette = $derived(({
    running: { bg: N.activeBg, fg: N.active },
    review:  { bg: N.needsBg,  fg: N.needs },
    idle:    { bg: N.idleBg,   fg: N.ink3 },
    blocked: { bg: N.blockedBg, fg: N.blocked },
  })[tone] || { bg: N.idleBg, fg: N.ink3 });
</script>

<span
  class="avatar"
  style:width="{size}px"
  style:height="{size}px"
  style:background={palette.bg}
  style:color={palette.fg}
  style:font-size="{size * 0.5}px"
>{ch}</span>
