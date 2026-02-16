<script lang="ts">
	import { Mic, MicOff, ArrowUp, Square, Plus, RotateCcw, X, Clock, FileText } from 'lucide-svelte';

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
		isDraggingOver?: boolean;
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
		isDraggingOver = false,
		onSend,
		onCancel,
		onCancelQueued,
		onNewSession,
		onToggleVoice
	}: Props = $props();

	let textareaElement: HTMLTextAreaElement | undefined = $state();
	let fileInputElement: HTMLInputElement | undefined = $state();

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

	/**
	 * Extract file paths from dropped files or file input.
	 * In Wails/Electron, File objects may have a `path` property.
	 * In a standard browser, we fall back to the file:// URI from dataTransfer,
	 * or just the filename as a last resort.
	 */
	function extractFilePaths(dataTransfer: DataTransfer | null, files?: FileList | null): string[] {
		const paths: string[] = [];

		// Try to get file:// URIs from drag data (works when dragging from Finder/Explorer)
		if (dataTransfer) {
			const uriList = dataTransfer.getData('text/uri-list');
			if (uriList) {
				for (const uri of uriList.split('\n')) {
					const trimmed = uri.trim();
					if (trimmed && !trimmed.startsWith('#')) {
						if (trimmed.startsWith('file://')) {
							// Decode the file URI to a local path
							paths.push(decodeURIComponent(trimmed.replace('file://', '')));
						} else {
							paths.push(trimmed);
						}
					}
				}
			}

			// Also check text/plain for paths
			if (paths.length === 0) {
				const plainText = dataTransfer.getData('text/plain');
				if (plainText && (plainText.startsWith('/') || plainText.match(/^[A-Z]:\\/))) {
					paths.push(plainText.trim());
				}
			}
		}

		// Fall back to File objects
		const fileList = files || dataTransfer?.files;
		if (paths.length === 0 && fileList) {
			for (const file of fileList) {
				// Wails/Electron expose a `path` property on File objects
				const filePath = (file as File & { path?: string }).path;
				if (filePath) {
					paths.push(filePath);
				} else {
					// Last resort: just use the filename
					paths.push(file.name);
				}
			}
		}

		return paths;
	}

	function insertFilePaths(paths: string[]) {
		if (paths.length === 0) return;

		const prefix = value.trim() ? value.trimEnd() + ' ' : '';
		value = prefix + paths.join(' ');
		textareaElement?.focus();
	}

	function handleFileInput(e: Event) {
		const input = e.target as HTMLInputElement;
		if (input.files && input.files.length > 0) {
			const paths = extractFilePaths(null, input.files);
			insertFilePaths(paths);
		}
		// Reset so the same file can be selected again
		input.value = '';
	}

	const canSend = $derived(value.trim() && !disabled && !isRecording);

	export function focus() {
		textareaElement?.focus();
	}

	export function handleDrop(e: DragEvent) {
		const paths = extractFilePaths(e.dataTransfer);
		insertFilePaths(paths);
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

		<!-- Hidden file input for the Plus button -->
		<input
			bind:this={fileInputElement}
			type="file"
			multiple
			onchange={handleFileInput}
			class="hidden"
		/>

		<!-- Input container - modern rounded design -->
		<div
			class="bg-base-200 rounded-2xl border transition-colors {isDraggingOver
				? 'border-primary border-dashed bg-primary/5'
				: 'border-base-300 focus-within:border-base-content/20'}"
		>
			<!-- Drop zone overlay -->
			{#if isDraggingOver}
				<div class="flex items-center justify-center gap-2 px-4 pt-3 pb-2 text-primary">
					<FileText class="w-4 h-4" />
					<span class="text-sm font-medium">Drop files to add their path</span>
				</div>
			{:else}
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
			{/if}

			<!-- Actions row -->
			<div class="flex items-center justify-between px-3 pb-3">
				<div class="flex items-center gap-1">
					<!-- Attach file button -->
					<button
						type="button"
						onclick={() => fileInputElement?.click()}
						class="btn btn-ghost btn-sm btn-square rounded-lg text-base-content/50 hover:text-base-content"
						title="Attach file (inserts path)"
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
							disabled={false}
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
