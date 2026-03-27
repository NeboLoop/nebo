<script lang="ts">
	import { AudioLines, ArrowUp, Square, Plus, RotateCcw, X, Clock, FileText } from 'lucide-svelte';
	import { t } from 'svelte-i18n';
	import * as api from '$lib/api/nebo';
	import SlashCommandMenu from './SlashCommandMenu.svelte';
	import { getSlashCommandCompletions, type SlashCommand } from './slash-commands';

	interface QueuedMessage {
		id: string;
		content: string;
	}

	interface Props {
		value: string;
		placeholder?: string;
		disabled?: boolean;
		isLoading?: boolean;
		duplexActive?: boolean;
		audioLevel?: number;
		queuedMessages?: QueuedMessage[];
		isDraggingOver?: boolean;
		onSend: () => void;
		onCancel?: () => void;
		onCancelQueued?: (id: string) => void;
		onRecallQueue?: () => string | null;
		onNewSession?: () => void;
		onToggleDuplex?: () => void;
		onSlashSelect?: (command: SlashCommand) => void;
	}

	let {
		value = $bindable(),
		placeholder = '',
		disabled = false,
		isLoading = false,
		duplexActive = false,
		audioLevel = 0,
		queuedMessages = [],
		isDraggingOver = false,
		onSend,
		onCancel,
		onCancelQueued,
		onRecallQueue,
		onNewSession,
		onToggleDuplex,
		onSlashSelect
	}: Props = $props();

	let textareaElement: HTMLTextAreaElement | undefined = $state();
	let fileInputElement: HTMLInputElement | undefined = $state();
	let slashMenuRef: SlashCommandMenu | undefined = $state();

	// Slash command menu state
	let slashMenuVisible = $state(false);
	let slashMenuQuery = $state('');

	// Detect "/" prefix for slash command menu
	$effect(() => {
		if (value.startsWith('/')) {
			const afterSlash = value.slice(1);
			const spaceIndex = afterSlash.indexOf(' ');
			if (spaceIndex === -1) {
				// Still typing command name
				slashMenuQuery = afterSlash;
				slashMenuVisible = getSlashCommandCompletions(afterSlash).length > 0;
			} else {
				slashMenuVisible = false;
			}
		} else {
			slashMenuVisible = false;
		}
	});

	function handleSlashSelect(cmd: SlashCommand) {
		// Replace the current "/" input with the full command
		if (cmd.args) {
			value = `/${cmd.name} `;
		} else {
			value = `/${cmd.name}`;
			// Auto-execute commands with no args
			onSlashSelect?.(cmd);
		}
		slashMenuVisible = false;
		textareaElement?.focus();
	}

	function handleKeydown(e: KeyboardEvent) {
		// Slash menu keyboard navigation
		if (slashMenuVisible && slashMenuRef) {
			if (e.key === 'ArrowDown') {
				e.preventDefault();
				slashMenuRef.navigate('down');
				return;
			}
			if (e.key === 'ArrowUp') {
				e.preventDefault();
				slashMenuRef.navigate('up');
				return;
			}
			if (e.key === 'Tab' || (e.key === 'Enter' && !e.shiftKey)) {
				e.preventDefault();
				const selected = slashMenuRef.selectCurrent();
				if (selected) {
					handleSlashSelect(selected);
				}
				return;
			}
			if (e.key === 'Escape') {
				e.preventDefault();
				slashMenuVisible = false;
				return;
			}
		}

		if (e.key === 'Enter' && !e.shiftKey) {
			e.preventDefault();
			if (value.trim()) {
				onSend();
			}
		}
		// Up arrow at cursor position 0: recall last queued message for editing
		if (e.key === 'ArrowUp' && !e.shiftKey && onRecallQueue) {
			if (textareaElement && textareaElement.selectionStart === 0) {
				const recalled = onRecallQueue();
				if (recalled != null) {
					e.preventDefault();
					value = recalled;
				}
			}
		}
	}

	function handleSend() {
		if (value.trim() && !disabled && !duplexActive) {
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
	 * Priority: File.path (Electron) > file:// URIs > text/plain > filename fallback.
	 * Note: In Tauri, drag-and-drop is handled via native DragDropEvent custom events
	 * dispatched directly to Chat.svelte — this function is only for browser mode.
	 */
	function extractFilePaths(dataTransfer: DataTransfer | null, files?: FileList | null): string[] {
		const paths: string[] = [];

		// Electron/Wails method: File objects have a `path` property with the full path
		const fileList = files || dataTransfer?.files;
		if (fileList && fileList.length > 0) {
			for (const file of fileList) {
				const filePath = (file as File & { path?: string }).path;
				if (filePath) {
					paths.push(filePath);
				}
			}
		}

		// Secondary method: Try to get file:// URIs from drag data
		if (paths.length === 0 && dataTransfer) {
			const uriList = dataTransfer.getData('text/uri-list');
			if (uriList) {
				for (const uri of uriList.split('\n')) {
					const trimmed = uri.trim();
					if (trimmed && !trimmed.startsWith('#') && trimmed.startsWith('file://')) {
						try {
							const url = new URL(trimmed);
							paths.push(decodeURIComponent(url.pathname));
						} catch {
							paths.push(decodeURIComponent(trimmed.replace('file://', '')));
						}
					}
				}
			}

			// Tertiary method: Check text/plain for paths
			if (paths.length === 0) {
				const plainText = dataTransfer.getData('text/plain');
				if (plainText && (plainText.startsWith('/') || plainText.match(/^[A-Z]:\\/))) {
					paths.push(plainText.trim());
				}
			}
		}

		// Last resort fallback: Use filenames if we still have no paths
		if (paths.length === 0 && fileList) {
			for (const file of fileList) {
				paths.push(file.name);
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

	async function handleBrowseFiles() {
		try {
			const res = await api.pickFiles();
			if (res.paths && res.paths.length > 0) {
				insertFilePaths(res.paths);
				return;
			}
		} catch {
			// Native dialog not available (headless mode) — fall back to HTML input
			fileInputElement?.click();
		}
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

	const canSend = $derived(value.trim() && !disabled && !duplexActive);

	// Generate waveform bar heights from audio level (16 bars)
	const NUM_BARS = 16;
	const waveformBars = $derived.by(() => {
		const bars: number[] = [];
		const level = audioLevel;
		for (let i = 0; i < NUM_BARS; i++) {
			// Create a wave pattern — center bars taller, edges shorter
			const centerDist = Math.abs(i - (NUM_BARS - 1) / 2) / ((NUM_BARS - 1) / 2);
			const base = 0.15; // minimum bar height
			const wave = (1 - centerDist * 0.6) * level;
			// Add pseudo-random variation based on bar index and level
			const jitter = Math.sin(i * 2.7 + level * 20) * 0.15 * level;
			bars.push(Math.max(base, Math.min(1, wave + jitter)));
		}
		return bars;
	});

	export function focus() {
		textareaElement?.focus();
	}

	export function handleDrop(e: DragEvent) {
		const paths = extractFilePaths(e.dataTransfer);
		insertFilePaths(paths);
	}

	export { insertFilePaths };
</script>

<div class="sticky bottom-0 flex-shrink-0 mt-auto px-4 pb-2 pt-2 z-10">
	<div class="max-w-4xl mx-auto">
		<!-- Queued messages tray -->
		{#if queuedMessages.length > 0}
			<div class="flex flex-wrap gap-2 mb-2 px-1">
				{#each queuedMessages as queued (queued.id)}
					<div class="flex items-center gap-1.5 px-3 py-1.5 rounded-xl bg-primary/10 border border-primary/20 text-base text-base-content/80 animate-in fade-in slide-in-from-bottom-1">
						<Clock class="w-3 h-3 text-primary/50 flex-shrink-0" />
						<span class="truncate max-w-[200px]">{queued.content}</span>
						{#if onCancelQueued}
							<button
								type="button"
								onclick={() => onCancelQueued?.(queued.id)}
								class="p-0.5 rounded hover:bg-error/20 hover:text-error transition-colors flex-shrink-0"
								title={$t('chatInput.cancelQueued')}
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
		<div class="relative">
			<!-- Slash command menu (floats above input) -->
			<SlashCommandMenu
				bind:this={slashMenuRef}
				query={slashMenuQuery}
				visible={slashMenuVisible}
				onselect={handleSlashSelect}
				onclose={() => { slashMenuVisible = false; }}
			/>

		<div
			class="bg-base-200 rounded-2xl border transition-colors {isDraggingOver
				? 'border-primary border-dashed bg-primary/5'
				: 'border-base-300 focus-within:border-base-content/40'}"
		>
			<!-- Drop zone overlay -->
			{#if isDraggingOver}
				<div class="flex items-center justify-center gap-2 px-4 pt-3 pb-2 text-primary">
					<FileText class="w-4 h-4" />
					<span class="text-base font-medium">{$t('chat.dropFilesToAdd')}</span>
				</div>
			{:else if duplexActive}
				<!-- Audio waveform visualizer (replaces textarea during voice session) -->
				<div class="flex items-center justify-center gap-1 px-4 pt-3 pb-2 min-h-[40px]">
					<div class="flex items-center gap-0.5 h-6">
						{#each waveformBars as barHeight, i}
							<div
								class="w-1 rounded-full bg-success transition-all duration-100"
								style="height: {Math.max(4, barHeight * 24)}px"
							></div>
						{/each}
					</div>
					<span class="text-sm text-base-content/60 ml-2">{$t('chatInput.listening')}</span>
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
							? $t('chatInput.loadingPlaceholder')
							: (placeholder || $t('chatInput.placeholder'))}
						disabled={disabled || duplexActive}
						rows="1"
						class="w-full bg-transparent border-none outline-none resize-none text-base leading-relaxed placeholder:text-base-content/80 min-h-[24px] max-h-[200px]"
					></textarea>
				</div>
			{/if}

			<!-- Actions row -->
			<div class="flex items-center justify-between px-3 pb-3">
				<div class="flex items-center gap-1">
					<!-- Attach file button -->
					<button
						type="button"
						onclick={handleBrowseFiles}
						class="btn btn-ghost btn-sm btn-square rounded-lg text-base-content/90 hover:text-base-content"
						title={$t('chatInput.attachFile')}
					>
						<Plus class="w-4 h-4" />
					</button>

					<!-- New session button -->
					{#if onNewSession}
						<button
							type="button"
							onclick={onNewSession}
							class="btn btn-ghost btn-sm btn-square rounded-lg text-base-content/90 hover:text-base-content"
							{disabled}
							title={$t('chatInput.newSession')}
						>
							<RotateCcw class="w-4 h-4" />
						</button>
					{/if}
				</div>

				<div class="flex items-center gap-2">
					<!-- Voice conversation (full duplex) -->
					{#if onToggleDuplex}
						<button
							type="button"
							onclick={() => onToggleDuplex?.()}
							class="btn btn-ghost btn-sm btn-square rounded-lg {duplexActive
								? 'text-success animate-pulse'
								: 'text-base-content/90 hover:text-base-content'}"
							title={duplexActive ? $t('chatInput.endVoiceSession') : $t('chatInput.voiceConversation')}
						>
							<AudioLines class="w-4 h-4" />
						</button>
					{/if}

					<!-- Send / Stop button -->
					{#if isLoading && onCancel}
						<button
							type="button"
							onclick={onCancel}
							class="btn btn-sm btn-circle btn-error transition-all"
							title={$t('chatInput.stopGeneration')}
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
								: 'btn-ghost bg-base-300 text-base-content/90'}"
							title={$t('chatInput.sendMessage')}
						>
							<ArrowUp class="w-4 h-4" />
						</button>
					{/if}
				</div>
			</div>
		</div>
		</div>

		<!-- Disclaimer -->
		<p class="text-center text-sm text-base-content/60 mt-2">
			{$t('chat.disclaimer')}
		</p>
	</div>
</div>
