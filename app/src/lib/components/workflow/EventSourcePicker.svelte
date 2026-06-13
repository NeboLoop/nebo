<script lang="ts">
	import type { EventSourceOption } from '$lib/api/neboComponents';

	let {
		value = '',
		suggestions = [],
		placeholder = 'Type to search sources...',
		onchange,
	}: {
		/** Comma-separated source list (the trigger's wire format). */
		value?: string;
		suggestions?: EventSourceOption[];
		placeholder?: string;
		onchange?: (value: string) => void;
	} = $props();

	let query = $state('');
	let open = $state(false);
	let highlighted = $state(0);
	let inputEl = $state<HTMLInputElement | null>(null);

	const tokens = $derived(
		value.split(',').map((s) => s.trim()).filter(Boolean)
	);
	const filtered = $derived.by(() => {
		const taken = new Set(tokens);
		const q = query.trim().toLowerCase();
		return suggestions
			.filter((s) => !taken.has(s.value))
			.filter((s) => !q || s.value.toLowerCase().includes(q) || s.label.toLowerCase().includes(q))
			.slice(0, 30);
	});

	function emit(next: string[]) {
		onchange?.(next.join(', '));
	}

	function addToken(token: string) {
		const trimmed = token.trim();
		if (!trimmed || tokens.includes(trimmed)) return;
		emit([...tokens, trimmed]);
		query = '';
		highlighted = 0;
		inputEl?.focus();
	}

	function removeToken(token: string) {
		emit(tokens.filter((t) => t !== token));
		inputEl?.focus();
	}

	function onKeydown(e: KeyboardEvent) {
		if (e.key === 'Enter') {
			e.preventDefault();
			if (open && filtered[highlighted]) addToken(filtered[highlighted].value);
			else if (query.trim()) addToken(query); // free text: custom names + wildcards (email.*)
		} else if (e.key === 'Backspace' && !query && tokens.length > 0) {
			removeToken(tokens[tokens.length - 1]);
		} else if (e.key === 'ArrowDown') {
			e.preventDefault();
			open = true;
			highlighted = Math.min(highlighted + 1, filtered.length - 1);
		} else if (e.key === 'ArrowUp') {
			e.preventDefault();
			highlighted = Math.max(highlighted - 1, 0);
		} else if (e.key === 'Escape') {
			open = false;
		}
	}
</script>

<div class="relative">
	<!-- A label wrapper gives click-anywhere-to-focus natively. -->
	<label
		class="input input-sm input-bordered w-full h-auto min-h-9 py-1 px-2 flex flex-wrap items-center gap-1 cursor-text"
		aria-label="Event sources"
	>
		{#each tokens as token (token)}
			<span class="badge badge-sm gap-1 font-mono text-xs bg-primary/10 text-primary border-none">
				{token}
				<button
					class="cursor-pointer bg-transparent border-none p-0 leading-none text-primary/60 hover:text-error"
					aria-label="Remove {token}"
					onclick={(e) => { e.stopPropagation(); removeToken(token); }}
				>&times;</button>
			</span>
		{/each}
		<input
			bind:this={inputEl}
			bind:value={query}
			class="flex-1 min-w-28 bg-transparent outline-none border-none text-xs font-mono py-0.5"
			placeholder={tokens.length === 0 ? placeholder : ''}
			role="combobox"
			aria-expanded={open}
			aria-controls="event-source-options"
			onfocus={() => { open = true; highlighted = 0; }}
			onblur={() => setTimeout(() => { open = false; }, 150)}
			oninput={() => { open = true; highlighted = 0; }}
			onkeydown={onKeydown}
		/>
	</label>

	{#if open && filtered.length > 0}
		<div
			id="event-source-options"
			role="listbox"
			class="absolute left-0 right-0 top-full mt-1 z-30 rounded-lg border border-base-300 bg-base-100 shadow-lg max-h-52 overflow-y-auto"
		>
			{#each filtered as src, i (src.value)}
				<button
					role="option"
					aria-selected={i === highlighted}
					class="w-full text-left px-2.5 py-1.5 cursor-pointer border-none flex flex-col gap-0.5 {i === highlighted ? 'bg-primary/10' : 'bg-transparent hover:bg-base-200'}"
					onmousedown={(e) => { e.preventDefault(); addToken(src.value); }}
					onmouseenter={() => { highlighted = i; }}
				>
					<span class="text-xs font-mono">{src.value}</span>
					<span class="text-xs text-base-content/50">{src.label}{src.description ? ` — ${src.description}` : ''}</span>
				</button>
			{/each}
		</div>
	{/if}
</div>
