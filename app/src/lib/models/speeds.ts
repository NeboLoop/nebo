/**
 * Intelligence: the two choices a conversation (or an employee) makes about
 * the AI it runs on, read from and written to the same model string every
 * assignment already carries.
 *
 * - The LEVEL: Auto, then always the same five, Instant · Low · Medium ·
 *   High · Max (`types::packs::Effort`). A reference with no level (the
 *   pack's Auto, or its default) is Auto: Nebo picks the level per task.
 * - The PACK: which models those levels run on. Nebo AI is built in;
 *   the owner can add his own (Settings → Intelligence Packs).
 *
 * The built-in Nebo AI pack never shows a model, provider or gateway id: its
 * levels read as words. An owner's pack shows what he set, as he set it: its
 * name, and each level's provider and model. A raw model that maps to no
 * pack reads as "Custom".
 */
import * as api from '$lib/api/nebo';
import type { LevelEffort, PackChoice, PackLevels } from '$lib/api/neboComponents';

/** The Effort ladder, lowest first (`types::packs::Effort`). */
export const EFFORTS = ['instant', 'low', 'medium', 'high', 'max'] as const;
export type Effort = (typeof EFFORTS)[number];

/** The level rows, Auto first: Auto is a reference with no level. */
export const LEVELS = ['auto', ...EFFORTS] as const;
export type Level = (typeof LEVELS)[number];

/** A level row as the level a choice carries (Auto: none). */
export function levelOf(row: Level): Effort | null {
	return row === 'auto' ? null : row;
}

/** The built-in pack's id. */
export const NEBO_AI_ID = 'nebo-ai';

/**
 * A pack as the picker shows it. An owner's pack carries what each level
 * runs on (`provider/model` strings) and each level's provider effort.
 */
export type PackRow = {
	id: string;
	name: string;
	builtIn: boolean;
	levels?: PackLevels;
	levelEffort?: LevelEffort;
};

/**
 * The packs, the bot's default model string, and the models a pack can name
 * (read only when the owner has a pack of his own: it names their providers).
 */
export type Intelligence = { packs: PackRow[]; botDefault: string; choices: PackChoice[] };

/**
 * What a model string runs on: a pack at a level (null: the pack's Auto or
 * default, shown as Medium), or a raw model no pack names.
 */
export type Choice = { pack: string; level: Effort | null } | { custom: true };

export function isCustom(c: Choice): c is { custom: true } {
	return 'custom' in c;
}

/** What one model string runs on. '' is the built-in pack's Auto. */
export function resolve(value: string, packs: PackRow[]): Choice {
	const v = value.trim();
	if (!v) return { pack: NEBO_AI_ID, level: null };
	if (v.startsWith('pack/')) {
		const [id, level] = v.slice(5).split('/');
		const known = packs.some((p) => p.id === id);
		const effort = (EFFORTS as readonly string[]).includes(level ?? '') ? (level as Effort) : null;
		if (!id || (level && level !== 'auto' && !effort)) return { custom: true };
		// A deleted pack runs on Nebo AI at the same level (the bot says so).
		return { pack: known ? id : NEBO_AI_ID, level: effort };
	}
	return { custom: true };
}

/** Whether two choices run the same pack at the same level. */
export function sameChoice(a: Choice, b: Choice): boolean {
	if (isCustom(a) || isCustom(b)) return false;
	return a.pack === b.pack && a.level === b.level;
}

/**
 * The model string to write for a pick. `inherited` is what '' means where it
 * is written (a chat: the employee's choice, else the bot's default; an
 * employee: the bot's default): a pick equal to it clears the override.
 */
export function valueFor(pack: string, level: Effort | null, inherited: string, packs: PackRow[]): string {
	if (sameChoice(resolve(inherited, packs), { pack, level })) return '';
	return level ? `pack/${pack}/${level}` : `pack/${pack}`;
}

/** The level row a choice shows as checked: no level is Auto; none for a raw model. */
export function shownLevel(c: Choice): Level | null {
	return isCustom(c) ? null : (c.level ?? 'auto');
}

/** The packs and the bot's default; null when they can't be read (the chip hides). */
export async function loadIntelligence(): Promise<Intelligence | null> {
	try {
		const res = await api.listPacks();
		const packs: PackRow[] = (res.packs ?? []).map((p) => ({
			id: p.id,
			name: p.name,
			builtIn: !!p.builtIn,
			levels: p.levels,
			levelEffort: p.levelEffort
		}));
		if (!packs.some((p) => p.id === NEBO_AI_ID)) packs.unshift({ id: NEBO_AI_ID, name: 'Nebo AI', builtIn: true });
		const choices = packs.some((p) => !p.builtIn)
			? await (async () => (await api.listPackChoices()).chat ?? [])().catch(() => [] as PackChoice[])
			: [];
		return { packs, botDefault: res.default ?? '', choices };
	} catch {
		return null;
	}
}


/** A translator, as `$t` is. */
type Translate = (key: string, options?: { values?: Record<string, string> }) => string;

