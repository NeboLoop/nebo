<script lang="ts">
	import { Copy, Check, Pencil } from 'lucide-svelte';
	import { t } from 'svelte-i18n';
	import Markdown from '$lib/components/ui/Markdown.svelte';
	import ToolCard from './ToolCard.svelte';
	import ThinkingBlock from './ThinkingBlock.svelte';
	import ReadingIndicator from './ReadingIndicator.svelte';
	import AskWidget from './AskWidget.svelte';
	import type { AskWidgetDef } from './AskWidget.svelte';
	import SubagentTree from './SubagentTree.svelte';
	import type { SubagentState } from './SubagentTree.svelte';

	interface ToolCall {
		name: string;
		input: string;
		output?: string;
		status?: 'running' | 'complete' | 'error';
	}

	interface ContentBlock {
		type: 'text' | 'tool' | 'image' | 'ask' | 'subagent_tree';
		text?: string;
		toolCallIndex?: number;
		imageData?: string;
		imageMimeType?: string;
		imageURL?: string;
		askRequestId?: string;
		askPrompt?: string;
		askWidgets?: AskWidgetDef[];
		askResponse?: string;
		subagents?: SubagentState[];
	}

	// A resolved content block with tool data pre-resolved (no indirect lookup)
	interface ResolvedBlock {
		type: 'text' | 'tool' | 'image' | 'ask' | 'subagent_tree';
		key: string;
		text?: string;
		tool?: ToolCall;
		imageData?: string;
		imageMimeType?: string;
		imageURL?: string;
		askRequestId?: string;
		askPrompt?: string;
		askWidgets?: AskWidgetDef[];
		askResponse?: string;
		subagents?: SubagentState[];
		isLastBlock: boolean;
	}

	interface Message {
		id: string;
		role: 'user' | 'assistant' | 'system';
		content: string;
		contentHtml?: string;
		timestamp: Date;
		toolCalls?: ToolCall[];
		streaming?: boolean;
		thinking?: string;
		contentBlocks?: ContentBlock[];
		proactive?: boolean;
	}

	interface ResolvedMessage {
		id: string;
		message: Message;
		thinking: string | null;
		cleanContent: string;
		blocks: ResolvedBlock[];
	}

	interface Props {
		messages: Message[];
		role: 'user' | 'assistant';
		agentName?: string;
		onCopy?: (id: string, content: string) => void;
		copiedId?: string | null;
		onViewToolOutput?: (tool: ToolCall) => void;
		isStreaming?: boolean;
		onAskSubmit?: (requestId: string, value: string) => void;
		onEditMessage?: (id: string, content: string) => void;
	}

	let {
		messages,
		role,
		agentName = 'Nebo',
		onCopy,
		copiedId = null,
		onViewToolOutput,
		isStreaming = false,
		onAskSubmit,
		onEditMessage
	}: Props = $props();

	let editingId = $state<string | null>(null);
	let editText = $state('');

	function startEdit(id: string, content: string) {
		editingId = id;
		editText = content;
	}

	function cancelEdit() {
		editingId = null;
		editText = '';
	}

	function submitEdit() {
		if (!editingId || !editText.trim()) return;
		onEditMessage?.(editingId, editText.trim());
		editingId = null;
		editText = '';
	}

	function handleEditKeydown(e: KeyboardEvent) {
		if (e.key === 'Enter' && !e.shiftKey) {
			e.preventDefault();
			submitEdit();
		} else if (e.key === 'Escape') {
			cancelEdit();
		}
	}

	const groupTimestamp = $derived(messages[messages.length - 1]?.timestamp || messages[0]?.timestamp);
	const isProactive = $derived(messages.some(m => m.proactive));

	// Pre-resolve all message data including tool lookups into a flat structure.
	// This ensures Svelte's reactivity tracks every piece of data that affects rendering.
	const resolvedMessages = $derived.by((): ResolvedMessage[] => {
		return messages.map(message => {
			const { thinking, cleanContent } = extractThinking(message.content);
			const blocks: ResolvedBlock[] = [];

			if (message.contentBlocks?.length) {
				const totalBlocks = message.contentBlocks.length;
				for (let i = 0; i < totalBlocks; i++) {
					const block = message.contentBlocks[i];
					const isLast = i === totalBlocks - 1;

					if (block.type === 'tool' && block.toolCallIndex != null && message.toolCalls?.[block.toolCallIndex]) {
						const tc = message.toolCalls[block.toolCallIndex];
						blocks.push({
							type: 'tool',
							key: `tool-${i}-${tc.status ?? 'unknown'}`,
							tool: { ...tc },
							isLastBlock: isLast
						});
					} else if (block.type === 'image' && (block.imageData || block.imageURL)) {
						blocks.push({
							type: 'image',
							key: `image-${i}`,
							imageData: block.imageData,
							imageMimeType: block.imageMimeType,
							imageURL: block.imageURL,
							isLastBlock: isLast
						});
					} else if (block.type === 'text' && block.text) {
						blocks.push({
							type: 'text',
							key: `text-${i}`,
							text: block.text,
							isLastBlock: isLast
						});
					} else if (block.type === 'ask' && block.askRequestId) {
						blocks.push({
							type: 'ask',
							key: `ask-${i}-${block.askRequestId}`,
							askRequestId: block.askRequestId,
							askPrompt: block.askPrompt,
							askWidgets: block.askWidgets,
							askResponse: block.askResponse,
							isLastBlock: isLast
						});
					} else if (block.type === 'subagent_tree' && block.subagents?.length) {
						blocks.push({
							type: 'subagent_tree',
							key: `subagent-tree-${i}-${block.subagents.length}`,
							subagents: block.subagents,
							isLastBlock: isLast
						});
					}
				}
			}

			return {
				id: message.id,
				message,
				thinking,
				cleanContent,
				blocks
			};
		});
	});

	function formatTime(date: Date): string {
		return date.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' });
	}

	function handleCopy(id: string, content: string) {
		onCopy?.(id, content);
	}

	function handleViewToolOutput(tool: ToolCall) {
		onViewToolOutput?.(tool);
	}

	function extractThinking(content: string): { thinking: string | null; cleanContent: string } {
		const thinkingMatch = content.match(/<thinking>([\s\S]*?)<\/thinking>/);
		if (thinkingMatch) {
			return {
				thinking: thinkingMatch[1].trim(),
				cleanContent: content.replace(/<thinking>[\s\S]*?<\/thinking>/g, '').trim()
			};
		}
		return { thinking: null, cleanContent: content };
	}
