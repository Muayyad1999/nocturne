import type { ProviderTestResult } from '@/api/types/ai';

/**
 * What `ai.providers.test` found about tool calling, in words.
 *
 * This is the single most consequential thing on the Models screen, so it is
 * never a bare word: `none` means the model can talk about the network but
 * cannot look at it, and a person choosing a model has to be told that
 * plainly rather than be left to find out mid-incident.
 */
export const TOOL_CALLING: Record<
  ProviderTestResult['tool_calling'],
  { title: string; detail: string; tone: 'ok' | 'warn' | 'crit' }
> = {
  native: {
    title: 'Can use tools.',
    detail: 'It called the test tool through the model API, so it can read your network.',
    tone: 'ok',
  },
  prompted: {
    title: 'Can use tools, through the prompt.',
    detail:
      'It has no tool API, so Nocturne describes the tools in the prompt. That works, and it is slower and less reliable than a native one.',
    tone: 'warn',
  },
  none: {
    title: 'Cannot use tools.',
    detail:
      'It can talk about networking but cannot look at yours: no device, configuration or alert will reach it. Choose another model for anything but conversation.',
    tone: 'crit',
  },
};
