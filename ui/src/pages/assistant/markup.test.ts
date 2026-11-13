import { describe, expect, it } from 'vitest';
import { parseParts, parseSpans } from './markup';

describe('parseParts', () => {
  it('keeps a configuration the model wrote as a code block with its language', () => {
    const parts = parseParts('Add this:\n\n```cisco\ninterface Gi0/3\n no shutdown\n```\n\nThen save.');
    expect(parts).toEqual([
      { kind: 'paragraph', text: 'Add this:' },
      { kind: 'code', language: 'cisco', text: 'interface Gi0/3\n no shutdown', open: false },
      { kind: 'paragraph', text: 'Then save.' },
    ]);
  });

  it('shows a fence that is still being streamed rather than hiding it', () => {
    const parts = parseParts('Here it comes:\n```\nvlan 40\n name');
    expect(parts[1]).toEqual({ kind: 'code', language: '', text: 'vlan 40\n name', open: true });
  });

  it('groups bullets and numbers into lists, and folds wrapped lines into the item', () => {
    const parts = parseParts('Findings:\n- Gi0/3 is down\n- Gi0/4 is erroring\n  badly\n\n1. check the cable\n2. check the port');
    expect(parts).toEqual([
      { kind: 'paragraph', text: 'Findings:' },
      { kind: 'list', ordered: false, items: ['Gi0/3 is down', 'Gi0/4 is erroring badly'] },
      { kind: 'list', ordered: true, items: ['check the cable', 'check the port'] },
    ]);
  });

  it('reads a heading without its hashes', () => {
    expect(parseParts('## What I found\ntwo ports')).toEqual([
      { kind: 'heading', text: 'What I found' },
      { kind: 'paragraph', text: 'two ports' },
    ]);
  });

  it('is empty for empty text, so a turn with no words renders nothing', () => {
    expect(parseParts('   \n\n ')).toEqual([]);
  });
});

describe('parseSpans', () => {
  it('marks inline code and bold, leaving everything else alone', () => {
    expect(parseSpans('Run `show ip route` on **CORE-SW1** now')).toEqual([
      { kind: 'text', text: 'Run ' },
      { kind: 'code', text: 'show ip route' },
      { kind: 'text', text: ' on ' },
      { kind: 'strong', text: 'CORE-SW1' },
      { kind: 'text', text: ' now' },
    ]);
  });

  it('leaves text that only looks like markup as text', () => {
    expect(parseSpans('the mask is 255.255.255.0 and * is a wildcard')).toEqual([
      { kind: 'text', text: 'the mask is 255.255.255.0 and * is a wildcard' },
    ]);
  });
});
