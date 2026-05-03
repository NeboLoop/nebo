import type { A2UIViewsConfig } from '../types.js';

const coder: A2UIViewsConfig = {
  _nav: [
    { viewId: 'queue', label: 'Queue', icon: 'git-pull-request' },
    { viewId: 'active', label: 'Active', icon: 'eye' },
    { viewId: 'merged', label: 'Merged', icon: 'git-merge' },
  ],

  queue: {
    components: [
      { id: 'root', component: 'column', props: { gap: '2' }, children: { componentId: 'pr-card', path: '/pullRequests' } },
      { id: 'pr-card', component: 'card', props: { class: 'cursor-pointer hover:border-base-content/50 transition-all' }, children: ['pr-inner'] },
      { id: 'pr-inner', component: 'column', props: { gap: '1' }, children: ['pr-header', 'pr-meta'] },
      { id: 'pr-header', component: 'row', props: { align: 'center', gap: '2' }, children: ['pr-number', 'pr-title', 'pr-status'] },
      { id: 'pr-number', component: 'text', props: { text: { path: 'pr' }, variant: 'mono' } },
      { id: 'pr-title', component: 'text', props: { text: { path: 'title' }, variant: 'body-medium', class: 'flex-1' } },
      { id: 'pr-status', component: 'badge', props: { text: { path: 'statusLabel' }, variant: { path: 'statusVariant' } } },
      { id: 'pr-meta', component: 'row', props: { align: 'center', gap: '3' }, children: ['pr-author', 'pr-branch', 'pr-additions', 'pr-deletions', 'pr-files', 'pr-age'] },
      { id: 'pr-author', component: 'text', props: { text: { path: 'author' }, variant: 'caption' } },
      { id: 'pr-branch', component: 'text', props: { text: { path: 'branch' }, variant: 'mono' } },
      { id: 'pr-additions', component: 'text', props: { text: { path: 'additionsLabel' }, variant: 'accent' } },
      { id: 'pr-deletions', component: 'text', props: { text: { path: 'deletionsLabel' }, variant: 'body', class: 'text-error' } },
      { id: 'pr-files', component: 'text', props: { text: { path: 'filesLabel' }, variant: 'caption' } },
      { id: 'pr-age', component: 'text', props: { text: { path: 'ageLabel' }, variant: 'caption', class: 'ml-auto' } },
    ],
    data: {
      pullRequests: [
        { pr: '#347', title: 'feat: Add agent workflow editor', author: 'sarah-dev', branch: 'feat/workflow-editor', additionsLabel: '+482', deletionsLabel: '-89', filesLabel: '12 files', ageLabel: '2h ago', statusLabel: 'needs review', statusVariant: 'accent' },
        { pr: '#345', title: 'fix: Thread memory leak on reconnect', author: 'james-k', branch: 'fix/thread-leak', additionsLabel: '+23', deletionsLabel: '-8', filesLabel: '2 files', ageLabel: '5h ago', statusLabel: 'needs review', statusVariant: 'accent' },
        { pr: '#343', title: 'refactor: Extract A2UI surface manager', author: 'maria-g', branch: 'refactor/a2ui-surface', additionsLabel: '+156', deletionsLabel: '-210', filesLabel: '7 files', ageLabel: '1d ago', statusLabel: 'changes requested', statusVariant: 'warning' },
        { pr: '#341', title: 'feat: Marketplace search improvements', author: 'alex-t', branch: 'feat/marketplace-search', additionsLabel: '+94', deletionsLabel: '-31', filesLabel: '5 files', ageLabel: '2d ago', statusLabel: 'approved', statusVariant: 'success' },
        { pr: '#340', title: 'chore: Upgrade Tailwind to v4.1', author: 'david-l', branch: 'chore/tailwind-v4.1', additionsLabel: '+12', deletionsLabel: '-45', filesLabel: '3 files', ageLabel: '2d ago', statusLabel: 'needs review', statusVariant: 'accent' },
      ],
    },
  },

  active: {
    components: [
      { id: 'root', component: 'column', children: ['review-label', 'review-card'] },
      { id: 'review-label', component: 'text', props: { text: 'Currently Reviewing', variant: 'label' } },
      { id: 'review-card', component: 'card', children: ['review-inner'] },
      { id: 'review-inner', component: 'column', props: { gap: '3' }, children: ['review-header', 'review-stats', 'review-notes-label', 'review-notes', 'review-actions'] },
      { id: 'review-header', component: 'row', props: { align: 'center', gap: '2' }, children: ['review-pr', 'review-title'] },
      { id: 'review-pr', component: 'text', props: { text: { path: '/review/pr' }, variant: 'mono' } },
      { id: 'review-title', component: 'text', props: { text: { path: '/review/title' }, variant: 'body-medium' } },
      { id: 'review-stats', component: 'row', props: { gap: '5' }, children: ['stat-author', 'stat-files', 'stat-diff', 'stat-ai'] },
      { id: 'stat-author', component: 'column', props: { gap: '0' }, children: ['stat-author-label', 'stat-author-val'] },
      { id: 'stat-author-label', component: 'text', props: { text: 'Author', variant: 'mono' } },
      { id: 'stat-author-val', component: 'text', props: { text: { path: '/review/author' }, variant: 'body' } },
      { id: 'stat-files', component: 'column', props: { gap: '0' }, children: ['stat-files-label', 'stat-files-val'] },
      { id: 'stat-files-label', component: 'text', props: { text: 'Files', variant: 'mono' } },
      { id: 'stat-files-val', component: 'text', props: { text: { path: '/review/filesChanged' }, variant: 'body' } },
      { id: 'stat-diff', component: 'column', props: { gap: '0' }, children: ['stat-diff-label', 'stat-diff-val'] },
      { id: 'stat-diff-label', component: 'text', props: { text: 'Diff', variant: 'mono' } },
      { id: 'stat-diff-val', component: 'text', props: { text: { path: '/review/diff' }, variant: 'body' } },
      { id: 'stat-ai', component: 'column', props: { gap: '0' }, children: ['stat-ai-label', 'stat-ai-val'] },
      { id: 'stat-ai-label', component: 'text', props: { text: 'AI Review', variant: 'mono' } },
      { id: 'stat-ai-val', component: 'text', props: { text: { path: '/review/aiSuggestions' }, variant: 'body' } },
      { id: 'review-notes-label', component: 'text', props: { text: 'AI Review Notes', variant: 'label' } },
      { id: 'review-notes', component: 'column', props: { gap: '1' }, children: { componentId: 'note-item', path: '/review/notes' } },
      { id: 'note-item', component: 'row', props: { align: 'start', gap: '2', class: 'py-2 px-3 rounded-lg bg-base-100' }, children: ['note-dot', 'note-info'] },
      { id: 'note-dot', component: 'dot', props: { variant: { path: 'severity' } } },
      { id: 'note-info', component: 'column', props: { gap: '0' }, children: ['note-file', 'note-text'] },
      { id: 'note-file', component: 'text', props: { text: { path: 'file' }, variant: 'mono' } },
      { id: 'note-text', component: 'text', props: { text: { path: 'note' }, variant: 'body' } },
      { id: 'review-actions', component: 'row', props: { gap: '2' }, children: ['approve-btn', 'changes-btn'] },
      { id: 'approve-btn', component: 'button', props: { label: 'Approve', variant: 'primary' }, action: { name: 'approve-pr' } },
      { id: 'changes-btn', component: 'button', props: { label: 'Request Changes', variant: 'default' }, action: { name: 'request-changes' } },
    ],
    data: {
      review: {
        pr: '#347',
        title: 'feat: Add agent workflow editor',
        author: 'sarah-dev',
        filesChanged: '12 changed',
        diff: '+482 -89',
        aiSuggestions: '3 suggestions',
        notes: [
          { file: 'WorkflowEditor.svelte', note: 'Consider memoizing the workflow entries derived value', severity: 'success' },
          { file: 'mockData.js', note: 'Activity steps array should validate non-empty', severity: 'warning' },
          { file: 'a2ui-actions.rs', note: 'Missing error handling for navigate action timeout', severity: 'warning' },
        ],
      },
    },
  },

  merged: {
    components: [
      { id: 'root', component: 'card', children: ['merged-list'] },
      { id: 'merged-list', component: 'column', props: { gap: '0' }, children: { componentId: 'merged-row', path: '/mergedPRs' } },
      { id: 'merged-row', component: 'row', props: { align: 'center', gap: '3', class: 'px-2 py-3 border-b border-base-300 last:border-0' }, children: ['merged-check', 'merged-pr', 'merged-info', 'merged-diff'] },
      { id: 'merged-check', component: 'icon', props: { name: 'check', size: 14, class: 'text-success shrink-0' } },
      { id: 'merged-pr', component: 'text', props: { text: { path: 'pr' }, variant: 'meta', class: 'shrink-0' } },
      { id: 'merged-info', component: 'column', props: { gap: '0', class: 'flex-1 min-w-0' }, children: ['merged-title', 'merged-meta'] },
      { id: 'merged-title', component: 'text', props: { text: { path: 'title' }, variant: 'body-medium' } },
      { id: 'merged-meta', component: 'text', props: { text: { path: 'meta' }, variant: 'caption' } },
      { id: 'merged-diff', component: 'row', props: { gap: '2', class: 'shrink-0' }, children: ['merged-additions', 'merged-deletions'] },
      { id: 'merged-additions', component: 'text', props: { text: { path: 'additionsLabel' }, variant: 'accent' } },
      { id: 'merged-deletions', component: 'text', props: { text: { path: 'deletionsLabel' }, variant: 'body', class: 'text-error' } },
    ],
    data: {
      mergedPRs: [
        { pr: '#339', title: 'feat: WebSocket reconnection logic', meta: 'james-k \u00b7 Apr 27', additionsLabel: '+145', deletionsLabel: '-32' },
        { pr: '#337', title: 'fix: Calendar timezone handling', meta: 'maria-g \u00b7 Apr 26', additionsLabel: '+28', deletionsLabel: '-12' },
        { pr: '#335', title: 'feat: Agent permissions UI', meta: 'sarah-dev \u00b7 Apr 25', additionsLabel: '+312', deletionsLabel: '-45' },
        { pr: '#333', title: 'refactor: Unify theme loading', meta: 'alex-t \u00b7 Apr 24', additionsLabel: '+67', deletionsLabel: '-89' },
        { pr: '#331', title: 'fix: Memory bank pagination', meta: 'david-l \u00b7 Apr 23', additionsLabel: '+14', deletionsLabel: '-8' },
      ],
    },
  },
};

export default coder;
