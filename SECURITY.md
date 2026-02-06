# OpenClaw's security landscape: a comprehensive vulnerability inventory

**OpenClaw (formerly Clawdbot, then Moltbot) has accumulated at least three assigned CVEs, seven GitHub Security Advisories, and a sprawling ecosystem of supply-chain and architectural security concerns — all within roughly three months of its November 2025 launch.** The project, created by Peter Steinberger, is an open-source personal AI assistant with **~160,000 GitHub stars** that integrates with messaging platforms and can execute shell commands, manage files, and take autonomous actions on a user's machine. Its rapid growth has made it a high-value target, and security researchers from CrowdStrike, Cisco, Bitdefender, Tenable, JFrog, and others have published damning assessments. The project itself acknowledges: "There is no 'perfectly secure' setup."

## The naming history matters for tracking vulnerabilities

Clawdbot launched in November 2025 and was renamed **Moltbot** on January 27, 2026, after trademark pressure from Anthropic over similarity to "Claude." Just two days later, on January 29, it became **OpenClaw**. The npm package remains listed as `clawdbot`. All three names appear interchangeably across CVE databases, advisories, and security reports. The repository lives at `github.com/openclaw/openclaw`, with legacy URLs redirecting from the previous organization names. During the rebrand chaos, crypto scammers seized the abandoned `@clawdbot` handles on X and GitHub, promoting fake $CLAWD tokens to **60,000+ followers** — a supply-chain identity attack in its own right.

## Three CVEs with assigned identifiers target OpenClaw directly

**CVE-2026-25253 (CVSS 8.8, High) — One-click RCE via gateway token exfiltration.** Discovered by Mav Levin of depthfirst and independently by Ethiack's autonomous hacking agent "Hackian," this is the most widely reported vulnerability. The Control UI accepted a `gatewayUrl` parameter from the query string without validation and auto-connected on page load, transmitting the stored gateway authentication token via WebSocket. An attacker could craft a malicious link that, when clicked, exfiltrated the token in milliseconds. Combined with a Cross-Site WebSocket Hijacking flaw (no origin header validation), the full kill chain was: token theft → WebSocket hijack → disable sandbox → escape Docker container → arbitrary command execution on host. **This worked even on localhost-only instances** because the victim's browser acted as the bridge. Patched in v2026.1.29 on January 30, 2026. GitHub Advisory: GHSA-g8p2-7wf7-98mq.

**CVE-2026-24763 (CVSS 8.8, High) — Command injection in Docker sandbox via PATH variable.** Unsafe handling of the PATH environment variable when constructing shell commands in Docker sandbox execution allowed authenticated users who could control environment variables to execute arbitrary commands within the container. Affected versions through v2026.1.24; patched in v2026.1.29. GitHub Advisory: GHSA-mc68-q9jw-2h3v. A public proof-of-concept exists at `github.com/ethiack/moltbot-1click-rce`.

**CVE-2026-25157 (High) — OS command injection via SSH project root path.** Two related flaws in the macOS application's `CommandResolver.swift`: the `sshNodeCommand` function constructed a shell script without escaping user-supplied project paths, and `parseSSHTarget` failed to validate SSH targets beginning with dashes, enabling `-oProxyCommand=...` injection. Scope was limited to the macOS desktop app in SSH remote mode only — CLI, web gateway, and mobile apps were unaffected. Patched in v2026.1.29. GitHub Advisory: GHSA-q284-4pvr-m585.

A fourth CVE, **CVE-2026-22708**, has been referenced in security research from Penligent and CrowdStrike concerning **indirect prompt injection via web browsing** — OpenClaw's failure to sanitize web content before feeding it into the LLM context window allows hidden instructions in webpages, emails, or documents to trigger unauthorized command execution.

## Seven GitHub Security Advisories cover additional attack surfaces

Beyond the three formally CVE'd vulnerabilities, the project has published or acknowledged additional advisories:

- **GHSA-g55j-c2v4-pjcg (High, February 4, 2026)** — Unauthenticated local RCE via WebSocket `config.apply` endpoint, allowing any local process to modify the agent's configuration and trigger code execution.
- **GHSA-r8g4-86fx-92mq (Moderate, February 4, 2026)** — Local file inclusion via the MEDIA path extraction mechanism in the media parser, enabling unauthorized file reads.
- **GHSA-mr32-vwc2-5j6h** — Credential theft via Chrome extension relay. Reported by ZeroPath (with PoC at `ZeroPathAI/clawdbot-stealer-poc`), malicious websites could exploit OpenClaw's Chrome extension to steal browser session credentials from open tabs (Gmail, Microsoft 365) via an unvalidated `/cdp` WebSocket endpoint accepting arbitrary Chrome DevTools Protocol commands.
- **GHSA-4mhr-g7xj-cg8j** — Arbitrary execution via `lobsterPath`/`cwd` injection in the Lobster workflow engine.

The project's changelogs reveal dozens of additional security-relevant fixes that never received formal advisories, including **path traversal in WhatsApp accountId handling**, **sandbox root validation bypasses in message-tool file paths**, **SSRF via DNS pinning in web auto-reply**, **LD_PRELOAD/DYLD_INSERT environment variable injection for host execution**, and **CWE-400 timeout issues in SSE client fetch calls**.

