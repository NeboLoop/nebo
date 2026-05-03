<script lang="ts">
	import { Play } from 'lucide-svelte';

	interface MediaItem {
		id: string;
		url: string;
		mediaType: string;
		thumbnailUrl?: string;
		position: number;
	}

	let { media = [] }: { media: MediaItem[] } = $props();

	let lightboxUrl = $state<string | null>(null);
	let lightboxType = $state<string>('image');

	function openLightbox(item: MediaItem) {
		lightboxUrl = item.url;
		lightboxType = item.mediaType;
	}

	function closeLightbox() {
		lightboxUrl = null;
	}
</script>

{#if media.length > 0}
	<div class="px-5 py-5 border-b border-base-content/5">
		<h3 class="text-sm font-semibold text-base-content/40 uppercase tracking-wider mb-3">Preview</h3>
		<div class="flex gap-3 overflow-x-auto pb-2 -mx-1 px-1">
			{#each media as item}
				{#if item.mediaType === 'video'}
					<button type="button" onclick={() => openLightbox(item)} class="shrink-0 w-72 h-48 rounded-2xl bg-base-content/5 overflow-hidden relative group">
						{#if item.thumbnailUrl}
							<img src={item.thumbnailUrl} alt="" class="w-full h-full object-cover" />
						{:else}
							<div class="w-full h-full bg-base-content/10"></div>
						{/if}
						<div class="absolute inset-0 flex items-center justify-center bg-black/20 group-hover:bg-black/30 transition-colors">
							<div class="w-12 h-12 rounded-full bg-base-100/90 flex items-center justify-center">
								<Play class="w-5 h-5 text-base-content ml-0.5" />
							</div>
						</div>
					</button>
				{:else}
					<button type="button" onclick={() => openLightbox(item)} class="shrink-0 w-72 h-48 rounded-2xl bg-base-content/5 overflow-hidden group">
						<img src={item.url} alt="" class="w-full h-full object-cover group-hover:scale-105 transition-transform duration-300" />
					</button>
				{/if}
			{/each}
		</div>
	</div>
{/if}

<!-- Lightbox -->
{#if lightboxUrl}
	<div class="fixed inset-0 z-50 bg-black/80 backdrop-blur-sm flex items-center justify-center p-4">
		<button type="button" class="absolute inset-0" onclick={closeLightbox}></button>
		<div class="relative max-w-4xl max-h-[90vh] w-full">
			{#if lightboxType === 'video'}
				<!-- svelte-ignore a11y_media_has_caption -->
				<video src={lightboxUrl} controls autoplay class="w-full max-h-[90vh] rounded-2xl"></video>
			{:else}
				<img src={lightboxUrl} alt="" class="w-full max-h-[90vh] object-contain rounded-2xl" />
			{/if}
		</div>
	</div>
{/if}
