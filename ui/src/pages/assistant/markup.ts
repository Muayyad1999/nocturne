/**
 * The little formatting a model's prose actually needs.
 *
 * Models write fenced code blocks, `inline code`, bullet lists and headings
 * whatever you ask of them, and a wall of raw markdown is unreadable. This is
 * not a markdown implementation: it splits text into paragraphs, lists and
 * fenced blocks, and marks the spans inside a line. Nothing here produces
 * HTML — the renderer maps these parts onto components — so there is no way
 * for device output quoted by the model to become markup.
 */

export interface CodePart {
  kind: 'code';
  /** The language after the fence, lower-cased; empty when there was none. */
  language: string;
  text: string;
  /** The fence was never closed: the model is still writing it. */
  open: boolean;
}

export interface ListPart {
  kind: 'list';
  ordered: boolean;
  items: string[];
}

export interface HeadingPart {
  kind: 'heading';
  text: string;
}

export interface ParagraphPart {
  kind: 'paragraph';
  text: string;
}

export type TextPart = CodePart | ListPart | HeadingPart | ParagraphPart;

export type Span = { kind: 'text' | 'code' | 'strong'; text: string };

const FENCE = /^\s{0,3}(?:```|~~~)\s*([\w+#-]*)\s*$/;
const BULLET = /^\s{0,3}[-*•]\s+(.*)$/;
const NUMBERED = /^\s{0,3}(\d{1,3})[.)]\s+(.*)$/;
const HEADING = /^\s{0,3}#{1,6}\s+(.*?)\s*#*\s*$/;

/** Splits a model's message into the parts a transcript renders. */
export function parseParts(text: string): TextPart[] {
  const parts: TextPart[] = [];
  const lines = text.split('\n');
  let paragraph: string[] = [];
  let list: ListPart | null = null;

  const endParagraph = () => {
    const joined = paragraph.join('\n').trim();
    paragraph = [];
    if (joined) parts.push({ kind: 'paragraph', text: joined });
  };
  const endList = () => {
    if (list) parts.push(list);
    list = null;
  };
  const endBoth = () => {
    endParagraph();
    endList();
  };

  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index] ?? '';
    const fence = FENCE.exec(line);
    if (fence) {
      endBoth();
      const language = (fence[1] ?? '').toLowerCase();
      const body: string[] = [];
      let open = true;
      index += 1;
      for (; index < lines.length; index += 1) {
        const inner = lines[index] ?? '';
        if (FENCE.test(inner)) {
          open = false;
          break;
        }
        body.push(inner);
      }
      // A block still being streamed has no closing fence yet; it is shown
      // all the same, so the user sees the configuration as it is written.
      parts.push({ kind: 'code', language, text: body.join('\n'), open });
      continue;
    }

    const heading = HEADING.exec(line);
    if (heading) {
      endBoth();
      parts.push({ kind: 'heading', text: heading[1] ?? '' });
      continue;
    }

    const bullet = BULLET.exec(line);
    const numbered = NUMBERED.exec(line);
    if (bullet || numbered) {
      endParagraph();
      const ordered = Boolean(numbered);
      const item = (bullet?.[1] ?? numbered?.[2] ?? '').trim();
      if (list && list.ordered === ordered) list.items.push(item);
      else {
        endList();
        list = { kind: 'list', ordered, items: [item] };
      }
      continue;
    }

    if (line.trim() === '') {
      endBoth();
      continue;
    }
    // A wrapped continuation of the item above belongs to it.
    if (list) {
      const last = list.items.length - 1;
      list.items[last] = `${list.items[last] ?? ''} ${line.trim()}`;
      continue;
    }
    paragraph.push(line);
  }

  endBoth();
  return parts;
}

const SPAN = /(`+)([^`]+?)\1|\*\*([^*]+?)\*\*/g;

/** Marks the inline spans of one line: `code` and **strong**. */
export function parseSpans(text: string): Span[] {
  const spans: Span[] = [];
  let last = 0;
  for (const match of text.matchAll(SPAN)) {
    const at = match.index;
    if (at > last) spans.push({ kind: 'text', text: text.slice(last, at) });
    if (match[2] !== undefined) spans.push({ kind: 'code', text: match[2] });
    else if (match[3] !== undefined) spans.push({ kind: 'strong', text: match[3] });
    last = at + match[0].length;
  }
  if (last < text.length) spans.push({ kind: 'text', text: text.slice(last) });
  return spans;
}
