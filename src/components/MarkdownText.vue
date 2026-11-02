<script setup>
import { computed } from 'vue'
import MarkdownIt from 'markdown-it'
import linkAttributes from 'markdown-it-link-attributes'
import mdKatexImport from '@vscode/markdown-it-katex'
import DOMPurify from 'dompurify'
import { highlightHtmlText } from '../course/searchHighlight.js'
import 'katex/dist/katex.min.css'

// The plugin ships as CommonJS ({ default: fn }); take the callable.
const markdownItKatex = mdKatexImport?.default || mdKatexImport

const props = defineProps({
  text: {
    type: String,
    default: '',
  },
  loading: {
    type: Boolean,
    default: false,
  },
  highlightQuery: { type: String, default: '' },
})

const md = new MarkdownIt({
  html: false,
  linkify: true,
  breaks: true,
  highlight(code, language) {
    const lang = language ? ` data-language="${escapeHtml(language)}"` : ''
    return `<pre class="md-code-block"${lang}><code>${escapeHtml(code)}</code></pre>`
  },
})

md.use(linkAttributes, {
  attrs: {
    target: '_blank',
    rel: 'noopener noreferrer',
  },
})

// Render LaTeX math ($...$, $$...$$, and — after normalization — \(...\), \[...\]).
// throwOnError:false renders a red error span instead of breaking the whole message.
md.use(markdownItKatex, { throwOnError: false })

// Stable heading IDs allow same-note Markdown anchor links to stay in the app.
md.renderer.rules.heading_open = (tokens, index, options, env, renderer) => {
  const content = tokens[index + 1]?.content || ''
  const base = content.toLowerCase().replace(/[^\p{L}\p{N}\p{M}\s_-]/gu, '').trim().replace(/\s/g, '-') || 'section'
  env.headingIds ||= new Set()
  let id = base
  let suffix = 0
  while (env.headingIds.has(id)) id = `${base}-${++suffix}`
  env.headingIds.add(id)
  tokens[index].attrSet('id', id)
  return renderer.renderToken(tokens, index, options)
}

// Defense-in-depth: `html: false` already escapes raw HTML, but sanitize the
// rendered output too — markdown-it/KaTeX emit markup and web-clip bodies are
// external content. DOMPurify allows MathML/SVG by default so KaTeX survives;
// keep `target` so external links still open in a new tab.
function renderSafe(text) {
  const html = md.render(normalizeMathDelimiters(text || ''), {})
  return DOMPurify.sanitize(html, { ADD_ATTR: ['target'] })
}

const renderedMarkdown = computed(() => highlightHtmlText(renderSafe(props.text), props.highlightQuery))
const renderedWithStreamBubble = computed(() => (
  `${renderedMarkdown.value}<span class="stream-bubble" aria-hidden="true"></span>`
))

// LLMs emit math with mixed delimiters; map them all to $-delimiters the KaTeX
// plugin handles uniformly. First collapse any double-escaped backslashes (\\( ->
// \(), then convert \[...\] to block $$ and \(...\) to inline $.
function normalizeMathDelimiters(value) {
  return String(value || '')
    // Treat a "$" immediately before a digit as currency, not inline math, so
    // "it cost $5 and $10" doesn't render as one KaTeX formula. Escaping it makes
    // markdown-it emit a literal $. Runs first, before the \(…\)->$ conversions
    // below, so it never touches generated math. (Rare digit-leading inline math
    // like 3x should be written $$…$$ or \(3x\).) No look-behind — some WKWebView
    // builds lack it — so capture and re-emit the preceding char instead.
    .replace(/(^|[^$\\])\$(?=\d)/g, '$1\\$')
    .replace(/\\\\\(/g, '\\(')
    .replace(/\\\\\)/g, '\\)')
    .replace(/\\\\\[/g, '\\[')
    .replace(/\\\\\]/g, '\\]')
    .replace(/\\\[([\s\S]+?)\\\]/g, (_, body) => `\n\n$$\n${body.trim()}\n$$\n\n`)
    .replace(/\\\(([\s\S]+?)\\\)/g, (_, body) => `$${body.trim()}$`)
}

function escapeHtml(value) {
  return String(value || '')
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
}
</script>

<template>
  <div class="markdown-text" v-html="loading ? renderedWithStreamBubble : renderedMarkdown"></div>
</template>

<style scoped>
.markdown-text {
  color: var(--ink);
  font-size: 13px;
  line-height: 1.58;
  white-space: normal;
  overflow-wrap: anywhere;
}

.markdown-text :deep(> :first-child) {
  margin-top: 0;
}

.markdown-text :deep(> :last-child) {
  margin-bottom: 0;
}

.markdown-text :deep(p),
.markdown-text :deep(ul),
.markdown-text :deep(ol),
.markdown-text :deep(blockquote),
.markdown-text :deep(pre),
.markdown-text :deep(table) {
  margin: 0 0 8px;
}

.markdown-text :deep(h1),
.markdown-text :deep(h2),
.markdown-text :deep(h3),
.markdown-text :deep(h4),
.markdown-text :deep(h5),
.markdown-text :deep(h6) {
  margin: 12px 0 7px;
  color: var(--ink);
  font-weight: var(--w-strong);
  line-height: 1.28;
}

.markdown-text :deep(h1) {
  font-size: 17px;
}

.markdown-text :deep(h2) {
  font-size: 15px;
}

.markdown-text :deep(h3),
.markdown-text :deep(h4),
.markdown-text :deep(h5),
.markdown-text :deep(h6) {
  font-size: 13px;
}

.markdown-text :deep(ul),
.markdown-text :deep(ol) {
  padding-left: 18px;
}

.markdown-text :deep(li + li) {
  margin-top: 3px;
}

.markdown-text :deep(li > p) {
  margin: 0;
}

.markdown-text :deep(a) {
  color: var(--accent-ink);
  text-decoration: none;
  border-bottom: 1px dashed var(--accent-line);
}

.markdown-text :deep(blockquote) {
  padding-left: 12px;
  border-left: 3px solid var(--accent-line);
  color: var(--ink-2);
}

.markdown-text :deep(code) {
  border: 1px solid var(--line);
  border-radius: var(--r-xs);
  padding: 1px 5px;
  background: var(--surface-hover);
  color: var(--ink);
  font-family: var(--font-mono);
  font-size: 12px;
}

.markdown-text :deep(.md-code-block) {
  overflow: auto;
  border: 1px solid var(--line);
  border-radius: var(--r-md);
  padding: 11px 12px;
  background: var(--surface-inset);
}

.markdown-text :deep(.md-code-block code) {
  border: 0;
  padding: 0;
  background: transparent;
}

.markdown-text :deep(table) {
  display: block;
  width: 100%;
  overflow: auto;
  border-collapse: collapse;
}

.markdown-text :deep(th),
.markdown-text :deep(td) {
  border: 1px solid var(--line);
  padding: 6px 8px;
}

.markdown-text :deep(.stream-bubble) {
  display: inline-block;
  width: 7px;
  height: 7px;
  margin-left: 6px;
  border-radius: var(--r-pill);
  vertical-align: middle;
  background: var(--accent);
  animation: stream-bubble-pulse 1.2s ease-in-out infinite;
}

@keyframes stream-bubble-pulse {
  0%,
  100% {
    opacity: 0.35;
    transform: scale(0.85);
  }

  50% {
    opacity: 1;
    transform: scale(1);
  }
}
</style>
