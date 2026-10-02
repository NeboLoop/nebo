import { describe, it, expect } from 'vitest';
import shell from './+layout.svelte?raw';
import chatPane from '$lib/components/chat/ChatPane.svelte?raw';
import flowsPane from '$lib/components/flows/FlowsPane.svelte?raw';
import runDetail from '$lib/components/runs/RunDetail.svelte?raw';

// The workspace shell renders only with the whole app behind it, so these
// read the source: one way to each place, and builder tools gated.
describe('workspace menus', () => {
	it('has no settings gear on a sidebar row: the chat header gear goes to the same place', () => {
		expect(shell).not.toContain("goto(`/${a.id}/settings/general`)");
		expect(shell).not.toContain('SettingsIcon');
	});

	it('makes the mark and the word ONE link Home', () => {
		const leading = shell.slice(shell.indexOf('{#snippet leading()}'), shell.indexOf('{/snippet}', shell.indexOf('{#snippet leading()}')));
		expect(leading.match(/<a /g)?.length).toBe(1);
		expect(leading).toMatch(/<a href="\/"[^>]*>\s*<BrandMark[^>]*\/>\s*<span[^>]*>Nebo<\/span>\s*<\/a>/);
	});

	it('gates Copy employee ID behind Developer mode', () => {
		const at = shell.indexOf("ctxAction('copy-id')");
		expect(shell.lastIndexOf('{#if $devMode}', at)).toBeGreaterThan(shell.lastIndexOf('{/if}', at));
	});

	it('gates the workflow canvas from a flow row behind Developer mode, keeping the switch', () => {
		const at = flowsPane.indexOf('ctx.openWorkflow(name, wf)');
		expect(flowsPane.lastIndexOf('{#if $devMode}', at)).toBeGreaterThan(-1);
		expect(flowsPane).toContain('onchange={() => ctx.toggleWorkflow(name)}');
	});

	it('gates Preview/Code and Publish in the chat pane', () => {
		expect(chatPane).toContain('{#if $devMode && activeArtifact?.url && (activeArtifact.codeUrl');
		expect(chatPane).toMatch(/const canPublish = \$derived\([^)]*\$appDeveloperMode\)/);
	});

	it('gates raw run input behind Developer mode', () => {
		expect(runDetail).toContain('{#if $devMode && (runInputData || inputSummary)}');
	});
});
