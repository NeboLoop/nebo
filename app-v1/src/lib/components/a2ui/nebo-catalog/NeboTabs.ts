import { html, nothing } from 'lit';
import { TabsApi } from '@a2ui/web_core/v0_9/basic_catalog';
import { A2uiLitElement, A2uiController } from '@a2ui/lit/v0_9';

export class NeboTabsElement extends A2uiLitElement<typeof TabsApi> {
	createRenderRoot() {
		return this;
	}

	// Reactive state — replaces @state() accessor activeIndex = 0
	private _activeIndex = 0;
	get activeIndex() {
		return this._activeIndex;
	}
	set activeIndex(v: number) {
		this._activeIndex = v;
		this.requestUpdate();
	}

	protected createController() {
		return new A2uiController(this, TabsApi);
	}

	render() {
		const props = this.controller.props;
		if (!props?.tabs) return nothing;

		return html`
			<div class="nebo-tabs">
				<div class="nebo-tabs-headers" role="tablist">
					${props.tabs.map(
						(tab: any, i: number) => html`
							<button
								class="nebo-tab-btn ${i === this.activeIndex ? 'active' : ''}"
								role="tab"
								aria-selected=${i === this.activeIndex}
								@click=${() => (this.activeIndex = i)}
							>
								${tab.title}
							</button>
						`
					)}
				</div>
				<div class="nebo-tab-content" role="tabpanel">
					${props.tabs[this.activeIndex]
						? this.renderNode(props.tabs[this.activeIndex].child)
						: nothing}
				</div>
			</div>
		`;
	}
}

customElements.define('nebo-a2ui-tabs', NeboTabsElement);
export const NeboTabs = { ...TabsApi, tagName: 'nebo-a2ui-tabs' };
