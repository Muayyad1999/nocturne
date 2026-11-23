// The Clips panel: what was copied through a control that asked for it to
// be remembered. The store itself is in lib/clipboard.ts; a screen opts a
// copy button in with `copyText(text, { clip: { label, source } })`.
export { ClipsPanel, clipPreview, useClips, type ClipsPanelProps } from './ClipsPanel';
