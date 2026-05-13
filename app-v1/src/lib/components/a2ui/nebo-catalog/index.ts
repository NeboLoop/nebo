/**
 * Nebo A2UI Catalog — custom component implementations with DaisyUI styling.
 *
 * Replaces the basic catalog's shadow-DOM components with light-DOM versions
 * that render real DaisyUI/Tailwind classes. Uses the same catalog ID as the
 * basic catalog so no backend changes are needed.
 */
import { Catalog } from '@a2ui/web_core/v0_9';
import { BASIC_FUNCTIONS } from '@a2ui/web_core/v0_9/basic_catalog';
import type { LitComponentApi } from '@a2ui/lit/v0_9';

import { NeboButton } from './NeboButton';
import { NeboCard } from './NeboCard';
import { NeboText } from './NeboText';
import { NeboColumn } from './NeboColumn';
import { NeboRow } from './NeboRow';
import { NeboList } from './NeboList';
import { NeboTabs } from './NeboTabs';
import { NeboModal } from './NeboModal';
import { NeboChoicePicker } from './NeboChoicePicker';
import { NeboTextField } from './NeboTextField';
import { NeboCheckBox } from './NeboCheckBox';
import { NeboSlider } from './NeboSlider';
import { NeboDivider } from './NeboDivider';
import { NeboImage } from './NeboImage';
import { NeboIcon } from './NeboIcon';
import { NeboDateTimeInput } from './NeboDateTimeInput';
import { NeboAudioPlayer } from './NeboAudioPlayer';
import { NeboVideo } from './NeboVideo';

// Same catalog ID as basic catalog — backend hardcodes this in createSurface messages.
export const neboCatalog = new Catalog<LitComponentApi>(
	'https://a2ui.org/specification/v0_9/basic_catalog.json',
	[
		NeboButton,
		NeboCard,
		NeboText,
		NeboColumn,
		NeboRow,
		NeboList,
		NeboTabs,
		NeboModal,
		NeboChoicePicker,
		NeboTextField,
		NeboCheckBox,
		NeboSlider,
		NeboDivider,
		NeboImage,
		NeboIcon,
		NeboDateTimeInput,
		NeboAudioPlayer,
		NeboVideo,
	],
	BASIC_FUNCTIONS
);
