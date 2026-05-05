import type { A2UIViewsConfig } from '../types.js';
import researcher from './researcher.js';
import coder from './coder.js';
import social from './social.js';
import ops from './ops.js';
import marketer from './marketer.js';
import chiefOfStaff from './chief-of-staff.js';
import inboxManager from './inbox-manager.js';
import dailyBriefer from './daily-briefer.js';
import contentStrategist from './content-strategist.js';
import competitiveIntel from './competitive-intel.js';

export const AGENT_VIEWS: Record<string, A2UIViewsConfig> = {
  'research-analyst': researcher,
  'product-builder': coder,
  'social-media-manager': social,
  'outreach-coach': ops,
  'marketing-manager': marketer,
  'chief-of-staff': chiefOfStaff,
  'inbox-manager': inboxManager,
  'daily-briefer': dailyBriefer,
  'content-strategist': contentStrategist,
  'competitive-intel': competitiveIntel,
};
