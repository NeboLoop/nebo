<script lang="ts">
	let { emoji, bg, size = 'md' }: { emoji: string; bg: string; size?: 'sm' | 'md' | 'lg' | 'xl' } = $props();

	const sizes: Record<string, string> = {
		sm: 'w-10 h-10 rounded-xl text-lg',
		md: 'w-12 h-12 rounded-xl text-2xl',
		lg: 'w-14 h-14 rounded-2xl text-2xl',
		xl: 'w-18 h-18 rounded-2xl text-4xl'
	};
</script>

<div class="{sizes[size]} {bg} flex items-center justify-center shrink-0">{emoji}</div>
