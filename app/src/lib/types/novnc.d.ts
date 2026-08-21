// Minimal typings for @novnc/novnc 1.7 — only the surface DesktopView uses.
declare module '@novnc/novnc' {
	export default class RFB extends EventTarget {
		constructor(
			target: HTMLElement,
			url: string,
			options?: { shared?: boolean; credentials?: { password?: string } }
		);
		viewOnly: boolean;
		scaleViewport: boolean;
		resizeSession: boolean;
		disconnect(): void;
	}
}
