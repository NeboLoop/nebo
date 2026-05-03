<script lang="ts">
	import { onMount } from 'svelte';
	import * as api from '$lib/api/nebo';

	let input = $state('');
	let sending = $state(false);
	let sent = $state(false);
	let inputEl: HTMLInputElement | undefined = $state();

	onMount(() => {
		inputEl?.focus();

		// Close on Escape
		function onKey(e: KeyboardEvent) {
			if (e.key === 'Escape') {
				window.close();
			}
		}
		window.addEventListener('keydown', onKey);
		return () => window.removeEventListener('keydown', onKey);
	});

	async function send() {
		const text = input.trim();
		if (!text || sending) return;
		sending = true;
		try {
			await api.sendMessage({ chatId: 'main', content: text });
			input = '';
			sent = true;
			setTimeout(() => { sent = false; }, 1200);
		} catch {
			// Silently fail — server may be briefly unavailable
		} finally {
			sending = false;
			inputEl?.focus();
		}
	}

	function handleKeydown(e: KeyboardEvent) {
		if (e.key === 'Enter' && !e.shiftKey) {
			e.preventDefault();
			send();
		}
	}
</script>

<div class="prompt-bar" data-theme="dark">
	<div class="prompt-inner">
		<span class="prompt-icon">N</span>
		<input
			bind:this={inputEl}
			bind:value={input}
			onkeydown={handleKeydown}
			placeholder={sent ? 'Sent!' : 'Ask Nebo anything...'}
			disabled={sending}
			class="prompt-input"
			spellcheck="false"
			autocomplete="off"
		/>
		{#if input.trim()}
			<button onclick={send} disabled={sending} class="prompt-send">
				<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="22" y1="2" x2="11" y2="13"/><polygon points="22 2 15 22 11 13 2 9 22 2"/></svg>
			</button>
		{/if}
	</div>
</div>
