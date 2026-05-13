import { html, nothing } from 'lit';
import { unsafeHTML } from 'lit/directives/unsafe-html.js';
import { until } from 'lit/directives/until.js';
import { ContextConsumer } from '@lit/context';
import { TextApi } from '@a2ui/web_core/v0_9/basic_catalog';
import { A2uiLitElement, A2uiController, Context } from '@a2ui/lit/v0_9';

type MarkdownFn = (text: string) => Promise<string>;

export class NeboTextElement extends A2uiLitElement<typeof TextApi> {
	createRenderRoot() {
		return this;
	}

	// Replaces @consume({ context: Context.markdown, subscribe: true }) accessor markdownRenderer
	private _markdownConsumer = new ContextConsumer(this, {
		context: Context.markdown,
		subscribe: true
	});

	protected createController() {
		return new A2uiController(this, TextApi);
	}

	private renderMarkdown(text: string) {
		const renderer = this._markdownConsumer.value as MarkdownFn | undefined;
		if (renderer) {
			return until(
				renderer(text).then((rendered) => unsafeHTML(rendered)),
				html`<span>${text}</span>`
			);
		}
		return html`<span>${text}</span>`;
	}

	render() {
		const props = this.controller.props;
		if (!props) return nothing;

		let text = props.text || '';

		// Prepend heading markers for markdown rendering (same pattern as basic catalog)
		switch (props.variant) {
			case 'h1':
				text = `# ${text}`;
				break;
			case 'h2':
				text = `## ${text}`;
				break;
			case 'h3':
				text = `### ${text}`;
				break;
			case 'h4':
				text = `#### ${text}`;
				break;
			case 'h5':
				text = `##### ${text}`;
				break;
		}

		if (props.variant === 'caption') {
			return html`<span class="nebo-caption">${this.renderMarkdown(text)}</span>`;
		}

		return this.renderMarkdown(text);
	}
}

customElements.define('nebo-a2ui-text', NeboTextElement);
export const NeboText = { ...TextApi, tagName: 'nebo-a2ui-text' };
