import { html, nothing } from 'lit';
import { VideoApi } from '@a2ui/web_core/v0_9/basic_catalog';
import { A2uiLitElement, A2uiController } from '@a2ui/lit/v0_9';

export class NeboVideoElement extends A2uiLitElement<typeof VideoApi> {
	createRenderRoot() {
		return this;
	}

	protected createController() {
		return new A2uiController(this, VideoApi);
	}

	render() {
		const props = this.controller.props;
		if (!props) return nothing;

		return html`<video src=${props.url} controls class="w-full rounded-lg"></video>`;
	}
}

customElements.define('nebo-a2ui-video', NeboVideoElement);
export const NeboVideo = { ...VideoApi, tagName: 'nebo-a2ui-video' };
