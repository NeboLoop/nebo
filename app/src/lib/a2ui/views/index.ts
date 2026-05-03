import type { A2UIViewsConfig } from '../types.js';
import researcher from './researcher.js';
import coder from './coder.js';
import social from './social.js';
import ops from './ops.js';
import marketer from './marketer.js';

export const AGENT_VIEWS: Record<string, A2UIViewsConfig> = {
  researcher,
  coder,
  social,
  ops,
  marketer,
};
