// The page side of the built-in browser (CDP), evaluated in the page before
// every read or ref action. It keeps the Nebo Chrome extension's contract
// (chrome-extension/src/content/accessibility-tree.ts): the same tree lines
// (`role "name" [ref_N] href=… type=… placeholder=…`), the same `filter`,
// `depth`, `maxChars` and `refId`, refs that stay stable across reads, and
// the same ref resolution (live element first, then role + name + href +
// type). The model reads one format and clicks one kind of ref whichever
// browser serves it. One difference, and the extension should take it too:
// what the page does not render (display: none, visibility: hidden) is left
// out of every read. The extension keeps it in an unfiltered read, so a
// hidden result reads as if the page showed it (v0.16.0 proof: the text a
// click reveals was in the tree before the click, next to a spent
// "Loading..."). Defining is idempotent: a page keeps its map across
// reads, and a navigation starts a fresh one. The file is ONE expression,
// evaluated as `(<this file>, <call>)`: Obscura returns the value of a single
// expression only, never a script's completion value.
(() => {
  if (window.__neboGenerateAccessibilityTree && window.__neboResolveRef) return;

  window.__neboElementMap || (window.__neboElementMap = {});
  window.__neboRefCounter || (window.__neboRefCounter = 0);
  window.__neboElementMeta || (window.__neboElementMeta = {});
  window.__neboElementToRef || (window.__neboElementToRef = new WeakMap());

  const SKIP_TAGS = ['script', 'style', 'meta', 'link', 'title', 'noscript'];

  function getRole(el) {
    const explicit = el.getAttribute('role');
    if (explicit) return explicit;
    const tag = el.tagName.toLowerCase();
    const type = el.getAttribute('type');
    const roles = {
      a: 'link',
      button: 'button',
      input: type === 'submit' || type === 'button' ? 'button'
        : type === 'checkbox' ? 'checkbox'
        : type === 'radio' ? 'radio'
        : type === 'file' ? 'button'
        : 'textbox',
      select: 'combobox',
      textarea: 'textbox',
      h1: 'heading', h2: 'heading', h3: 'heading',
      h4: 'heading', h5: 'heading', h6: 'heading',
      img: 'image',
      nav: 'navigation',
      main: 'main',
      header: 'banner',
      footer: 'contentinfo',
      section: 'region',
      article: 'article',
      aside: 'complementary',
      form: 'form',
      table: 'table',
      ul: 'list',
      ol: 'list',
      li: 'listitem',
      label: 'label',
    };
    return roles[tag] || 'generic';
  }

  function getAccessibleName(el) {
    const tag = el.tagName.toLowerCase();
    if (tag === 'select') {
      const opt = el.querySelector('option[selected]') || el.options[el.selectedIndex];
      if (opt && opt.textContent) return opt.textContent.trim();
    }
    const ariaLabel = el.getAttribute('aria-label');
    if (ariaLabel && ariaLabel.trim()) return ariaLabel.trim();
    const labelledBy = el.getAttribute('aria-labelledby');
    if (labelledBy) {
      const parts = labelledBy.split(/\s+/).map(id => {
        const ref = document.getElementById(id);
        return (ref && ref.textContent && ref.textContent.trim()) || '';
      }).filter(Boolean);
      if (parts.length > 0) return parts.join(' ');
    }
    const placeholder = el.getAttribute('placeholder');
    if (placeholder && placeholder.trim()) return placeholder.trim();
    const title = el.getAttribute('title');
    if (title && title.trim()) return title.trim();
    const alt = el.getAttribute('alt');
    if (alt && alt.trim()) return alt.trim();
    if (el.id) {
      const label = document.querySelector(`label[for="${el.id}"]`);
      if (label && label.textContent && label.textContent.trim()) return label.textContent.trim();
    }
    const parentLabel = el.closest('label');
    if (parentLabel && parentLabel !== el) {
      const clone = parentLabel.cloneNode(true);
      clone.querySelectorAll('input, select, textarea').forEach(c => c.remove());
      const text = clone.textContent && clone.textContent.trim();
      if (text) return text;
    }
    if (tag === 'input') {
      const inputType = el.getAttribute('type') || '';
      const attrValue = el.getAttribute('value');
      if (inputType === 'submit' && attrValue && attrValue.trim()) return attrValue.trim();
      if (el.value && el.value.length < 50 && el.value.trim()) return el.value.trim();
    }
    if (['button', 'a', 'summary'].includes(tag)) {
      let text = '';
      for (let i = 0; i < el.childNodes.length; i++) {
        const node = el.childNodes[i];
        if (node.nodeType === Node.TEXT_NODE) text += node.textContent;
      }
      if (text.trim()) return text.trim();
    }
    if (tag.match(/^h[1-6]$/)) {
      const text = el.textContent;
      if (text && text.trim()) return text.trim().substring(0, 100);
    }
    if (tag === 'img') return '';
    let directText = '';
    for (let i = 0; i < el.childNodes.length; i++) {
      const node = el.childNodes[i];
      if (node.nodeType === Node.TEXT_NODE) directText += node.textContent;
    }
    if (directText && directText.trim() && directText.trim().length >= 3) {
      const clean = directText.trim();
      return clean.length > 100 ? clean.substring(0, 100) + '...' : clean;
    }
    return '';
  }

  // The computed style says what the page renders; Obscura computes only
  // part of it, so the element's own inline style (how scripts show and
  // hide things) is read too.
  function unrendered(el, prop, value) {
    return window.getComputedStyle(el)[prop] === value || el.style[prop] === value;
  }

  function isVisible(el) {
    if (!(el instanceof HTMLElement)) return true;
    const style = window.getComputedStyle(el);
    return style.display !== 'none' &&
      style.visibility !== 'hidden' &&
      style.opacity !== '0' &&
      el.offsetWidth > 0 &&
      el.offsetHeight > 0;
  }

  function isInteractive(el) {
    const tag = el.tagName.toLowerCase();
    return ['a', 'button', 'input', 'select', 'textarea', 'details', 'summary'].includes(tag) ||
      el.getAttribute('onclick') !== null ||
      el.getAttribute('tabindex') !== null ||
      el.getAttribute('role') === 'button' ||
      el.getAttribute('role') === 'link' ||
      el.getAttribute('contenteditable') === 'true';
  }

  function isStructural(el) {
    const tag = el.tagName.toLowerCase();
    return ['h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'nav', 'main', 'header', 'footer', 'section', 'article', 'aside'].includes(tag) ||
      el.getAttribute('role') !== null;
  }

  function shouldInclude(el, opts) {
    const tag = el.tagName.toLowerCase();
    if (SKIP_TAGS.includes(tag)) return false;
    if (el instanceof HTMLElement && unrendered(el, 'visibility', 'hidden')) return false;
    if (opts.filter !== 'all' && el.getAttribute('aria-hidden') === 'true') return false;
    if (opts.filter !== 'all' && !isVisible(el)) return false;
    if (opts.filter !== 'all' && !opts.refId) {
      const rect = el.getBoundingClientRect();
      if (!(rect.top < window.innerHeight && rect.bottom > 0 &&
            rect.left < window.innerWidth && rect.right > 0)) return false;
    }
    if (opts.filter === 'interactive') return isInteractive(el);
    if (isInteractive(el)) return true;
    if (isStructural(el)) return true;
    if (getAccessibleName(el).length > 0) return true;
    const role = getRole(el);
    return role !== null && role !== 'generic' && role !== 'image';
  }

  function walkDOM(el, depth, maxDepth, lines, opts) {
    if (depth > maxDepth) return;
    if (!el || !el.tagName) return;
    const tag = el.tagName.toLowerCase();
    if (SKIP_TAGS.includes(tag)) return;
    // Nothing under display: none is on the page.
    if (el instanceof HTMLElement && unrendered(el, 'display', 'none')) return;
    const included = shouldInclude(el, opts) || (opts.refId !== null && depth === 0);
    if (included) {
      const role = getRole(el);
      const name = getAccessibleName(el);
      let ref = window.__neboElementToRef.get(el);
      if (!ref) {
        ref = 'ref_' + (++window.__neboRefCounter);
        window.__neboElementToRef.set(el, ref);
      }
      window.__neboElementMap[ref] = new WeakRef(el);
      const meta = { tag: el.tagName.toLowerCase(), role, name };
      const elHref = el.getAttribute('href');
      if (elHref) meta.href = elHref;
      const elType = el.getAttribute('type');
      if (elType) meta.type = elType;
      window.__neboElementMeta[ref] = meta;

      let line = '  '.repeat(depth) + role;
      if (name) {
        const sanitized = name.replace(/\s+/g, ' ').substring(0, 100).replace(/"/g, '\\"');
        line += ` "${sanitized}"`;
      }
      line += ` [${ref}]`;
      if (el.getAttribute('href')) line += ` href="${el.getAttribute('href')}"`;
      if (el.getAttribute('type')) line += ` type="${el.getAttribute('type')}"`;
      if (el.getAttribute('placeholder')) line += ` placeholder="${el.getAttribute('placeholder')}"`;
      lines.push(line);

      if (tag === 'select') {
        const options = el.options;
        for (let i = 0; i < options.length; i++) {
          const opt = options[i];
          let optLine = '  '.repeat(depth + 1) + 'option';
          const optText = opt.textContent ? opt.textContent.trim() : '';
          if (optText) {
            const sanitized = optText.replace(/\s+/g, ' ').substring(0, 100).replace(/"/g, '\\"');
            optLine += ` "${sanitized}"`;
          }
          if (opt.selected) optLine += ' (selected)';
          if (opt.value && opt.value !== optText) {
            optLine += ` value="${opt.value.replace(/"/g, '\\"')}"`;
          }
          lines.push(optLine);
        }
      }
    }
    if (el.children && depth < maxDepth) {
      for (let i = 0; i < el.children.length; i++) {
        walkDOM(el.children[i], included ? depth + 1 : depth, maxDepth, lines, opts);
      }
    }
  }

  window.__neboGenerateAccessibilityTree = function (filter, depth, maxChars, refId) {
    const viewport = { width: window.innerWidth, height: window.innerHeight };
    const lines = [];
    const maxDepth = depth == null ? 15 : depth;
    const opts = { filter: filter || 'all', refId: refId || null };
    if (refId) {
      const weakRef = window.__neboElementMap[refId];
      if (!weakRef) {
        return {
          error: `Element with ref_id '${refId}' not found. It may have been removed from the page. Use read_page without ref_id to get the current page state.`,
          pageContent: '',
          viewport,
        };
      }
      const el = weakRef.deref();
      if (!el) {
        return {
          error: `Element with ref_id '${refId}' no longer exists. It may have been removed from the page. Use read_page without ref_id to get the current page state.`,
          pageContent: '',
          viewport,
        };
      }
      walkDOM(el, 0, maxDepth, lines, opts);
    } else {
      for (const key of Object.keys(window.__neboElementMap)) {
        if (!window.__neboElementMap[key].deref()) {
          delete window.__neboElementMap[key];
          delete window.__neboElementMeta[key];
        }
      }
      if (document.body) walkDOM(document.body, 0, maxDepth, lines, opts);
    }
    const pageContent = lines.join('\n');
    if (maxChars != null && pageContent.length > maxChars) {
      let errorMsg = `Output exceeds ${maxChars} character limit (${pageContent.length} characters). `;
      if (refId) {
        errorMsg += 'The specified element has too much content. Try specifying a smaller depth parameter or focus on a more specific child element.';
      } else if (depth != null) {
        errorMsg += 'Try specifying an even smaller depth parameter or use ref_id to focus on a specific element.';
      } else {
        errorMsg += 'Try specifying a depth parameter (e.g., depth: 5) or use ref_id to focus on a specific element from the page.';
      }
      return { error: errorMsg, pageContent: '', viewport };
    }
    return { pageContent, viewport };
  };

  // The element a ref names, scrolled to the middle of the viewport, and the
  // point to aim at: the live element first, else the one element with the
  // same tag, role, name, href and type (a re-render replaced it).
  window.__neboResolveRef = function (refId) {
    let el = null;
    const map = window.__neboElementMap;
    if (map[refId]) {
      el = map[refId].deref() || null;
      if (el && !document.contains(el)) {
        delete map[refId];
        el = null;
      }
    }
    if (!el) {
      const meta = window.__neboElementMeta[refId];
      if (meta) {
        const candidates = document.querySelectorAll(meta.tag);
        const matches = [];
        for (let i = 0; i < candidates.length; i++) {
          const c = candidates[i];
          const cRole = c.getAttribute('role') || '';
          const roleMatch = cRole === meta.role || !meta.role || meta.role === 'generic';
          if (!roleMatch) continue;
          const cLabel = c.getAttribute('aria-label') || c.getAttribute('placeholder') || c.getAttribute('title') || '';
          const cText = (c.textContent && c.textContent.trim().substring(0, 100)) || '';
          const nameMatch = !meta.name || cLabel === meta.name || cText === meta.name || cText.startsWith(meta.name);
          if (!nameMatch) continue;
          if (meta.href && c.getAttribute('href') !== meta.href) continue;
          if (meta.type && c.getAttribute('type') !== meta.type) continue;
          matches.push(c);
        }
        if (matches.length === 1) {
          el = matches[0];
          map[refId] = new WeakRef(el);
        }
      }
    }
    if (!el) return null;
    el.scrollIntoView({ behavior: 'instant', block: 'center', inline: 'center' });
    const rect = el.getBoundingClientRect();
    return [rect.left + rect.width / 2, rect.top + rect.height / 2];
  };

  // Resolves once the DOM has been quiet for `quietMs`, or after `maxMs`:
  // what a click or keystroke changed has landed before the page is read.
  window.__neboDomStable = function (quietMs, maxMs) {
    return new Promise(resolve => {
      let timer = setTimeout(done, quietMs);
      const cap = setTimeout(done, maxMs);
      const observer = new MutationObserver(() => {
        clearTimeout(timer);
        timer = setTimeout(done, quietMs);
      });
      observer.observe(document.documentElement || document, { childList: true, subtree: true, attributes: true, characterData: true });
      function done() {
        observer.disconnect();
        clearTimeout(timer);
        clearTimeout(cap);
        resolve(true);
      }
    });
  };
})()
