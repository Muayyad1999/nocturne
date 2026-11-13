import { describe, expect, test } from 'vitest';
import { stepStartLines } from './playbook';

describe('placing a playbook problem on a line', () => {
  const yaml = [
    'name: Check', //            1
    'vars: { min: 2 }', //       2
    'steps:', //                 3
    '  - name: Collect', //      4
    '    collect: [facts]', //   5
    '  - name: Read', //         6
    '    commands:', //          7
    '      - show version', //   8
    '  - name: Wait', //         9
    '    pause: 3', //          10
  ].join('\n');

  test('each step is found by the line it starts on', () => {
    expect(stepStartLines(yaml)).toEqual([4, 6, 9]);
  });

  test('a nested list item is part of its step, not a step', () => {
    expect(stepStartLines(yaml)).not.toContain(8);
  });

  test('a key after the steps ends the list', () => {
    expect(stepStartLines(`steps:\n  - name: One\nname: After\n  - not: a step`)).toEqual([2]);
  });

  test('a document with no steps places nothing', () => {
    expect(stepStartLines('name: Empty\n')).toEqual([]);
    expect(stepStartLines('')).toEqual([]);
  });
});