## Architectural and ecosystem vulnerabilities dwarf the CVE count

The formal CVEs represent only the tip of the iceberg. Multiple independent security audits have identified deep architectural concerns:

**Plaintext credential storage** is perhaps the most systemic risk. OX Security, 1Password, and Hudson Rock have documented that OpenClaw stores API keys, OAuth tokens, bot tokens, user profiles, and conversation memories in **plaintext Markdown and JSON files** under `~/.openclaw/`. Even "deleted" credentials persist in `.bak.X` backup files. OX Security further found `eval` used **100 times** and `execSync` **9 times** in the codebase. Infostealer malware families — RedLine, Lumma, Vidar — are already building specific harvesting capabilities targeting this file structure.

**Massive public exposure** compounds every vulnerability. Censys scan data from January 31, 2026 identified over **21,000 publicly accessible OpenClaw instances**, approximately 30% hosted on Alibaba Cloud. Many lacked any authentication. The root cause is a "localhost trust" assumption — the code treated any request from 127.0.0.1 as the owner, which collapses behind reverse proxies where attackers can spoof headers.

**Supply-chain poisoning through ClawHub skills** has reached alarming scale. Koi Security audited 2,857 skills on ClawHub and found **341 malicious ones (~12%)**, with a campaign codenamed "ClawHavoc" distributing Atomic Stealer (AMOS) malware. Bitdefender independently reported **~17% of analyzed skills exhibited malicious behavior** in the first week of February 2026, with 54% of malicious skills targeting cryptocurrency users. A single account (`sakaen736jih`) published **199 malicious skills**. A trojanized "ClawdBot Agent" VS Code extension was also discovered installing remote access tools — the official team never published a VS Code extension.

## Dependency-level CVEs add further exposure

OpenClaw's security posture is also affected by vulnerabilities in its dependency chain. The project's own `SECURITY.md` requires **Node.js 22.12.0+** to incorporate patches for CVE-2025-59466 (async_hooks denial of service) and CVE-2026-21636 (permission model bypass). The broader MCP protocol ecosystem that OpenClaw relies on has its own critical vulnerabilities: **CVE-2025-49596** (CVSS 9.4, unauthenticated access in MCP Inspector) and **CVE-2025-6514** (CVSS 9.6, command injection in mcp-remote).

## What the security community concludes

Simon Willison, who coined the term "prompt injection," identified OpenClaw as embodying the **"lethal trifecta"**: access to private data, exposure to untrusted content, and ability to communicate externally. Palo Alto Networks added a critical fourth element — **persistent memory** — enabling time-shifted prompt injection and logic-bomb-style attacks. Laurie Voss, founding CTO of npm, was more blunt: "OpenClaw is a security dumpster fire."

The project has no bug bounty program and no budget for paid security reports. Responsible disclosure is requested via GitHub Security Advisories. A built-in `openclaw security audit --deep` command exists for self-assessment, and Tenable has released detection plugins. The `SECURITY.md` explicitly marks public internet exposure and prompt injection attacks as **out of scope** — a telling admission given that both are actively exploited in the wild.

| CVE / Advisory | Severity | Description | Fixed Version |
|---|---|---|---|
| CVE-2026-25253 / GHSA-g8p2-7wf7-98mq | High (8.8) | 1-click RCE via gatewayUrl token exfiltration | v2026.1.29 |
| CVE-2026-24763 / GHSA-mc68-q9jw-2h3v | High (8.8) | Docker sandbox command injection via PATH | v2026.1.29 |
| CVE-2026-25157 / GHSA-q284-4pvr-m585 | High | SSH command injection in macOS app | v2026.1.29 |
| CVE-2026-22708 | — | Indirect prompt injection via web content | Unconfirmed |
| GHSA-g55j-c2v4-pjcg | High | Unauthenticated local RCE via WebSocket config.apply | Feb 4, 2026 patch |
| GHSA-r8g4-86fx-92mq | Moderate | Local file inclusion via MEDIA path extraction | Feb 4, 2026 patch |
| GHSA-mr32-vwc2-5j6h | — | Chrome extension relay credential theft | Patched (commit a1e89af) |
| GHSA-4mhr-g7xj-cg8j | — | Arbitrary exec via lobsterPath/cwd injection | Patched |

## Conclusion

OpenClaw's vulnerability surface extends far beyond its three formally assigned CVEs. The combination of **at least 7 GitHub Security Advisories**, dozens of security-relevant code fixes, architectural weaknesses (plaintext credentials, localhost trust assumptions), and a supply-chain ecosystem where **12–17% of community skills are malicious** makes it one of the most security-challenged popular open-source projects of early 2026. Every vulnerability was discovered and disclosed within a compressed three-month window, suggesting the codebase has not yet undergone thorough security hardening. Users running any version prior to v2026.1.29 face critical remote code execution risks, and even patched installations remain exposed to prompt injection, credential theft via infostealers, and malicious skill installation. The project's own documentation concedes there is no perfectly secure configuration — a rare and honest admission that prospective deployers should take seriously.
