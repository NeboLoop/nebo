<script lang="ts">
	import { Mic, MicOff, ArrowUp, Square, Plus, RotateCcw, X, Clock } from 'lucide-svelte';

	interface QueuedMessage {
		id: string;
		content: string;
	}

	interface Props {
		value: string;
		placeholder?: string;
		disabled?: boolean;
		isLoading?: boolean;
		isRecording?: boolean;
		voiceMode?: boolean;
		queuedMessages?: QueuedMessage[];
		onSend: () => void;
		onCancel?: () => void;
		onCancelQueued?: (id: string) => void;
		onNewSession?: () => void;
		onToggleVoice?: () => void;
	}

	let {
		value = $bindable(),
		placeholder = 'Reply...',
		disabled = false,
		isLoading = false,
		isRecording = false,
		voiceMode = false,
		queuedMessages = [],
		onSend,
		onCancel,
		onCancelQueued,
		onNewSession,
		onToggleVoice
	}: Props = $props();

	let textareaElement: HTMLTextAreaElement | undefined = $state();

	function handleKeydown(e: KeyboardEvent) {
		if (e.key === 'Enter' && !e.shiftKey) {
			e.preventDefault();
			if (value.trim()) {
				onSend();
			}
		}
	}

	function handleSend() {
		if (value.trim() && !disabled && !isRecording) {
			onSend();
		}
	}

	function adjustHeight() {
		if (textareaElement) {
			textareaElement.style.height = 'auto';
			textareaElement.style.height = Math.min(textareaElement.scrollHeight, 200) + 'px';
		}
	}

	$effect(() => {
		value;
		adjustHeight();
	});

	const canSend = $derived(value.trim() && !disabled && !isRecording);

	export function focus() {
		textareaElement?.focus();
	}
</script>

<div class="sticky bottom-0 flex-shrink-0 mt-auto px-4 pb-2 pt-2 z-10">
	<div class="max-w-4xl mx-auto">
		<!-- Queued messages tray -->
		{#if queuedMessages.length > 0}
			<div class="flex flex-wrap gap-2 mb-2 px-1">
				{#each queuedMessages as queued (queued.id)}
					<div class="flex items-center gap-1.5 px-3 py-1.5 rounded-xl bg-primary/10 border border-primary/20 text-sm text-base-content/70 animate-in fade-in slide-in-from-bottom-1">
						<Clock class="w-3 h-3 text-primary/50 flex-shrink-0" />
						<span class="truncate max-w-[200px]">{queued.content}</span>
						{#if onCancelQueued}
							<button
								type="button"
								onclick={() => onCancelQueued?.(queued.id)}
								class="p-0.5 rounded hover:bg-error/20 hover:text-error transition-colors flex-shrink-0"
								title="Cancel queued message"
							>
								<X class="w-3 h-3" />
							</button>
						{/if}
					</div>
				{/each}
			</div>
		{/if}

		<!-- Input container - modern rounded design -->
		<div
			class="bg-base-200 rounded-2xl border border-base-300 focus-within:border-base-content/20 transition-colors"
		>
			<!-- Textarea row -->
			<div class="px-4 pt-3 pb-2">
				<textarea
					bind:this={textareaElement}
					bind:value
					onkeydown={handleKeydown}
					oninput={adjustHeight}
					placeholder={isLoading
						? 'Type to queue your next message...'
						: placeholder}
					disabled={disabled || isRecording}
					rows="1"
					class="w-full bg-transparent border-none outline-none resize-none text-sm leading-relaxed placeholder:text-base-content/40 min-h-[24px] max-h-[200px]"
				></textarea>
			</div>

			<!-- Actions row -->
			<div class="flex items-center justify-between px-3 pb-3">
				<div class="flex items-center gap-1">
					<!-- Attachment button (placeholder for future) -->
					<button
						type="button"
						class="btn btn-ghost btn-sm btn-square rounded-lg text-base-content/50 hover:text-base-content"
						title="Add attachment"
						disabled
					>
						<Plus class="w-4 h-4" />
					</button>

					<!-- New session button -->
					{#if onNewSession}
						<button
							type="button"
							onclick={onNewSession}
							class="btn btn-ghost btn-sm btn-square rounded-lg text-base-content/50 hover:text-base-content"
							{disabled}
							title="New session"
						>
							<RotateCcw class="w-4 h-4" />
						</button>
					{/if}
				</div>

				<div class="flex items-center gap-2">
					<!-- Voice toggle -->
					{#if onToggleVoice}
						<button
							type="button"
							onclick={() => onToggleVoice?.()}
							disabled={isLoading}
							class="btn btn-ghost btn-sm btn-square rounded-lg {voiceMode
								? 'text-error'
								: 'text-base-content/50 hover:text-base-content'}"
							title={voiceMode ? 'Exit voice mode' : 'Voice input'}
						>
							{#if voiceMode}
								<MicOff class="w-4 h-4" />
							{:else}
								<Mic class="w-4 h-4" />
							{/if}
						</button>
					{/if}

					<!-- Send / Stop button -->
					{#if isLoading && onCancel}
						<button
							type="button"
							onclick={onCancel}
							class="btn btn-sm btn-circle btn-error transition-all"
							title="Stop generation"
						>
							<Square class="w-3.5 h-3.5 fill-current" />
						</button>
					{:else}
						<button
							type="button"
							onclick={handleSend}
							disabled={!canSend}
							class="btn btn-sm btn-circle transition-all {canSend
								? 'btn-primary'
								: 'btn-ghost bg-base-300 text-base-content/30'}"
							title="Send message"
						>
							<ArrowUp class="w-4 h-4" />
						</button>
					{/if}
				</div>
			</div>
		</div>

		<!-- Disclaimer -->
		<p class="text-center text-xs text-base-content/40 mt-2">
			Nebo can make mistakes. Verify important information.
		</p>
	</div>
</div>
