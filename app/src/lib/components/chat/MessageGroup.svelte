<script lang="ts">
	import { Copy, Check } from 'lucide-svelte';
	import Markdown from '$lib/components/ui/Markdown.svelte';
	import ToolCard from './ToolCard.svelte';
	import ThinkingBlock from './ThinkingBlock.svelte';
	import ReadingIndicator from './ReadingIndicator.svelte';

	interface ToolCall {
		name: string;
		input: string;
		output?: string;
		status?: 'running' | 'complete' | 'error';
	}

	interface ContentBlock {
		type: 'text' | 'tool' | 'image';
		text?: string;
		toolCallIndex?: number;
		imageData?: string;
		imageMimeType?: string;
		imageURL?: string;
	}

	// A resolved content block with tool data pre-resolved (no indirect lookup)
	interface ResolvedBlock {
		type: 'text' | 'tool' | 'image';
		key: string;
		text?: string;
		tool?: ToolCall;
		imageData?: string;
		imageMimeType?: string;
		imageURL?: string;
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
	}

	let {
		messages,
		role,
		agentName = 'Nebo',
		onCopy,
		copiedId = null,
		onViewToolOutput,
		isStreaming = false
	}: Props = $props();

	const groupTimestamp = $derived(messages[messages.length - 1]?.timestamp || messages[0]?.timestamp);

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
									alt="Shared content"
									class="max-w-full h-auto rounded-xl"
								/>
							</a>
						{:else if block.type === 'text' && block.text}
							<div
								class="relative rounded-xl px-3.5 py-2.5 max-w-full break-words transition-colors duration-150 mb-1 {role === 'user' ? 'bg-primary/10 hover:bg-primary/15' : 'bg-base-200 hover:bg-base-200/80'} {resolved.message.streaming && block.isLastBlock ? 'animate-pulse-border' : ''}"
							>
								<div class="prose prose-sm prose-invert max-w-none text-sm leading-relaxed">
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
										class="absolute top-1.5 right-2 p-1 rounded opacity-0 group-hover:opacity-100 transition-opacity bg-base-100 hover:bg-base-300 text-base-content/50 hover:text-base-content"
										title="Copy"
									>
										{#if copiedId === resolved.id}
											<Check class="w-3.5 h-3.5 text-success" />
										{:else}
											<Copy class="w-3.5 h-3.5" />
										{/if}
									</button>
								{/if}
							</div>
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
						<div
							class="relative rounded-xl px-3.5 py-2.5 max-w-full break-words transition-colors duration-150 {role === 'user' ? 'bg-primary/10 hover:bg-primary/15' : 'bg-base-200 hover:bg-base-200/80'} {resolved.message.streaming ? 'animate-pulse-border' : ''}"
						>
							{#if resolved.message.streaming && !resolved.cleanContent}
								<ReadingIndicator />
							{:else}
								<div class="prose prose-sm prose-invert max-w-none text-sm leading-relaxed">
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
									class="absolute top-1.5 right-2 p-1 rounded opacity-0 group-hover:opacity-100 transition-opacity bg-base-100 hover:bg-base-300 text-base-content/50 hover:text-base-content"
									title="Copy"
								>
									{#if copiedId === resolved.id}
										<Check class="w-3.5 h-3.5 text-success" />
									{:else}
										<Copy class="w-3.5 h-3.5" />
									{/if}
								</button>
							{/if}
						</div>
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
			<span class="text-xs font-medium text-base-content/50">{role === 'user' ? 'You' : agentName}</span>
			{#if groupTimestamp}
				<span class="text-xs text-base-content/40">{formatTime(groupTimestamp)}</span>
			{/if}
		</div>
	</div>
</div>
