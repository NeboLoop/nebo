import { html, nothing } from 'lit';
import { ImageApi } from '@a2ui/web_core/v0_9/basic_catalog';
import { A2uiLitElement, A2uiController } from '@a2ui/lit/v0_9';

const FIT_MAP: Record<string, string> = {
	contain: 'object-contain',
	cover: 'object-cover',
	fill: 'object-fill',
	none: 'object-none',
	scaleDown: 'object-scale-down',
};

const SIZE_MAP: Record<string, string> = {
	icon: 'w-6 h-6',
	avatar: 'w-10 h-10 rounded-full',
	smallFeature: 'max-w-32',
	mediumFeature: 'max-w-64',
	largeFeature: 'max-w-full',
	header: 'w-full',
};

export class NeboImageElement extends A2uiLitElement<typeof ImageApi> {
	createRenderRoot() {
		return this;
	}

	protected createController() {
		return new A2uiController(this, ImageApi);
	}

	render() {
		const props = this.controller.props;
		if (!props) return nothing;

		const fit = FIT_MAP[props.fit || 'fill'] || 'object-fill';
		const size = SIZE_MAP[props.variant || 'mediumFeature'] || 'max-w-64';

		return html`<img
			src=${props.url}
			alt=${props.description || ''}
			class="rounded-lg ${fit} ${size}"
		/>`;
	}
}

customElements.define('nebo-a2ui-image', NeboImageElement);
export const NeboImage = { ...ImageApi, tagName: 'nebo-a2ui-image' };