/** A pack's name on screen: Nebo AI's own; the owner's exactly as he named it. */
export function packName(p: PackRow, t: Translate): string {
	if (p.builtIn) return t('modelPick.neboAi');
	return p.name;
}

/**
 * The composer chip: the pack's name and the level ("Nebo AI · Auto",
 * "Work keys · High"); "Custom" for a raw model.
 */
export function choiceLabel(c: Choice, packs: PackRow[], t: Translate): string {
	if (isCustom(c)) return t('modelPick.custom');
	const level = t(`settingsPacks.level.${shownLevel(c)}`);
	const pack = packs.find((p) => p.id === c.pack);
	return `${pack ? packName(pack, t) : t('modelPick.neboAi')} · ${level}`;
}

/**
 * Any stored model string as words: a pack as the chip names it, any other model by its own id without the provider or connection
 * in front ("anthropic@x/claude-…" → "claude-…").
 */
export function modelName(value: string, packs: PackRow[], t: Translate): string {
	const c = resolve(value, packs);
	if (!isCustom(c)) return choiceLabel(c, packs, t);
	return value.split('/').pop() || value;
}

/** How long a pick's checkmark shows before the picker closes. */
export const CLOSE_AFTER_MS = 150;

/**
 * A pick in a picker that closes on selection: the row already in use (the
 * checked one) closes without a write; any other is picked at once, so its
 * checkmark shows, and the picker closes a moment later.
 */
export function selectThenClose(
	current: Choice,
	pack: string,
	level: Effort | null,
	onpick: (pack: string, level: Effort | null) => void,
	onclose: () => void,
	delay = CLOSE_AFTER_MS
): void {
	const inUse = !isCustom(current) && current.pack === pack && current.level === level;
	if (inUse) return onclose();
	onpick(pack, level);
	setTimeout(onclose, delay);
}

/** Provider kinds as words, for a model no longer among the pack choices. */
const KIND_LABELS: Record<string, string> = {
	anthropic: 'Anthropic',
	openai: 'OpenAI',
	google: 'Google',
	openrouter: 'OpenRouter',
	deepseek: 'DeepSeek',
	ollama: 'Ollama',
	janus: 'Nebo AI'
};

/**
 * What an owner's pack set one level to, as he set it: the provider's label
 * and the model, by the name its provider gives it ("Nebo AI · High and Max")
 * else its id verbatim ("OpenAI · gpt-6.1").
 * Only the connection key (`openai@<id>`) and a linked agent's id stay out.
 */
export function runsOnLine(value: string, choices: PackChoice[]): string {
	const slash = value.indexOf('/');
	const head = slash < 0 ? '' : value.slice(0, slash);
	const kind = head.split('@')[0];
	let model = slash < 0 ? value : value.slice(slash + 1);
	if (kind === 'linked') model = model.slice(model.indexOf('/') + 1);
	const hit = choices.find((c) => c.value === value);
	const provider = hit?.provider ?? KIND_LABELS[kind] ?? kind;
	return [provider, hit?.name || hit?.modelId || model].filter(Boolean).join(' · ');
}

/**
 * A level row's subtitle. Nebo AI: what the level is for. An owner's pack:
 * what that pack runs the level on (with its provider effort) and Auto as
 * picking from the pack's levels. A level the pack leaves empty runs on Nebo
 * AI's model for that level: Nebo AI's line with a quiet "from Nebo AI"
 * (`muted`).
 */
export function levelSubtitle(
	pack: PackRow | undefined,
	row: Level,
	choices: PackChoice[],
	t: Translate
): { text: string; muted: boolean } {
	if (!pack || pack.builtIn) return { text: t(`modelPick.levelDesc.${row}`), muted: false };
	if (row === 'auto') return { text: t('modelPick.autoCustom'), muted: false };
	const value = pack.levels?.[row]?.trim();
	if (!value) return { text: `${t(`modelPick.levelDesc.${row}`)} · ${t('modelPick.fromNeboAi')}`, muted: true };
	const effort = pack.levelEffort?.[row];
	const line = runsOnLine(value, choices);
	return {
		text: effort
			? `${line} · ${t('settingsPacks.withEffort', { values: { effort: t(`settingsPacks.level.${effort}`).toLocaleLowerCase() } })}`
			: line,
		muted: false
	};
}

/** A pack row's subtitle: Nebo AI's own line, an owner's pack by its providers ("OpenAI, Anthropic"). */
export function packSubtitle(pack: PackRow, choices: PackChoice[], t: Translate): string {
	if (pack.builtIn) return t('modelPick.packBuiltIn');
	const providers: string[] = [];
	for (const e of EFFORTS) {
		const value = pack.levels?.[e]?.trim();
		const provider = value ? runsOnLine(value, choices).split(' · ')[0] : '';
		if (provider && !providers.includes(provider)) providers.push(provider);
	}
	return providers.length ? providers.join(', ') : t('modelPick.packOwn');
}
