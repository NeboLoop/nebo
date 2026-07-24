<script lang="ts">
  // NeboAI mark. Mountain inherits currentColor so it adapts to the theme
  // (navy in light mode, white in dark); the diamond stays brand-blue.
  let { class: klass = 'w-6 h-6' }: { class?: string } = $props();
</script>

<svg viewBox="83 101 560 560" class={klass} role="img" aria-label="Nebo">
  <path fill="currentColor" d="M111 571 L276 314 L399 453 L455 322 L615 571 L424 571 L284 413 L217 571 Z" />
  <path fill="#2F60C9" d="M372.5 191 L436.5 305 L383 384 L311 306 Z" />
</svg>
