import { html, nothing } from 'lit';
import { AudioPlayerApi } from '@a2ui/web_core/v0_9/basic_catalog';
import { A2uiLitElement, A2uiController } from '@a2ui/lit/v0_9';

export class NeboAudioPlayerElement extends A2uiLitElement<typeof AudioPlayerApi> {
	createRenderRoot() {
		return this;
	}

	protected createController() {
		return new A2uiController(this, AudioPlayerApi);
	}

	render() {
		const props = this.controller.props;
		if (!props) return nothing;

		return html`
			<div class="flex flex-col gap-1">
				${props.description ? html`<p class="text-sm">${props.description}</p>` : nothing}
				<audio src=${props.url} controls class="w-full"></audio>
			</div>
		`;
	}
}

customElements.define('nebo-a2ui-audioplayer', NeboAudioPlayerElement);
export const NeboAudioPlayer = { ...AudioPlayerApi, tagName: 'nebo-a2ui-audioplayer' };
