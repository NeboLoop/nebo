import type { A2UIViewsConfig } from '../types.js';

const chiefOfStaff: A2UIViewsConfig = {
  _nav: [
    { viewId: 'briefing', label: 'Briefing', icon: 'layout-dashboard' },
    { viewId: 'inbox', label: 'Inbox', icon: 'mail' },
    { viewId: 'calendar', label: 'Calendar', icon: 'calendar' },
    { viewId: 'priorities', label: 'Priorities', icon: 'flag' },
  ],

  briefing: {
    components: [
      { id: 'root', component: 'column', children: ['stats-row', 'content-row'] },
      { id: 'stats-row', component: 'row', props: { class: 'grid grid-cols-4' }, children: ['stat-emails', 'stat-meetings', 'stat-tasks', 'stat-urgent'] },
      { id: 'stat-emails', component: 'stat', props: { label: { path: '/metrics/emails/label' }, value: { path: '/metrics/emails/value' }, change: { path: '/metrics/emails/change' } } },
      { id: 'stat-meetings', component: 'stat', props: { label: { path: '/metrics/meetings/label' }, value: { path: '/metrics/meetings/value' }, change: { path: '/metrics/meetings/change' } } },
      { id: 'stat-tasks', component: 'stat', props: { label: { path: '/metrics/tasks/label' }, value: { path: '/metrics/tasks/value' }, change: { path: '/metrics/tasks/change' } } },
      { id: 'stat-urgent', component: 'stat', props: { label: { path: '/metrics/urgent/label' }, value: { path: '/metrics/urgent/value' }, change: { path: '/metrics/urgent/change' } } },
      { id: 'content-row', component: 'row', props: { class: 'grid grid-cols-2', align: 'start' }, children: ['today-card', 'urgent-card'] },
      { id: 'today-card', component: 'card', children: ['today-inner'] },
      { id: 'today-inner', component: 'column', props: { gap: '2' }, children: ['today-label', 'today-list'] },
      { id: 'today-label', component: 'text', props: { text: "Today's Schedule", variant: 'label' } },
      { id: 'today-list', component: 'column', props: { gap: '0' }, children: { componentId: 'meeting-row', path: '/todaySchedule' } },
      { id: 'meeting-row', component: 'row', props: { align: 'center', gap: '3', class: 'py-2.5 border-b border-base-300 last:border-0' }, children: ['meeting-time', 'meeting-info'] },
      { id: 'meeting-time', component: 'text', props: { text: { path: 'time' }, variant: 'mono', class: 'w-[80px] shrink-0' } },
      { id: 'meeting-info', component: 'column', props: { gap: '0', class: 'flex-1 min-w-0' }, children: ['meeting-title', 'meeting-with'] },
      { id: 'meeting-title', component: 'text', props: { text: { path: 'title' }, variant: 'body-medium' } },
      { id: 'meeting-with', component: 'text', props: { text: { path: 'with' }, variant: 'caption' } },
      { id: 'urgent-card', component: 'card', children: ['urgent-inner'] },
      { id: 'urgent-inner', component: 'column', props: { gap: '2' }, children: ['urgent-label', 'urgent-list'] },
      { id: 'urgent-label', component: 'text', props: { text: 'Needs Attention', variant: 'label' } },
      { id: 'urgent-list', component: 'column', props: { gap: '0' }, children: { componentId: 'urgent-row', path: '/urgentItems' } },
      { id: 'urgent-row', component: 'row', props: { align: 'center', gap: '2', class: 'py-2 border-b border-base-300 last:border-0' }, children: ['urgent-dot', 'urgent-info'] },
      { id: 'urgent-dot', component: 'dot', props: { variant: { path: 'severity' } } },
      { id: 'urgent-info', component: 'column', props: { gap: '0', class: 'flex-1' }, children: ['urgent-title', 'urgent-source'] },
      { id: 'urgent-title', component: 'text', props: { text: { path: 'title' }, variant: 'body' } },
      { id: 'urgent-source', component: 'text', props: { text: { path: 'source' }, variant: 'caption' } },
    ],
    data: {
      metrics: {
        emails: { label: 'Unread Emails', value: '14', change: '6 flagged as important' },
        meetings: { label: "Today's Meetings", value: '5', change: 'Next in 45 min' },
        tasks: { label: 'Open Tasks', value: '8', change: '3 due today' },
        urgent: { label: 'Urgent Items', value: '2', change: 'Needs your attention' },
      },
      todaySchedule: [
        { time: '9:00 AM', title: 'Team Standup', with: 'Engineering team' },
        { time: '10:30 AM', title: 'Client Review: Acme Corp', with: 'Sarah Chen, James Park' },
        { time: '12:00 PM', title: 'Lunch with Advisor', with: 'Mark Thompson' },
        { time: '2:00 PM', title: 'Product Strategy Session', with: 'Product team' },
        { time: '4:30 PM', title: 'Investor Update Call', with: 'Board members' },
      ],
      urgentItems: [
        { title: 'Contract renewal deadline tomorrow', severity: 'error', source: 'Acme Corp \u00b7 Email from legal@acme.co' },
        { title: 'Board deck needs final review', severity: 'warning', source: 'Due by 4:00 PM today' },
        { title: 'New partnership inquiry from DataFlow', severity: 'success', source: 'Email \u00b7 Received 1h ago' },
      ],
    },
  },

  inbox: {
    components: [
      { id: 'root', component: 'column', children: ['inbox-header', 'inbox-list'] },
      { id: 'inbox-header', component: 'row', props: { justify: 'between', align: 'center' }, children: ['inbox-label', 'triage-btn'] },
      { id: 'inbox-label', component: 'text', props: { text: 'Email Triage', variant: 'label' } },
      { id: 'triage-btn', component: 'button', props: { label: 'Auto-Triage', variant: 'primary' }, action: { name: 'auto-triage' } },
      { id: 'inbox-list', component: 'column', props: { gap: '0' }, children: { componentId: 'email-row', path: '/emails' } },
      { id: 'email-row', component: 'row', props: { align: 'center', gap: '3', class: 'py-3 border-b border-base-300 last:border-0 cursor-pointer hover:bg-base-200/50 transition-colors' }, children: ['email-priority', 'email-info', 'email-category', 'email-time'] },
      { id: 'email-priority', component: 'dot', props: { variant: { path: 'priority' } } },
      { id: 'email-info', component: 'column', props: { gap: '0', class: 'flex-1 min-w-0' }, children: ['email-from', 'email-subject'] },
      { id: 'email-from', component: 'text', props: { text: { path: 'from' }, variant: 'body-medium' } },
      { id: 'email-subject', component: 'text', props: { text: { path: 'subject' }, variant: 'caption' } },
      { id: 'email-category', component: 'badge', props: { text: { path: 'category' }, variant: { path: 'categoryVariant' } } },
      { id: 'email-time', component: 'text', props: { text: { path: 'time' }, variant: 'meta' } },
    ],
    data: {
      emails: [
        { from: 'Sarah Chen (Acme Corp)', subject: 'RE: Contract renewal \u2014 final terms attached', priority: 'error', category: 'Action Required', categoryVariant: 'error', time: '9:12 AM' },
        { from: 'Mark Thompson', subject: 'Lunch confirmed for noon \u2014 see you at La Piazza', priority: 'success', category: 'FYI', categoryVariant: 'info', time: '8:45 AM' },
        { from: 'DataFlow Labs', subject: 'Partnership proposal \u2014 agent marketplace integration', priority: 'warning', category: 'Review', categoryVariant: 'warning', time: '8:30 AM' },
        { from: 'GitHub Notifications', subject: '[nebo] PR #347 merged: feat: workflow editor', priority: 'success', category: 'Automated', categoryVariant: '', time: '7:55 AM' },
        { from: 'James Park (Board)', subject: 'Q3 board deck feedback \u2014 two comments', priority: 'warning', category: 'Review', categoryVariant: 'warning', time: '7:22 AM' },
        { from: 'Stripe', subject: 'Monthly revenue report \u2014 April 2026', priority: 'success', category: 'FYI', categoryVariant: 'info', time: 'Yesterday' },
      ],
    },
  },

  calendar: {
    components: [
      { id: 'root', component: 'column', children: ['week-card', 'upcoming-card'] },
      { id: 'week-card', component: 'card', children: ['week-inner'] },
      { id: 'week-inner', component: 'column', props: { gap: '3' }, children: ['week-header', 'week-grid'] },
      { id: 'week-header', component: 'text', props: { text: 'This Week', variant: 'body-medium' } },
      { id: 'week-grid', component: 'row', props: { gap: '2' }, children: { componentId: 'day-col', path: '/weekDays' } },
      { id: 'day-col', component: 'column', props: { gap: '1', class: 'flex-1 rounded-lg bg-base-100 p-2.5 min-h-[100px]' }, children: ['day-label', 'day-events'] },
      { id: 'day-label', component: 'text', props: { text: { path: 'label' }, variant: 'mono' } },
      { id: 'day-events', component: 'column', props: { gap: '1' }, children: { componentId: 'event-chip', path: 'events' } },
      { id: 'event-chip', component: 'text', props: { text: { path: 'title' }, variant: 'body-medium', class: 'rounded py-0.5 px-1.5 bg-base-200/50 truncate' } },
      { id: 'upcoming-card', component: 'card', children: ['upcoming-inner'] },
      { id: 'upcoming-inner', component: 'column', props: { gap: '2' }, children: ['upcoming-label', 'upcoming-list'] },
      { id: 'upcoming-label', component: 'text', props: { text: 'Upcoming This Week', variant: 'label' } },
      { id: 'upcoming-list', component: 'column', props: { gap: '0' }, children: { componentId: 'cal-row', path: '/upcoming' } },
      { id: 'cal-row', component: 'row', props: { align: 'center', gap: '3', class: 'py-2.5 border-b border-base-300 last:border-0' }, children: ['cal-day', 'cal-info'] },
      { id: 'cal-day', component: 'text', props: { text: { path: 'day' }, variant: 'mono', class: 'w-[70px] shrink-0' } },
      { id: 'cal-info', component: 'column', props: { gap: '0', class: 'flex-1 min-w-0' }, children: ['cal-title', 'cal-time'] },
      { id: 'cal-title', component: 'text', props: { text: { path: 'title' }, variant: 'body-medium' } },
      { id: 'cal-time', component: 'text', props: { text: { path: 'time' }, variant: 'caption' } },
    ],
    data: {
      weekDays: [
        { label: 'Mon', events: [{ title: 'Standup' }, { title: 'Client Call' }] },
        { label: 'Tue', events: [{ title: 'Strategy' }, { title: 'Lunch' }, { title: 'Investor' }] },
        { label: 'Wed', events: [{ title: 'All-Hands' }] },
        { label: 'Thu', events: [{ title: 'Board Prep' }, { title: '1:1s' }] },
        { label: 'Fri', events: [{ title: 'Review' }] },
      ],
      upcoming: [
        { day: 'Tomorrow', title: 'All-Hands Meeting', time: '10:00 AM \u2013 11:00 AM' },
        { day: 'Tomorrow', title: 'Product Demo Prep', time: '2:00 PM \u2013 3:00 PM' },
        { day: 'Thursday', title: 'Board Prep Session', time: '9:00 AM \u2013 10:30 AM' },
        { day: 'Thursday', title: '1:1 with Head of Sales', time: '11:00 AM \u2013 11:30 AM' },
        { day: 'Friday', title: 'Weekly Review & Planning', time: '3:00 PM \u2013 4:00 PM' },
      ],
    },
  },

  priorities: {
    components: [
      { id: 'root', component: 'column', children: ['today-label', 'priorities-list'] },
      { id: 'today-label', component: 'text', props: { text: "Today's Priorities", variant: 'label' } },
      { id: 'priorities-list', component: 'column', props: { gap: '2' }, children: { componentId: 'priority-card', path: '/priorities' } },
      { id: 'priority-card', component: 'card', children: ['priority-inner'] },
      { id: 'priority-inner', component: 'row', props: { align: 'center', gap: '3' }, children: ['priority-dot', 'priority-info', 'priority-status'] },
      { id: 'priority-dot', component: 'dot', props: { variant: { path: 'urgency' } } },
      { id: 'priority-info', component: 'column', props: { gap: '0', class: 'flex-1 min-w-0' }, children: ['priority-title', 'priority-context'] },
      { id: 'priority-title', component: 'text', props: { text: { path: 'title' }, variant: 'body-medium' } },
      { id: 'priority-context', component: 'text', props: { text: { path: 'context' }, variant: 'caption' } },
      { id: 'priority-status', component: 'badge', props: { text: { path: 'status' }, variant: { path: 'statusVariant' } } },
    ],
    data: {
      priorities: [
        { title: 'Review and sign Acme Corp contract', context: 'Deadline: Tomorrow 5:00 PM \u00b7 Legal has approved', urgency: 'error', status: 'Urgent', statusVariant: 'error' },
        { title: 'Finalize board deck for Thursday', context: 'James Park left 2 comments to address', urgency: 'error', status: 'Due Today', statusVariant: 'warning' },
        { title: 'Respond to DataFlow partnership inquiry', context: 'Promising marketplace integration opportunity', urgency: 'warning', status: 'This Week', statusVariant: 'info' },
        { title: 'Schedule Q2 planning offsite', context: 'Need to book venue and send invites', urgency: 'warning', status: 'This Week', statusVariant: 'info' },
        { title: 'Review hiring pipeline for engineering', context: '3 candidates in final round', urgency: 'success', status: 'Ongoing', statusVariant: '' },
      ],
    },
  },
};

export default chiefOfStaff;
