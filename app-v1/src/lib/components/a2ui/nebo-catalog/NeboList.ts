import { html, nothing } from 'lit';
import { map } from 'lit/directives/map.js';
import { ListApi } from '@a2ui/web_core/v0_9/basic_catalog';
import { A2uiLitElement, A2uiController } from '@a2ui/lit/v0_9';

const ALIGN: Record<string, string> = {
	start: 'items-start',
	center: 'items-center',
	end: 'items-end',
	stretch: 'items-stretch',
};

export class NeboListElement extends A2uiLitElement<typeof ListApi> {
	createRenderRoot() {
		return this;
	}

	protected createController() {
		return new A2uiController(this, ListApi);
	}

	render() {
		const props = this.controller.props;
		if (!props) return nothing;

		const children = Array.isArray(props.children) ? props.children : [];
		const dir = props.direction === 'horizontal' ? 'flex-row' : 'flex-col';
		const align = ALIGN[props.align || 'stretch'] || 'items-stretch';

		return html`
			<div class="flex ${dir} gap-2 overflow-auto ${align}">
				${map(children, (child: any) => this.renderNode(child))}
			</div>
		`;
	}
}

customElements.define('nebo-a2ui-list', NeboListElement);
export const NeboList = { ...ListApi, tagName: 'nebo-a2ui-list' };
