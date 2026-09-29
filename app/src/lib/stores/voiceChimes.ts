/**
 * The two sounds that open and close a voice call: the connect and
 * disconnect sounds bundled in `$lib/assets/sounds/`. The phone app plays the
 * same two files (`assets/sounds/` in the mobile repo).
 *
 * They are decoded for the call's playback context, which resamples them to
 * its own rate; their loudness is left as it is in the files.
 */
import connectUrl from '$lib/assets/sounds/nebo-voice-connect.wav';
import disconnectUrl from '$lib/assets/sounds/nebo-voice-disconnect.wav';

export interface VoiceChimes {
	/** The call is live. */
	connect: AudioBuffer;
	/** The call has ended, or the line was lost. */
	disconnect: AudioBuffer;
}

/** Both sounds, decoded for [ctx]. */
export async function loadVoiceChimes(ctx: BaseAudioContext): Promise<VoiceChimes> {
	const decode = async (url: string) =>
		ctx.decodeAudioData(await (await fetch(url)).arrayBuffer());
	const [connect, disconnect] = await Promise.all([decode(connectUrl), decode(disconnectUrl)]);
	return { connect, disconnect };
}
