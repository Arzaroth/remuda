import { describe, expect, it } from 'vitest';
import { sections, spans } from './notes';

describe('release notes', () => {
  it('reads each section and joins wrapped bullets', () => {
    const notes = [
      '### Added',
      '',
      '- Plain `remuda` shows a table of every stored login,',
      '  in colour on a terminal.',
      '- Another one.',
      '',
      '### Fixed',
      '',
      '- A fix.',
    ].join('\n');
    expect(sections(notes)).toEqual([
      {
        heading: 'Added',
        items: [
          [
            { kind: 'text', text: 'Plain ' },
            { kind: 'code', text: 'remuda' },
            { kind: 'text', text: ' shows a table of every stored login, in colour on a terminal.' },
          ],
          [{ kind: 'text', text: 'Another one.' }],
        ],
      },
      { heading: 'Fixed', items: [[{ kind: 'text', text: 'A fix.' }]] },
    ]);
  });

  it("stops at GitHub's generated list", () => {
    const notes = "### Added\n\n- One.\n\n\n\n## What's Changed\n* a PR by someone\n\n**Full Changelog**: x";
    expect(sections(notes)).toEqual([{ heading: 'Added', items: [[{ kind: 'text', text: 'One.' }]] }]);
  });

  it('keeps text before any heading', () => {
    expect(sections('Just a line.\r\n')).toEqual([{ heading: null, items: [[{ kind: 'text', text: 'Just a line.' }]] }]);
    expect(sections('')).toEqual([]);
  });

  it('reads code inside bold', () => {
    expect(spans('**Waybar runs `tokengauge --waybar`.**')).toEqual([
      {
        kind: 'strong',
        spans: [
          { kind: 'text', text: 'Waybar runs ' },
          { kind: 'code', text: 'tokengauge --waybar' },
          { kind: 'text', text: '.' },
        ],
      },
    ]);
  });

  it('renders bold, code and web links only', () => {
    expect(spans('**Waybar** runs `x`, see [the docs](https://example.com) or [this](javascript:alert(1)).')).toEqual([
      { kind: 'strong', spans: [{ kind: 'text', text: 'Waybar' }] },
      { kind: 'text', text: ' runs ' },
      { kind: 'code', text: 'x' },
      { kind: 'text', text: ', see ' },
      { kind: 'link', text: 'the docs', href: 'https://example.com' },
      { kind: 'text', text: ' or [this](javascript:alert(1)).' },
    ]);
  });
});
