<script lang="ts">
	import { Mic, MicOff, ArrowUp, Plus, RotateCcw } from 'lucide-svelte';

	interface Props {
		value: string;
		placeholder?: string;
		disabled?: boolean;
		isLoading?: boolean;
		isRecording?: boolean;
		voiceMode?: boolean;
		onSend: () => void;
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
		onSend,
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
					placeholder={isLoading ? 'Type to queue your next message...' : placeholder}
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

					<!-- Send button -->
					<button
						type="button"
						onclick={handleSend}
						disabled={!canSend}
						class="btn btn-sm btn-circle transition-all {canSend
							? 'btn-primary'
							: 'btn-ghost bg-base-300 text-base-content/30'}"
						title={isLoading ? 'Queue message' : 'Send message'}
					>
						<ArrowUp class="w-4 h-4" />
					</button>
				</div>
			</div>
		</div>

		<!-- Disclaimer -->
		<p class="text-center text-xs text-base-content/40 mt-2">
			Nebo can make mistakes. Verify important information.
		</p>
	</div>
</div>
