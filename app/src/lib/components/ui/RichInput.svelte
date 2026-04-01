<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import { Editor, Node, mergeAttributes, type Extensions } from '@tiptap/core';
	import StarterKit from '@tiptap/starter-kit';
	import { Markdown as TiptapMarkdown } from 'tiptap-markdown';
	import Placeholder from '@tiptap/extension-placeholder';
	import Suggestion from '@tiptap/suggestion';
	import { PluginKey } from '@tiptap/pm/state';
	import SlashPicker from './SlashPicker.svelte';
	import { loadResources, type Resource } from '$lib/utils/resources';
	import { mount, unmount } from 'svelte';

	let {
		value = $bindable(''),
		placeholder = '',
		currentAgentId = '',
		mode = 'minimal',
		onchange,
	}: {
		value: string;
		placeholder?: string;
		currentAgentId?: string;
		mode?: 'minimal' | 'full';
		onchange?: (val: string) => void;
	} = $props();

	let editorEl: HTMLDivElement | undefined = $state();
	let editor: Editor | undefined = $state();
	let resources = $state<Resource[]>([]);

	// Slash command picker state
	let selectedIdx = $state(0);

	// Token icons
	const typeIcon: Record<string, string> = { mcp: '🔌', skill: '📄', agent: '🤖', cmd: '⚡' };

	// ResourceChip — inline node for {{type:id:name}} tokens
	const ResourceChip = Node.create({
		name: 'resourceChip',
		group: 'inline',
		inline: true,
		atom: true,

		addAttributes() {
			return {
				type: { default: '' },
				id: { default: '' },
				name: { default: '' },
			};
		},

		parseHTML() {
			return [{
				tag: 'span[data-resource-chip]',
				getAttrs: (el: HTMLElement) => ({
					type: el.getAttribute('data-type') || el.getAttribute('type') || '',
					id: el.getAttribute('data-id') || el.getAttribute('id') || '',
					name: el.getAttribute('data-name') || el.getAttribute('name') || '',
				}),
			}];
		},

		renderHTML({ HTMLAttributes }) {
			const chipType = HTMLAttributes.type || '';
			const chipId = HTMLAttributes.id || '';
			const chipName = HTMLAttributes.name || '';
			const icon = typeIcon[chipType] || '▶';
			return [
				'span',
				mergeAttributes({
					'data-resource-chip': '',
					'data-type': chipType,
					'data-id': chipId,
					'data-name': chipName,
					'contenteditable': 'false',
					'class': `step-chip step-chip-${chipType}`,
				}),
				`${icon}\u00A0${chipName}`,
			];
		},

		addStorage() {
			return {
				markdown: {
					serialize(state: any, node: any) {
						state.write(`{{${node.attrs.type}:${node.attrs.id}:${node.attrs.name}}}`);
					},
					parse: {},
				},
			};
		},
	});

	// SlashCommand extension using @tiptap/suggestion
	const slashPluginKey = new PluginKey('slashCommand');

	const SlashCommand = Node.create({
		name: 'slashCommand',
		group: 'inline',
		inline: true,

		addProseMirrorPlugins() {
			return [
				Suggestion({
					editor: this.editor,
					char: '/',
					pluginKey: slashPluginKey,
					allowSpaces: false,
					startOfLine: false,
					command: ({ editor: ed, range, props: resource }) => {
						ed.chain()
							.focus()
							.deleteRange(range)
							.insertContent([
								{
									type: 'resourceChip',
									attrs: { type: resource.type, id: resource.id, name: resource.name },
								},
								{ type: 'text', text: ' ' },
							])
							.run();
					},
					items: ({ query: q }) => {
						const lower = q.toLowerCase();
						return resources.filter((r) => !lower || r.name.toLowerCase().includes(lower));
					},
					render: () => {
						let popup: HTMLDivElement | null = null;
						let component: ReturnType<typeof mount> | null = null;
						let commandFn: ((item: Resource) => void) | null = null;
						let currentQuery = '';

						function getFilteredItems(): Resource[] {
							const q = currentQuery.toLowerCase();
							return resources.filter((r) => !q || r.name.toLowerCase().includes(q));
						}

						function mountPicker(target: HTMLDivElement) {
							if (component) {
								try { unmount(component); } catch {}
							}
							component = mount(SlashPicker, {
								target,
								props: {
									resources,
									query: currentQuery,
									selectedIdx,
									onselect: (resource: Resource) => {
										commandFn?.(resource);
									},
								},
							});
						}

						return {
							onStart(props: any) {
								commandFn = props.command;
								currentQuery = props.query || '';
								selectedIdx = 0;

								popup = document.createElement('div');
								popup.style.position = 'fixed';
								popup.style.zIndex = '9999';
								document.body.appendChild(popup);

								mountPicker(popup);
								updatePosition(props);
							},
							onUpdate(props: any) {
								if (!popup) return;
								commandFn = props.command;
								currentQuery = props.query || '';
								selectedIdx = 0;

								mountPicker(popup);
								updatePosition(props);
							},
							onKeyDown(props: any) {
								const { event } = props;
								if (event.key === 'Escape') {
									return true;
								}
								const items = getFilteredItems();
								if (event.key === 'ArrowDown') {
									selectedIdx = Math.min(selectedIdx + 1, items.length - 1);
									if (popup) mountPicker(popup);
									return true;
								}
								if (event.key === 'ArrowUp') {
									selectedIdx = Math.max(selectedIdx - 1, 0);
									if (popup) mountPicker(popup);
									return true;
								}
								if (event.key === 'Enter' && items.length > 0) {
									commandFn?.(items[selectedIdx]);
									return true;
								}
								return false;
							},
							onExit() {
								if (component) {
									try { unmount(component); } catch {}
								}
								if (popup) popup.remove();
								popup = null;
								component = null;
								commandFn = null;
								selectedIdx = 0;
							},
						};

						function updatePosition(props: any) {
							if (!popup) return;
							const rect = props.clientRect?.();
							if (!rect) return;
							popup.style.position = 'fixed';
							popup.style.zIndex = '9999';
							const popupHeight = popup.offsetHeight || 300;
							const spaceBelow = window.innerHeight - rect.bottom;
							const spaceAbove = rect.top;
							if (spaceBelow >= popupHeight || spaceBelow >= spaceAbove) {
								popup.style.top = `${rect.bottom + 4}px`;
								popup.style.bottom = 'auto';
							} else {
								popup.style.bottom = `${window.innerHeight - rect.top + 4}px`;
								popup.style.top = 'auto';
							}
							popup.style.left = `${Math.min(rect.left, window.innerWidth - 280)}px`;
						}
					},
				}),
			];
		},
	});

	// Convert {{type:id:name}} in markdown to chip JSON for Tiptap
	function markdownToTiptap(md: string): string {
		return md.replace(
			/\{\{(mcp|skill|agent|cmd):([^:]+):([^}]+)\}\}/g,
			(_, type, id, name) => `<span data-resource-chip="" data-type="${type}" data-id="${id}" data-name="${name}" contenteditable="false" class="step-chip step-chip-${type}">${typeIcon[type] || '▶'}\u00A0${name}</span>`
		);
	}

	// Serialize editor to token string
	function serializeEditor(): string {
		if (!editor) return value;

		// Walk the document and serialize to token string
		let result = '';
		const doc = editor.state.doc;

		doc.descendants((node, _pos) => {
			if (node.type.name === 'resourceChip') {
				result += `{{${node.attrs.type}:${node.attrs.id}:${node.attrs.name}}}`;
				return false;
			}
			if (node.isText) {
				result += node.text || '';
				return false;
			}
			if (node.type.name === 'hardBreak') {
				result += '\n';
				return false;
			}
			if (node.type.name === 'paragraph') {
				if (result.length > 0 && !result.endsWith('\n')) {
					result += '\n';
				}
				return true; // recurse into children
			}
			if (node.type.name === 'heading') {
				if (result.length > 0 && !result.endsWith('\n')) {
					result += '\n';
				}
				const level = node.attrs.level || 1;
				result += '#'.repeat(level) + ' ';
				return true;
			}
			if (node.type.name === 'bulletList' || node.type.name === 'orderedList') {
				return true;
			}
			if (node.type.name === 'listItem') {
				if (result.length > 0 && !result.endsWith('\n')) {
					result += '\n';
				}
				result += '- ';
				return true;
			}
			if (node.type.name === 'blockquote') {
				if (result.length > 0 && !result.endsWith('\n')) {
					result += '\n';
				}
				result += '> ';
				return true;
			}
			if (node.type.name === 'codeBlock') {
				if (result.length > 0 && !result.endsWith('\n')) {
					result += '\n';
				}
				result += '```\n' + (node.textContent || '') + '\n```\n';
				return false;
			}
			return true;
		});

		// Trim trailing newline
		return result.replace(/\n$/, '');
	}

	// For full mode, use tiptap-markdown for proper serialization
	function serializeEditorFull(): string {
		if (!editor) return value;
		const md = (editor.storage as any).markdown?.getMarkdown() || '';
		return md;
	}

	function getSerializedValue(): string {
		if (mode === 'full') {
			return serializeEditorFull();
		}
		return serializeEditor();
	}

	// Syncing flags
	let updatingFromEditor = false;
	let updatingFromProp = false;

	onMount(async () => {
		// Load resources for slash commands
		try {
			resources = await loadResources(currentAgentId);
		} catch {
			// ignore
		}

		if (!editorEl) return;

		const extensions: Extensions = [
			ResourceChip,
			SlashCommand,
			TiptapMarkdown.configure({
				html: true,
				transformPastedText: true,
				transformCopiedText: true,
			}),
			Placeholder.configure({
				placeholder,
				showOnlyWhenEditable: true,
				showOnlyCurrent: false,
				emptyEditorClass: 'is-editor-empty',
				emptyNodeClass: 'is-empty',
			}),
		];

		if (mode === 'minimal') {
			extensions.push(
				StarterKit.configure({
					heading: false,
					bold: false,
					italic: false,
					strike: false,
					bulletList: false,
					orderedList: false,
					blockquote: false,
					codeBlock: false,
					code: false,
					horizontalRule: false,
					hardBreak: { keepMarks: true },
				})
			);
		} else {
			extensions.push(
				StarterKit.configure({
					hardBreak: { keepMarks: true },
				})
			);
		}

		// Convert initial value — replace {{tokens}} with HTML chips for parsing
		const initialContent = value ? markdownToTiptap(value) : '';

		editor = new Editor({
			element: editorEl,
			extensions,
			content: initialContent,
			editorProps: {
				attributes: {
					class: mode === 'minimal' ? 'rich-input-minimal' : 'rich-input-full',
					role: 'textbox',
					'aria-multiline': 'true',
				},
			},
			onUpdate: ({ editor: ed }) => {
				if (updatingFromProp) return;
				updatingFromEditor = true;
				const newVal = getSerializedValue();
				value = newVal;
				onchange?.(newVal);
				updatingFromEditor = false;
			},
		});
	});

	// Sync external value changes into editor
	$effect(() => {
		if (!editor || updatingFromEditor) return;

		const currentVal = getSerializedValue();
		if (value !== currentVal) {
			updatingFromProp = true;
			const htmlContent = markdownToTiptap(value || '');
			editor.commands.setContent(htmlContent);
			updatingFromProp = false;
		}
	});

	onDestroy(() => {
		editor?.destroy();
	});
</script>

<div class="rich-input-wrapper">
	<div
		class="rounded-xl border border-base-content/20 bg-base-100 transition-colors focus-within:border-primary/50 focus-within:ring-2 focus-within:ring-primary/10"
		bind:this={editorEl}
	></div>
</div>
