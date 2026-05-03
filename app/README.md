# Nebo V2

Nebo is a multi-agent orchestration platform. V2 is a complete UI redesign built with SvelteKit, Tailwind CSS v4, and DaisyUI v5.

## Stack

- **SvelteKit 2** with Svelte 5 runes (`$state`, `$derived`, `$props`, `$effect`)
- **Tailwind CSS v4** + **DaisyUI v5** — semantic color tokens, 11 themes
- **TypeScript** across all components
- **Geist** typeface (body) + **Geist Mono** (data/code)
- **svelte-i18n** — 25 languages, lazy-loaded

## Getting Started

```sh
pnpm install
pnpm dev
```

Build for production:

```sh
pnpm build
pnpm preview
```

## Project Structure

```
src/
  routes/              # SvelteKit pages (45 routes)
    +layout.svelte     # Root layout: header nav, Cmd+K, onboarding redirect
    +page.svelte       # Main 3-column agent layout
    schedule/          # Calendar day/week/month views
    workspaces/        # Agent-powered workspace apps (CRM, Analytics, etc.)
    team/              # Org chart with drag-to-reparent
    marketplace/       # Marketplace with own layout (agents, skills, plugins, connectors)
    settings/          # 22 settings pages with dev mode gating
    onboarding/        # 4-step setup wizard
  lib/
    components/        # Reusable components (24 total)
      chat/            # ChatPane, ChatComposer, SlashCommandMenu
    stores/            # Svelte stores (sidebar, theme, marketplace, devmode, etc.)
    mockData.ts        # All mock data for the prototype
    tokens.js          # Agent color utilities
    i18n/              # Translation files (25 languages)
  app.css              # Global styles, theme config, animations
```

## Key Features

- **3-column agent layout** — agent roster, thread/run/settings list, chat canvas
- **Agent settings** — General (with delete), Identity, Persona, Configure, Workflows, Skills, Memory, Permissions
- **Agent editability** — `editable` flag from agent.json; read-only agents show disabled forms
- **Marketplace** — install/uninstall with dependency cascading, install codes, MCP connectors
- **Command Palette** — Cmd+K, 29 items, arrow key navigation, grouped categories
- **Calendar** — day/week/month views with agent color coding
- **Org chart** — drag-to-reparent with pan/zoom
- **Workspace apps** — CRM, Content Calendar, Analytics, Code Review
- **Settings** — 22 pages, dev mode gating, theme picker (11 themes)
- **Billing** — Stripe-ready with upgrade overlay

## Style Guide

See `STYLE-GUIDE.md` for the full design system. Key rules:

- All colors via DaisyUI semantic tokens — never hardcode hex values
- Typography hierarchy: `text-base` (titles) → `text-sm` (body) → `text-xs` (secondary/meta)
- Surface hierarchy: `bg-base-200` (sidebar) → `bg-base-100` (content) → `bg-base-100 + shadow` (cards)
- Styles only in `app.css` — no `<style>` blocks in components
