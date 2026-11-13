import { describe, expect, it } from 'vitest';
import type { AiPreset } from '@/api/types/ai';
import { relevantPresets } from './context';

const preset = (id: string, needs: AiPreset['needs']): AiPreset => ({
  id,
  title: id,
  description: '',
  needs,
  prompt: '',
});

describe('relevantPresets', () => {
  const all = [
    preset('explain_output', ['selection']),
    preset('why_down', ['device']),
    preset('open_alerts', ['none']),
  ];

  it('offers only what the context can answer', () => {
    expect(relevantPresets(all, {}).map((item) => item.id)).toEqual(['open_alerts']);
    expect(relevantPresets(all, { selection: 'Gi0/3 is down' }).map((item) => item.id)).toEqual([
      'explain_output',
      'open_alerts',
    ]);
    expect(relevantPresets(all, { device_ids: ['d1'] }).map((item) => item.id)).toEqual([
      'why_down',
      'open_alerts',
    ]);
  });

  it('needs every requirement, not any of them', () => {
    const both = [preset('draft', ['device', 'selection'])];
    expect(relevantPresets(both, { device_ids: ['d1'] })).toEqual([]);
    expect(relevantPresets(both, { device_ids: ['d1'], selection: 'x' })).toHaveLength(1);
  });

  it('treats an empty device list as no device', () => {
    expect(relevantPresets(all, { device_ids: [] }).map((item) => item.id)).toEqual(['open_alerts']);
  });
});
