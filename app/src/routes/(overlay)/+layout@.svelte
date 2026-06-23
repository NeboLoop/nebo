<script lang="ts">
  import '../../app.css';
  let { children } = $props();
</script>

<!-- Bare, transparent shell for floating always-on-top windows (notification HUD).
     The window itself is transparent; only the card paints. -->
<div class="nebo-overlay-root min-h-screen w-screen overflow-hidden bg-transparent">
  {@render children()}
</div>
