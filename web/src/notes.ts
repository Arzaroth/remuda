export type Span =
  | { kind: 'text' | 'code'; text: string }
  | { kind: 'strong'; spans: Span[] }
  | { kind: 'link'; text: string; href: string };

export type Section = { heading: string | null; items: Span[][] };

/** A release's notes as its CHANGELOG section wrote them, up to the first
 * `##` heading, which is GitHub's generated list of pull requests. */
export function sections(notes: string): Section[] {
  const out: Section[] = [];
  let section: Section | null = null;
  let item: string[] | null = null;
  const close = () => {
    if (item && section) section.items.push(spans(item.join(' ')));
    item = null;
  };
  for (const raw of notes.replace(/\r/g, '').split('\n')) {
    const line = raw.trimEnd();
    if (/^##\s/.test(line)) break;
    const heading = line.match(/^###\s+(.*)/);
    if (heading) {
      close();
      section = { heading: heading[1].trim(), items: [] };
      out.push(section);
      continue;
    }
    if (!line.trim()) {
      close();
      continue;
    }
    if (!section) {
      section = { heading: null, items: [] };
      out.push(section);
    }
    const bullet = line.match(/^\s*[-*]\s+(.*)/);
    if (bullet) {
      close();
      item = [bullet[1]];
    } else if (item) {
      item.push(line.trim());
    } else {
      item = [line.trim()];
    }
  }
  close();
  return out.filter((s) => s.items.length);
}

const INLINE = /\*\*(.+?)\*\*|`([^`]+)`|\[([^\]]+)\]\((https?:\/\/[^)\s]+)\)/g;

export function spans(text: string): Span[] {
  const out: Span[] = [];
  let at = 0;
  for (const m of text.matchAll(INLINE)) {
    if (m.index! > at) out.push({ kind: 'text', text: text.slice(at, m.index) });
    if (m[1] !== undefined) out.push({ kind: 'strong', spans: spans(m[1]) });
    else if (m[2] !== undefined) out.push({ kind: 'code', text: m[2] });
    else out.push({ kind: 'link', text: m[3], href: m[4] });
    at = m.index! + m[0].length;
  }
  if (at < text.length) out.push({ kind: 'text', text: text.slice(at) });
  return out;
}