</script>

<!-- Chat group - user on right, assistant on left -->
<div class="flex gap-3 mb-4 {role === 'user' ? 'flex-row-reverse' : ''}">
	<!-- Messages container -->
	<div class="flex flex-col gap-0.5 max-w-[min(900px,100%)] {role === 'user' ? 'items-end' : 'items-start'}">
		<!-- Messages -->
		{#each resolvedMessages as resolved (resolved.id)}
			<div class="group w-full">
				<!-- Thinking block (always renders first, above content blocks) -->
				{#if (resolved.thinking || resolved.message.thinking) && role === 'assistant'}
					<div class="mb-2">
						<ThinkingBlock
							content={resolved.thinking || resolved.message.thinking || ''}
							initiallyCollapsed={true}
							isStreaming={resolved.message.streaming && !resolved.cleanContent}
						/>
					</div>
				{/if}

				<!-- Content blocks: interleaved text and tool calls (pre-resolved) -->
				{#if resolved.blocks.length > 0}
					{#each resolved.blocks as block (block.key)}
						{#if block.type === 'tool' && block.tool}
							<div class="max-w-md mb-2">
								<ToolCard
									name={block.tool.name}
									input={block.tool.input}
									output={block.tool.output}
									status={block.tool.status}
									onclick={() => handleViewToolOutput(block.tool!)}
								/>
							</div>
						{:else if block.type === 'image' && (block.imageData || block.imageURL)}
							{@const imgSrc = block.imageData
								? `data:${block.imageMimeType || 'image/png'};base64,${block.imageData}`
								: block.imageURL ?? ''}
							<a href={imgSrc} target="_blank" rel="noopener" class="block rounded-xl overflow-hidden mb-1 max-w-sm cursor-zoom-in">
								<img
									src={imgSrc}
									alt={$t('chat.sharedContent')}
									class="max-w-full h-auto rounded-xl"
								/>
							</a>
						{:else if block.type === 'ask' && block.askRequestId}
							<AskWidget
								requestId={block.askRequestId}
								prompt={block.askPrompt ?? ''}
								widgets={block.askWidgets ?? []}
								response={block.askResponse}
								disabled={!resolved.message.streaming}
								onSubmit={(id, val) => onAskSubmit?.(id, val)}
							/>
						{:else if block.type === 'subagent_tree' && block.subagents?.length}
							<div class="mb-2 max-w-lg">
								<SubagentTree agents={block.subagents} />
							</div>
						{:else if block.type === 'text' && block.text}
							{#if editingId === resolved.id}
								<div class="w-full mb-1">
									<textarea
										class="textarea textarea-bordered w-full min-h-[60px] text-sm resize-y bg-base-200"
										bind:value={editText}
										onkeydown={handleEditKeydown}
									></textarea>
									<div class="flex gap-2 mt-1.5">
										<button type="button" class="btn btn-primary btn-sm" onclick={submitEdit}>{$t('chat.resubmit')}</button>
										<button type="button" class="btn btn-ghost btn-sm" onclick={cancelEdit}>{$t('common.cancel')}</button>
									</div>
								</div>
							{:else}
								<div
									class="relative rounded-xl px-3.5 py-2.5 max-w-full break-words transition-colors duration-150 mb-1 {role === 'user' ? 'bg-primary/10 hover:bg-primary/15' : 'bg-base-200 hover:bg-base-200/80'} {resolved.message.streaming && block.isLastBlock ? 'animate-pulse-border' : ''} {resolved.message.proactive ? 'proactive-message' : ''}"
								>
									<div class="prose prose-invert max-w-none leading-relaxed">
										<Markdown content={block.text} />
									</div>
									{#if resolved.message.streaming && block.isLastBlock}
										<span class="inline-block w-0.5 h-3 bg-primary/60 animate-pulse ml-0.5 align-text-bottom rounded-full"></span>
									{/if}

									<!-- Copy button on last text block -->
									{#if !resolved.message.streaming && block.isLastBlock && role === 'assistant'}
										<button
											type="button"
											onclick={() => handleCopy(resolved.id, resolved.cleanContent)}
											class="absolute top-1.5 right-2 p-1 rounded opacity-0 group-hover:opacity-100 transition-opacity bg-base-100 hover:bg-base-300 text-base-content/90 hover:text-base-content"
											title={$t('common.copy')}
										>
											{#if copiedId === resolved.id}
												<Check class="w-3.5 h-3.5 text-success" />
											{:else}
												<Copy class="w-3.5 h-3.5" />
											{/if}
										</button>
									{/if}

									<!-- Edit button on last text block for user messages -->
									{#if !resolved.message.streaming && !isStreaming && block.isLastBlock && role === 'user' && onEditMessage}
										<button
											type="button"
											onclick={() => startEdit(resolved.id, resolved.cleanContent)}
											class="absolute top-1.5 right-2 p-1 rounded opacity-0 group-hover:opacity-100 transition-opacity bg-base-100 hover:bg-base-300 text-base-content/90 hover:text-base-content"
											title={$t('common.edit')}
										>
											<Pencil class="w-3.5 h-3.5" />
										</button>
									{/if}
								</div>
							{/if}
						{/if}
					{/each}

					<!-- Reading indicator when streaming with no content blocks yet -->
					{#if resolved.message.streaming && resolved.blocks.length === 0}
						<div class="rounded-xl bg-base-200 px-3.5 py-2.5 animate-pulse-border">
							<ReadingIndicator />
						</div>
					{/if}
				{:else}
					<!-- Legacy fallback: messages without contentBlocks -->
					{#if resolved.message.toolCalls?.length && role === 'assistant'}
						{#each resolved.message.toolCalls as tool}
							<div class="max-w-md mb-2">
								<ToolCard
									name={tool.name}
									input={tool.input}
									output={tool.output}
									status={tool.status}
									onclick={() => handleViewToolOutput(tool)}
								/>
							</div>
						{/each}
					{/if}

					{#if resolved.cleanContent || resolved.message.streaming}
						{#if editingId === resolved.id}
							<div class="w-full">
								<textarea
									class="textarea textarea-bordered w-full min-h-[60px] text-sm resize-y bg-base-200"
									bind:value={editText}
									onkeydown={handleEditKeydown}
								></textarea>
								<div class="flex gap-2 mt-1.5">
									<button type="button" class="btn btn-primary btn-sm" onclick={submitEdit}>{$t('chat.resubmit')}</button>
									<button type="button" class="btn btn-ghost btn-sm" onclick={cancelEdit}>{$t('common.cancel')}</button>
								</div>
							</div>
						{:else}
							<div
								class="relative rounded-xl px-3.5 py-2.5 max-w-full break-words transition-colors duration-150 {role === 'user' ? 'bg-primary/10 hover:bg-primary/15' : 'bg-base-200 hover:bg-base-200/80'} {resolved.message.streaming ? 'animate-pulse-border' : ''} {resolved.message.proactive ? 'proactive-message' : ''}"
							>
								{#if resolved.message.streaming && !resolved.cleanContent}
									<ReadingIndicator />
								{:else}
									<div class="prose prose-invert max-w-none leading-relaxed">
										<Markdown content={resolved.cleanContent} preRenderedHtml={resolved.message.contentHtml} />
									</div>
									{#if resolved.message.streaming}
										<span class="inline-block w-0.5 h-3 bg-primary/60 animate-pulse ml-0.5 align-text-bottom rounded-full"></span>
									{/if}
								{/if}

								<!-- Copy button -->
								{#if !resolved.message.streaming && resolved.cleanContent && role === 'assistant'}
									<button
										type="button"
										onclick={() => handleCopy(resolved.id, resolved.cleanContent)}
										class="absolute top-1.5 right-2 p-1 rounded opacity-0 group-hover:opacity-100 transition-opacity bg-base-100 hover:bg-base-300 text-base-content/90 hover:text-base-content"
										title={$t('common.copy')}
									>
										{#if copiedId === resolved.id}
											<Check class="w-3.5 h-3.5 text-success" />
										{:else}
											<Copy class="w-3.5 h-3.5" />
										{/if}
									</button>
								{/if}

								<!-- Edit button for user messages -->
								{#if !resolved.message.streaming && !isStreaming && resolved.cleanContent && role === 'user' && onEditMessage}
									<button
										type="button"
										onclick={() => startEdit(resolved.id, resolved.cleanContent)}
										class="absolute top-1.5 right-2 p-1 rounded opacity-0 group-hover:opacity-100 transition-opacity bg-base-100 hover:bg-base-300 text-base-content/90 hover:text-base-content"
										title={$t('common.edit')}
									>
										<Pencil class="w-3.5 h-3.5" />
									</button>
								{/if}
							</div>
						{/if}
					{/if}
				{/if}
			</div>
		{/each}

		<!-- Loading indicator when streaming but no messages -->
		{#if isStreaming && messages.length === 0}
			<div class="rounded-xl bg-base-200 px-3.5 py-2.5 animate-pulse-border">
				<ReadingIndicator />
			</div>
		{/if}

		<!-- Footer: sender name + timestamp -->
		<div class="flex gap-2 items-baseline mt-1.5 {role === 'user' ? 'flex-row-reverse' : ''}">
			<span class="text-sm font-medium text-base-content/60">{role === 'user' ? $t('common.you') : agentName}</span>
			{#if groupTimestamp}
				<span class="text-sm text-base-content/60">{formatTime(groupTimestamp)}</span>
			{/if}
		</div>
	</div>
</div>
